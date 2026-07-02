//! Backfill `library_events.batch_id` from the owning scan run.
//!
//! The event writer's `with_batch` stamp shipped in M5 of the
//! observability split but was never called — every event row landed
//! with `batch_id = NULL`, leaving the scan-batch "Changes" manifest
//! permanently empty. The forward fix stamps new events at write time;
//! this recovers history via the `scan_run_id → scan_runs.batch_id`
//! join, which is exact (a run belongs to at most one batch).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"UPDATE library_events le
                  SET batch_id = sr.batch_id
                  FROM scan_runs sr
                  WHERE le.scan_run_id = sr.id
                    AND le.batch_id IS NULL
                    AND sr.batch_id IS NOT NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data backfill: rows stamped here are indistinguishable from rows
        // stamped at write time, so a selective revert is impossible and a
        // blanket NULL-out would destroy real data. Intentional no-op.
        Ok(())
    }
}
