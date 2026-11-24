//! Per-reader "skip this event" (linked reading lists, PR 2).
//!
//! A reader can skip an event linked into a reading list: its band folds
//! to one line and Up Next stops offering it. Skipping is the reader's own
//! state, so it lives beside the link, not on it (a shared list's link is
//! the same for every reader).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS reading_list_link_skips (
                    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    link_id UUID NOT NULL REFERENCES reading_list_links(id) ON DELETE CASCADE,
                    skipped_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                    PRIMARY KEY (user_id, link_id)
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS reading_list_link_skips")
            .await?;
        Ok(())
    }
}
