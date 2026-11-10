//! `GET /admin/queue-depth` — current pending-job counts for the apalis
//! queues that drive scans (spec §3 + §8). Polled by the admin topbar so an
//! operator can see when a backlog is draining.
//!
//! "Pending" here means every job that has not finished: waiting for a
//! worker, waiting out a retry delay, or fetched by a worker (executing or
//! in its prefetch buffer) — [`crate::jobs::JobRuntime::queue_counts`].
//! apalis's own `Storage::len()` is only the first of those, so it reads 0
//! while the last jobs of a drain are still running.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use apalis::prelude::{FetchById, TaskId, TaskSink};
use axum::{Extension, Json, extract::Query, http::StatusCode, response::IntoResponse};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(queue_depth))
        .routes(routes!(dead_letters))
        .routes(routes!(clear_queue))
        .routes(routes!(dead_jobs))
        .routes(routes!(retry_dead_job))
        .routes(routes!(purge_dead_jobs))
        .routes(routes!(queue_jobs))
}

/// The apalis queue labels, matching the keys in
/// [`crate::jobs::JobRuntime::dead_letter_counts`]. The dead-job list / retry
/// / purge endpoints validate their `queue` arg against this set so a typo
/// returns 422 instead of silently operating on a non-existent key.
const DEAD_QUEUES: &[&str] = &[
    "scan",
    "scan_series",
    "post_scan_thumbs",
    "post_scan_search",
    "post_scan_dictionary",
    "metadata_search_series",
    "metadata_search_issue",
    "metadata_apply_series",
    "metadata_apply_issue",
    "rewrite_issue_sidecars",
    "archive_edit",
    "backfill",
    "hash_backfill",
    "relationship_suggest",
    "provider_coverage",
    "entity_enrich",
    "sidecar_reemit",
];

/// One queue's outstanding jobs, split by where they are held.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QueueDepthEntry {
    /// Queue label, e.g. `post_scan_thumbs`.
    pub queue: String,
    /// Not yet fetched by a worker.
    pub waiting: i64,
    /// Waiting out a retry delay.
    pub scheduled: i64,
    /// Fetched by a worker: executing, or in its prefetch buffer.
    pub in_flight: i64,
}

/// Outstanding jobs per queue. Each per-queue number is everything not yet
/// finished (`waiting + scheduled + in_flight`); `queues` carries the split.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QueueDepthView {
    pub scan: i64,
    pub scan_series: i64,
    pub post_scan_thumbs: i64,
    pub post_scan_search: i64,
    pub post_scan_dictionary: i64,
    /// Pending provider metadata searches (series + issue scope; UX-16).
    pub metadata_search_series: i64,
    pub metadata_search_issue: i64,
    /// Pending provider metadata applies (series + issue scope; UX-16).
    pub metadata_apply_series: i64,
    pub metadata_apply_issue: i64,
    /// Pending sidecar-XML rewrite jobs (writeback path; UX-16).
    pub rewrite_issue_sidecars: i64,
    /// Pending archive page-edit jobs (single + bulk; M7).
    pub archive_edit: i64,
    /// Pending backfill drains (cover-phash / variant-cover; B17).
    pub backfill: i64,
    /// Pending first-import content-hash drains (one per library; WP-3.2).
    pub hash_backfill: i64,
    /// Pending relationship-suggestion runs (one per library; WP-7.2).
    pub relationship_suggest: i64,
    /// Pending provider coverage analyses (one per series request).
    pub provider_coverage: i64,
    /// Entity profile fetches (character / team / location / arc pages).
    pub entity_enrich: i64,
    /// Library-wide sidecar re-emit ticks (one chain per running run).
    pub sidecar_reemit: i64,
    /// Sum across all queues — convenient for the topbar pill.
    pub total: i64,
    /// Of `total`, the jobs a worker has fetched (executing or prefetched).
    pub in_flight: i64,
    /// Per-queue split, one entry per queue in a stable order.
    pub queues: Vec<QueueDepthEntry>,
}

/// One queue's dead-letter count (OPS-3 follow-up).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeadLetterCount {
    pub queue: String,
    pub count: i64,
}

