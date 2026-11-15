//! Mylar snapshot ingest (upcoming-releases 1.0, part B).
//!
//! A script on the operator's Mylar host pushes a read-only snapshot to
//! `POST /integrations/mylar/snapshot`. Folio never calls Mylar. Each
//! accepted push replaces the previous one, so these tables only ever
//! hold the latest snapshot.
//!
//! - `integration_tokens` — the ingest token, as a SHA-256 digest. One
//!   row per integration (`name = 'mylar'`). The token is not a user and
//!   opens exactly one endpoint.
//! - `mylar_snapshot` — a single row (`id = 1`): when the snapshot was
//!   received (Folio's clock, the one staleness is judged by) and made,
//!   its content hash, and the blocks only the admin panel shows
//!   (`queue`, `jobs`, `history`) as JSON.
//! - `mylar_watch` — Mylar's watchlist: ComicVine volume id → status.
//!   What makes a release "tracked", for every future week.
//! - `mylar_pull` — the pull-list lines for the weeks Mylar holds. A
//!   surrogate key on purpose: a second printing repeats series + issue.
//! - `mylar_wanted` — issues Mylar wants, has snatched, or failed on.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS integration_tokens (
                name          TEXT PRIMARY KEY,
                digest        BYTEA NOT NULL,
                created_by    UUID NULL REFERENCES users(id) ON DELETE SET NULL,
                created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                last_used_at  TIMESTAMPTZ NULL,
                last_used_ip  TEXT NULL
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS mylar_snapshot (
                id                SMALLINT PRIMARY KEY CHECK (id = 1),
                received_at       TIMESTAMPTZ NOT NULL,
                generated_at      TIMESTAMPTZ NOT NULL,
                schema_version    INTEGER NOT NULL,
                content_hash      TEXT NOT NULL,
                mylar_db_version  INTEGER NULL,
                annuals_included  BOOLEAN NOT NULL DEFAULT FALSE,
                pull_count        INTEGER NOT NULL,
                watch_count       INTEGER NOT NULL,
                wanted_count      INTEGER NOT NULL,
                queue             JSONB NOT NULL,
                jobs              JSONB NOT NULL,
                history           JSONB NOT NULL
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS mylar_watch (
                cv_series_id  BIGINT PRIMARY KEY,
                status        TEXT NOT NULL,
                continuing    BOOLEAN NOT NULL
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS mylar_pull (
                id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                ship_date     DATE NOT NULL,
                publisher     TEXT NULL,
                series_name   TEXT NOT NULL,
                issue_number  TEXT NOT NULL,
                status        TEXT NOT NULL,
                cv_series_id  BIGINT NULL,
                cv_issue_id   BIGINT NULL,
                series_year   INTEGER NULL
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS mylar_pull_ship_date_idx ON mylar_pull (ship_date)",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS mylar_pull_cv_series_idx
                ON mylar_pull (cv_series_id) WHERE cv_series_id IS NOT NULL",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS mylar_wanted (
                id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                cv_series_id  BIGINT NULL,
                cv_issue_id   BIGINT NULL,
                series_name   TEXT NOT NULL,
                issue_number  TEXT NOT NULL,
                store_date    DATE NULL,
                status        TEXT NOT NULL,
                added_at      TIMESTAMPTZ NULL
            )",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS mylar_wanted_cv_series_idx
                ON mylar_wanted (cv_series_id) WHERE cv_series_id IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for table in [
            "mylar_wanted",
            "mylar_pull",
            "mylar_watch",
            "mylar_snapshot",
            "integration_tokens",
        ] {
            conn.execute_unprepared(&format!("DROP TABLE IF EXISTS {table}"))
                .await?;
        }
        Ok(())
    }
}
