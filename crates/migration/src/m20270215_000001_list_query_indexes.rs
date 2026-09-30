//! List-query indexes from the WP-3.6 load baseline (audit R25 / OP-6).
//!
//! `just perf-explain` drives the hottest list/filter/sort endpoints
//! against a 50,000-issue stress library with `auto_explain` on; each
//! index below removes a sequential scan one of those plans showed.
//! `docs/dev/load-testing.md` records the before/after plans.
//!
//! - `issues_active_created_idx` — cross-library issue browse / "New
//!   issues" (`GET /issues?sort=created_at`) and the recent-issues rail
//!   walk the active set newest-first. Partial on the active predicate
//!   every list query carries, so it also serves as a cheap index-only
//!   `COUNT(*)` source for the first-page total.
//! - `issues_active_id_series_idx` — `(id) INCLUDE (series_id)` over the
//!   active set. The per-user rails (On Deck's started / in-progress
//!   series, …) join the caller's `progress_records` to `issues` only to
//!   read `series_id`; with the default `random_page_cost` the planner
//!   hashes the whole `issues` heap (65 MB at 50k issues) for that. This
//!   5 MB covering index turns it into an index-only scan (~3x faster).
//! - `folio_issue_facet_keys(...)` + `issues_facet_keys_gin` — the
//!   cross-library issue facets (`writers=`, `genres=`, `characters=`, …)
//!   match against the denormalized CSV read-cache columns with an
//!   `EXISTS (unnest(regexp_split_to_array(...)))` predicate, which can
//!   only be evaluated row by row. The function computes the same
//!   `lower(trim(piece))` keys (prefixed with the column name so one GIN
//!   index serves all 13 facets) and `api::issues` filters with
//!   `folio_issue_facet_keys(...) && $1`. The split rule (`;` when the
//!   value contains one, else `,`) is copied verbatim from the old
//!   predicate so results are identical. Empty pieces are dropped: a
//!   facet value is never empty (`split_csv` filters them), so they
//!   could never match.
//! - `series_created_idx` / `series_updated_idx` — the "Recently added" /
//!   "Recently updated" series rails (`sort=created_at|updated_at`).
//! - `series_publisher_idx` — the publisher facet on the series grid and
//!   the `publisher` saved-view predicate.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// Column order is the function's signature — `api::issues` builds the
/// call from the same list (`ISSUE_FACET_COLUMNS`); keep them in sync.
const FACET_FN: &str = r#"
CREATE OR REPLACE FUNCTION folio_issue_facet_keys(
    p_genre text, p_tags text, p_writer text, p_penciller text, p_inker text,
    p_colorist text, p_letterer text, p_cover_artist text, p_editor text,
    p_translator text, p_characters text, p_teams text, p_locations text
) RETURNS text[]
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $fn$
DECLARE
    vals text[] := ARRAY[p_genre, p_tags, p_writer, p_penciller, p_inker,
                         p_colorist, p_letterer, p_cover_artist, p_editor,
                         p_translator, p_characters, p_teams, p_locations];
    names text[] := ARRAY['genre', 'tags', 'writer', 'penciller', 'inker',
                          'colorist', 'letterer', 'cover_artist', 'editor',
                          'translator', 'characters', 'teams', 'locations'];
    keys text[] := '{}';
    v text;
BEGIN
    FOR i IN 1..13 LOOP
        v := coalesce(vals[i], '');
        IF v <> '' THEN
            keys := keys || ARRAY(
                SELECT names[i] || ':' || lower(trim(piece))
                FROM unnest(regexp_split_to_array(
                    v, CASE WHEN v LIKE '%;%' THEN ';' ELSE ',' END)) AS piece
                WHERE lower(trim(piece)) <> ''
            );
        END IF;
    END LOOP;
    RETURN keys;
END
$fn$
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issues_active_created_idx \
             ON issues (created_at DESC, id DESC) \
             WHERE state = 'active' AND removed_at IS NULL",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issues_active_id_series_idx \
             ON issues (id) INCLUDE (series_id) \
             WHERE state = 'active' AND removed_at IS NULL",
        )
        .await?;
        db.execute_unprepared(FACET_FN).await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issues_facet_keys_gin ON issues USING gin (\
                folio_issue_facet_keys(genre, tags, writer, penciller, inker, \
                    colorist, letterer, cover_artist, editor, translator, \
                    characters, teams, locations))",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_created_idx \
             ON series (created_at DESC, id DESC)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_updated_idx \
             ON series (updated_at DESC, id DESC)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_publisher_idx \
             ON series (publisher) WHERE publisher IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for stmt in [
            "DROP INDEX IF EXISTS series_publisher_idx",
            "DROP INDEX IF EXISTS series_updated_idx",
            "DROP INDEX IF EXISTS series_created_idx",
            "DROP INDEX IF EXISTS issues_facet_keys_gin",
            "DROP FUNCTION IF EXISTS folio_issue_facet_keys(\
                text, text, text, text, text, text, text, \
                text, text, text, text, text, text)",
            "DROP INDEX IF EXISTS issues_active_id_series_idx",
            "DROP INDEX IF EXISTS issues_active_created_idx",
        ] {
            db.execute_unprepared(stmt).await?;
        }
        Ok(())
    }
}
