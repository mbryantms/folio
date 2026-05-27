//! Matching-accuracy-1.0 M3 — per-library publisher blacklist.
//!
//! Adds `library.metadata_publisher_blacklist JSONB NOT NULL DEFAULT
//! '[]'::jsonb`. Each entry is an opaque publisher-name string (the
//! comparison is sanitized + case-insensitive at filter time, so
//! operators don't have to worry about exact casing). The orchestrator's
//! pre-filter consults this column before scoring so a hard-mismatch
//! candidate never reaches the matcher — closing the gap where wrong-
//! publisher candidates scored Medium pre-M3 because their text shape
//! still partially matched.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS metadata_publisher_blacklist JSONB \
             NOT NULL DEFAULT '[]'::jsonb",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS metadata_publisher_blacklist",
        )
        .await?;
        Ok(())
    }
}
