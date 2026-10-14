//! Partial index for the home-page rails' "what is this user still
//! reading?" probes (post-audit T3.5).
//!
//! Continue Reading selects `progress_records WHERE user_id = $1 AND
//! finished = false AND last_page > 0 ORDER BY updated_at DESC LIMIT 24`,
//! and On Deck's `in_progress` CTE asks the same question per series. The
//! existing `(user_id, updated_at)` index serves the order but has to
//! visit every one of the user's rows (finished ones included) to find the
//! unfinished ones; for a heavy reader that is thousands of heap fetches
//! per home-page load. The partial index holds only the rows those probes
//! can return, in the order they want them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS progress_records_unfinished_idx \
                 ON progress_records (user_id, updated_at DESC) \
                 WHERE finished = false AND last_page > 0",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS progress_records_unfinished_idx")
            .await?;
        Ok(())
    }
}
