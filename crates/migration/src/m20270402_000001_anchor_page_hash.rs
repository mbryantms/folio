//! Adds `markers.page_hash` and `progress_records.page_hash` — the
//! content hash of the page image an anchor was recorded on (roadmap
//! WP-6.2, audit R28).
//!
//! Every per-user anchor is an `(issue_id, page_index)` ordinal. WP-1.2
//! remaps those ordinals when Folio's own archive editor moves pages,
//! but a replaced archive (re-downloaded release, an external tool's
//! re-pack) can reorder pages without Folio knowing the old→new map.
//! With the page hash captured at write time, the scanner re-resolves
//! `page_index` by hash on a content change and only falls back to the
//! ordinal map when the image is gone.
//!
//! Hex BLAKE3 of the page entry's decompressed bytes
//! (`reading::page_hash`). Nullable: rows written before this migration,
//! and rows whose page could not be read at capture time, keep the
//! ordinal-only behaviour.
//!
//! A rescan asks "does this issue have any hashed anchor?" for every
//! changed file. Neither table leads an index with `issue_id` (both are
//! `(user_id, issue_id, …)`), so two small partial indexes keep that
//! probe an index lookup that only covers hashed rows.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE markers ADD COLUMN IF NOT EXISTS page_hash TEXT")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE progress_records ADD COLUMN IF NOT EXISTS page_hash TEXT",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_issue_hashed_idx \
             ON markers(issue_id) WHERE page_hash IS NOT NULL",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS progress_records_issue_hashed_idx \
             ON progress_records(issue_id) WHERE page_hash IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS progress_records_issue_hashed_idx")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS markers_issue_hashed_idx")
            .await?;
        db.execute_unprepared("ALTER TABLE progress_records DROP COLUMN IF EXISTS page_hash")
            .await?;
        db.execute_unprepared("ALTER TABLE markers DROP COLUMN IF EXISTS page_hash")
            .await?;
        Ok(())
    }
}
