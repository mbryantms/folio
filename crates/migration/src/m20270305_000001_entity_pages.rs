//! Entity landing pages (WP-5.5, audit R21 / UX-7).
//!
//! `/characters/{slug}`, `/teams/{slug}`, `/arcs/{slug}` and
//! `/publishers/{slug}` resolve their slug against the M0 entity tables
//! (`character`, `team`, `story_arc`, `publisher`). The M0 migration
//! (`m20261228`) backfilled those tables once from the string-keyed
//! junctions, but the scanner has been minting junction rows with
//! novel names ever since without an entity row behind them — so those
//! names had no slug and no page. Two changes:
//!
//! 1. **Re-run the entity backfill** for names that appeared after M0:
//!    `issue_characters` / `series_characters`, `issue_teams` /
//!    `series_teams`, the `issues.story_arc` CSV read-cache, and
//!    `series.publisher`. Going forward the series rollup keeps the
//!    tables topped up (`writers::ensure_series_entity_rows`).
//!
//!    Slugs: same `[^a-z0-9]+ → -` shape as M0. A base slug already
//!    taken by an existing row (or shared by two new names) gets an
//!    8-hex-char `md5(normalized_name)` suffix instead of M0's `-2`
//!    counter, so the backfill can't collide with the rows M0 or the
//!    providers already allocated. `ON CONFLICT DO NOTHING` (no target)
//!    covers both unique constraints.
//!
//! 2. **Link the rows by id** (owner decision 2026-09-30): fill NULL
//!    `issue_characters.character_id` / `series_characters.character_id`
//!    / `issue_teams.team_id` / `series_teams.team_id` /
//!    `series.publisher_id` by normalized name (never re-pointing an
//!    existing, possibly provider-set, FK), and reconcile `issue_arcs` to
//!    the `story_arc` CSV for issues whose arcs are file-owned (no
//!    provider / user `field_provenance`). The series rollup keeps this
//!    up to date afterwards (`metadata_rollup::link_series_entity_ids`).
//!
//! 3. **Expression indexes** on the lowercased/trimmed name columns so
//!    the per-entity lookups (`… OR btrim(lower(character)) = $n`) and
//!    the `publisher` name fallback are index scans, not heap scans.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// `(entity table, SELECT yielding candidate display names as nm)`.
const BACKFILL_SOURCES: &[(&str, &str)] = &[
    (
        "character",
        "SELECT character AS nm FROM issue_characters \
         UNION ALL SELECT character AS nm FROM series_characters",
    ),
    (
        "team",
        "SELECT team AS nm FROM issue_teams \
         UNION ALL SELECT team AS nm FROM series_teams",
    ),
    (
        "story_arc",
        // Same split rule as `metadata_rollup::split_csv` and the
        // `folio_issue_facet_keys` function: `;` when present, else `,`.
        "SELECT regexp_split_to_table(story_arc, \
             CASE WHEN story_arc LIKE '%;%' THEN ';' ELSE ',' END) AS nm \
         FROM issues WHERE story_arc IS NOT NULL AND story_arc <> ''",
    ),
    (
        "publisher",
        "SELECT publisher AS nm FROM series WHERE publisher IS NOT NULL",
    ),
];

/// Issues whose story arcs are file-owned: no `field_provenance` row for
/// `story_arcs` outside the scanner's file tiers (mirror of
/// `writers::FILE_SOURCED_SET_BY` — keep in sync).
const ARCS_FILE_OWNED: &str = "NOT EXISTS (SELECT 1 FROM field_provenance fp \
     WHERE fp.entity_type = 'issue' AND fp.entity_id = i.id \
       AND fp.field = 'story_arcs' \
       AND fp.set_by NOT IN ('comicinfo', 'metroninfo', 'series_json', \
                             'scanner_inference', 'scanner_folder_tag'))";

const ARC_SPLIT: &str = "regexp_split_to_table(i.story_arc, \
     CASE WHEN i.story_arc LIKE '%;%' THEN ';' ELSE ',' END)";

