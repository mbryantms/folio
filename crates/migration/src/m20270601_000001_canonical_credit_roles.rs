//! Canonical credit roles + creator names on the credit junctions
//! (WP-8.1, roadmap M8 "data correctness").
//!
//! Two bugs in the provider apply path's `writers::set_issue_credits`
//! (fixed in the same change) left bad rows behind on non-writeback
//! libraries:
//!
//! 1. **Role casing.** The ComicVine / Metron / GCD mappers emit the
//!    ComicInfo PascalCase names (`Writer`, `CoverArtist`), but the
//!    per-role CSV read-cache rebuild, the filters and the UI match the
//!    lowercase snake_case keys (`writer`, `cover_artist`). The
//!    `issue_credits` rows were right, but `issues.writer` & co. stayed
//!    empty, so writer filters and full-text search missed the issue.
//! 2. **UUID in `person`.** The junction's `person` column (part of the
//!    PK) is the creator's *name* everywhere else, but the apply path
//!    stashed the person UUID there. `/creators`, `/people`, the
//!    saved-view credit filters and `series_credits` (copied from
//!    `issue_credits` by the series rollup) key on the name, so the
//!    credits were invisible to them; worse, the rollup's
//!    `ensure_persons_for_series` then minted a "ghost" `person` row
//!    *named* with that UUID and re-pointed `person_id` at it.
//!
//! `up`, set-based, for `issue_credits` and `series_credits`:
//!
//! - canonicalize `role` with the same table as
//!   `server::metadata::provider::canonical_credit_role` (every known
//!   spelling → one of the eight keys; anything else lowercased and
//!   snake_cased);
//! - where `person` holds the UUID of a real `person` row, replace it
//!   with that person's name and point `person_id` back at it;
//! - delete rows that collide on the PK after normalization (keeping a
//!   row that was already canonical, else the lowest `ordinal`);
//! - rebuild the eight per-role CSV columns of every issue that had a
//!   non-canonical row (same aggregation as
//!   `writers::rebuild_issue_csv_cache`; other issues are left alone —
//!   scanner-written CSVs keep their file spelling);
//! - delete the ghost `person` rows (named with another person's UUID)
//!   once nothing references them.
//!
//! `down` is a documented no-op: the original casing / UUID spellings
//! are not recoverable and nothing depends on them (the schema is
//! unchanged, so the migration round-trip gate is unaffected).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// SQL twin of `canonical_credit_role` over the column expression `col`.
/// Keep the word lists in sync with
/// `server::metadata::provider::canonicalize_role`.
fn canon_role_sql(col: &str) -> String {
    let key =
        format!("btrim(regexp_replace(lower(translate({col}, '_-', '  ')), '\\s+', ' ', 'g'))");
    format!(
        "(CASE \
           WHEN {key} IN ('writer','writers','script','scripter','story','plotter','plot') \
             THEN 'writer' \
           WHEN {key} IN ('penciler','penciller','pencils','artist','art') THEN 'penciller' \
           WHEN {key} IN ('inker','inkers','inks') THEN 'inker' \
           WHEN {key} IN ('colorist','colorists','colors','colourist','colours') \
             THEN 'colorist' \
           WHEN {key} IN ('letterer','letterers','letters') THEN 'letterer' \
           WHEN {key} IN ('cover','covers','cover artist','coverartist','cover art') \
             THEN 'cover_artist' \
           WHEN {key} IN ('editor','editors','editor in chief','executive editor', \
                          'consulting editor','associate editor','assistant editor', \
                          'senior editor','managing editor','group editor') THEN 'editor' \
           WHEN {key} IN ('translator','translators','translation') THEN 'translator' \
           ELSE replace({key}, ' ', '_') \
         END)"
    )
}

const UUID_RE: &str =
    "'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'";

/// `SELECT ctid, <key>, role, person, ordinal, new_role, new_person,
/// new_person_id` over one credit junction. `q` is the person whose UUID
/// was stashed in `person` (NULL for name-shaped rows).
fn normalized_rows(table: &str, key: &str) -> String {
    let role = canon_role_sql("c.role");
    format!(
        "SELECT c.ctid AS row_ctid, c.{key} AS owner, c.role, c.person, \
                {ordinal} AS ord, \
                {role} AS new_role, \
                COALESCE(q.name, c.person) AS new_person, \
                c.person_id AS old_person_id, \
                COALESCE(q.id, c.person_id) AS new_person_id \
           FROM {table} c \
           LEFT JOIN person q \
             ON c.person ~ {UUID_RE} AND q.id = (CASE WHEN c.person ~ {UUID_RE} \
                                                    THEN c.person::uuid END)",
        ordinal = if table == "issue_credits" {
            "c.ordinal"
        } else {
            "0"
        },
    )
}

