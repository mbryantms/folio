//! Audit 2026-11 N-M9 / DI-17 — opt-in provider search for series a scan
//! creates.
//!
//! Adds `libraries.auto_search_new_series BOOLEAN NOT NULL DEFAULT FALSE`.
//! When on, a library scan that creates new series enqueues one
//! `SearchSeriesJob` per created, still-unmatched series
//! (`trigger_kind = 'scanner'`), within a rolling per-library budget
//! (`library::scanner::auto_search::SCAN_AUTO_SEARCH_CAP`).
//! Default off, so existing libraries keep today's behaviour: new folders
//! wait for a manual match or the weekly refresh.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS auto_search_new_series BOOLEAN \
             NOT NULL DEFAULT FALSE",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE libraries DROP COLUMN IF EXISTS auto_search_new_series")
            .await?;
        Ok(())
    }
}