/// Per-queue counts of jobs apalis has given up on (moved to `{queue}:dead`
/// after exhausting attempts). `total > 0` is the operator's cue that work is
/// failing permanently and silently; the per-queue breakdown points at which.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeadLetterView {
    pub queues: Vec<DeadLetterCount>,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueueClearTarget {
    All,
    Scans,
    Thumbnails,
    /// Provider metadata work: series / issue searches and applies, sidecar
    /// rewrites, library-wide sidecar re-emit runs and coverage analyses.
    /// Their pending runs close as `cancelled` and their batches finish.
    Metadata,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QueueClearReq {
    pub target: QueueClearTarget,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QueueClearResp {
    pub target: QueueClearTarget,
    pub deleted_keys: usize,
    /// `queued` scan runs closed as `cancelled` because their job was
    /// cleared (scan targets only).
    pub cancelled_scan_runs: u64,
    /// Pending metadata runs (queued / searching / awaiting quota) closed
    /// as `cancelled` (metadata targets only).
    pub cancelled_metadata_runs: u64,
    pub before: QueueDepthView,
    pub after: QueueDepthView,
    /// Redis queue clearing is immediate, but a job already executing in a
    /// worker may finish and emit its normal completion events.
    pub running_jobs_may_finish: bool,
}

#[utoipa::path(
    operation_id = "admin_queue_queue_depth",    get,
    path = "/admin/queue-depth",
    responses(
        (status = 200, body = QueueDepthView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn queue_depth(
    axum::extract::State(app): axum::extract::State<AppState>,
    _admin: RequireAdmin,
) -> impl IntoResponse {
    match queue_depth_counts(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "queue_depth: redis len failed");
            internal()
        }
    }
}

#[utoipa::path(
    operation_id = "admin_queue_dead_letters",    get,
    path = "/admin/queue/dead-letters",
    responses(
        (status = 200, body = DeadLetterView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn dead_letters(
    axum::extract::State(app): axum::extract::State<AppState>,
    _admin: RequireAdmin,
) -> impl IntoResponse {
    match app.jobs.dead_letter_counts().await {
        Ok(counts) => {
            let total = counts.iter().map(|(_, n)| *n).sum();
            let queues = counts
                .into_iter()
                .map(|(queue, count)| DeadLetterCount {
                    queue: queue.to_owned(),
                    count,
                })
                .collect();
            Json(DeadLetterView { queues, total }).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "dead_letters: redis zcard failed");
            internal()
        }
    }
}

#[utoipa::path(
    operation_id = "admin_queue_clear_queue",    post,
    path = "/admin/queue/clear",
    request_body = QueueClearReq,
    responses(
        (status = 200, body = QueueClearResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn clear_queue(
    axum::extract::State(app): axum::extract::State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<QueueClearReq>,
) -> impl IntoResponse {
    let before = match queue_depth_counts(&app).await {
        Ok(view) => view,
        Err(e) => {
            tracing::error!(error = %e, "clear_queue: queue depth before clear failed");
            return internal();
        }
    };

    // Taken before the delete: only runs enqueued before now lose their job.
    let clear_started_at = chrono::Utc::now().fixed_offset();
    let mut conn = app.jobs.redis.clone();
    let mut deleted_keys = 0;
    for pattern in clear_patterns(req.target) {
        match delete_matching_keys(&mut conn, pattern).await {
            Ok(n) => deleted_keys += n,
            Err(e) => {
                tracing::error!(error = %e, pattern, "clear_queue: redis key delete failed");
                return internal();
            }
        }
    }

    if matches!(
        req.target,
        QueueClearTarget::All | QueueClearTarget::Thumbnails
    ) {
        app.clear_thumb_job_marks().await;
        // With the marks gone no finishing job will flush for the others;
        // write the buffered cover stamps now instead of on the timer.
        crate::jobs::post_scan::flush_done_stamps(&app).await;
    }

    // The jobs that would have started the `queued` scan runs are gone;
    // close the rows so they (and their scan-all batch) don't read as in
    // progress forever.
    let mut cancelled_runs = 0;
    if matches!(req.target, QueueClearTarget::All | QueueClearTarget::Scans) {
        match crate::library::scanner::cancel_queued_runs(
            &app,
            "queue cleared by admin",
            clear_started_at,
        )
        .await
        {
            Ok(n) => cancelled_runs = n,
            Err(e) => tracing::error!(error = %e, "clear_queue: closing queued scan runs failed"),
        }
    }
    // The jobs that would have advanced the pending metadata runs are gone:
    // close them (and their batches) so the review page and the Background
    // work page don't show work that will never move.
    let cancelled_metadata_runs = if matches!(
        req.target,
        QueueClearTarget::All | QueueClearTarget::Metadata
    ) {
        match crate::metadata::orchestrator::cancel_pending_runs(
            &app.db,
            crate::metadata::orchestrator::CancelScope::StartedBefore(clear_started_at),
            "queue cleared by admin",
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::error!(error = %e, "clear_queue: cancelling pending metadata runs failed");
                0
            }
        }
    } else {
        0
    };

    let after = match queue_depth_counts(&app).await {
        Ok(view) => view,
        Err(e) => {
            tracing::error!(error = %e, "clear_queue: queue depth after clear failed");
            return internal();
        }
    };

    // Per CLAUDE.md, every mutating admin handler emits via
    // `crate::audit::record`. The queue isn't a single addressable
    // entity so `target_id` stays None; the payload carries the target
    // tag + before/after depths so the audit row is self-documenting.
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: "admin.queue.clear",
            target_type: Some("queue"),
            target_id: None,
            payload: serde_json::json!({
                "target": req.target,
                "deleted_keys": deleted_keys,
                "cancelled_scan_runs": cancelled_runs,
                "cancelled_metadata_runs": cancelled_metadata_runs,
                "before": before,
                "after": after,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(QueueClearResp {
        target: req.target,
        deleted_keys,
        cancelled_scan_runs: cancelled_runs,
        cancelled_metadata_runs,
        running_jobs_may_finish: before.total > after.total,
        before,
        after,
    })
    .into_response()
}

// ───────────────────────── dead-job inspection (D8b) ─────────────────────────
//
// apalis moves a job to the `{queue}:dead` SET after it exhausts its
// attempts ([`JOB_MAX_ATTEMPTS`](crate::jobs::JOB_MAX_ATTEMPTS)), writing the
// kill time (`done_at`), final `status` and the last error (`result`) into the
// job's `{queue}:meta:{id}` hash. `dead_letters` already surfaces the per-queue
// COUNT; these endpoints let an operator see the individual failures and
// either retry one (re-enqueue a fresh copy via the typed storage) or purge
// the queue's dead set. "Manual retry + retention" — dead jobs are kept until
// an operator acts, never auto-discarded.

/// One dead-lettered job surfaced for inspection / retry.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeadJob {
    /// apalis task id (ULID) — the retry key.
    pub task_id: String,
    /// Unix seconds when apalis moved the job to the dead set.
    pub failed_at: Option<i64>,
    /// The error apalis stored on the final failed attempt.
    pub error: Option<String>,
    /// The job's `args` payload as opaque JSON, so the UI can show which
    /// library / issue / series the dead job targeted without per-queue code.
    #[schema(value_type = Object, nullable = true)]
    pub payload: Option<serde_json::Value>,
    /// The issue / series / library the payload names, resolved to labels
    /// and slugs (`api::work_targets`) — an issue id is a content hash, a
    /// series id a UUID, neither readable on their own. Empty when the
    /// payload names nothing resolvable (or the row was removed).
    pub targets: Vec<crate::api::work_targets::WorkTargetView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeadJobsView {
    pub queue: String,
    pub jobs: Vec<DeadJob>,
    /// Total dead jobs in the queue — the list pages, it never silently caps.
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct DeadJobsQuery {
    /// Queue label — one of the eleven apalis queues.
    pub queue: String,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeadJobRetryReq {
    pub queue: String,
    pub task_id: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeadJobRetryResp {
    pub queue: String,
    pub task_id: String,
    pub retried: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeadJobsPurgeReq {
    pub queue: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeadJobsPurgeResp {
    pub queue: String,
    pub purged: usize,
}

#[utoipa::path(
    operation_id = "admin_queue_dead_jobs", get,
    path = "/admin/queue/dead-jobs",
    params(DeadJobsQuery),
    responses(
        (status = 200, body = DeadJobsView),
        (status = 403, description = "admin only"),
        (status = 422, description = "unknown queue"),
    )
)]
#[handler]
pub async fn dead_jobs(
    axum::extract::State(app): axum::extract::State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<DeadJobsQuery>,
) -> impl IntoResponse {
    let Some(keys) = dead_keys(&app, &q.queue) else {
        return unknown_queue(&q.queue);
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (i64::from(page) - 1) * i64::from(page_size);
    match list_dead(&app, &keys, offset, i64::from(page_size)).await {
        Ok((mut jobs, total)) => {
            // One bulk lookup for the page's payload ids → labels + slugs.
            let mut refs = crate::api::work_targets::TargetRefs::default();
            for j in &jobs {
                if let Some(p) = &j.payload {
                    crate::api::work_targets::collect_refs(p, &mut refs);
                }
            }
            match crate::api::work_targets::resolve(&app, refs).await {
                Ok(resolved) => {
                    for j in &mut jobs {
                        if let Some(p) = &j.payload {
                            j.targets = crate::api::work_targets::targets_for(p, &resolved);
                        }
                    }
                }
                // Labels are a convenience; the raw payload still shows.
                Err(e) => {
                    tracing::warn!(error = %e, queue = %q.queue, "dead_jobs: target labels failed")
                }
            }
            Json(DeadJobsView {
                queue: q.queue,
                jobs,
                total,
                page,
                page_size,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, queue = %q.queue, "dead_jobs: redis read failed");
            internal()
        }
    }
}

#[utoipa::path(
    operation_id = "admin_queue_retry_dead_job", post,
    path = "/admin/queue/dead-jobs/retry",
    request_body = DeadJobRetryReq,
    responses(
        (status = 200, body = DeadJobRetryResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "no dead job with that id"),
        (status = 422, description = "unknown queue or malformed task id"),
    )
)]
#[handler]
pub async fn retry_dead_job(
    axum::extract::State(app): axum::extract::State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<DeadJobRetryReq>,
) -> impl IntoResponse {
    if !DEAD_QUEUES.contains(&req.queue.as_str()) {
        return unknown_queue(&req.queue);
    }
    if TaskId::from_str(&req.task_id).is_err() {
        return super::error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_task_id",
            "task_id is not a valid job id",
        );
    }
    match retry_one(&app, &req.queue, &req.task_id).await {
        Ok(true) => {
            audit::record(
                &app.db,
                AuditEntry {
                    actor_id: admin.0.id,
                    action: "admin.queue.job.retry",
                    target_type: Some("queue_job"),
                    target_id: Some(req.task_id.clone()),
                    payload: serde_json::json!({ "queue": req.queue, "task_id": req.task_id }),
                    ip: ctx.ip_string(),
                    user_agent: ctx.user_agent.clone(),
                },
            )
            .await;
            Json(DeadJobRetryResp {
                queue: req.queue,
                task_id: req.task_id,
                retried: true,
            })
            .into_response()
        }
        Ok(false) => super::error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no dead job with that id in this queue",
        ),
        Err(e) => {
            tracing::error!(error = %e, queue = %req.queue, "retry_dead_job failed");
            internal()
        }
    }
}

#[utoipa::path(
    operation_id = "admin_queue_purge_dead_jobs", post,
    path = "/admin/queue/dead-jobs/purge",
    request_body = DeadJobsPurgeReq,
    responses(
        (status = 200, body = DeadJobsPurgeResp),
        (status = 403, description = "admin only"),
        (status = 422, description = "unknown queue"),
    )
)]
#[handler]
pub async fn purge_dead_jobs(
    axum::extract::State(app): axum::extract::State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<DeadJobsPurgeReq>,
) -> impl IntoResponse {
    let Some(keys) = dead_keys(&app, &req.queue) else {
        return unknown_queue(&req.queue);
    };
    match purge_dead(&app, &keys).await {
        Ok(purged) => {
            audit::record(
                &app.db,
                AuditEntry {
                    actor_id: admin.0.id,
                    action: "admin.queue.dead.purge",
                    target_type: Some("queue"),
                    target_id: Some(req.queue.clone()),
                    payload: serde_json::json!({ "queue": req.queue, "purged": purged }),
                    ip: ctx.ip_string(),
                    user_agent: ctx.user_agent.clone(),
                },
            )
            .await;
            Json(DeadJobsPurgeResp {
                queue: req.queue,
                purged,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, queue = %req.queue, "purge_dead_jobs failed");
            internal()
        }
    }
}

/// `(dead_jobs_set, job_data_hash, job_meta_hash)` Redis keys for a queue
/// label, resolved from each typed storage's apalis `Config` so they track
/// the lib's key scheme. `None` for an unknown label.
fn dead_keys(app: &AppState, queue: &str) -> Option<DeadKeys> {
    let j = &app.jobs;
    macro_rules! keys {
        ($s:expr) => {
            Some(DeadKeys {
                dead_set: $s.get_config().dead_jobs_set(),
                data_hash: $s.get_config().job_data_hash(),
                meta_hash: $s.get_config().job_meta_hash(),
            })
        };
    }
    match queue {
        "scan" => keys!(j.scan_storage),
        "scan_series" => keys!(j.scan_series_storage),
        "post_scan_thumbs" => keys!(j.post_scan_thumbs_storage),
        "post_scan_search" => keys!(j.post_scan_search_storage),
        "post_scan_dictionary" => keys!(j.post_scan_dictionary_storage),
        "metadata_search_series" => keys!(j.metadata_search_series_storage),
        "metadata_search_issue" => keys!(j.metadata_search_issue_storage),
        "metadata_apply_series" => keys!(j.metadata_apply_series_storage),
        "metadata_apply_issue" => keys!(j.metadata_apply_issue_storage),
        "rewrite_issue_sidecars" => keys!(j.rewrite_issue_sidecars_storage),
        "archive_edit" => keys!(j.archive_edit_storage),
        "backfill" => keys!(j.backfill_storage),
        "hash_backfill" => keys!(j.hash_backfill_storage),
        "relationship_suggest" => keys!(j.relationship_suggest_storage),
        "provider_coverage" => keys!(j.provider_coverage_storage),
        "entity_enrich" => keys!(j.entity_enrich_storage),
        "sidecar_reemit" => keys!(j.sidecar_reemit_storage),
        _ => None,
    }
}

/// The three Redis keys a queue's dead-letter handling touches.
struct DeadKeys {
    dead_set: String,
    data_hash: String,
    /// Prefix of the per-job meta hash: `{meta_hash}:{task_id}`.
    meta_hash: String,
}

impl DeadKeys {
    fn meta_key(&self, task_id: &str) -> String {
        format!("{}:{task_id}", self.meta_hash)
    }
}

/// The dead-letter fields of one job's `{queue}:meta:{id}` hash.
struct DeadMeta {
    done_at: Option<i64>,
    result: Option<String>,
}

/// `HMGET done_at result` for every id, pipelined.
async fn dead_meta(
    conn: &mut redis::aio::ConnectionManager,
    keys: &DeadKeys,
    ids: &[String],
) -> redis::RedisResult<Vec<DeadMeta>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut pipe = redis::pipe();
    for id in ids {
        pipe.cmd("HMGET")
            .arg(keys.meta_key(id))
            .arg("done_at")
            .arg("result");
    }
    let rows: Vec<(Option<String>, Option<String>)> = pipe.query_async(conn).await?;
    Ok(rows
        .into_iter()
        .map(|(done_at, result)| DeadMeta {
            done_at: done_at.and_then(|s| s.parse::<i64>().ok()),
            result,
        })
        .collect())
}

/// Read one page of a queue's dead set, newest-killed first. Type-erased: the
/// payload is the stored args as opaque JSON, so no per-queue code is needed
/// to list any of the queues. The dead set is unordered (a SET), so the page
/// is cut after sorting every member by its `done_at`; dead sets are small by
/// construction (an operator either retries or purges them).
async fn list_dead(
    app: &AppState,
    keys: &DeadKeys,
    offset: i64,
    limit: i64,
) -> redis::RedisResult<(Vec<DeadJob>, i64)> {
    let mut conn = app.jobs.redis.clone();
    let ids: Vec<String> = conn.smembers(&keys.dead_set).await?;
    let total = ids.len() as i64;
    if total == 0 || limit <= 0 {
        return Ok((Vec::new(), total));
    }
    let metas = dead_meta(&mut conn, keys, &ids).await?;
    let mut rows: Vec<(String, DeadMeta)> = ids.into_iter().zip(metas).collect();
    // Newest kill first; ties (and missing timestamps) fall back to the id,
    // which is a ULID and therefore time-ordered itself.
    rows.sort_by(|a, b| b.1.done_at.cmp(&a.1.done_at).then_with(|| b.0.cmp(&a.0)));
    let page: Vec<(String, DeadMeta)> = rows
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit as usize)
        .collect();
    if page.is_empty() {
        return Ok((Vec::new(), total));
    }
    let page_ids: Vec<String> = page.iter().map(|(id, _)| id.clone()).collect();
    let blobs: Vec<Option<String>> = redis::cmd("HMGET")
        .arg(&keys.data_hash)
        .arg(&page_ids)
        .query_async(&mut conn)
        .await?;
    let jobs = page
        .into_iter()
        .enumerate()
        .map(|(i, (task_id, meta))| {
            let payload = blobs
                .get(i)
                .and_then(|b| b.as_deref())
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok());
            DeadJob {
                task_id,
                failed_at: meta.done_at,
                error: meta.result,
                payload,
                targets: Vec::new(),
            }
        })
        .collect();
    Ok((jobs, total))
}

/// Re-enqueue a single dead job. Robust by construction: it fetches the stored
/// job via apalis `fetch_by_id`, decodes the args with the queue's own codec
/// (JSON) and `push`es a fresh copy (new task id, attempt counter reset to 0)
/// so the retry doesn't instantly re-die on the exhausted counter — then drops
/// the old dead entry and its meta hash. Returns `false` when the id isn't
/// actually in the queue's dead set (so a stale id can't double-enqueue a live
/// job).
async fn retry_one(app: &AppState, queue: &str, task_id: &str) -> anyhow::Result<bool> {
    let Some(keys) = dead_keys(app, queue) else {
        return Ok(false);
    };
    let mut conn = app.jobs.redis.clone();
    let dead: bool = conn.sismember(&keys.dead_set, task_id).await?;
    if !dead {
        return Ok(false);
    }
    let tid = TaskId::from_str(task_id)?;
    macro_rules! try_retry {
        ($s:expr) => {{
            let mut st = $s.clone();
            match st.fetch_by_id(&tid).await? {
                Some(task) => {
                    let args = serde_json::from_slice(&task.args)?;
                    st.push(args).await?;
                    true
                }
                None => false,
            }
        }};
    }
    let pushed = match queue {
        "scan" => try_retry!(app.jobs.scan_storage),
        "scan_series" => try_retry!(app.jobs.scan_series_storage),
        "post_scan_thumbs" => try_retry!(app.jobs.post_scan_thumbs_storage),
        "post_scan_search" => try_retry!(app.jobs.post_scan_search_storage),
        "post_scan_dictionary" => try_retry!(app.jobs.post_scan_dictionary_storage),
        "metadata_search_series" => try_retry!(app.jobs.metadata_search_series_storage),
        "metadata_search_issue" => try_retry!(app.jobs.metadata_search_issue_storage),
        "metadata_apply_series" => try_retry!(app.jobs.metadata_apply_series_storage),
        "metadata_apply_issue" => try_retry!(app.jobs.metadata_apply_issue_storage),
        "rewrite_issue_sidecars" => try_retry!(app.jobs.rewrite_issue_sidecars_storage),
        "archive_edit" => try_retry!(app.jobs.archive_edit_storage),
        "backfill" => try_retry!(app.jobs.backfill_storage),
        "hash_backfill" => try_retry!(app.jobs.hash_backfill_storage),
        "relationship_suggest" => try_retry!(app.jobs.relationship_suggest_storage),
        "provider_coverage" => try_retry!(app.jobs.provider_coverage_storage),
        "entity_enrich" => try_retry!(app.jobs.entity_enrich_storage),
        "sidecar_reemit" => try_retry!(app.jobs.sidecar_reemit_storage),
        _ => return Ok(false),
    };
    if !pushed {
        return Ok(false);
    }
    let _: () = redis::pipe()
        .srem(&keys.dead_set, task_id)
        .ignore()
        .hdel(&keys.data_hash, task_id)
        .ignore()
        .del(keys.meta_key(task_id))
        .ignore()
        .query_async(&mut conn)
        .await?;
    Ok(true)
}

/// Drop every dead job in a queue (member ids + their data + meta hashes).
/// Returns the count removed.
async fn purge_dead(app: &AppState, keys: &DeadKeys) -> redis::RedisResult<usize> {
    let mut conn = app.jobs.redis.clone();
    let ids: Vec<String> = conn.smembers(&keys.dead_set).await?;
    let n = ids.len();
    if n == 0 {
        return Ok(0);
    }
    let mut pipe = redis::pipe();
    pipe.del(&keys.dead_set).ignore();
    for id in &ids {
        pipe.hdel(&keys.data_hash, id).ignore();
        pipe.del(keys.meta_key(id)).ignore();
    }
    let _: () = pipe.query_async(&mut conn).await?;
    Ok(n)
}

// ───────────────────────── job lanes (Flair phase 5 B4) ─────────────────────────
//
// The Queue page's lanes show individual jobs moving Queued → Running → Done.
// Waiting / scheduled / running come from each queue's Redis keys (the same
// layout `JobRuntime::queue_counts` documents); Done comes from the
// process-local completion ring in `AppState::job_activity`, which also knows
// when each running job actually started.

/// Where a job card sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobCardState {
    /// Waiting for a worker.
    Waiting,
    /// Waiting out a retry delay until `next_run_at`.
    Scheduled,
    /// Fetched by a worker. `started_at` is set once it is executing (a
    /// prefetched job has none yet).
    Running,
    /// Finished (`ok` says how).
    Done,
}

/// The library a job's target belongs to.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema, PartialEq, Eq)]
pub struct JobTargetLibrary {
    pub id: String,
    pub name: String,
    pub slug: String,
}

/// The entity a job works on, resolved for display.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema, PartialEq, Eq)]
pub struct JobTarget {
    /// `issue` | `series` | `library`.
    pub kind: String,
    pub id: String,
    /// `"The Flash #12 — Learning Curve"`, `"The Flash (1987)"`, `"DC"`.
    pub title: String,
    /// Thumbnail URL: the issue's cover, or the series cover.
    pub cover_url: Option<String>,
    pub library: Option<JobTargetLibrary>,
    /// Slugs for links (issue and series targets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_slug: Option<String>,
}

/// One job in a lane.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobCard {
    /// apalis task id (ULID); stable across lanes, so it keys the card.
    pub task_id: String,
    pub queue: String,
    pub state: JobCardState,
    /// What the job does, e.g. `"Cover thumbnail"`, `"Library scan"`.
    pub label: String,
    /// Attempts so far (the running one included), when known.
    pub attempt: Option<u32>,
    /// Scheduled jobs: when the retry is due.
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Running and done jobs: when execution started.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Done jobs.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: Option<u64>,
    /// Done jobs: `true` on success.
    pub ok: Option<bool>,
    /// Done jobs that failed: the handler's error.
    pub error: Option<String>,
    /// The issue / series / library the job targets, when its payload names
    /// one that still exists.
    pub target: Option<JobTarget>,
}

/// One queue's lanes. Each list is capped at the request's `limit`; the
/// `*_count` fields are exact.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QueueJobsLane {
    pub queue: String,
    /// Next to run first.
    pub waiting: Vec<JobCard>,
    pub waiting_count: i64,
    /// Soonest retry first.
    pub scheduled: Vec<JobCard>,
    pub scheduled_count: i64,
    /// Longest-running first; prefetched (not yet started) jobs last.
    pub running: Vec<JobCard>,
    pub running_count: i64,
    /// Newest first. Process-local: empty after a restart.
    pub recent: Vec<JobCard>,
    pub recent_count: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct QueueJobsView {
    /// One entry per queue (or just the requested one), in a stable order.
    pub queues: Vec<QueueJobsLane>,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct QueueJobsQuery {
    /// Queue label; omit for every queue.
    pub queue: Option<String>,
    /// Cards per lane (default 20, max 100).
    pub limit: Option<u32>,
}

/// Default and ceiling for [`QueueJobsQuery::limit`].
const JOBS_DEFAULT_LIMIT: u32 = 20;
const JOBS_MAX_LIMIT: u32 = 100;

#[utoipa::path(
    operation_id = "admin_queue_jobs", get,
    path = "/admin/queue/jobs",
    params(QueueJobsQuery),
    responses(
        (status = 200, body = QueueJobsView),
        (status = 403, description = "admin only"),
        (status = 422, description = "unknown queue"),
    )
)]
#[handler]
pub async fn queue_jobs(
    axum::extract::State(app): axum::extract::State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<QueueJobsQuery>,
) -> impl IntoResponse {
    let limit = q
        .limit
        .unwrap_or(JOBS_DEFAULT_LIMIT)
        .clamp(1, JOBS_MAX_LIMIT) as usize;
    let labels: Vec<&'static str> = match q.queue.as_deref().filter(|s| !s.is_empty()) {
        Some(name) => match DEAD_QUEUES.iter().find(|l| **l == name) {
            Some(l) => vec![*l],
            None => return unknown_queue(name),
        },
        None => DEAD_QUEUES.to_vec(),
    };
    match list_queue_jobs(&app, &labels, limit).await {
        Ok(queues) => Json(QueueJobsView {
            queues,
            generated_at: chrono::Utc::now(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "queue_jobs: redis read failed");
            internal()
        }
    }
}

