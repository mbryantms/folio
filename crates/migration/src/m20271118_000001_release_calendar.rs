//! Release calendar — what publishers have announced for the coming
//! weeks, as Metron lists it (upcoming-releases 1.0, part C).
//!
//! Three tables, all filled by `jobs::release_calendar`:
//!
//! - `release_calendar_series` — one row per Metron series that has an
//!   issue in the window. The list endpoint only gives the name, volume
//!   and start year; publisher, series type and the ComicVine volume id
//!   come from one series-detail request, made once per series by the
//!   enrich sweep (`detail_fetched_at` is null until then).
//! - `release_calendar_issue` — one row per Metron issue with a store
//!   date in the window. `last_seen_at` is how a complete list pass
//!   finds rows Metron no longer returns.
//! - `release_calendar_state` — a single row (`id = 1`): the list
//!   pass's resume point, its last success, and its last failure with
//!   the time before which it must not run again.
//!
//! Metron ids are stored as BIGINT, never as text: they are joined to
//! each other here and to `external_ids.external_id` (text) by cast in
//! the reconcile queries.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS release_calendar_series (
                metron_series_id   BIGINT PRIMARY KEY,
                name               TEXT NOT NULL,
                volume             INTEGER NULL,
                year_began         INTEGER NULL,
                publisher          TEXT NULL,
                series_type        TEXT NULL,
                cv_series_id       BIGINT NULL,
                detail_fetched_at  TIMESTAMPTZ NULL,
                created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS release_calendar_series_cv_idx
                ON release_calendar_series (cv_series_id)
                WHERE cv_series_id IS NOT NULL",
        )
        .await?;
        // The enrich sweep's pick: never fetched first, then oldest.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS release_calendar_series_detail_idx
                ON release_calendar_series (detail_fetched_at NULLS FIRST)",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS release_calendar_issue (
                metron_issue_id   BIGINT PRIMARY KEY,
                metron_series_id  BIGINT NOT NULL
                    REFERENCES release_calendar_series(metron_series_id) ON DELETE CASCADE,
                number            TEXT NOT NULL,
                store_date        DATE NOT NULL,
                cover_date        DATE NULL,
                image_url         TEXT NULL,
                cover_hash        TEXT NULL,
                metron_modified   TIMESTAMPTZ NULL,
                first_seen_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS release_calendar_issue_store_date_idx
                ON release_calendar_issue (store_date, metron_issue_id)",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS release_calendar_issue_series_idx
                ON release_calendar_issue (metron_series_id)",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS release_calendar_state (
                id                 SMALLINT PRIMARY KEY CHECK (id = 1),
                pass_started_at    TIMESTAMPTZ NULL,
                pass_window_start  DATE NULL,
                pass_window_end    DATE NULL,
                pass_next_week     DATE NULL,
                last_finished_at   TIMESTAMPTZ NULL,
                last_error         TEXT NULL,
                last_error_at      TIMESTAMPTZ NULL,
                next_attempt_at    TIMESTAMPTZ NULL,
                updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .await?;
        conn.execute_unprepared(
            "INSERT INTO release_calendar_state (id) VALUES (1) ON CONFLICT (id) DO NOTHING",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DROP TABLE IF EXISTS release_calendar_state")
            .await?;
        conn.execute_unprepared("DROP TABLE IF EXISTS release_calendar_issue")
            .await?;
        conn.execute_unprepared("DROP TABLE IF EXISTS release_calendar_series")
            .await?;
        Ok(())
    }
}
