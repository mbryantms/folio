//! Writeback hardening (roadmap WP-2.6, audit DI-11 / DI-14).
//!
//!   - `issues.metron_info_raw JSONB` — the parsed `MetronInfo.xml` struct
//!     (`serde_json::to_value(&MetronInfo)`), mirroring `comic_info_raw`.
//!     Its `raw` map carries the top-level elements Folio doesn't model,
//!     so the sidecar composer can pass them through on the next rewrite
//!     instead of deleting them. NULL = no MetronInfo.xml at the last
//!     scan, or scanned before this column existed (a rescan backfills).
//!   - `issues.last_sidecar_rewrite_at TIMESTAMPTZ` — stamped **only** by
//!     the sidecar rewrite. `last_rewrite_at` is also bumped by page
//!     edits and restores, which used to hide user-edit drift after a
//!     page edit (the drift predicate compared pins against it).
//!     Backfilled from `last_rewrite_at` where the last rewrite was a
//!     sidecar one, so existing drift state is unchanged.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE issues ADD COLUMN IF NOT EXISTS metron_info_raw JSONB")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE issues ADD COLUMN IF NOT EXISTS last_sidecar_rewrite_at TIMESTAMPTZ",
        )
        .await?;
        db.execute_unprepared(
            "UPDATE issues SET last_sidecar_rewrite_at = last_rewrite_at \
             WHERE last_rewrite_kind = 'sidecar' AND last_sidecar_rewrite_at IS NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE issues DROP COLUMN IF EXISTS last_sidecar_rewrite_at")
            .await?;
        db.execute_unprepared("ALTER TABLE issues DROP COLUMN IF EXISTS metron_info_raw")
            .await?;
        Ok(())
    }
}
