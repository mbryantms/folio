//! Make per-series metadata auto-sync **opt-in**.
//!
//! `series.metadata_sync_paused` previously defaulted to FALSE, so the
//! weekly refresh cron (when enabled) would touch *every* active series.
//! Metadata for the bulk of a library rarely changes — only new or
//! popular issues do as the upstream community fills it in — so blanket
//! auto-sync wastes provider quota. Flip the model to opt-in:
//!
//!   - New series default to `metadata_sync_paused = TRUE` (auto-sync off).
//!   - Existing series are reset to TRUE; operators turn auto-sync back
//!     on for the specific series they care about (per-series toggle on
//!     the series Details tab + the new "Auto-synced" admin tab).
//!
//! The global weekly-cron toggle already defaults off; this just makes
//! the per-series default conservative too.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE series ALTER COLUMN metadata_sync_paused SET DEFAULT TRUE",
        ))
        .await?;
        // One-time reset: existing series become opt-in too. Operators
        // re-enable auto-sync per series afterwards.
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "UPDATE series SET metadata_sync_paused = TRUE WHERE metadata_sync_paused = FALSE",
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Revert the column default only — the one-time data reset isn't
        // meaningfully reversible (we can't know which series were
        // originally opted in).
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE series ALTER COLUMN metadata_sync_paused SET DEFAULT FALSE",
        ))
        .await?;
        Ok(())
    }
}
