//! Add `metadata_cache.etag` + `metadata_cache.last_modified` so cached
//! provider payloads can be revalidated with a conditional request
//! (WP-2.9).
//!
//! Metron's detail endpoints honour `If-Modified-Since` (and send
//! `Last-Modified`); some also send an `ETag`. Storing whichever
//! validator came back lets an expired row cost a `304` instead of a
//! full download — a real saving under Metron's 5,000/day budget.
//! Both columns are nullable; rows written by an unconditional fetch
//! (ComicVine, or a provider that sent no validator) keep NULL and are
//! re-fetched as before.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE metadata_cache \
             ADD COLUMN IF NOT EXISTS etag TEXT NULL, \
             ADD COLUMN IF NOT EXISTS last_modified TEXT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE metadata_cache \
             DROP COLUMN IF EXISTS etag, \
             DROP COLUMN IF EXISTS last_modified",
        )
        .await?;
        Ok(())
    }
}
