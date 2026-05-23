//! Adds `progress_records.is_backfill` so bulk-mark / sync writes can
//! be flagged "this is cataloging, not active reading" and excluded
//! from time-bound activity surfaces (reading log feed, heatmap,
//! daily-pages stat, streak counter, Just Finished sort).
//!
//! `finished` itself stays as the source of truth for whether the
//! issue is read — On Deck dedup, Continue Reading carve-out,
//! completion %, read badges, OPDS progression all keep their current
//! semantics. The flag is a *display* hint for activity surfaces, not
//! a state change.
//!
//! Default `false` so every existing row keeps showing up in the
//! reading log (back-compat for users who already bulk-marked under
//! pre-v0.5.7 semantics).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE progress_records \
             ADD COLUMN IF NOT EXISTS is_backfill BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        // Partial index — the typical row has `is_backfill = false`,
        // so an index on the rare `true` case keeps cleanup queries
        // (e.g. "show me everything I backfilled this session") cheap
        // without bloating the main reading-log scans.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS progress_records_backfill_idx \
             ON progress_records (user_id, finished_at) \
             WHERE is_backfill = TRUE",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "DROP INDEX IF EXISTS progress_records_backfill_idx",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE progress_records DROP COLUMN IF EXISTS is_backfill",
        )
        .await?;
        Ok(())
    }
}
