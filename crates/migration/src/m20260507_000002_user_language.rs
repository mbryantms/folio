//! M3 of the human-readable-URLs plan: add `users.language` so locale moves
//! out of the URL into a per-user preference. Default `en` matches the
//! single supported locale in `web/i18n/request.ts`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    Language,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::Language)
                            .string_len(8)
                            .not_null()
                            .default("en"),
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
                    .drop_column(Users::Language)
                    .to_owned(),
            )
            .await
    }
}
