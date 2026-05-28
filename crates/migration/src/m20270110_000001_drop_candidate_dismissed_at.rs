//! Drop the vestigial `metadata_run_candidate.dismissed_at` column.
//!
//! The column only ever backed the admin review-queue surface (a
//! dismiss-only list with no apply path), which was removed. Nothing
//! writes it anymore — the orchestrator inserted it as NULL and the
//! Runs drill-down merely echoed it — so the column carried no
//! information. Dropping it also removes the now-orphaned partial index
//! `metadata_run_candidate_review` (Postgres drops indexes that depend
//! on a dropped column automatically); that index existed solely to
//! paginate the review queue.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE metadata_run_candidate DROP COLUMN IF EXISTS dismissed_at",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE metadata_run_candidate ADD COLUMN IF NOT EXISTS dismissed_at TIMESTAMPTZ",
        )
        .await?;
        // Restore the partial index the original table migration created.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_run_candidate_review \
             ON metadata_run_candidate (bucket) WHERE applied_at IS NULL AND dismissed_at IS NULL",
        )
        .await?;
        Ok(())
    }
}
