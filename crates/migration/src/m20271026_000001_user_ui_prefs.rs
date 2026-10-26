//! Two more design-system preferences next to theme / accent / density
//! (shadcn plan PR 6, docs/dev/shadcn-theming-audit.md):
//!
//! - `corner_radius` — `'sharp' | 'default' | 'round'`; drives the
//!   `--radius` token the whole upstream radius scale derives from.
//! - `motion` — `'system' | 'reduced'`; `reduced` applies the app's
//!   prefers-reduced-motion rules regardless of the OS setting.
//!
//! Both nullable text like the M4 columns: null means the default.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    CornerRadius,
    Motion,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::CornerRadius).text().null())
                    .add_column(ColumnDef::new(Users::Motion).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::CornerRadius)
                    .drop_column(Users::Motion)
                    .to_owned(),
            )
            .await
    }
}