/// One queue's raw read before cards are built.
struct RawLane {
    label: &'static str,
    data_hash: String,
    meta_prefix: String,
    waiting: Vec<String>,
    waiting_count: i64,
    /// `(task_id, run_at unix secs)`.
    scheduled: Vec<(String, f64)>,
    scheduled_count: i64,
    running: Vec<String>,
    running_count: i64,
}

async fn list_queue_jobs(
    app: &AppState,
    labels: &[&'static str],
    limit: usize,
) -> anyhow::Result<Vec<QueueJobsLane>> {
    let all = app.jobs.queue_configs();
    let queues: Vec<(&'static str, &apalis_redis::Config)> = labels
        .iter()
        .filter_map(|l| all.iter().find(|(q, _)| q == l).copied())
        .collect();
    let mut conn = app.jobs.redis.clone();
    let now = crate::jobs::unix_now_secs();
    let lim = limit as isize;

    // Round-trip 1: every list head + count, and the worker names.
    let mut pipe = redis::pipe();
    for (_, cfg) in &queues {
        pipe.lrange(cfg.active_jobs_list(), 0, lim - 1)
            .llen(cfg.active_jobs_list())
            .cmd("ZRANGEBYSCORE")
            .arg(cfg.scheduled_jobs_set())
            .arg("-inf")
            .arg(now)
            .arg("LIMIT")
            .arg(0)
            .arg(limit)
            .zcount(cfg.scheduled_jobs_set(), "-inf", now)
            .cmd("ZRANGEBYSCORE")
            .arg(cfg.scheduled_jobs_set())
            .arg(format!("({now}"))
            .arg("+inf")
            .arg("WITHSCORES")
            .arg("LIMIT")
            .arg(0)
            .arg(limit)
            .zcount(cfg.scheduled_jobs_set(), format!("({now}"), "+inf")
            .zrange(cfg.workers_set(), 0, -1);
    }
    type First = (
        Vec<String>,
        i64,
        Vec<String>,
        i64,
        Vec<(String, f64)>,
        i64,
        Vec<String>,
    );
    let flat: Vec<redis::Value> = if queues.is_empty() {
        Vec::new()
    } else {
        pipe.query_async(&mut conn).await?
    };
    let mut it = flat.into_iter();
    let mut firsts: Vec<First> = Vec::with_capacity(queues.len());
    for _ in &queues {
        let mut next = || {
            it.next()
                .ok_or_else(|| anyhow::anyhow!("short redis reply"))
        };
        firsts.push((
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
            redis::from_redis_value(next()?)?,
        ));
    }

    // Round-trip 2: every worker's in-flight set.
    let mut pipe = redis::pipe();
    let mut any = false;
    for ((_, cfg), first) in queues.iter().zip(&firsts) {
        for w in &first.6 {
            pipe.smembers(crate::jobs::inflight_set_key(cfg, w));
            any = true;
        }
    }
    let sets: Vec<Vec<String>> = if any {
        pipe.query_async(&mut conn).await?
    } else {
        Vec::new()
    };
    let mut sets = sets.into_iter();

    let mut raws = Vec::with_capacity(queues.len());
    for ((label, cfg), (active, active_len, due, due_count, future, future_count, workers)) in
        queues.iter().zip(firsts)
    {
        let mut running: Vec<String> = sets.by_ref().take(workers.len()).flatten().collect();
        running.sort();
        running.dedup();
        // Due `scheduled` entries are waiting too (apalis-redis 1.0 pushes
        // every new job there with `run_at = now`); they run after the list.
        let mut waiting = active;
        waiting.extend(due);
        waiting.truncate(limit);
        raws.push(RawLane {
            label,
            data_hash: cfg.job_data_hash(),
            meta_prefix: cfg.job_meta_hash(),
            waiting,
            waiting_count: active_len + due_count,
            scheduled: future,
            scheduled_count: future_count,
            running_count: running.len() as i64,
            running,
        });
    }

    // The ring is process-local; read it and the running clock once.
    let mut lanes_recent = Vec::with_capacity(raws.len());
    let mut lanes_running = Vec::with_capacity(raws.len());
    for raw in &mut raws {
        let started: HashMap<String, crate::jobs::activity::RunningJob> =
            app.job_activity.running_in(raw.label).into_iter().collect();
        // Executing jobs first (longest-running first), then prefetched.
        raw.running
            .sort_by(|a, b| match (started.get(a), started.get(b)) {
                (Some(x), Some(y)) => x.started_at.cmp(&y.started_at),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.cmp(b),
            });
        raw.running.truncate(limit);
        let recent = app.job_activity.recent(raw.label);
        lanes_recent.push((
            recent.len() as i64,
            recent.into_iter().take(limit).collect::<Vec<_>>(),
        ));
        lanes_running.push(started);
    }

    // Round-trip 3: payloads + meta (attempts, lock time) for every card.
    let mut pipe = redis::pipe();
    let mut card_ids: Vec<Vec<String>> = Vec::with_capacity(raws.len());
    for (raw, (_, recent)) in raws.iter().zip(&lanes_recent) {
        let ids: Vec<String> = raw
            .waiting
            .iter()
            .cloned()
            .chain(raw.scheduled.iter().map(|(id, _)| id.clone()))
            .chain(raw.running.iter().cloned())
            .chain(recent.iter().map(|r| r.task_id.clone()))
            .collect();
        for id in &ids {
            pipe.hget(&raw.data_hash, id);
            pipe.cmd("HMGET")
                .arg(format!("{}:{id}", raw.meta_prefix))
                .arg("attempts")
                .arg("locked_at");
        }
        card_ids.push(ids);
    }
    let total_ids: usize = card_ids.iter().map(Vec::len).sum();
    type Meta = (Option<String>, Option<String>);
    let flat: Vec<redis::Value> = if total_ids == 0 {
        Vec::new()
    } else {
        pipe.query_async(&mut conn).await?
    };
    let mut it = flat.into_iter();
    let mut payloads: HashMap<(&'static str, String), serde_json::Value> = HashMap::new();
    let mut metas: HashMap<(&'static str, String), Meta> = HashMap::new();
    for (raw, ids) in raws.iter().zip(&card_ids) {
        for id in ids {
            let (Some(data), Some(meta)) = (it.next(), it.next()) else {
                break;
            };
            let data: Option<String> = redis::from_redis_value(data).unwrap_or(None);
            if let Some(v) = data.and_then(|d| serde_json::from_str(&d).ok()) {
                payloads.insert((raw.label, id.clone()), v);
            }
            let meta: Meta = redis::from_redis_value(meta).unwrap_or((None, None));
            metas.insert((raw.label, id.clone()), meta);
        }
    }

    let targets = resolve_job_targets(app, payloads.values()).await;

    let card = |label: &'static str, id: &str, state: JobCardState| -> JobCard {
        let key = (label, id.to_owned());
        let payload = payloads.get(&key);
        let (attempts, _) = metas.get(&key).cloned().unwrap_or((None, None));
        JobCard {
            task_id: id.to_owned(),
            queue: label.to_owned(),
            state,
            label: job_label(label, payload),
            attempt: attempts
                .and_then(|a| a.parse::<u32>().ok())
                .filter(|a| *a > 0),
            next_run_at: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            ok: None,
            error: None,
            target: payload.and_then(|p| targets.primary(p)),
        }
    };

    let mut out = Vec::with_capacity(raws.len());
    for ((raw, (recent_count, recent)), started) in raws.iter().zip(lanes_recent).zip(lanes_running)
    {
        let waiting = raw
            .waiting
            .iter()
            .map(|id| card(raw.label, id, JobCardState::Waiting))
            .collect();
        let scheduled = raw
            .scheduled
            .iter()
            .map(|(id, at)| {
                let mut c = card(raw.label, id, JobCardState::Scheduled);
                c.next_run_at = chrono::DateTime::from_timestamp(*at as i64, 0);
                c
            })
            .collect();
        let running = raw
            .running
            .iter()
            .map(|id| {
                let mut c = card(raw.label, id, JobCardState::Running);
                if let Some(r) = started.get(id) {
                    c.started_at = Some(r.started_at);
                    c.attempt = c.attempt.or(r.attempt);
                }
                c
            })
            .collect();
        let recent = recent
            .into_iter()
            .map(|r| {
                let mut c = card(raw.label, &r.task_id, JobCardState::Done);
                c.attempt = r.attempt.or(c.attempt);
                c.started_at = Some(r.started_at);
                c.finished_at = Some(r.finished_at);
                c.duration_ms = Some(r.duration_ms);
                c.ok = Some(r.ok);
                c.error = r.error;
                c
            })
            .collect();
        out.push(QueueJobsLane {
            queue: raw.label.to_owned(),
            waiting,
            waiting_count: raw.waiting_count,
            scheduled,
            scheduled_count: raw.scheduled_count,
            running,
            running_count: raw.running_count,
            recent,
            recent_count,
        });
    }
    Ok(out)
}

/// A human label for a job, from its queue and (when it refines it) payload.
pub(crate) fn job_label(queue: &str, payload: Option<&serde_json::Value>) -> String {
    let field = |k: &str| payload.and_then(|p| p.get(k)).and_then(|v| v.as_str());
    let label = match queue {
        "scan" => {
            let scoped = payload
                .and_then(|p| p.get("scope"))
                .is_some_and(|s| s.as_array().is_some_and(|a| !a.is_empty()));
            match (
                scoped,
                payload
                    .and_then(|p| p.get("force"))
                    .and_then(|f| f.as_bool()),
            ) {
                (true, _) => "Folder scan",
                (false, Some(true)) => "Full library rescan",
                _ => "Library scan",
            }
        }
        "scan_series" => match field("kind") {
            Some("issue") => "Issue rescan",
            _ => "Series rescan",
        },
        "post_scan_thumbs" => match field("kind") {
            Some("strip") => "Page thumbnails",
            Some("cover_and_strip") => "Cover + page thumbnails",
            _ => "Cover thumbnail",
        },
        "post_scan_search" => "Search index update",
        "post_scan_dictionary" => "Search dictionary rebuild",
        "metadata_search_series" => "Metadata search (series)",
        "metadata_search_issue" => "Metadata search (issue)",
        "metadata_apply_series" => "Metadata apply (series)",
        "metadata_apply_issue" => "Metadata apply (issue)",
        "rewrite_issue_sidecars" => "Sidecar rewrite",
        "archive_edit" => "Archive edit",
        "backfill" => match field("kind") {
            Some("cover_phash") => "Cover fingerprint backfill",
            Some("variant_cover") => "Variant cover backfill",
            Some("cover_variant") => "Cover variant backfill",
            Some("name_suffixes") => "Name suffix backfill",
            Some("csv_cache") => "Search cache backfill",
            _ => "Backfill",
        },
        "hash_backfill" => "Content hash backfill",
        "relationship_suggest" => "Relationship suggestions",
        "provider_coverage" => "Provider coverage analysis",
        "entity_enrich" => {
            return match field("kind") {
                Some(k) => format!("Profile fetch ({})", k.replace('_', " ")),
                None => "Profile fetch".to_owned(),
            };
        }
        "sidecar_reemit" => "Sidecar re-emit",
        other => return other.replace('_', " "),
    };
    label.to_owned()
}

/// Display targets for a set of payloads: [`work_targets`] labels and slugs,
/// plus each target's cover and library.
///
/// [`work_targets`]: crate::api::work_targets
struct JobTargets {
    resolved: crate::api::work_targets::Resolved,
    issue_covers: HashSet<String>,
    series_covers: HashMap<uuid::Uuid, String>,
    /// Owning library per issue / series / library id.
    library_of: HashMap<String, uuid::Uuid>,
    libraries: HashMap<uuid::Uuid, JobTargetLibrary>,
}

impl JobTargets {
    /// The payload's first target (issue, then series, then library).
    fn primary(&self, payload: &serde_json::Value) -> Option<JobTarget> {
        let t = crate::api::work_targets::targets_for(payload, &self.resolved)
            .into_iter()
            .next()?;
        let cover_url = match t.kind.as_str() {
            "issue" if self.issue_covers.contains(&t.id) => {
                Some(format!("/issues/{}/pages/0/thumb", t.id))
            }
            "series" => uuid::Uuid::parse_str(&t.id)
                .ok()
                .and_then(|u| self.series_covers.get(&u))
                .map(|issue| format!("/issues/{issue}/pages/0/thumb")),
            _ => None,
        };
        let library = self
            .library_of
            .get(&t.id)
            .and_then(|l| self.libraries.get(l))
            .cloned();
        Some(JobTarget {
            kind: t.kind,
            id: t.id,
            title: t.label,
            cover_url,
            library,
            series_slug: t.series_slug,
            issue_slug: t.issue_slug,
        })
    }
}

async fn resolve_job_targets<'a>(
    app: &AppState,
    payloads: impl Iterator<Item = &'a serde_json::Value>,
) -> JobTargets {
    use entity::{issue, library, series};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let payloads: Vec<&serde_json::Value> = payloads.collect();
    let mut refs = crate::api::work_targets::TargetRefs::default();
    for p in &payloads {
        crate::api::work_targets::collect_refs(p, &mut refs);
    }
    let resolved = match crate::api::work_targets::resolve(app, refs).await {
        Ok(r) => r,
        Err(e) => {
            // Labels are a convenience; the cards still show.
            tracing::warn!(error = %e, "queue_jobs: target labels failed");
            crate::api::work_targets::Resolved::default()
        }
    };
    let mut out = JobTargets {
        resolved,
        issue_covers: HashSet::new(),
        series_covers: HashMap::new(),
        library_of: HashMap::new(),
        libraries: HashMap::new(),
    };
    // Only the primary target of each payload is shown.
    let primaries: Vec<crate::api::work_targets::WorkTargetView> = payloads
        .iter()
        .filter_map(|p| {
            crate::api::work_targets::targets_for(p, &out.resolved)
                .into_iter()
                .next()
        })
        .collect();
    let issue_ids: HashSet<String> = primaries
        .iter()
        .filter(|t| t.kind == "issue")
        .map(|t| t.id.clone())
        .collect();
    let series_ids: HashSet<uuid::Uuid> = primaries
        .iter()
        .filter(|t| t.kind == "series")
        .filter_map(|t| uuid::Uuid::parse_str(&t.id).ok())
        .collect();
    let mut library_ids: HashSet<uuid::Uuid> = primaries
        .iter()
        .filter(|t| t.kind == "library")
        .filter_map(|t| uuid::Uuid::parse_str(&t.id).ok())
        .collect();
    for l in &library_ids {
        out.library_of.insert(l.to_string(), *l);
    }
    if !issue_ids.is_empty() {
        let rows: Vec<(String, uuid::Uuid, issue::IssueState)> = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .column(issue::Column::LibraryId)
            .column(issue::Column::State)
            .filter(issue::Column::Id.is_in(issue_ids.iter().cloned()))
            .into_tuple()
            .all(&app.db)
            .await
            .unwrap_or_default();
        for (id, lib, state) in rows {
            if state == issue::IssueState::Active {
                out.issue_covers.insert(id.clone());
            }
            library_ids.insert(lib);
            out.library_of.insert(id, lib);
        }
    }
    if !series_ids.is_empty() {
        let ids: Vec<uuid::Uuid> = series_ids.iter().copied().collect();
        out.series_covers = crate::api::series::series_cover_issue_ids(
            &app.db,
            &ids,
            &crate::library::access::VisibleLibraries::unrestricted(),
        )
        .await;
        let rows: Vec<(uuid::Uuid, uuid::Uuid)> = series::Entity::find()
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::LibraryId)
            .filter(series::Column::Id.is_in(ids))
            .into_tuple()
            .all(&app.db)
            .await
            .unwrap_or_default();
        for (id, lib) in rows {
            library_ids.insert(lib);
            out.library_of.insert(id.to_string(), lib);
        }
    }
    if !library_ids.is_empty() {
        let rows: Vec<(uuid::Uuid, String, String)> = library::Entity::find()
            .select_only()
            .column(library::Column::Id)
            .column(library::Column::Name)
            .column(library::Column::Slug)
            .filter(library::Column::Id.is_in(library_ids))
            .into_tuple()
            .all(&app.db)
            .await
            .unwrap_or_default();
        for (id, name, slug) in rows {
            out.libraries.insert(
                id,
                JobTargetLibrary {
                    id: id.to_string(),
                    name,
                    slug,
                },
            );
        }
    }
    out
}

fn unknown_queue(queue: &str) -> axum::response::Response {
    super::error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown_queue",
        &format!("unknown queue: {queue}"),
    )
}

