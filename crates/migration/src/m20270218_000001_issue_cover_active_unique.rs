//! Scope the cover-slot uniqueness to **active** rows.
//!
//! `issue_cover` / `series_cover` were created (M0,
//! `m20261228_000001_metadata_providers_schema`) with a table-level
//! `UNIQUE (issue_id, kind, ordinal)` / `UNIQUE (series_id, kind,
//! ordinal)`. The design, though, is "many rows per slot, `is_active`
//! picks the served one": `writers::apply_cover` deactivates the current
//! primary and inserts the replacement, and the post-scan phash worker
//! keeps an **inactive** `archive_extracted` `primary/0` row on every
//! scanned issue as the matcher's side-channel. Under the non-partial
//! constraint an inactive row still occupies the slot, so every provider
//! primary-cover apply on a scanned issue failed with a unique violation
//! (surfaced only as `cover_skipped_reason = "write_failed: …"`).
//!
//! `up` swaps each constraint for a partial unique index
//! `… (issue_id, kind, ordinal) WHERE is_active`: at most one active row
//! per slot, any number of inactive ones. `series_cover` has no writer
//! yet but carries the identical design, so it gets the same treatment
//! before one ships.
//!
//! `down` is **lossy**: the old constraint can't hold with several rows
//! per slot, so it first deletes the surplus rows — keeping the active
//! row when there is one, otherwise the most recently fetched — then
//! restores the table constraint. The deleted rows' on-disk files are
//! not removed (the thumbnail orphan sweep reclaims them).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// `(table, parent column, legacy constraint name, new index name)`.
const TABLES: &[(&str, &str, &str, &str)] = &[
    (
        "issue_cover",
        "issue_id",
        "issue_cover_issue_id_kind_ordinal_key",
        "issue_cover_active_slot_uniq",
    ),
    (
        "series_cover",
        "series_id",
        "series_cover_series_id_kind_ordinal_key",
        "series_cover_active_slot_uniq",
    ),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (table, parent, constraint, index) in TABLES {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {constraint}"
            ))
            .await?;
            db.execute_unprepared(&format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {index} \
                 ON {table} ({parent}, kind, ordinal) WHERE is_active"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (table, parent, constraint, index) in TABLES {
            // Lossy: collapse each slot to one row (active first, then
            // newest) so the table-level constraint can be restored.
            db.execute_unprepared(&format!(
                "DELETE FROM {table} WHERE id IN ( \
                     SELECT id FROM ( \
                         SELECT id, row_number() OVER ( \
                             PARTITION BY {parent}, kind, ordinal \
                             ORDER BY is_active DESC, fetched_at DESC, id DESC \
                         ) AS rn \
                         FROM {table} \
                     ) ranked WHERE rn > 1 \
                 )"
            ))
            .await?;
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {index}"))
                .await?;
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} ADD CONSTRAINT {constraint} \
                 UNIQUE ({parent}, kind, ordinal)"
            ))
            .await?;
        }
        Ok(())
    }
}
