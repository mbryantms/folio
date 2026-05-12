//! `GET /admin/queue-depth` — current pending-job counts for the apalis
//! queues that drive scans (spec §3 + §8). Polled by the admin topbar so an
//! operator can see when a backlog is draining.
//!
//! "Pending" here means `len()` from `apalis::prelude::Storage`, which is
//! `HLEN(job_data_hash) - ZCOUNT(done_jobs_set)` — i.e., all jobs minus
//! finished ones. In-flight jobs are still counted as pending.

use apalis::prelude::Storage;
use axum::{
    Json, Router,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/queue-depth", get(queue_depth))
        .route("/admin/queue/clear", post(clear_queue))
}

#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct QueueDepthView {
    pub scan: i64,
    pub scan_series: i64,
    pub post_scan_thumbs: i64,
    pub post_scan_search: i64,
    pub post_scan_dictionary: i64,
    /// Sum across all queues — convenient for the topbar pill.
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
    get,
    path = "/admin/queue-depth",
    responses(
        (status = 200, body = QueueDepthView),
        (status = 403, description = "admin only"),
    )
)]
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
    post,
    path = "/admin/queue/clear",
    request_body = QueueClearReq,
    responses(
        (status = 200, body = QueueClearResp),
        (status = 403, description = "admin only"),
    )
)]
pub async fn clear_queue(
    axum::extract::State(app): axum::extract::State<AppState>,
    _admin: RequireAdmin,
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

    Json(QueueClearResp {
        target: req.target,
        deleted_keys,
        before,
        after,
        running_jobs_may_finish: before.total > after.total,
    })
    .into_response()
}

async fn queue_depth_counts(app: &AppState) -> anyhow::Result<QueueDepthView> {
    let mut scan = app.jobs.scan_storage.clone();
    let mut scan_series = app.jobs.scan_series_storage.clone();
    let mut thumbs = app.jobs.post_scan_thumbs_storage.clone();
    let mut search = app.jobs.post_scan_search_storage.clone();
    let mut dictionary = app.jobs.post_scan_dictionary_storage.clone();

    let (scan_n, scan_series_n, thumbs_n, search_n, dictionary_n) = tokio::try_join!(
        scan.len(),
        scan_series.len(),
        thumbs.len(),
        search.len(),
        dictionary.len(),
    )?;

    let total = scan_n + scan_series_n + thumbs_n + search_n + dictionary_n;
    Ok(QueueDepthView {
        scan: scan_n,
        scan_series: scan_series_n,
        post_scan_thumbs: thumbs_n,
        post_scan_search: search_n,
        post_scan_dictionary: dictionary_n,
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
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": {"code": "internal", "message": "internal"}})),
    )
        .into_response()
}
