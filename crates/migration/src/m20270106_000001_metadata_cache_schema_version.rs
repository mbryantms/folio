//! Add `metadata_cache.schema_version` so cached provider payloads
//! self-invalidate when the normalized `GenericMetadata` mapping
//! changes.
//!
//! Without this, extending `GenericMetadata` with a new field (e.g.
//! `variants` for ComicVine `associated_images`) leaves payloads cached
//! under the old mapping serving empty values for the new field until
//! their TTL expires — serde happily fills additive fields with their
//! defaults, so the existing deserialize-failure guard never fires.
//!
//! `cache::get` compares the row's `schema_version` against the current
//! `cache::CACHE_SCHEMA_VERSION` const and treats a mismatch as a miss.
//! DEFAULT 0 means every pre-existing row is older than the first
//! stamped version (1) and is re-fetched on next access.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE metadata_cache \
             ADD COLUMN IF NOT EXISTS schema_version INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE metadata_cache DROP COLUMN IF EXISTS schema_version")
            .await?;
        Ok(())
    }
}