pub(crate) async fn queue_depth_counts(app: &AppState) -> anyhow::Result<QueueDepthView> {
    let counts = app.jobs.queue_counts().await?;
    let of = |label: &str| {
        counts
            .iter()
            .find(|c| c.queue == label)
            .map_or(0, crate::jobs::QueueCount::outstanding)
    };
    Ok(QueueDepthView {
        scan: of("scan"),
        scan_series: of("scan_series"),
        post_scan_thumbs: of("post_scan_thumbs"),
        post_scan_search: of("post_scan_search"),
        post_scan_dictionary: of("post_scan_dictionary"),
        metadata_search_series: of("metadata_search_series"),
        metadata_search_issue: of("metadata_search_issue"),
        metadata_apply_series: of("metadata_apply_series"),
        metadata_apply_issue: of("metadata_apply_issue"),
        rewrite_issue_sidecars: of("rewrite_issue_sidecars"),
        archive_edit: of("archive_edit"),
        backfill: of("backfill"),
        hash_backfill: of("hash_backfill"),
        relationship_suggest: of("relationship_suggest"),
        provider_coverage: of("provider_coverage"),
        entity_enrich: of("entity_enrich"),
        sidecar_reemit: of("sidecar_reemit"),
        total: counts
            .iter()
            .map(crate::jobs::QueueCount::outstanding)
            .sum(),
        in_flight: counts.iter().map(|c| c.in_flight).sum(),
        queues: counts
            .iter()
            .map(|c| QueueDepthEntry {
                queue: c.queue.to_owned(),
                waiting: c.waiting,
                scheduled: c.scheduled,
                in_flight: c.in_flight,
            })
            .collect(),
    })
}

