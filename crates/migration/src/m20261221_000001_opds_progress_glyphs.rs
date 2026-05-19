//! OPDS sync cleanup — per-user opt-out for inline progress glyphs +
//! `(N / M)` page-count suffix on OPDS entry titles.
//!
//! M3 of `opds-sync-cleanup-1.0` annotates every reading-sequence entry
//! with `◯`, `◐`, or `●` plus a `(N / M)` page-count suffix so OPDS
//! clients that ignore the PSE `pse:last_read` attribute (Komga, KOReader,
//! older Tachiyomi) still see "what's left" at a glance. The flag lets
//! a user disable the visual clutter if their client renders progress
//! natively.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    OpdsProgressGlyphs,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::OpdsProgressGlyphs)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::OpdsProgressGlyphs)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
