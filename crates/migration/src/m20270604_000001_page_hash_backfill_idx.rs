//! Partial indexes for the lazy page-hash backfill (roadmap WP-8.4).
//!
//! Markers and progress rows written before WP-6.2 have no `page_hash`.
//! When the page server next opens an issue's archive,
//! `reading::page_hash_backfill` asks "which pages of this issue have
//! anchors without a hash?". `markers` already leads an index with
//! `issue_id` (`markers(issue_id, page_index)`); `progress_records` does
//! not (its keys are `(user_id, issue_id, …)`), so without this index the
//! probe would scan the whole table on every archive open. The index only
//! covers unhashed rows, so it shrinks as the backfill proceeds.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS progress_records_issue_unhashed_idx \
                 ON progress_records(issue_id) WHERE page_hash IS NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS progress_records_issue_unhashed_idx")
            .await?;
        Ok(())
    }
}