async fn normalize_table<C: ConnectionTrait>(db: &C, table: &str, key: &str) -> Result<(), DbErr> {
    let rows = normalized_rows(table, key);
    // 1. Drop rows that would collide on (owner, role, person) once
    //    normalized. Keep the already-canonical row when there is one,
    //    so the UPDATE below never hits a transient PK conflict.
    db.execute_unprepared(&format!(
        "DELETE FROM {table} t USING ( \
           SELECT row_ctid FROM ( \
             SELECT n.row_ctid, row_number() OVER ( \
                      PARTITION BY n.owner, n.new_role, n.new_person \
                      ORDER BY (n.role = n.new_role AND n.person = n.new_person) DESC, \
                               n.ord, n.person) AS rn \
               FROM ({rows}) n \
           ) d WHERE d.rn > 1 \
         ) x WHERE t.ctid = x.row_ctid"
    ))
    .await?;
    // 2. Rewrite the survivors in place.
    db.execute_unprepared(&format!(
        "UPDATE {table} t \
            SET role = n.new_role, person = n.new_person, person_id = n.new_person_id \
           FROM ({rows}) n \
          WHERE t.ctid = n.row_ctid \
            AND (n.role, n.person, n.old_person_id) \
                IS DISTINCT FROM (n.new_role, n.new_person, n.new_person_id)"
    ))
    .await?;
    Ok(())
}

const AFFECTED: &str = "folio_m20270601_affected_issue";

fn csv_col(role: &str) -> String {
    format!(
        "{role} = (SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '') \
                     FROM issue_credits ic JOIN person p ON p.id = ic.person_id \
                    WHERE ic.issue_id = issues.id AND ic.role = '{role}')"
    )
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Issues whose credits are about to change — their per-role CSV
        // columns get rebuilt from the normalized junction afterwards.
        db.execute_unprepared(&format!(
            "DROP TABLE IF EXISTS {AFFECTED}; \
             CREATE TABLE {AFFECTED} (issue_id TEXT PRIMARY KEY)"
        ))
        .await?;
        db.execute_unprepared(&format!(
            "INSERT INTO {AFFECTED} (issue_id) \
             SELECT DISTINCT n.owner FROM ({rows}) n \
              WHERE (n.role, n.person, n.old_person_id) \
                    IS DISTINCT FROM (n.new_role, n.new_person, n.new_person_id)",
            rows = normalized_rows("issue_credits", "issue_id"),
        ))
        .await?;

        normalize_table(db, "issue_credits", "issue_id").await?;
        normalize_table(db, "series_credits", "series_id").await?;

        let sets = [
            "writer",
            "penciller",
            "inker",
            "colorist",
            "letterer",
            "cover_artist",
            "editor",
            "translator",
        ]
        .iter()
        .map(|r| csv_col(r))
        .collect::<Vec<_>>()
        .join(", ");
        db.execute_unprepared(&format!(
            "UPDATE issues SET {sets} WHERE id IN (SELECT issue_id FROM {AFFECTED})"
        ))
        .await?;

        // Ghost people: rows the series rollup minted from a stashed
        // UUID (`name` = another person's id). Nothing should point at
        // them any more; keep any that something still references.
        db.execute_unprepared(&format!(
            "DELETE FROM person g \
              WHERE g.normalized_name ~ {UUID_RE} \
                AND EXISTS (SELECT 1 FROM person q \
                             WHERE q.id = (CASE WHEN g.normalized_name ~ {UUID_RE} \
                                                THEN g.normalized_name::uuid END) \
                               AND q.id <> g.id) \
                AND NOT EXISTS (SELECT 1 FROM issue_credits ic WHERE ic.person_id = g.id) \
                AND NOT EXISTS (SELECT 1 FROM series_credits sc WHERE sc.person_id = g.id) \
                AND NOT EXISTS (SELECT 1 FROM issue_cover cv \
                                 WHERE cv.variant_artist_person_id = g.id)"
        ))
        .await?;

        db.execute_unprepared(&format!("DROP TABLE {AFFECTED}"))
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Intentional no-op: the pre-normalization spellings (`Writer`,
        // stashed UUIDs) are not recoverable and nothing reads them. The
        // schema is untouched, so there is nothing structural to undo.
        Ok(())
    }
}
