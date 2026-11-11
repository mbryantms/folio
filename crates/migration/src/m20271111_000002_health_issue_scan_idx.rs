//! Index `library_health_issues(scan_id)` for the per-run finding count on
//! scan-run lists (one grouped `scan_id IN (…)` query per page) and the
//! `scan_id` filter on `GET /admin/health-issues`. Partial: rows written
//! before scan ids were recorded, or whose run was pruned, carry NULL.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS library_health_issues_scan_idx \
                 ON library_health_issues(scan_id) WHERE scan_id IS NOT NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS library_health_issues_scan_idx")
            .await?;
        Ok(())
    }
}
