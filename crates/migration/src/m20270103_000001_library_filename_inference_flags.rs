//! Matching-accuracy-1.0 M7 — per-library filename-inference toggles.
//!
//! Adds two BOOLEAN columns to `libraries`:
//!
//! - `filename_ignore_leading_numbers` — drop the leading numeric
//!   token from a filename before parsing the series. Closes the
//!   common Mylar-style numbering case where files like
//!   `001 - Saga (2012).cbz` would otherwise parse as series=`001`
//!   instead of `Saga`.
//!
//! - `filename_assume_issue_one` — when no issue number is detected
//!   in the filename, infer `1`. Closes the one-shot / first-issue
//!   case where the operator's curation strips the `#1` because
//!   "everyone knows it's a #1".
//!
//! Both default OFF — they're per-library overrides, not global
//! policy, and the wrong default would mis-classify the other
//! population.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS filename_ignore_leading_numbers BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS filename_assume_issue_one BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS filename_ignore_leading_numbers",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS filename_assume_issue_one",
        )
        .await?;
        Ok(())
    }
}
