//! Sticky manual override for `series.status`.
//!
//! Once a user PATCHes `/series/{slug}` with an explicit `status`
//! (e.g. they set `"hiatus"` or `"cancelled"` — values the scanner
//! can't infer), we record the timestamp here. The post-scan
//! reconciliation step skips the status write for any series with
//! `status_user_set_at IS NOT NULL`, so manual edits aren't clobbered
//! on the next library scan. The total_issues refresh is independent
//! of this flag — counts still flow through so the Complete /
//! Incomplete UI stays accurate even on user-pinned series.
//!
//! Modeled on the existing `series.match_key` sticky-override pattern
//! in `library/identity.rs`. Backfill: nothing — the column lands
//! NULL on existing rows, which is exactly the "scanner may overwrite"
//! state we want for series whose status hasn't been hand-tuned.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    StatusUserSetAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(
                        ColumnDef::new(Series::StatusUserSetAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::StatusUserSetAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
