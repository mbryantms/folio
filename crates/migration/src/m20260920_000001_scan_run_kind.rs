//! Scan-run discriminator + targets.
//!
//! The History tab on each library shows rows from `scan_runs`. Three kinds
//! of scan write to that table — full library scans (`POST /libraries/{id}/scan`),
//! per-series scans (`POST /series/{id}/scan` and the file-watch worker),
//! and per-issue scans (`POST /issues/{id}/scan`). Without a discriminator,
//! the UI can't tell them apart, so admins see a flat list with no way to
//! filter.
//!
//! New columns:
//!   - `kind text NOT NULL DEFAULT 'library'` — `'library' | 'series' | 'issue'`.
//!     Default backfills existing rows as full-library scans, which is
//!     correct for the only kind that existed before this migration.
//!   - `series_id uuid NULL` — the series that was scanned (kind in
//!     {'series','issue'}). NULL on full-library scans.
//!   - `issue_id text NULL` — the specific issue that triggered an issue
//!     scan. The scanner's actual unit of work is still the parent series
//!     folder; this column records *who clicked the button*. NULL on
//!     library and series scans.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum ScanRuns {
    Table,
    Kind,
    SeriesId,
    IssueId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ScanRuns::Table)
                    .add_column(
                        ColumnDef::new(ScanRuns::Kind)
                            .text()
                            .not_null()
                            .default("library"),
                    )
                    .add_column(ColumnDef::new(ScanRuns::SeriesId).uuid().null())
                    .add_column(ColumnDef::new(ScanRuns::IssueId).text().null())
                    .to_owned(),
            )
            .await?;

        // Index for the series-scoped queries the future Series → History
        // affordance will use; cheap to add now, cheaper than re-indexing
        // a full table later.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS scan_runs_series_idx \
                 ON scan_runs(series_id) \
                 WHERE series_id IS NOT NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS scan_runs_series_idx")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ScanRuns::Table)
                    .drop_column(ScanRuns::Kind)
                    .drop_column(ScanRuns::SeriesId)
                    .drop_column(ScanRuns::IssueId)
                    .to_owned(),
            )
            .await
    }
}
