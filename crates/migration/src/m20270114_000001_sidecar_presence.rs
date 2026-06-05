//! Persist which metadata sidecar files the scanner found, so the issue
//! Metadata tab can report source-file presence authoritatively instead of
//! inferring it.
//!
//!   - `issues.metroninfo_present` — was a `MetronInfo.xml` present inside the
//!     archive at scan time? (ComicInfo presence stays inferred from the
//!     existing `comic_info_raw` column, which is already authoritative.)
//!   - `series.series_json_present` — was a Mylar3 `series.json` present in the
//!     series folder at scan time? (Per-folder file, so it lives on the series
//!     row, not per issue.)
//!
//! Both are **nullable with no default**: `NULL` means "scanned before this
//! column existed — unknown until the next rescan", which the UI renders
//! distinctly from a definite "absent" (`FALSE`). A rescan backfills them.

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
            "ALTER TABLE issues ADD COLUMN metroninfo_present BOOLEAN",
        ))
        .await?;
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE series ADD COLUMN series_json_present BOOLEAN",
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE issues DROP COLUMN metroninfo_present",
        ))
        .await?;
        db.execute(Statement::from_string(
            db.get_database_backend(),
            "ALTER TABLE series DROP COLUMN series_json_present",
        ))
        .await?;
        Ok(())
    }
}
