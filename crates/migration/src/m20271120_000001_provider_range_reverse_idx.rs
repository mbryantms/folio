//! Reverse lookup on `series_provider_range` (upcoming-releases 1.0).
//!
//! The table was only ever read by local series (`series_id, source`).
//! The Upcoming view asks the opposite question for every release on a
//! page: "which local series maps to this provider series?" The primary
//! `external_ids` table already has `(source, external_id)`; this gives
//! the range table the same.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS series_provider_range_by_provider
                    ON series_provider_range (source, provider_series_id)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS series_provider_range_by_provider")
            .await?;
        Ok(())
    }
}
