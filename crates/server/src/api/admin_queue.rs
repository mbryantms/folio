//! `GET /admin/queue-depth` — current pending-job counts for the apalis
//! queues that drive scans (spec §3 + §8). Polled by the admin topbar so an
//! operator can see when a backlog is draining.
//!
//! "Pending" here means `len()` from `apalis::prelude::Storage`, which is
//! `HLEN(job_data_hash) - ZCOUNT(done_jobs_set)` — i.e., all jobs minus
//! finished ones. In-flight jobs are still counted as pending.

use std::str::FromStr;

use apalis::prelude::{Storage, TaskId};
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

/// The eleven apalis queue labels, matching the keys in
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
];

#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct QueueDepthView {
    pub scan: i64,
    pub scan_series: i64,
    pub post_scan_thumbs: i64,
    pub post_scan_search: i64,
    pub post_scan_dictionary: i64,
    /// Pending archive page-edit jobs (single + bulk; M7).
    pub archive_edit: i64,
    /// Pending backfill drains (cover-phash / variant-cover; B17).
    pub backfill: i64,
    /// Sum across all queues — convenient for the topbar pill.
    pub total: i64,
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
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct QueueClearReq {
    pub target: QueueClearTarget,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QueueClearResp {
    pub target: QueueClearTarget,
    pub deleted_keys: usize,
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
    }

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
        before,
        after,
        running_jobs_may_finish: before.total > after.total,
    })
    .into_response()
}

