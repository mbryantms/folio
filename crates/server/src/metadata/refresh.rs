//! Bulk metadata-refresh dispatch — fan out search jobs across a
//! library scope. Drives both the user-triggered
//! `POST /libraries/{slug}/metadata/refresh?scope=…` endpoint and
//! the weekly cron in [`crate::jobs::scheduler`].
//!
//! **Scope rules** (mirrors the M7 plan's "stale" definition):
//!
//! - `unmatched` — series with zero rows in `external_ids` for any
//!   provider source AND `metadata_sync_paused=false`.
//! - `stale`     — series that are unmatched OR whose
//!   `last_metadata_sync_at IS NULL OR <
//!   now() - INTERVAL 'stale_after_days days'`, paused excluded.
//! - `all`       — every active series in the library, paused
//!   excluded.
//! - `recent`    — Mylar-pattern "recently published" window:
//!   series whose `last_issue_added_at >= now() - window_days`. The
//!   weekly cron unions this with `stale` so newly-published series
//!   refresh weekly while older ones only refresh once they cross
//!   `stale_after_days`.
//!
//! Every scope excludes paused series (`series.metadata_sync_paused
//! = true`) and the chunking cap (200 per run) guards against
//! runaway provider-quota burn — once the cap is hit the remainder
//! waits for the next call.
//!
//! metadata-providers-1.0 M7.

use crate::state::AppState;
use entity::series;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, FromQueryResult, QueryFilter,
    Statement,
};
use uuid::Uuid;

/// Hard cap so a "scope=all" refresh can't fan out tens of thousands
/// of provider calls in one click. Operators can re-trigger to
/// process the rest; the cron's per-week cadence handles the rest
/// for unattended deploys.
pub const REFRESH_BATCH_CAP: usize = 200;

/// Which series belong to a given scope. Cheap query — index hits
/// on `series.library_id` + `series.metadata_sync_paused`. The
/// `unmatched` and `stale` shapes call into raw SQL because the
/// external_ids existence check is most concisely expressed as
/// `NOT EXISTS (...)` rather than via SeaORM relations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshScope {
    Unmatched,
    Stale,
    All,
    Recent,
}

impl RefreshScope {
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshScope::Unmatched => "unmatched",
            RefreshScope::All => "all",
            RefreshScope::Stale => "stale",
            RefreshScope::Recent => "recent",
        }
    }
}

impl std::str::FromStr for RefreshScope {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unmatched" => Ok(RefreshScope::Unmatched),
            "stale" => Ok(RefreshScope::Stale),
            "all" => Ok(RefreshScope::All),
            "recent" => Ok(RefreshScope::Recent),
            _ => Err(()),
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct SeriesIdRow {
    id: Uuid,
}

/// Walk the eligible series for `scope` in `library_id`, capped at
/// [`REFRESH_BATCH_CAP`]. Order is `created_at ASC` so successive
/// calls re-process the earliest deferred rows first.
pub async fn eligible_series_for_scope<C: ConnectionTrait>(
    db: &C,
    library_id: Uuid,
    scope: RefreshScope,
    stale_after_days: u32,
    window_days: u32,
) -> Result<Vec<Uuid>, sea_orm::DbErr> {
    let limit = REFRESH_BATCH_CAP as i64;
    let stmt = match scope {
        RefreshScope::All => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
              ORDER BY s.created_at ASC
              LIMIT $2",
            [library_id.into(), limit.into()],
        ),
        RefreshScope::Unmatched => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND NOT EXISTS (
                    SELECT 1 FROM external_ids x
                    WHERE x.entity_type = 'series'
                      AND x.entity_id = s.id::text
                )
              ORDER BY s.created_at ASC
              LIMIT $2",
            [library_id.into(), limit.into()],
        ),
        RefreshScope::Stale => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // Three OR'd staleness predicates:
            //   1. unmatched (no provider rows yet)
            //   2. never synced
            //   3. last sync older than stale_after_days
            // Paused series excluded regardless.
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND (
                    NOT EXISTS (
                        SELECT 1 FROM external_ids x
                        WHERE x.entity_type = 'series'
                          AND x.entity_id = s.id::text
                    )
                    OR s.last_metadata_sync_at IS NULL
                    OR s.last_metadata_sync_at < NOW() - ($2 || ' days')::interval
                )
              ORDER BY s.created_at ASC
              LIMIT $3",
            [library_id.into(), stale_after_days.into(), limit.into()],
        ),
        RefreshScope::Recent => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // Series whose latest active issue's created_at falls
            // within the recent-window. Falls back to series.created_at
            // when no issues exist yet (fresh series tripped by the
            // window). `last_issue_added_at` isn't a stored column —
            // we compute it via correlated subquery (cheap; one
            // index hit per series on issues.series_id). Paused
            // excluded.
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND COALESCE(
                    (
                        SELECT MAX(i.created_at)
                        FROM issues i
                        WHERE i.series_id = s.id
                          AND i.state = 'active'
                          AND i.removed_at IS NULL
                    ),
                    s.created_at
                ) >= NOW() - ($2 || ' days')::interval
              ORDER BY s.created_at ASC
              LIMIT $3",
            [library_id.into(), window_days.into(), limit.into()],
        ),
    };
    let rows = SeriesIdRow::find_by_statement(stmt).all(db).await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Result of a single bulk-refresh fan-out — surfaces what was
