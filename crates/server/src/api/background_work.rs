//! `GET /admin/background-work` — one snapshot of everything the server is
//! doing in the background, across libraries and job types.
//!
//! Before this, the pieces lived on separate surfaces: scan progress on each
//! library's Live scan page, thumbnail progress on its Thumbnails sub-tab,
//! the per-queue counts on `/admin/queue`, scan-all on the scan dashboard.
//! An operator with 16k queued jobs had to visit every library to learn what
//! they were. This endpoint joins them so the header pill, the dashboard and
//! the Background work page all read the same numbers:
//!
//! - **per library**: the active scan run (queued or running, with its
//!   persisted progress), cover-thumbnail readiness from the issue rows,
//!   this process's thumbnail jobs by state, and content-hash backlog;
//! - **per queue**: waiting / retry-delayed / held-by-a-worker / dead;
//! - **metadata batches** still running or waiting on provider quota.
//!
//! Page-strip readiness is deliberately absent: it is counted from files on
//! disk (one stat per page), too slow for a polled cross-library snapshot.
//! It stays on the per-library `thumbnails-status` endpoint.
//!
//! Read-only admin GET → allowlisted in the audit-check tool.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use entity::{library, scan_run};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement,
};
use serde::Serialize;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::admin_thumbs::{ThumbJobCounts, thumbnail_job_counts_by_library};
use super::error;
use crate::auth::RequireAdmin;
use crate::library::scanner::process::HASH_ALGORITHM_PENDING;
use crate::library::thumbnails::THUMBNAIL_VERSION;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(background_work))
}

/// Server-wide roll-up of [`BackgroundWorkView`].
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct BackgroundWorkTotals {
    /// Scan runs executing (any kind: library, series, issue).
    pub scans_running: i64,
    /// Scan runs accepted but not started.
    pub scans_queued: i64,
    /// Unfinished scan runs with no job behind them (see
    /// [`ActiveScanView::stalled`]); not counted in the two above.
    pub scans_stalled: i64,
    /// Issues that still have cover work (thumbnail or perceptual hash).
    pub covers_remaining: i64,
    /// Issues whose content hash is still to compute.
    pub hash_pending: i64,
    /// Unfinished jobs across every queue.
    pub jobs_outstanding: i64,
    /// Of `jobs_outstanding`, jobs a worker has fetched.
    pub jobs_in_flight: i64,
    /// Jobs that exhausted their retries.
    pub jobs_dead: i64,
    /// Unfinished thumbnail jobs (cover + page-strip), server-wide.
    pub thumbs_outstanding: i64,
    /// Thumbnail jobs finished per minute over the last two minutes;
    /// `None` when too few have finished to measure.
    pub thumbs_per_min: Option<f64>,
    /// `thumbs_outstanding` at the current rate, in seconds.
    pub thumbs_eta_secs: Option<u64>,
    /// `true` when anything above is in progress. Dead jobs, stalled runs
    /// and work nobody has queued (`covers_remaining`, `hash_pending`) do
    /// not count.
    pub busy: bool,
}

/// A library's scan run that has not finished.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActiveScanView {
    pub id: String,
    /// `queued` | `running`.
    pub state: String,
    /// `library` | `series` | `issue`.
    pub kind: String,
    pub started_at: String,
    /// The scan-all batch this run belongs to, if any.
    pub batch_id: Option<String>,
    /// Scanner phase (`planning`, `scanning`, `reconciling`,
    /// `enqueueing_thumbnails`, …); `None` until the first progress write.
    pub phase: Option<String>,
    pub completed: Option<u64>,
    pub total: Option<u64>,
    pub current_label: Option<String>,
    pub files_per_sec: Option<f64>,
    /// The row says `queued` / `running` but its queue holds no job at all
    /// (waiting or with a worker), so nothing will advance or close it — a
    /// leftover from a crash or a queue clear. Excluded from `busy`. (A
    /// scan whose queue was cleared mid-run also reads stalled until it
    /// finishes on its own.)
    pub stalled: bool,
}

