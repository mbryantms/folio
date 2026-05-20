//! Add `users.default_page_animation` — per-user reader preference
//! for the visual transition when navigating to the next or previous
//! page. Values are validated by the PATCH /me/preferences handler;
//! the column itself stores any string so adding new variants (fade,
//! curl, …) later doesn't require another migration.
//!
//! Default: NULL (reader falls back to its built-in default of
//! `'slide'`). NULL was chosen over a populated default so the
//! reader can change the built-in default without rewriting every
//! existing row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    DefaultPageAnimation,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::DefaultPageAnimation)
                            .text()
                            .null(),
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
                    .drop_column(Users::DefaultPageAnimation)
                    .to_owned(),
            )
            .await
    }
}
