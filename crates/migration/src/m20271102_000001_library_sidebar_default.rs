//! Start Here + default pins M1 — per-library sidebar default.
//!
//! Adds `libraries.show_in_sidebar_by_default BOOLEAN NOT NULL DEFAULT TRUE`.
//! `sidebar_layout::compute_layout` uses it as the default visibility of the
//! library's sidebar row for any user without an explicit override, so an
//! admin can keep a server with many libraries down to a curated few in a
//! new account's sidebar. Every library stays reachable via "All Libraries"
//! and the sidebar editor. Default true keeps today's behaviour.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS show_in_sidebar_by_default BOOLEAN \
             NOT NULL DEFAULT TRUE",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS show_in_sidebar_by_default",
        )
        .await?;
        Ok(())
    }
}
