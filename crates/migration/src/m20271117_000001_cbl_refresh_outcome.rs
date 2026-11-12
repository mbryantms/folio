//! Outcome of a reading list's last refresh attempt.
//!
//! A list whose source keeps failing used to show only an old "last
//! refreshed" date; the failure reached the log and nowhere else. The list
//! row now records the last attempt and, while it is failing, why:
//!
//! - `cbl_lists.last_refresh_attempt_at TIMESTAMPTZ NULL` — when the last
//!   refresh (scheduled or manual) finished, successful or not. NULL until
//!   the first refresh after this migration.
//! - `cbl_lists.last_refresh_error_kind TEXT NULL` — the failure kind
//!   (`entity::cbl_list::CblRefreshErrorKind`); NULL when the last attempt
//!   succeeded. Pinned by `cbl_lists_refresh_error_kind_check`.
//! - `cbl_lists.last_refresh_error_detail TEXT NULL` — a short note the
//!   server composes from safe values (an HTTP status number, a limit). Never
//!   remote text.
//! - `cbl_lists.refresh_failing_since TIMESTAMPTZ NULL` — the first failed
//!   attempt of the current run of failures.
//! - `cbl_lists.refresh_failure_count INTEGER NOT NULL DEFAULT 0` — failed
//!   attempts in a row; a success resets it. Drives the sweep's backoff.
//!
//! A CHECK keeps the failure fields consistent: a kind, a start time and a
//! positive count go together.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             ADD COLUMN IF NOT EXISTS last_refresh_attempt_at TIMESTAMPTZ NULL, \
             ADD COLUMN IF NOT EXISTS last_refresh_error_kind TEXT NULL, \
             ADD COLUMN IF NOT EXISTS last_refresh_error_detail TEXT NULL, \
             ADD COLUMN IF NOT EXISTS refresh_failing_since TIMESTAMPTZ NULL, \
             ADD COLUMN IF NOT EXISTS refresh_failure_count INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             DROP CONSTRAINT IF EXISTS cbl_lists_refresh_error_kind_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             ADD CONSTRAINT cbl_lists_refresh_error_kind_check \
             CHECK (last_refresh_error_kind IS NULL OR last_refresh_error_kind IN ( \
               'unreachable', 'http_status', 'missing', 'address_refused', \
               'too_large', 'too_many_entries', 'invalid_list', 'other'))",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             DROP CONSTRAINT IF EXISTS cbl_lists_refresh_failure_shape_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             ADD CONSTRAINT cbl_lists_refresh_failure_shape_check \
             CHECK ((last_refresh_error_kind IS NULL \
                     AND refresh_failing_since IS NULL \
                     AND refresh_failure_count = 0 \
                     AND last_refresh_error_detail IS NULL) \
                 OR (last_refresh_error_kind IS NOT NULL \
                     AND refresh_failing_since IS NOT NULL \
                     AND refresh_failure_count > 0 \
                     AND last_refresh_attempt_at IS NOT NULL))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             DROP CONSTRAINT IF EXISTS cbl_lists_refresh_failure_shape_check, \
             DROP CONSTRAINT IF EXISTS cbl_lists_refresh_error_kind_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE cbl_lists \
             DROP COLUMN IF EXISTS refresh_failure_count, \
             DROP COLUMN IF EXISTS refresh_failing_since, \
             DROP COLUMN IF EXISTS last_refresh_error_detail, \
             DROP COLUMN IF EXISTS last_refresh_error_kind, \
             DROP COLUMN IF EXISTS last_refresh_attempt_at",
        )
        .await?;
        Ok(())
    }
}
