//! `library.auto_convert_cbr_on_scan` — per-library opt-in: when ON (and
//! `allow_archive_writeback` is also ON), the scanner converts each `.cbr`
//! it finds into a sibling `.cbz` in place — keeping the original as
//! `.cbr.bak` — and then ingests the resulting `.cbz` normally. When OFF
//! (default), CBRs stay skipped with an `UnsupportedArchiveFormat` health
//! issue. Reuses the CBR→CBZ machinery built for the page editor
//! (`archive-rewrite-1.0` M6); the dependency on `allow_archive_writeback`
//! mirrors `metadata_writeback_enabled`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    AutoConvertCbrOnScan,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::AutoConvertCbrOnScan)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .drop_column(Libraries::AutoConvertCbrOnScan)
                    .to_owned(),
            )
            .await
    }
}
