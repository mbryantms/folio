//! Per-series reading direction override (M2 of
//! `manga-and-bulk-metadata-1.0`).
//!
//! Before this migration, the reader's resolution chain was
//! ComicInfo `<Manga>` → user pref → LTR. There was no series-level
//! knob, so setting a whole series as RTL required either editing
//! every CBZ's ComicInfo.xml or flipping the Manga dropdown on each
//! issue drawer. M1 added `library.default_reading_direction` to the
//! chain; this migration adds the **series-level** layer between
//! ComicInfo and the user pref.
//!
//! Nullable on purpose: `NULL` = "Auto, inherit from user/library".
//! Existing rows default to `NULL` so the change is invisible until
//! an admin or the M3 scanner heuristic actually pins a value.
//! Recognized values are `"ltr"` / `"rtl"`; the schema also tolerates
//! `"ttb"` for future webtoon support per R6.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    ReadingDirection,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(ColumnDef::new(Series::ReadingDirection).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::ReadingDirection)
                    .to_owned(),
            )
            .await
    }
}
