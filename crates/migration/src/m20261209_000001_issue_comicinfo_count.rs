//! Per-issue `<Count>` capture so the scanner can MAX-reduce a
//! per-series total without re-reading every CBZ on every scan.
//!
//! `series.total_issues` was already populated from the FIRST issue's
//! ComicInfo at series creation, but never refreshed. Storing the
//! Count on each issue lets the post-scan reconcile step compute
//! `max(comicinfo_count) WHERE series_id = ...` — robust to one issue
//! lacking the field, surviving relaunches, and refreshable on every
//! re-scan without parsing archives again.
//!
//! Backfill: nothing here. New column lands NULL; the next scan
//! populates it lazily as it touches each issue.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Issues {
    Table,
    ComicinfoCount,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(ColumnDef::new(Issues::ComicinfoCount).integer().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::ComicinfoCount)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
