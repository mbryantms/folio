//! Requests to add something to Mylar (add-to-Mylar 1.0).
//!
//! An admin picks missing reading-list entries and asks Folio to add them
//! to Mylar. Each target gets one row here; the once-a-minute sweep
//! (`mylar_control::sweep`) carries the rows out against Mylar's API and
//! records the outcome.
//!
//! - `cv_issue_id IS NULL` — add the series (ComicVine volume) to Mylar's
//!   watchlist.
//! - `cv_issue_id` set — mark that one issue Wanted. Mylar only knows an
//!   issue once it has imported the series, so these rows wait.
//!
//! One row per target (`NULLS NOT DISTINCT`): asking again for a failed
//! target resets its row instead of adding a second.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS mylar_add_request (
                id               UUID PRIMARY KEY,
                cv_series_id     BIGINT NOT NULL,
                cv_issue_id      BIGINT NULL,
                series_name      TEXT NOT NULL,
                issue_number     TEXT NULL,
                status           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'done', 'failed')),
                attempts         INTEGER NOT NULL DEFAULT 0,
                last_error       TEXT NULL,
                next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                requested_by     UUID NULL REFERENCES users(id) ON DELETE SET NULL,
                created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                CONSTRAINT mylar_add_request_target_key
                    UNIQUE NULLS NOT DISTINCT (cv_series_id, cv_issue_id)
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS mylar_add_request_due_idx
                ON mylar_add_request (next_attempt_at)
                WHERE status = 'pending'",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS mylar_add_request")
            .await?;
        Ok(())
    }
}
