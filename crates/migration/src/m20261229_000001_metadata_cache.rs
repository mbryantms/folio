//! Metadata Providers 1.0 — M1: `metadata_cache` response cache.
//!
//! Per-provider per-entity TTL-bounded JSON cache for normalized
//! `GenericMetadata` payloads. ComicVine's TOS encourages caching;
//! Metron has no per-resource cache header but the response shape is
//! stable. The cache lets the Apply jobs (M4) re-fetch the same record
//! after the user clicks "Apply" without burning a new request budget
//! slot, and lets bulk-refresh runs dedup repeat hits in a single pass.
//!
//! TTLs are policy (`metadata.cache_ttl_hours.*` settings), enforced at
//! read time in [`crate::metadata::cache`] — the table itself stores
//! only the raw `fetched_at`. The cleanup job (M4) walks
//! `fetched_at < now() - max(TTLs) * 2` and deletes stale rows.
//!
//! Conditional GET (`ETag`/`If-Modified-Since`) is deferred — neither
//! provider documents support; revisit if quota pressure becomes real.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS metadata_cache (
                provider     TEXT NOT NULL,
                entity       TEXT NOT NULL,
                external_id  TEXT NOT NULL,
                payload      JSONB NOT NULL,
                fetched_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (provider, entity, external_id)
            )"#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_cache_fetched ON metadata_cache (fetched_at)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS metadata_cache")
            .await?;
        Ok(())
    }
}
