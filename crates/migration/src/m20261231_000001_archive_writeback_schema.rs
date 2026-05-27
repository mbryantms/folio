//! Archive writeback schema (M0 of `metadata-sidecar-writeback-1.0`; also
//! consumed by the sister plan `archive-rewrite-1.0`).
//!
//! Two per-library policy toggles + retention knobs + per-issue stamps:
//!
//!   - `library.metadata_writeback_enabled` — opt-in: when ON, provider
//!     apply takes the new XML-first path (worker writes ComicInfo.xml +
//!     MetronInfo.xml, then enqueues a scoped rescan). When OFF (default),
//!     apply takes the legacy DB-direct path. Per the migration plan,
//!     operators flip this per-library after eyeballing one library's
//!     XML output.
//!   - `library.allow_archive_writeback` — hard prerequisite for the toggle
//!     above and for the sister plan's page-edit feature. Default OFF so
//!     no library starts rewriting bytes without explicit consent.
//!   - `library.archive_backup_retain_count` — how many `.bak` siblings to
//!     keep per archive (1..=5). Default 1 (one rollback slot).
//!   - `library.archive_backup_retain_days` — auto-prune `.bak` files older
//!     than this. Default 30; 0 = forever.
//!   - `issue.last_rewrite_at` / `issue.last_rewrite_kind` — bookkeeping
//!     for "Metadata last written from <provider> on <date>" surfaces and
//!     for the audit-log drill-down. Kind is `'sidecar'` or `'edit'`.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    AllowArchiveWriteback,
    MetadataWritebackEnabled,
    ArchiveBackupRetainCount,
    ArchiveBackupRetainDays,
}

#[derive(Iden)]
enum Issues {
    Table,
    LastRewriteAt,
    LastRewriteKind,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::AllowArchiveWriteback)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::MetadataWritebackEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::ArchiveBackupRetainCount)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::ArchiveBackupRetainDays)
                            .integer()
                            .not_null()
                            .default(30),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(
                        ColumnDef::new(Issues::LastRewriteAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Issues::LastRewriteKind).string().null())
                    .to_owned(),
            )
            .await?;

        // CHECK constraints enforced at DB level so a hand-crafted PATCH
        // (or future code path that bypasses the validator) can't store
        // out-of-range values. Backend handlers also validate the same
        // ranges to surface 400s with a friendly message instead of
        // letting the DB error out — defense in depth.
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE libraries \
             ADD CONSTRAINT libraries_archive_backup_retain_count_chk \
             CHECK (archive_backup_retain_count BETWEEN 1 AND 5)",
        ))
        .await?;
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE libraries \
             ADD CONSTRAINT libraries_archive_backup_retain_days_chk \
             CHECK (archive_backup_retain_days >= 0)",
        ))
        .await?;
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE issues \
             ADD CONSTRAINT issues_last_rewrite_kind_chk \
             CHECK (last_rewrite_kind IS NULL \
                    OR last_rewrite_kind IN ('sidecar', 'edit'))",
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for stmt in [
            "ALTER TABLE issues DROP CONSTRAINT IF EXISTS issues_last_rewrite_kind_chk",
            "ALTER TABLE libraries DROP CONSTRAINT IF EXISTS libraries_archive_backup_retain_days_chk",
            "ALTER TABLE libraries DROP CONSTRAINT IF EXISTS libraries_archive_backup_retain_count_chk",
        ] {
            db.execute(Statement::from_string(db.get_database_backend(), stmt))
                .await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::LastRewriteKind)
                    .drop_column(Issues::LastRewriteAt)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .drop_column(Libraries::ArchiveBackupRetainDays)
                    .drop_column(Libraries::ArchiveBackupRetainCount)
                    .drop_column(Libraries::MetadataWritebackEnabled)
                    .drop_column(Libraries::AllowArchiveWriteback)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