/// enqueued so the caller can render a useful toast / log line.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct RefreshOutcome {
    /// Total eligible series for this scope after applying the
    /// [`REFRESH_BATCH_CAP`].
    pub series_eligible: usize,
    /// Per-series search jobs enqueued. Lower than `series_eligible`
    /// when a per-entity coalesce gate found an in-flight run.
    pub jobs_enqueued: usize,
    /// Per-series search jobs that hit the coalesce gate (the
    /// existing in-flight run will land first; no extra work).
    pub jobs_coalesced: usize,
    /// Per-series search jobs that failed to enqueue (queue push
    /// error, missing series row mid-flight, etc.).
    pub jobs_failed: usize,
}

/// Fan out a search job per eligible series, honoring the per-entity
/// Redis coalesce gate the search-job module already implements.
/// Bound by [`REFRESH_BATCH_CAP`].
pub async fn fan_out_scope(
    state: &AppState,
    library_id: Uuid,
    scope: RefreshScope,
    trigger_kind: &'static str,
) -> Result<RefreshOutcome, sea_orm::DbErr> {
    let cfg = state.cfg();
    let stale_after = cfg.metadata_stale_after_days;
    let window = cfg.metadata_weekly_refresh_window_days;
    let ids = eligible_series_for_scope(&state.db, library_id, scope, stale_after, window).await?;
    let series_eligible = ids.len();
    let mut jobs_enqueued = 0usize;
    let mut jobs_coalesced = 0usize;
    let mut jobs_failed = 0usize;
    for id in ids {
        match crate::jobs::metadata_search::enqueue_series_search(
            state,
            id,
            None,
            trigger_kind,
        )
        .await
        {
            Ok(outcome) => {
                if outcome.coalesced {
                    jobs_coalesced += 1;
                } else {
                    jobs_enqueued += 1;
                }
            }
            Err(e) => {
                tracing::warn!(series_id = %id, error = %e, "refresh fan-out: enqueue failed");
                jobs_failed += 1;
            }
        }
    }
    Ok(RefreshOutcome {
        series_eligible,
        jobs_enqueued,
        jobs_coalesced,
        jobs_failed,
    })
}

/// Series-row liveness check used by [`fan_out_scope`]; exposed for
/// unit tests + the weekly cron's per-library walker. Surface is
/// intentionally narrow so the scope-walker doesn't need to import
/// the full series entity.
pub async fn series_exists(
    db: &sea_orm::DatabaseConnection,
    series_id: Uuid,
) -> Result<bool, sea_orm::DbErr> {
    Ok(series::Entity::find_by_id(series_id)
        .filter(series::Column::RemovedAt.is_null())
        .one(db)
        .await?
        .is_some())
}
