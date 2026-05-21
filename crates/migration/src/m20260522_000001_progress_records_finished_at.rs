//! Add `progress_records.finished_at` — authoritative timestamp for
//! when a (user, issue) pair flipped to `finished = TRUE`.
//!
//! Stats v2 already aggregates "what did the user read and when" via
//! `reading_sessions`, but the **reading log** wants to surface
//! discrete "issue finished" / "series finished" events with their
//! own timestamps. Until now the only signal we had was
//! `updated_at`, which gets bumped on every per-page write — so the
//! finish moment was indistinguishable from any later progress edit
//! on the same row.
//!
//! Backfill: for every row where `finished = TRUE` and the new
//! column is NULL, copy `updated_at`. That's the best approximation
//! we have for legacy rows. Going forward the write paths in
//! `crates/server/src/api/progress.rs` set `finished_at = now()` on
//! the flip and `NULL` when the user un-finishes.
//!
//! Index: a partial DESC index on `(user_id, finished_at)` filtered
//! to `finished_at IS NOT NULL` makes the log's reverse-chronological
//! event feed cheap to page even for users with years of history.
//! The partial keeps the index tiny on fresh accounts.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum ProgressRecords {
    Table,
    FinishedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ProgressRecords::Table)
                    .add_column(
                        ColumnDef::new(ProgressRecords::FinishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();
        conn.execute_unprepared(
            "UPDATE progress_records \
             SET finished_at = updated_at \
             WHERE finished = TRUE AND finished_at IS NULL",
        )
        .await?;

        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS progress_records_user_finished_at \
             ON progress_records (user_id, finished_at DESC) \
             WHERE finished_at IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP INDEX IF EXISTS progress_records_user_finished_at")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ProgressRecords::Table)
                    .drop_column(ProgressRecords::FinishedAt)
                    .to_owned(),
            )
            .await
    }
}
