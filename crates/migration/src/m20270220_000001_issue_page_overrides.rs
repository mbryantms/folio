//! `issue_page_overrides` — per-user, per-issue manual spread controls
//! for the double-page reader (roadmap WP-4.3, audit R18 / UX-2).
//!
//! Automatic pairing depends on ComicInfo `<Page DoublePage>` or the
//! page's aspect ratio; offset scans and unflagged spreads pair the
//! wrong pages. One row per `(user, issue)` records the reader's
//! corrections, and wins over both signals:
//!
//! - `spread_pages`  — page indices forced to render solo as a spread.
//! - `single_pages`  — page indices forced to pair as ordinary pages
//!   even when flagged `DoublePage` or landscape.
//! - `shift_pairing` — shift the pairing parity by one page (an offset
//!   scan where every pair is off by one).
//!
//! Both page lists are JSONB arrays of 0-based indices (the server
//! normalizes them sorted + de-duplicated + disjoint). Deleting the row
//! resets the issue to automatic pairing. Rows cascade with the user
//! and the issue.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS issue_page_overrides (
                    user_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    issue_id       TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                    shift_pairing  BOOLEAN NOT NULL DEFAULT FALSE,
                    spread_pages   JSONB NOT NULL DEFAULT '[]'::jsonb,
                    single_pages   JSONB NOT NULL DEFAULT '[]'::jsonb,
                    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    PRIMARY KEY (user_id, issue_id)
                );
                CREATE INDEX IF NOT EXISTS issue_page_overrides_issue_idx
                    ON issue_page_overrides (issue_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS issue_page_overrides")
            .await?;
        Ok(())
    }
}
