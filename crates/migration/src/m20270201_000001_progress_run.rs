//! Adds `progress_records.run INTEGER NOT NULL DEFAULT 0` — the
//! "reading run" counter behind the cross-device conflict rule
//! (roadmap WP-1.3, audit UX-1).
//!
//! Within one run the server keeps the furthest page an implicit
//! per-page write reports; an explicit "start re-read" opens run `n+1`
//! at page 0, and any write still tagged with an older run is ignored,
//! so a device left open on the previous read can never drag the new
//! one backwards. Existing rows start at run 0 (their first read).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE progress_records \
                 ADD COLUMN IF NOT EXISTS run INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE progress_records DROP COLUMN IF EXISTS run")
            .await?;
        Ok(())
    }
}