// ───────────────────────── dead-job inspection (D8b) ─────────────────────────
//
// apalis moves a job to `{queue}:dead` after it exhausts its 5 attempts,
// stamping the kill time as the ZSET score and the final error into a
// `{queue}:data::result` hash. `dead_letters` already surfaces the per-queue
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
    let Some((dead_set, data_hash)) = dead_keys(&app, &q.queue) else {
        return unknown_queue(&q.queue);
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (i64::from(page) - 1) * i64::from(page_size);
    match list_dead(&app, &dead_set, &data_hash, offset, i64::from(page_size)).await {
        Ok((jobs, total)) => Json(DeadJobsView {
            queue: q.queue,
            jobs,
            total,
            page,
            page_size,
        })
        .into_response(),
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
    let Some((dead_set, data_hash)) = dead_keys(&app, &req.queue) else {
        return unknown_queue(&req.queue);
    };
    match purge_dead(&app, &dead_set, &data_hash).await {
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

/// `(dead_jobs_set, job_data_hash)` Redis keys for a queue label, resolved from
/// each typed storage's apalis `Config` so they track the lib's key scheme.
/// `None` for an unknown label.
fn dead_keys(app: &AppState, queue: &str) -> Option<(String, String)> {
    let j = &app.jobs;
    macro_rules! keys {
        ($s:expr) => {
            Some((
                $s.get_config().dead_jobs_set(),
                $s.get_config().job_data_hash(),
            ))
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
        _ => None,
    }
}

/// Read one page of a queue's dead set, newest-killed first. Type-erased: the
/// payload is the stored `Request`'s `args` sub-object as opaque JSON, so no
/// per-queue code is needed to list any of the eleven queues.
async fn list_dead(
    app: &AppState,
    dead_set: &str,
    data_hash: &str,
    offset: i64,
    limit: i64,
) -> redis::RedisResult<(Vec<DeadJob>, i64)> {
    let mut conn = app.jobs.redis.clone();
    let total: i64 = conn.zcard(dead_set).await?;
    if total == 0 || limit <= 0 {
        return Ok((Vec::new(), total));
    }
    let stop = offset + limit - 1;
    let pairs: Vec<(String, i64)> = conn
        .zrevrange_withscores(dead_set, offset as isize, stop as isize)
        .await?;
    if pairs.is_empty() {
        return Ok((Vec::new(), total));
    }
    let ids: Vec<String> = pairs.iter().map(|(id, _)| id.clone()).collect();
    let result_hash = format!("{data_hash}::result");
    let blobs: Vec<Option<String>> = redis::cmd("HMGET")
        .arg(data_hash)
        .arg(&ids)
        .query_async(&mut conn)
        .await?;
    let errors: Vec<Option<String>> = redis::cmd("HMGET")
        .arg(&result_hash)
        .arg(&ids)
        .query_async(&mut conn)
        .await?;
    let jobs = pairs
        .into_iter()
        .enumerate()
        .map(|(i, (task_id, score))| {
            let payload = blobs
                .get(i)
                .and_then(|b| b.as_deref())
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("args").cloned());
            let error = errors.get(i).and_then(Clone::clone);
            DeadJob {
                task_id,
                failed_at: Some(score),
                error,
                payload,
            }
        })
        .collect();
    Ok((jobs, total))
}

/// Re-enqueue a single dead job. Robust by construction: it fetches the typed
/// job via apalis `fetch_by_id` and `push`es a fresh copy (new task id, attempt
/// counter reset to 0) so the retry doesn't instantly re-die on the exhausted
/// counter — then drops the old dead entry. Returns `false` when the id isn't
/// actually in the queue's dead set (so a stale id can't double-enqueue a live
/// job).
async fn retry_one(app: &AppState, queue: &str, task_id: &str) -> anyhow::Result<bool> {
    let Some((dead_set, data_hash)) = dead_keys(app, queue) else {
        return Ok(false);
    };
    let mut conn = app.jobs.redis.clone();
    let score: Option<i64> = conn.zscore(&dead_set, task_id).await?;
    if score.is_none() {
        return Ok(false);
    }
    let tid = TaskId::from_str(task_id)?;
    macro_rules! try_retry {
        ($s:expr) => {{
            let mut st = $s.clone();
            match st.fetch_by_id(&tid).await? {
                Some(req) => {
                    st.push(req.args).await?;
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
        _ => return Ok(false),
    };
    if !pushed {
        return Ok(false);
    }
    let result_hash = format!("{data_hash}::result");
    let _: () = redis::pipe()
        .zrem(&dead_set, task_id)
        .ignore()
        .hdel(&data_hash, task_id)
        .ignore()
        .hdel(&result_hash, task_id)
        .ignore()
        .query_async(&mut conn)
        .await?;
    Ok(true)
}

/// Drop every dead job in a queue (member ids + their data + result rows).
/// Returns the count removed.
async fn purge_dead(app: &AppState, dead_set: &str, data_hash: &str) -> redis::RedisResult<usize> {
    let mut conn = app.jobs.redis.clone();
    let ids: Vec<String> = conn.zrange(dead_set, 0, -1).await?;
    let n = ids.len();
    if n == 0 {
        return Ok(0);
    }
    let result_hash = format!("{data_hash}::result");
    let mut pipe = redis::pipe();
    pipe.del(dead_set).ignore();
    for id in &ids {
        pipe.hdel(data_hash, id).ignore();
        pipe.hdel(&result_hash, id).ignore();
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
    let mut scan = app.jobs.scan_storage.clone();
    let mut scan_series = app.jobs.scan_series_storage.clone();
    let mut thumbs = app.jobs.post_scan_thumbs_storage.clone();
    let mut search = app.jobs.post_scan_search_storage.clone();
    let mut dictionary = app.jobs.post_scan_dictionary_storage.clone();
    let mut archive_edit = app.jobs.archive_edit_storage.clone();
    let mut backfill = app.jobs.backfill_storage.clone();

    let (scan_n, scan_series_n, thumbs_n, search_n, dictionary_n, archive_edit_n, backfill_n) = tokio::try_join!(
        scan.len(),
        scan_series.len(),
        thumbs.len(),
        search.len(),
        dictionary.len(),
        archive_edit.len(),
        backfill.len(),
    )?;

    let total =
        scan_n + scan_series_n + thumbs_n + search_n + dictionary_n + archive_edit_n + backfill_n;
    Ok(QueueDepthView {
        scan: scan_n,
        scan_series: scan_series_n,
        post_scan_thumbs: thumbs_n,
        post_scan_search: search_n,
        post_scan_dictionary: dictionary_n,
        archive_edit: archive_edit_n,
        backfill: backfill_n,
        total,
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
