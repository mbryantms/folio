//! `library.auto_convert_cb7_on_scan` — per-library opt-in (WP-6.5): when ON
//! (and `allow_archive_writeback` is also ON, on a writable mount), the
//! scanner converts each `.cb7` (7z) it finds into a sibling `.cbz` in place
//! — keeping the original as `.cb7.bak` — and then ingests the resulting
//! `.cbz` normally. When OFF (default), CB7s stay skipped with an
//! `UnsupportedArchiveFormat` health issue, exactly as before CB7 support.
//!
//! A separate flag from `auto_convert_cbr_on_scan` on purpose: a library that
//! opted into rewriting its RAR files has not thereby consented to Folio
//! rewriting its 7z files, which were never touched before this migration.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    AutoConvertCb7OnScan,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::AutoConvertCb7OnScan)
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
                    .drop_column(Libraries::AutoConvertCb7OnScan)
                    .to_owned(),
            )
            .await
    }
}