fn clear_patterns(target: QueueClearTarget) -> &'static [&'static str] {
    match target {
        QueueClearTarget::All => &[
            "*server::jobs::scan::Job*",
            "*server::jobs::scan_series::Job*",
            "*server::jobs::post_scan::ThumbsJob*",
            "*server::jobs::post_scan::SearchJob*",
            "*server::jobs::post_scan::DictionaryJob*",
            "scan:*",
            "*server::jobs::metadata_search::SearchSeriesJob*",
            "*server::jobs::metadata_search::SearchIssueJob*",
            "*server::jobs::metadata_apply::ApplySeriesJob*",
            "*server::jobs::metadata_apply::ApplyIssueJob*",
            "*server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob*",
            "*server::jobs::provider_coverage::ProviderCoverageJob*",
            "*server::jobs::sidecar_reemit::SidecarReemitJob*",
            "sidecar_reemit:*",
        ],
        QueueClearTarget::Metadata => &[
            "*server::jobs::metadata_search::SearchSeriesJob*",
            "*server::jobs::metadata_search::SearchIssueJob*",
            "*server::jobs::metadata_apply::ApplySeriesJob*",
            "*server::jobs::metadata_apply::ApplyIssueJob*",
            "*server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob*",
            "*server::jobs::provider_coverage::ProviderCoverageJob*",
            "*server::jobs::sidecar_reemit::SidecarReemitJob*",
            "sidecar_reemit:*",
        ],
        QueueClearTarget::Scans => &[
            "*server::jobs::scan::Job*",
            "*server::jobs::scan_series::Job*",
            "scan:*",
        ],
        QueueClearTarget::Thumbnails => &["*server::jobs::post_scan::ThumbsJob*"],
    }
}

/// `{queue}:workers` (liveness zset) and `{queue}:workers:metadata:{name}`.
fn is_worker_registration(key: &str) -> bool {
    key.ends_with(":workers") || key.contains(":workers:metadata:")
}

async fn delete_matching_keys(
    conn: &mut ConnectionManager,
    pattern: &str,
) -> redis::RedisResult<usize> {
    let mut cursor = 0_u64;
    let mut keys = Vec::<String>::new();

    loop {
        let (next, mut batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(1000)
            .query_async(&mut *conn)
            .await?;
        keys.append(&mut batch);
        cursor = next;
        if cursor == 0 {
            break;
        }
    }

    // Keep the live workers' registrations. apalis-redis refuses to fetch
    // for a worker missing from `{queue}:workers` ("worker not registered"),
    // so deleting it killed the queue's worker until the job monitor
    // restarted. Clearing is about jobs, not workers.
    keys.retain(|k| !is_worker_registration(k));

    let mut deleted = 0_usize;
    for chunk in keys.chunks(500) {
        let n: usize = redis::cmd("DEL").arg(chunk).query_async(&mut *conn).await?;
        deleted += n;
    }
    Ok(deleted)
}

fn internal() -> axum::response::Response {
    super::error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}
