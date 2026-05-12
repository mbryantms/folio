//! M4: extend `users` with reader defaults, theme, and keybind overrides.
//!
//! New columns are all nullable / defaulted so existing rows keep working
//! without a backfill. App-layer validation gates the textual values
//! (`default_fit_mode`, `default_view_mode`, `theme`, `accent_color`).
//! `keybinds` is a JSON object keyed by action name → key string; an empty
//! object means "use defaults".

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    DefaultFitMode,
    DefaultViewMode,
    DefaultPageStrip,
    Theme,
    AccentColor,
    Density,
    Keybinds,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::DefaultFitMode).text().null())
                    .add_column(ColumnDef::new(Users::DefaultViewMode).text().null())
                    .add_column(
                        ColumnDef::new(Users::DefaultPageStrip)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(ColumnDef::new(Users::Theme).text().null())
                    .add_column(ColumnDef::new(Users::AccentColor).text().null())
                    .add_column(ColumnDef::new(Users::Density).text().null())
                    .add_column(
                        ColumnDef::new(Users::Keybinds)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
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
                    .drop_column(Users::DefaultFitMode)
                    .drop_column(Users::DefaultViewMode)
                    .drop_column(Users::DefaultPageStrip)
                    .drop_column(Users::Theme)
                    .drop_column(Users::AccentColor)
                    .drop_column(Users::Density)
                    .drop_column(Users::Keybinds)
                    .to_owned(),
            )
            .await
    }
}
