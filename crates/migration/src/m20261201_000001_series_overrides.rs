//! Series-level admin overrides: genre + tags.
//!
//! The `series_view` API surfaces aggregated genre / tag CSVs from issues
//! (frequency-ordered). Curators want to set those once at the series
//! level — "this is a Sci-Fi/Action series" — without touching every
//! issue. These nullable columns hold the override; when set, the API
//! prefers them over the per-issue aggregation.
//!
//! Same string shape as the issue columns (CSV / semicolon-separated)
//! so client-side splitters don't fork. `summary` already exists on the
//! series table from the original schema, so it doesn't need a migration.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    Genre,
    Tags,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(ColumnDef::new(Series::Genre).text().null())
                    .add_column(ColumnDef::new(Series::Tags).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::Genre)
                    .drop_column(Series::Tags)
                    .to_owned(),
            )
            .await
    }
}
