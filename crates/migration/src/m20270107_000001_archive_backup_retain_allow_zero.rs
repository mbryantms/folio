//! Relax `libraries_archive_backup_retain_count_chk` from
//! `BETWEEN 1 AND 5` to `BETWEEN 0 AND 5`.
//!
//! `0` now means "validated overwrite, no `.bak`": the sidecar rewrite
//! validates the freshly-built archive (all original entries preserved +
//! both sidecars present + parseable) BEFORE the atomic swap, so the
//! original is never replaced by a corrupt rewrite. This eliminates the
//! transient ~2x library-size doubling from retaining a full-size `.bak`
//! per rewritten archive. `1..=5` still keep that many rollback slots
//! for operators who want them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             DROP CONSTRAINT IF EXISTS libraries_archive_backup_retain_count_chk",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD CONSTRAINT libraries_archive_backup_retain_count_chk \
             CHECK (archive_backup_retain_count BETWEEN 0 AND 5)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             DROP CONSTRAINT IF EXISTS libraries_archive_backup_retain_count_chk",
        )
        .await?;
        // Best-effort restore of the original range. Rows already storing
        // 0 would violate it, so clamp them to 1 first.
        db.execute_unprepared(
            "UPDATE libraries SET archive_backup_retain_count = 1 \
             WHERE archive_backup_retain_count = 0",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD CONSTRAINT libraries_archive_backup_retain_count_chk \
             CHECK (archive_backup_retain_count BETWEEN 1 AND 5)",
        )
        .await?;
        Ok(())
    }
}
