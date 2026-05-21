//! Add `log_widgets` — per-user customizable widget grid for the
//! Reading Log page (`/log`).
//!
//! Each row is one widget the user has on their log page. `kind` is
//! a string discriminator (e.g. `'chrono_feed'`, `'heatmap'`) that
//! drives the renderer + the shape of `config`. `position` is a
//! 0-based dense rank so the page can render the grid in order
//! without a secondary sort; reorder mutations rewrite the column.
//!
//! Default layout is **not** seeded by the migration. The
//! `GET /me/log/widgets` handler inserts the four M2 defaults
//! (chrono_feed + stats_hero + heatmap + top_creators) the first
//! time a user touches the endpoint — same auto-seed pattern as
//! Want-to-Read collections. That keeps the migration cheap on
//! existing installs (no per-user write) and means a "Reset to
//! defaults" path can re-trigger the same code.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum LogWidgets {
    Table,
    Id,
    UserId,
    Kind,
    Position,
    Config,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LogWidgets::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LogWidgets::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LogWidgets::UserId).uuid().not_null())
                    .col(ColumnDef::new(LogWidgets::Kind).text().not_null())
                    .col(ColumnDef::new(LogWidgets::Position).integer().not_null())
                    .col(
                        ColumnDef::new(LogWidgets::Config)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(
                        ColumnDef::new(LogWidgets::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(LogWidgets::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(LogWidgets::Table, LogWidgets::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("log_widgets_user_position")
                    .table(LogWidgets::Table)
                    .col(LogWidgets::UserId)
                    .col(LogWidgets::Position)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LogWidgets::Table).to_owned())
            .await
    }
}