/// Everything in flight for one library.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryWorkView {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// The library-wide scan run in flight (running preferred over queued).
    pub scan: Option<ActiveScanView>,
    /// Series- or issue-scoped scan runs in flight for this library
    /// (stalled ones excluded).
    pub scoped_scans: i64,
    /// Active issues.
    pub issues_total: i64,
    /// Issues whose cover thumbnail is current.
    pub covers_ready: i64,
    /// Issues that still have cover work: thumbnail missing or stale, or
    /// (`covers_hash_only`) only the perceptual hash to compute.
    pub covers_remaining: i64,
    pub covers_hash_only: i64,
    /// Issues whose last thumbnail attempt failed.
    pub covers_errored: i64,
    pub cover_jobs_queued: u64,
    pub cover_jobs_running: u64,
    pub page_jobs_queued: u64,
    pub page_jobs_running: u64,
    /// Issues whose content hash is still to compute.
    pub hash_pending: i64,
    /// `true` when a live scan or a thumbnail job is in flight for this
    /// library. Unqueued backlog (`covers_remaining`, `hash_pending`) and
    /// stalled runs do not count.
    pub busy: bool,
}

/// One queue's jobs by where they are held.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QueueWorkView {
    pub queue: String,
    pub waiting: i64,
    pub scheduled: i64,
    pub in_flight: i64,
    pub dead: i64,
}

/// A metadata batch with member runs still to finish.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetadataBatchWorkView {
    pub id: String,
    pub library_id: Option<String>,
    /// `series_issues` | `saved_view` | `library_refresh`.
    pub scope: String,
    /// `running` | `awaiting_quota`.
    pub status: String,
    pub items_total: i64,
    /// Member runs that reached a terminal state.
    pub items_finished: i64,
    pub created_at: String,
    /// `running` with member runs outstanding, but the metadata queues hold
    /// no job — nothing will advance it. Excluded from `busy`.
    pub stalled: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackgroundWorkView {
    pub generated_at: String,
    pub totals: BackgroundWorkTotals,
    /// Every library, in name order (idle ones included, so the page can
    /// show the whole estate).
    pub libraries: Vec<LibraryWorkView>,
    /// Every queue, in a stable order.
    pub queues: Vec<QueueWorkView>,
    pub metadata_batches: Vec<MetadataBatchWorkView>,
}

