//! `library_backup_stats` — the stored `.bak` footprint one library's
//! health page shows (audit 2026-11, N-L4).
//!
//! `GET /libraries/{slug}/backup-storage` used to `walkdir` the whole
//! library root on every request to count backup files. One row per
//! library now holds the last tally:
//!
//! - written by the daily `backup_prune` sweep (which walks the tree
//!   anyway) and by the first request for a library that has no row yet;
//! - `stale` is flipped on by a scan that changed the catalogue (every
//!   archive rewrite ends in a scoped rescan), so the next request
//!   recounts once the stored tally is older than the recount guard;
//! - deleted with the library (`ON DELETE CASCADE`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS library_backup_stats (
                    library_id          UUID PRIMARY KEY REFERENCES libraries(id) ON DELETE CASCADE,
                    file_count          BIGINT NOT NULL CHECK (file_count >= 0),
                    total_bytes         BIGINT NOT NULL CHECK (total_bytes >= 0),
                    oldest_modified_at  TIMESTAMPTZ NULL,
                    newest_modified_at  TIMESTAMPTZ NULL,
                    computed_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    stale               BOOLEAN NOT NULL DEFAULT FALSE
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS library_backup_stats")
            .await?;
        Ok(())
    }
}
