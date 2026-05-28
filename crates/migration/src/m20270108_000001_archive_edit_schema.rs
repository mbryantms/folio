//! Archive page-edit schema (M0 of `archive-rewrite-1.0`).
//!
//! The sister plan `metadata-sidecar-writeback-1.0` already shipped the
//! shared writeback columns (`allow_archive_writeback`,
//! `archive_backup_retain_count/days`, `issue.last_rewrite_*`). This
//! migration adds the two knobs unique to the page-editor feature:
//!
//!   - `library.archive_writeback_jpeg_quality` — encoder quality used
//!     when the editor re-encodes a rotated / replaced JPEG page. Range
//!     60..=100; default 92. (PNG / WebP pages stay lossless.) Shared
//!     with the sidecar plan's image needs if any later arise.
//!   - `library.cbr_convert_confirmed_at` — first-time gate for the
//!     CBR→CBZ conversion confirm dialog (M6). NULL until the operator
//!     acknowledges the conversion once for the library; thereafter
//!     subsequent CBR edits don't re-prompt.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    ArchiveWritebackJpegQuality,
    CbrConvertConfirmedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::ArchiveWritebackJpegQuality)
                            .integer()
                            .not_null()
                            .default(92),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::CbrConvertConfirmedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // DB-level guard mirrors the handler-side validator (defense in
        // depth — a hand-edited row can't store an out-of-range quality).
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE libraries \
             ADD CONSTRAINT libraries_archive_writeback_jpeg_quality_chk \
             CHECK (archive_writeback_jpeg_quality BETWEEN 60 AND 100)",
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE libraries \
             DROP CONSTRAINT IF EXISTS libraries_archive_writeback_jpeg_quality_chk",
        ))
        .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .drop_column(Libraries::CbrConvertConfirmedAt)
                    .drop_column(Libraries::ArchiveWritebackJpegQuality)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
