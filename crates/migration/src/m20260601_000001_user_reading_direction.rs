//! Phase 3: per-user default reading direction (§7.3, spec §19 line 1409).
//!
//! Stored as plain `TEXT NULL`. No CHECK constraint — accept any string for
//! forward-compat with TTB (top-to-bottom) and any future direction tokens
//! the spec might add. The application layer validates against the active
//! enum on read.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    DefaultReadingDirection,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::DefaultReadingDirection).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::DefaultReadingDirection)
                    .to_owned(),
            )
            .await
    }
}
