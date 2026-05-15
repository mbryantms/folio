//! `progress_records` — authoritative per-(user, issue) reading-state
//! store. The file is named `_placeholder` because the spec's original
//! §9 plan was to replace this with Automerge CRDT documents in Phase
//! 4; that plan was reconsidered and dropped on 2026-05-15 (see spec
//! §9 decision note). The table is permanent; the filename is
//! retained for git history continuity.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum ProgressRecords {
    Table,
    UserId,
    IssueId,
    LastPage,
    Percent,
    Finished,
    UpdatedAt,
    Device,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ProgressRecords::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ProgressRecords::UserId).uuid().not_null())
                    .col(ColumnDef::new(ProgressRecords::IssueId).text().not_null())
                    .col(
                        ColumnDef::new(ProgressRecords::LastPage)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(ProgressRecords::Percent)
                            .double()
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(ProgressRecords::Finished)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ProgressRecords::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(ProgressRecords::Device).text().null())
                    .primary_key(
                        Index::create()
                            .col(ProgressRecords::UserId)
                            .col(ProgressRecords::IssueId),
                    )
                    .to_owned(),
            )
            .await?;

        // Sync delta queries (§5.8). The spec calls for a partial index gated on
        // `updated_at > now() - interval '30 days'`, but Postgres requires the
        // predicate to be IMMUTABLE — `now()` is not. Use a plain composite index
        // instead; revisit (BRIN, or a periodically-refreshed materialized view)
        // if the table grows large enough to feel it.
        manager
            .get_connection()
            .execute(sea_orm::Statement::from_string(
                manager.get_database_backend(),
                "CREATE INDEX IF NOT EXISTS progress_records_user_updated_idx \
                 ON progress_records (user_id, updated_at)"
                    .to_string(),
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ProgressRecords::Table).to_owned())
            .await
    }
}
