//! First-import lazy-hash mode (roadmap WP-3.2, audit §3.1 "Import").
//!
//!   - `libraries.trust_fingerprint_on_first_import BOOLEAN NOT NULL
//!     DEFAULT false` — per-library opt-in. While the library has never
//!     completed a full scan (`last_scan_at IS NULL`), the scanner ingests
//!     new files on size+mtime alone: the issue id is BLAKE3 of the path
//!     (spec §5.1.2's path-identity option) and the full-file BLAKE3 is
//!     deferred to the `hash_backfill` job.
//!   - `issues_hash_pending_idx` — partial index over the rows whose
//!     content hash is still pending (`hash_algorithm = 0`; `1` = BLAKE3 of
//!     the bytes). The backfill job and the progress endpoint both filter
//!     on it; the index is empty once every library has drained.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries ADD COLUMN IF NOT EXISTS \
             trust_fingerprint_on_first_import BOOLEAN NOT NULL DEFAULT false",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issues_hash_pending_idx \
             ON issues (library_id, id) WHERE hash_algorithm = 0",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS issues_hash_pending_idx")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS trust_fingerprint_on_first_import",
        )
        .await?;
        Ok(())
    }
}
