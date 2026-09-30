//! Index for the saved-view "has my notes / bookmarks / highlights"
//! filters (roadmap WP-5.7, audit R11).
//!
//! On series views the filter is `EXISTS (SELECT 1 FROM markers m WHERE
//! m.user_id = $1 AND m.kind = '…' AND m.series_id = series.id)`, correlated
//! per candidate series. The existing marker indexes lead with
//! `(user_id, issue_id, …)` / `(user_id, kind, updated_at)`, so the series
//! probe had no matching index; `(user_id, series_id, kind)` makes it an
//! index-only lookup. Issue views reuse `markers_user_issue_page_idx`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS markers_user_series_kind_idx \
                 ON markers(user_id, series_id, kind)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS markers_user_series_kind_idx")
            .await?;
        Ok(())
    }
}
