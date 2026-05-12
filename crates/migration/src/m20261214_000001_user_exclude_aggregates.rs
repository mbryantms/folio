//! Stats v2: opt-out flag for server-wide reading aggregates.
//!
//! When a user sets this to true, the admin dashboard's `/admin/stats/*`
//! endpoints exclude that user's `reading_sessions` from system-wide rollups
//! (top series, reads-per-day, DAU/WAU/MAU, completion funnels, etc.). The
//! user's own `/me/reading-stats` page still shows the data — this controls
//! visibility into aggregates, not capture.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    ExcludeFromAggregates,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::ExcludeFromAggregates)
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
                    .drop_column(Users::ExcludeFromAggregates)
                    .to_owned(),
            )
            .await
    }
}
