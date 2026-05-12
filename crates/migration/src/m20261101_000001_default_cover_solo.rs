//! Add `users.default_cover_solo` — per-user default for the reader's
//! "first page is cover (always solo)" toggle in double-page view. Default
//! `true` matches the printed-comic convention; per-series localStorage
//! still wins at runtime.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    DefaultCoverSolo,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::DefaultCoverSolo)
                            .boolean()
                            .not_null()
                            .default(true),
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
                    .drop_column(Users::DefaultCoverSolo)
                    .to_owned(),
            )
            .await
    }
}
