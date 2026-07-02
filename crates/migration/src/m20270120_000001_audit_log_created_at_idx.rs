//! PERF-5: index `audit_log(created_at DESC)`.
//!
//! `audit_log` is append-only and never pruned, so it only grows. The existing
//! composites all lead with `actor_id` / `target_id` / `action`, which serve
//! the per-actor / per-target / per-action activity feeds but cannot answer the
//! *unfiltered* reverse-chronological feed (`ORDER BY created_at DESC LIMIT n`)
//! — that degrades to a Seq Scan + Sort as the table grows. A dedicated
//! `created_at` index fixes it.
//!
//! Created non-`CONCURRENTLY` (consistent with every other index in this crate:
//! sea-orm wraps each migration in a transaction, inside which
//! `CREATE INDEX CONCURRENTLY` is illegal). `audit_log` is append-only with a
//! low write rate, so the brief build-time lock is acceptable at self-host
//! scale.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS audit_log_created_at_idx \
                 ON audit_log (created_at DESC)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS audit_log_created_at_idx")
            .await?;
        Ok(())
    }
}
