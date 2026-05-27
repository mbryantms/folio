//! Matching-accuracy-1.0 M6 — smart cover-page selection.
//!
//! Adds `issues.cover_page_index INTEGER NOT NULL DEFAULT 0`. The
//! scanner reads ComicInfo's `<Page Image="N" Type="FrontCover"/>`
//! marker (when present) and stamps `N` here; the thumbnail worker
//! and the phash computation pipeline both consult this column
//! instead of hardcoding page 0. Archives without a Pages block
//! fall back to 0, matching pre-M6 behavior.
//!
//! MetronInfo has no equivalent marker — the page-typing
//! vocabulary doesn't exist in the schema today — so this column is
//! ComicInfo-driven only. If MetronInfo ever adds one, the scanner
//! is the single place that reads it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE issues \
             ADD COLUMN IF NOT EXISTS cover_page_index INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE issues DROP COLUMN IF EXISTS cover_page_index")
            .await?;
        Ok(())
    }
}
