//! OPDS sync cleanup — per-entity opt-outs for the default up-next
//! reorder behavior.
//!
//! M2 of `opds-sync-cleanup-1.0` makes "up-next moves to position 0"
//! the default for every reading-sequence feed (series, CBL, WTR,
//! collections). Three new `preserve_canonical_order` boolean columns
//! let curators opt their lists out and keep strict canonical order
//! regardless of the caller's progress. WTR is system-owned (single
//! per user), so its toggle lives on `users` instead.
//!
//! All defaults preserve the new behavior: reorder ON, opt-out OFF.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    PreserveCanonicalOrder,
}

#[derive(Iden)]
enum CblLists {
    Table,
    PreserveCanonicalOrder,
}

#[derive(Iden)]
enum SavedViews {
    Table,
    PreserveCanonicalOrder,
}

#[derive(Iden)]
enum Users {
    Table,
    OpdsWtrReorder,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(
                        ColumnDef::new(Series::PreserveCanonicalOrder)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CblLists::Table)
                    .add_column(
                        ColumnDef::new(CblLists::PreserveCanonicalOrder)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .add_column(
                        ColumnDef::new(SavedViews::PreserveCanonicalOrder)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::OpdsWtrReorder)
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
                    .drop_column(Users::OpdsWtrReorder)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .drop_column(SavedViews::PreserveCanonicalOrder)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CblLists::Table)
                    .drop_column(CblLists::PreserveCanonicalOrder)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::PreserveCanonicalOrder)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
