//! Metadata bulk-fetch batch grouping (refine-bulk-metadata M1).
//!
//! A `metadata_batch` row groups the per-series/per-issue `metadata_run` rows
//! that one bulk fetch ("fetch all issues in a series", "fetch a saved view",
//! a user-triggered library refresh) enqueues, so the admin Review queue can
//! show aggregate live progress + a consolidated accept surface.
//!
//! Mirrors `scan_batch` + `scan_run.batch_id` exactly:
//!   1. Create `metadata_batch`.
//!   2. Add `metadata_run.batch_id` (nullable FK → `metadata_batch`, SET NULL
//!      on delete) so a single per-entity run still works with no batch.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum MetadataBatch {
    Table,
    Id,
    LibraryId,
    Scope,
    TriggerKind,
    Status,
    ItemsTotal,
    CreatedBy,
    CreatedAt,
    EndedAt,
}

#[derive(Iden)]
enum MetadataRun {
    Table,
    BatchId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MetadataBatch::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MetadataBatch::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    // NULL for cross-library saved-view batches.
                    .col(ColumnDef::new(MetadataBatch::LibraryId).uuid().null())
                    // 'series_issues' | 'saved_view' | 'library_refresh'.
                    // Free text (no CHECK) so new kinds don't need a migration.
                    .col(ColumnDef::new(MetadataBatch::Scope).text().not_null())
                    .col(ColumnDef::new(MetadataBatch::TriggerKind).text().not_null())
                    // 'running' | 'completed' | 'partial_failed' |
                    // 'awaiting_quota', derived from member-run aggregate.
                    .col(ColumnDef::new(MetadataBatch::Status).text().not_null())
                    // Child runs enqueued at fan-out (the only stored count;
                    // progress is otherwise a GROUP BY over member runs).
                    .col(ColumnDef::new(MetadataBatch::ItemsTotal).integer().not_null())
                    // Who triggered it. No FK — mirrors audit_log.actor_id.
                    .col(ColumnDef::new(MetadataBatch::CreatedBy).uuid().null())
                    .col(
                        ColumnDef::new(MetadataBatch::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(MetadataBatch::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();

        // Recent-batches list for the Review tab's picker.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_batch_created_idx \
             ON metadata_batch(created_at DESC)",
        )
        .await?;

        // metadata_run.batch_id → metadata_batch(id)
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataRun::Table)
                    .add_column(ColumnDef::new(MetadataRun::BatchId).uuid().null())
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("metadata_run_batch_fk")
                            .from_tbl(MetadataRun::Table)
                            .from_col(MetadataRun::BatchId)
                            .to_tbl(MetadataBatch::Table)
                            .to_col(MetadataBatch::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        // Batch-status aggregation + Review-queue child list are index hits.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_run_batch_idx \
             ON metadata_run(batch_id) WHERE batch_id IS NOT NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP INDEX IF EXISTS metadata_run_batch_idx")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataRun::Table)
                    .drop_foreign_key(Alias::new("metadata_run_batch_fk"))
                    .drop_column(MetadataRun::BatchId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(MetadataBatch::Table).to_owned())
            .await
    }
}
