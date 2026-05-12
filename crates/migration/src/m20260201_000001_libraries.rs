//! Library and core scan-state tables (Phase 1a).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    Id,
    Name,
    RootPath,
    DefaultLanguage,
    DefaultReadingDirection,
    DedupeByContent,
    ScanScheduleCron,
    CreatedAt,
    UpdatedAt,
    LastScanAt,
}

#[derive(Iden)]
enum ScanRuns {
    Table,
    Id,
    LibraryId,
    State,
    StartedAt,
    EndedAt,
    Stats,
    Error,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Libraries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Libraries::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Libraries::Name).text().not_null())
                    .col(
                        ColumnDef::new(Libraries::RootPath)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Libraries::DefaultLanguage)
                            .text()
                            .not_null()
                            .default("eng"),
                    )
                    .col(
                        ColumnDef::new(Libraries::DefaultReadingDirection)
                            .text()
                            .not_null()
                            .default("ltr"),
                    )
                    .col(
                        ColumnDef::new(Libraries::DedupeByContent)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Libraries::ScanScheduleCron).text().null())
                    .col(
                        ColumnDef::new(Libraries::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Libraries::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Libraries::LastScanAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // scan_runs — checkpoints + history per library (§B5 of review).
        manager
            .create_table(
                Table::create()
                    .table(ScanRuns::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ScanRuns::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(ScanRuns::LibraryId).uuid().not_null())
                    .col(ColumnDef::new(ScanRuns::State).text().not_null())
                    .col(
                        ColumnDef::new(ScanRuns::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ScanRuns::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ScanRuns::Stats)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(ScanRuns::Error).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(ScanRuns::Table, ScanRuns::LibraryId)
                            .to(Libraries::Table, Libraries::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("scan_runs_library_started_idx")
                    .table(ScanRuns::Table)
                    .col(ScanRuns::LibraryId)
                    .col(ScanRuns::StartedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ScanRuns::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Libraries::Table).to_owned())
            .await
    }
}
