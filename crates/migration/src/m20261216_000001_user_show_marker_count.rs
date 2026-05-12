//! Markers + Collections M8 polish: opt-in toggle for the Bookmarks
//! sidebar count badge.
//!
//! The Bookmarks row has been rendering a per-user marker count since
//! M8, but several users prefer a quiet sidebar. This flag flips the
//! default to "off"; users who want the badge can re-enable it from
//! /settings/account.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    ShowMarkerCount,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::ShowMarkerCount)
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
                    .table(Users::Table)
                    .drop_column(Users::ShowMarkerCount)
                    .to_owned(),
            )
            .await
    }
}
