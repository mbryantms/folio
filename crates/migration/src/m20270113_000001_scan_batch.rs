//! Observability split M5 — scan-all batch grouping.
//!
//! A `scan_batch` row groups the per-library `scan_runs` that one "Scan all"
//! action enqueues, so the admin Scan-all dashboard can show aggregate live
//! progress + a post-run roll-up across every library in the batch.
//!
//! Three schema changes, all here:
//!   1. Create `scan_batch`.
//!   2. Add `scan_runs.batch_id` (nullable FK → `scan_batch`, SET NULL on
//!      delete) so a single scan still works with no batch.
//!   3. Add the FK on `library_events.batch_id` that M1
//!      (`m20270112_000001_library_event_log`) left as a plain column — the
//!      parent table didn't exist yet. SET NULL on delete: pruning a batch
//!      must not cascade-delete the durable manifest rows.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum ScanBatch {
    Table,
    Id,
    Kind,
    ActorId,
    Force,
    StartedAt,
    EndedAt,
    LibraryCount,
    State,
}

#[derive(Iden)]
enum ScanRuns {
    Table,
    BatchId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ScanBatch::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ScanBatch::Id).uuid().not_null().primary_key())
                    // Discriminator for future batch triggers; today always
                    // `scan_all`. Free text (no CHECK) so new kinds don't need
                    // a migration.
                    .col(ColumnDef::new(ScanBatch::Kind).text().not_null())
                    // Who triggered it. NULL for system/cron-triggered batches.
                    // No FK — mirrors `audit_log.actor_id` (an actor row may be
                    // deleted without rewriting history).
                    .col(ColumnDef::new(ScanBatch::ActorId).uuid().null())
                    .col(ColumnDef::new(ScanBatch::Force).boolean().not_null())
                    .col(
                        ColumnDef::new(ScanBatch::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ScanBatch::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ScanBatch::LibraryCount)
                            .integer()
                            .not_null(),
                    )
                    // `running` | `complete` | `partial_failed` | `failed`,
                    // derived as the member runs finish.
                    .col(ColumnDef::new(ScanBatch::State).text().not_null())
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();

        conn.execute_unprepared(
            "ALTER TABLE scan_batch ADD CONSTRAINT scan_batch_state_chk \
             CHECK (state IN ('running','complete','partial_failed','failed'))",
        )
        .await?;

        // Recent-batches list for the dashboard.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS scan_batch_started_idx \
             ON scan_batch(started_at DESC)",
        )
        .await?;

        // 2. scan_runs.batch_id → scan_batch(id)
        manager
            .alter_table(
                Table::alter()
                    .table(ScanRuns::Table)
                    .add_column(ColumnDef::new(ScanRuns::BatchId).uuid().null())
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("scan_runs_batch_fk")
                            .from_tbl(ScanRuns::Table)
                            .from_col(ScanRuns::BatchId)
                            .to_tbl(ScanBatch::Table)
                            .to_col(ScanBatch::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS scan_runs_batch_idx \
             ON scan_runs(batch_id) WHERE batch_id IS NOT NULL",
        )
        .await?;

        // 3. library_events.batch_id FK (column added in M1).
        conn.execute_unprepared(
            "ALTER TABLE library_events \
             ADD CONSTRAINT library_events_batch_fk \
             FOREIGN KEY (batch_id) REFERENCES scan_batch(id) ON DELETE SET NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE library_events DROP CONSTRAINT IF EXISTS library_events_batch_fk",
        )
        .await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS scan_runs_batch_idx")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ScanRuns::Table)
                    .drop_foreign_key(Alias::new("scan_runs_batch_fk"))
                    .drop_column(ScanRuns::BatchId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ScanBatch::Table).to_owned())
            .await
    }
}
