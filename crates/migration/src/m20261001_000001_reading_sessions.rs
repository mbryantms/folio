//! M6a: per-user reading sessions table + reading-activity preference columns.
//!
//! Captures **intentional** reading (not browsing) — the client gates session
//! creation behind per-user thresholds so only sessions that meet the minimum
//! active duration AND distinct-pages-read counts ever land. Each session is
//! identified client-side by a UUID so heartbeats and the final-close flush are
//! idempotent over `(user_id, client_session_id)`.
//!
//! Coexists with `progress_records`; that table remains the source of truth for
//! "where do I resume" (debounced on every page turn). Sessions are the source
//! of truth for "how much have I read" (heartbeat + close).
//!
//! ACL is enforced at the handler layer via the existing `library_user_access`
//! join on `issues.library_id`, so the table itself does not denormalize
//! `library_id`. CASCADE through `issues`/`series`/`users` handles cleanup.
//!
//! Five new columns on `users`:
//!   - `activity_tracking_enabled` — opt-out kill switch (default true)
//!   - `timezone` — user's IANA tz for daily-bucket aggregations (default UTC)
//!   - `reading_min_active_ms` — server-enforced minimum active ms per session
//!   - `reading_min_pages` — server-enforced minimum distinct pages per session
//!   - `reading_idle_ms` — client-side idle-end threshold (also stored so the
//!     server can validate sane bounds on PATCH /me/preferences)

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum ReadingSessions {
    Table,
    Id,
    UserId,
    IssueId,
    SeriesId,
    ClientSessionId,
    StartedAt,
    EndedAt,
    LastHeartbeatAt,
    ActiveMs,
    DistinctPagesRead,
    PageTurns,
    StartPage,
    EndPage,
    FurthestPage,
    Device,
    ViewMode,
    ClientMeta,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
    ActivityTrackingEnabled,
    Timezone,
    ReadingMinActiveMs,
    ReadingMinPages,
    ReadingIdleMs,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id,
}

#[derive(Iden)]
enum Series {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ReadingSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ReadingSessions::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ReadingSessions::UserId).uuid().not_null())
                    .col(ColumnDef::new(ReadingSessions::IssueId).text().not_null())
                    .col(ColumnDef::new(ReadingSessions::SeriesId).uuid().not_null())
                    .col(
                        ColumnDef::new(ReadingSessions::ClientSessionId)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::LastHeartbeatAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::ActiveMs)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::DistinctPagesRead)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::PageTurns)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::StartPage)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::EndPage)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReadingSessions::FurthestPage)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ReadingSessions::Device).text().null())
                    .col(ColumnDef::new(ReadingSessions::ViewMode).text().null())
                    .col(
                        ColumnDef::new(ReadingSessions::ClientMeta)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("reading_sessions_user_fk")
                            .from(ReadingSessions::Table, ReadingSessions::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("reading_sessions_issue_fk")
                            .from(ReadingSessions::Table, ReadingSessions::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("reading_sessions_series_fk")
                            .from(ReadingSessions::Table, ReadingSessions::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Idempotency: heartbeats and final-close all hit the same client_session_id.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX IF NOT EXISTS reading_sessions_user_client_idx \
                 ON reading_sessions(user_id, client_session_id)",
            )
            .await?;

        // Per-user timeline + per-scope (series, issue) Activity tab queries.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS reading_sessions_user_started_idx \
                 ON reading_sessions(user_id, started_at DESC)",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS reading_sessions_issue_started_idx \
                 ON reading_sessions(issue_id, started_at DESC)",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS reading_sessions_series_started_idx \
                 ON reading_sessions(series_id, started_at DESC)",
            )
            .await?;

        // Sweeper: find dangling (no ended_at) sessions whose heartbeats stalled.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS reading_sessions_dangling_idx \
                 ON reading_sessions(last_heartbeat_at) \
                 WHERE ended_at IS NULL",
            )
            .await?;

        // Defense-in-depth: surface inverted page ranges before the row lands.
        // Client builds start_page/end_page as min/max of visited pages.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE reading_sessions \
                 ADD CONSTRAINT reading_sessions_page_range_chk \
                 CHECK (start_page >= 0 AND end_page >= start_page \
                        AND furthest_page >= end_page \
                        AND distinct_pages_read >= 0 AND page_turns >= 0 \
                        AND active_ms >= 0)",
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::ActivityTrackingEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .add_column(
                        ColumnDef::new(Users::Timezone)
                            .text()
                            .not_null()
                            .default("UTC"),
                    )
                    .add_column(
                        ColumnDef::new(Users::ReadingMinActiveMs)
                            .integer()
                            .not_null()
                            .default(30_000),
                    )
                    .add_column(
                        ColumnDef::new(Users::ReadingMinPages)
                            .integer()
                            .not_null()
                            .default(3),
                    )
                    .add_column(
                        ColumnDef::new(Users::ReadingIdleMs)
                            .integer()
                            .not_null()
                            .default(180_000),
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
                    .table(Users::Table)
                    .drop_column(Users::ActivityTrackingEnabled)
                    .drop_column(Users::Timezone)
                    .drop_column(Users::ReadingMinActiveMs)
                    .drop_column(Users::ReadingMinPages)
                    .drop_column(Users::ReadingIdleMs)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(ReadingSessions::Table).to_owned())
            .await?;

        Ok(())
    }
}