#[utoipa::path(
    operation_id = "admin_background_work",
    get,
    path = "/admin/background-work",
    responses(
        (status = 200, body = BackgroundWorkView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn background_work(
    State(app): State<AppState>,
    _admin: RequireAdmin,
) -> impl IntoResponse {
    match snapshot(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "background_work: snapshot failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct IssueCounts {
    library_id: Uuid,
    total: i64,
    ready: i64,
    hash_only: i64,
    errored: i64,
    hash_pending: i64,
}

/// Per-library issue roll-up in one pass, over active issues. The cover
/// predicates mirror `post_scan::needs_cover_work_filter`; `hash_pending`
/// is `hash_backfill::progress`'s pending count narrowed to active issues.
async fn issue_counts<C: ConnectionTrait>(db: &C) -> Result<Vec<IssueCounts>, DbErr> {
    let stmt = Statement::from_sql_and_values(
        db.get_database_backend(),
        r#"
        SELECT
            library_id,
            COUNT(*)::BIGINT AS total,
            COUNT(*) FILTER (
                WHERE thumbnails_generated_at IS NOT NULL
                  AND thumbnail_version >= $1
            )::BIGINT AS ready,
            COUNT(*) FILTER (
                WHERE thumbnails_generated_at IS NOT NULL
                  AND thumbnail_version >= $1
                  AND NOT EXISTS (
                      SELECT 1 FROM issue_cover ic
                      WHERE ic.issue_id = issues.id
                        AND ic.kind = 'primary'
                        AND ic.ordinal = 0
                        AND ic.source_provider = 'archive_extracted'
                        AND ic.phash IS NOT NULL
                  )
            )::BIGINT AS hash_only,
            COUNT(*) FILTER (WHERE thumbnails_error IS NOT NULL)::BIGINT AS errored,
            COUNT(*) FILTER (
                WHERE hash_algorithm = $2 AND removed_at IS NULL
            )::BIGINT AS hash_pending
        FROM issues
        WHERE state = 'active'
        GROUP BY library_id
        "#,
        [THUMBNAIL_VERSION.into(), HASH_ALGORITHM_PENDING.into()],
    );
    IssueCounts::find_by_statement(stmt).all(db).await
}

#[derive(Debug, FromQueryResult)]
struct BatchRow {
    id: Uuid,
    library_id: Option<Uuid>,
    scope: String,
    status: String,
    items_total: i32,
    items_finished: i64,
    created_at: chrono::DateTime<chrono::FixedOffset>,
}

async fn unfinished_metadata_batches<C: ConnectionTrait>(db: &C) -> Result<Vec<BatchRow>, DbErr> {
    let stmt = Statement::from_string(
        db.get_database_backend(),
        r#"
        SELECT * FROM (
            SELECT b.id, b.library_id, b.scope, b.status, b.items_total, b.created_at,
                   (SELECT COUNT(*) FROM metadata_run r
                     WHERE r.batch_id = b.id
                       AND r.status IN ('completed', 'failed', 'cancelled'))::BIGINT
                       AS items_finished
              FROM metadata_batch b
             WHERE b.status IN ('running', 'awaiting_quota')
        ) t
        -- `metadata_batch.status` is only re-derived when someone reads the
        -- batch, so a batch nobody reopened stays 'running' forever. Trust
        -- the member runs instead.
         WHERE t.items_finished < t.items_total
         ORDER BY t.created_at DESC
         LIMIT 50
        "#,
    );
    BatchRow::find_by_statement(stmt).all(db).await
}

fn active_scan_view(run: &scan_run::Model, stalled: bool) -> ActiveScanView {
    let progress = run.stats.get("progress");
    let str_of = |k: &str| {
        progress
            .and_then(|p| p.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    let u64_of = |k: &str| progress.and_then(|p| p.get(k)).and_then(|v| v.as_u64());
    ActiveScanView {
        id: run.id.to_string(),
        state: run.state.clone(),
        kind: run.kind.clone(),
        started_at: run.started_at.to_rfc3339(),
        batch_id: run.batch_id.map(|b| b.to_string()),
        phase: str_of("phase"),
        completed: u64_of("completed"),
        total: u64_of("total"),
        current_label: str_of("current_label"),
        files_per_sec: progress
            .and_then(|p| p.get("files_per_sec"))
            .and_then(|v| v.as_f64()),
        stalled,
    }
}

async fn snapshot(app: &AppState) -> anyhow::Result<BackgroundWorkView> {
    let libraries = library::Entity::find()
        .order_by_asc(library::Column::Name)
        .all(&app.db)
        .await?;
    let runs = scan_run::Entity::find()
        .filter(scan_run::Column::State.is_in(["queued", "running"]))
        .order_by_asc(scan_run::Column::StartedAt)
        .all(&app.db)
        .await?;
    let issues: HashMap<Uuid, IssueCounts> = issue_counts(&app.db)
        .await?
        .into_iter()
        .map(|c| (c.library_id, c))
        .collect();
    let thumb_jobs = thumbnail_job_counts_by_library(app).await;
    let queue_counts = app.jobs.queue_counts().await?;
    let dead: HashMap<&'static str, i64> =
        app.jobs.dead_letter_counts().await?.into_iter().collect();
    let batches = unfinished_metadata_batches(&app.db).await?;

    // A run row is only live while its queue still holds a job for it:
    // library scans ride `scan`, series/issue scans ride `scan_series`.
    let outstanding = |queue: &str| {
        queue_counts
            .iter()
            .find(|c| c.queue == queue)
            .map_or(0, crate::jobs::QueueCount::outstanding)
    };
    let library_scans_live = outstanding("scan") > 0;
    let scoped_scans_live = outstanding("scan_series") > 0;
    let is_stalled = |run: &scan_run::Model| {
        if run.kind == "library" {
            !library_scans_live
        } else {
            !scoped_scans_live
        }
    };
    let metadata_jobs_live = [
        "metadata_search_series",
        "metadata_search_issue",
        "metadata_apply_series",
        "metadata_apply_issue",
    ]
    .into_iter()
    .any(|q| outstanding(q) > 0);

    let mut totals = BackgroundWorkTotals::default();
    for run in &runs {
        if is_stalled(run) {
            totals.scans_stalled += 1;
        } else if run.state == "running" {
            totals.scans_running += 1;
        } else {
            totals.scans_queued += 1;
        }
    }

    let libraries: Vec<LibraryWorkView> = libraries
        .into_iter()
        .map(|lib| {
            let lib_runs = || runs.iter().filter(|r| r.library_id == lib.id);
            // Running beats queued; within a state the oldest run wins.
            let scan = lib_runs()
                .filter(|r| r.kind == "library")
                .min_by_key(|r| r.state != "running")
                .map(|r| active_scan_view(r, is_stalled(r)));
            let scoped_scans = lib_runs()
                .filter(|r| r.kind != "library" && !is_stalled(r))
                .count() as i64;
            let c = issues.get(&lib.id);
            let ready = c.map_or(0, |c| c.ready);
            let total = c.map_or(0, |c| c.total);
            let hash_only = c.map_or(0, |c| c.hash_only);
            let covers_remaining = (total - ready).max(0) + hash_only;
            let hash_pending = c.map_or(0, |c| c.hash_pending);
            let jobs = thumb_jobs
                .get(&lib.id)
                .copied()
                .unwrap_or(ThumbJobCounts::default());
            totals.covers_remaining += covers_remaining;
            totals.hash_pending += hash_pending;
            let thumbs_active = jobs.jobs > 0;
            LibraryWorkView {
                id: lib.id.to_string(),
                slug: lib.slug,
                name: lib.name,
                busy: scan.as_ref().is_some_and(|s| !s.stalled)
                    || scoped_scans > 0
                    || thumbs_active,
                scan,
                scoped_scans,
                issues_total: total,
                covers_ready: ready,
                covers_remaining,
                covers_hash_only: hash_only,
                covers_errored: c.map_or(0, |c| c.errored),
                cover_jobs_queued: jobs.cover_queued,
                cover_jobs_running: jobs.cover_running,
                page_jobs_queued: jobs.page_map_queued,
                page_jobs_running: jobs.page_map_running,
                hash_pending,
            }
        })
        .collect();

    let queues: Vec<QueueWorkView> = queue_counts
        .iter()
        .map(|c| {
            totals.jobs_outstanding += c.outstanding();
            if c.queue == "post_scan_thumbs" {
                totals.thumbs_outstanding = c.outstanding();
            }
            totals.jobs_in_flight += c.in_flight;
            let dead = dead.get(c.queue).copied().unwrap_or(0);
            totals.jobs_dead += dead;
            QueueWorkView {
                queue: c.queue.to_owned(),
                waiting: c.waiting,
                scheduled: c.scheduled,
                in_flight: c.in_flight,
                dead,
            }
        })
        .collect();

    let metadata_batches: Vec<MetadataBatchWorkView> = batches
        .into_iter()
        .map(|b| MetadataBatchWorkView {
            stalled: b.status == "running" && !metadata_jobs_live,
            id: b.id.to_string(),
            library_id: b.library_id.map(|l| l.to_string()),
            scope: b.scope,
            status: b.status,
            items_total: i64::from(b.items_total),
            items_finished: b.items_finished,
            created_at: b.created_at.to_rfc3339(),
        })
        .collect();

    if totals.thumbs_outstanding > 0 {
        totals.thumbs_per_min = app.thumbs_per_min();
        totals.thumbs_eta_secs = totals
            .thumbs_per_min
            .filter(|rate| *rate > 0.0)
            .map(|rate| (totals.thumbs_outstanding as f64 / rate * 60.0).ceil() as u64);
    }

    totals.busy = totals.scans_running + totals.scans_queued + totals.jobs_outstanding > 0
        || metadata_batches
            .iter()
            .any(|b| b.status == "running" && !b.stalled);

    Ok(BackgroundWorkView {
        generated_at: chrono::Utc::now().to_rfc3339(),
        totals,
        libraries,
        queues,
        metadata_batches,
    })
}