/// Id-link backfill (same statements the series rollup's
/// `link_series_entity_ids` runs per series, here over every row).
/// FK columns are filled only while NULL, never re-pointed.
fn link_statements() -> Vec<String> {
    vec![
        "UPDATE issue_characters j SET character_id = e.id FROM character e \
          WHERE j.character_id IS NULL AND e.normalized_name = btrim(lower(j.character))"
            .to_owned(),
        "UPDATE series_characters j SET character_id = e.id FROM character e \
          WHERE j.character_id IS NULL AND e.normalized_name = btrim(lower(j.character))"
            .to_owned(),
        "UPDATE issue_teams j SET team_id = e.id FROM team e \
          WHERE j.team_id IS NULL AND e.normalized_name = btrim(lower(j.team))"
            .to_owned(),
        "UPDATE series_teams j SET team_id = e.id FROM team e \
          WHERE j.team_id IS NULL AND e.normalized_name = btrim(lower(j.team))"
            .to_owned(),
        "UPDATE series s SET publisher_id = e.id FROM publisher e \
          WHERE s.publisher_id IS NULL AND e.normalized_name = btrim(lower(s.publisher))"
            .to_owned(),
        format!(
            "INSERT INTO issue_arcs (issue_id, arc_id, position_in_arc) \
             SELECT DISTINCT ON (i.id, e.id) i.id, e.id, \
                    CASE WHEN i.story_arc NOT LIKE '%;%' AND i.story_arc NOT LIKE '%,%' \
                          AND i.story_arc_number ~ '^\\s*[0-9]{{1,9}}\\s*$' \
                         THEN btrim(i.story_arc_number)::int4 END \
               FROM issues i \
               CROSS JOIN LATERAL {ARC_SPLIT} AS x(nm) \
               JOIN story_arc e ON e.normalized_name = btrim(lower(x.nm)) \
              WHERE i.story_arc IS NOT NULL AND i.story_arc <> '' AND {ARCS_FILE_OWNED} \
             ON CONFLICT DO NOTHING"
        ),
        format!(
            "DELETE FROM issue_arcs ia USING issues i, story_arc e \
              WHERE ia.issue_id = i.id AND e.id = ia.arc_id AND {ARCS_FILE_OWNED} \
                AND NOT EXISTS (SELECT 1 FROM {ARC_SPLIT} AS x(nm) \
                                 WHERE btrim(lower(x.nm)) = e.normalized_name)"
        ),
    ]
}

const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS issue_characters_norm_name_idx \
     ON issue_characters (btrim(lower(character)))",
    "CREATE INDEX IF NOT EXISTS series_characters_norm_name_idx \
     ON series_characters (btrim(lower(character)))",
    "CREATE INDEX IF NOT EXISTS issue_teams_norm_name_idx \
     ON issue_teams (btrim(lower(team)))",
    "CREATE INDEX IF NOT EXISTS series_teams_norm_name_idx \
     ON series_teams (btrim(lower(team)))",
    "CREATE INDEX IF NOT EXISTS series_publisher_norm_name_idx \
     ON series (btrim(lower(publisher))) WHERE publisher IS NOT NULL",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (table, source) in BACKFILL_SOURCES {
            db.execute_unprepared(&format!(
                r#"
                WITH names AS ({source}),
                normalized AS (
                    SELECT
                        btrim(nm)        AS display_name,
                        btrim(lower(nm)) AS normalized_name,
                        regexp_replace(
                            regexp_replace(btrim(lower(nm)), '[^a-z0-9]+', '-', 'g'),
                            '(^-+|-+$)', '', 'g'
                        )                AS base_slug
                    FROM names
                    WHERE nm IS NOT NULL
                ),
                missing AS (
                    SELECT
                        min(display_name) AS display_name,
                        normalized_name,
                        CASE WHEN min(base_slug) = '' THEN 'untitled'
                             ELSE min(base_slug) END AS base_slug
                    FROM normalized n
                    WHERE normalized_name <> ''
                      AND NOT EXISTS (
                          SELECT 1 FROM {table} e
                          WHERE e.normalized_name = n.normalized_name
                      )
                    GROUP BY normalized_name
                ),
                ranked AS (
                    SELECT m.*,
                           count(*) OVER (PARTITION BY base_slug) AS base_uses
                    FROM missing m
                )
                INSERT INTO {table} (slug, name, normalized_name)
                SELECT
                    CASE
                        WHEN base_uses = 1
                             AND NOT EXISTS (SELECT 1 FROM {table} e WHERE e.slug = base_slug)
                            THEN base_slug
                        ELSE base_slug || '-' || substr(md5(normalized_name), 1, 8)
                    END,
                    display_name,
                    normalized_name
                FROM ranked
                ON CONFLICT DO NOTHING
                "#
            ))
            .await?;
        }
        for sql in link_statements() {
            db.execute_unprepared(&sql).await?;
        }
        for sql in INDEXES {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // The backfilled entity rows are indistinguishable from rows the
        // rollup would have created; leave them. Only drop the indexes.
        for idx in [
            "issue_characters_norm_name_idx",
            "series_characters_norm_name_idx",
            "issue_teams_norm_name_idx",
            "series_teams_norm_name_idx",
            "series_publisher_norm_name_idx",
        ] {
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {idx}"))
                .await?;
        }
        Ok(())
    }
}
