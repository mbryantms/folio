//! Index `reading_list_link_skips(link_id)` (linked reading lists 2.0).
//!
//! The primary key is `(user_id, link_id)`, so the cascade from deleting a
//! link — up to 500 per deleted list — had no index to find its skips by
//! and scanned the table once per link.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS reading_list_link_skips_link_idx
                    ON reading_list_link_skips (link_id)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS reading_list_link_skips_link_idx")
            .await?;
        Ok(())
    }
}
