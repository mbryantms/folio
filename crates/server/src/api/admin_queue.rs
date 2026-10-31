//! `GET /admin/queue-depth` — current pending-job counts for the apalis
//! queues that drive scans (spec §3 + §8). Polled by the admin topbar so an
//! operator can see when a backlog is draining.
//!
//! "Pending" here means every job that has not finished: waiting for a
//! worker, waiting out a retry delay, or fetched by a worker (executing or
//! in its prefetch buffer) — [`crate::jobs::JobRuntime::queue_counts`].
//! apalis's own `Storage::len()` is only the first of those, so it reads 0
//! while the last jobs of a drain are still running.

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
    /// rewrites and coverage analyses. Their pending runs close as
    /// `cancelled` and their batches finish.
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
        ],
        QueueClearTarget::Metadata => &[
            "*server::jobs::metadata_search::SearchSeriesJob*",
            "*server::jobs::metadata_search::SearchIssueJob*",
            "*server::jobs::metadata_apply::ApplySeriesJob*",
            "*server::jobs::metadata_apply::ApplyIssueJob*",
            "*server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob*",
            "*server::jobs::provider_coverage::ProviderCoverageJob*",
        ],
        QueueClearTarget::Scans => &[
            "*server::jobs::scan::Job*",
            "*server::jobs::scan_series::Job*",
            "scan:*",
        ],
        QueueClearTarget::Thumbnails => &["*server::jobs::post_scan::ThumbsJob*"],
    }
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
