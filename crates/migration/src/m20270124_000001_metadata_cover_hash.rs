//! `metadata_cover_hash` — search-time cover perceptual-hash cache
//! (WP-2.9, audit DI-16).
//!
//! Every metadata search fetches + hashes up to 25 candidate covers per
//! provider so the matcher can use cover similarity as its primary
//! discriminant. The hashes are a pure function of the image bytes, and
//! provider cover URLs are content-addressed enough (a changed cover
//! gets a new URL) that a 30-day cache keyed by URL turns a repeat
//! search into zero downloads. Rows are written by
//! `metadata::cover_hash_cache::put` after a successful decode; a
//! failed fetch/decode writes nothing so a transient CDN error doesn't
//! poison the entry.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS metadata_cover_hash (\
                url TEXT PRIMARY KEY, \
                phash BIGINT NOT NULL, \
                dhash BIGINT NOT NULL, \
                ahash BIGINT NOT NULL, \
                fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()\
             )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_cover_hash_fetched_at_idx \
             ON metadata_cover_hash (fetched_at)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS metadata_cover_hash")
            .await?;
        Ok(())
    }
}
