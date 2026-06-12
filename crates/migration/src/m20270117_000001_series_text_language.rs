//! Per-series OCR text language (OCR rework 1.0).
//!
//! Before this migration the web client hardcoded `lang: "western"`
//! on every OCR request, making the manga recognizer unreachable
//! from the UI. The OCR handler now resolves the language as
//! `request override → series.text_language → reading_direction
//! == "rtl" ⇒ manga → western`, so the column is the middle layer
//! of that chain.
//!
//! Nullable on purpose: `NULL` = "Auto, infer from reading
//! direction". Resolution happens at request time (no backfill) so
//! later `reading_direction` edits and scanner auto-pinning are
//! respected without a second data migration. Recognized values are
//! `"western"` / `"manga"`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    TextLanguage,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(ColumnDef::new(Series::TextLanguage).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::TextLanguage)
                    .to_owned(),
            )
            .await
    }
}
