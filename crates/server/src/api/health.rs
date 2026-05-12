//! `/healthz` (liveness) and `/readyz` (readiness) — §12.1.

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize, utoipa::ToSchema)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_seconds: i64,
    /// Short git SHA captured at build time. Suffixed with `-dirty` when the
    /// working tree had uncommitted changes during compilation. `unknown`
    /// when not built inside a git checkout (e.g. tarball builds).
    pub build_sha: &'static str,
    /// Build timestamp (Unix epoch seconds, UTC). Lets `just dev-status`
    /// flag servers that are still running an old binary after a rebuild.
    pub build_epoch: u64,
}

const BUILD_SHA: &str = env!("COMIC_BUILD_SHA");
const BUILD_EPOCH: &str = env!("COMIC_BUILD_EPOCH");

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
}

#[utoipa::path(
    get,
    path = "/healthz",
    responses((status = 200, body = Health))
)]
pub async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = (chrono::Utc::now() - state.started_at).num_seconds();
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: uptime,
        build_sha: BUILD_SHA,
        build_epoch: BUILD_EPOCH.parse().unwrap_or(0),
    })
}

#[utoipa::path(
    get,
    path = "/readyz",
    responses(
        (status = 200, body = Health),
        (status = 503, description = "dependency unreachable")
    )
)]
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    // Probe both Postgres and Redis in parallel. The server hard-requires
    // both at boot (apalis fails-fast on Redis at startup) but a Redis
    // outage that happens *after* the server is up will silently break
    // scans, thumbs, rate limits, and the failed-auth lockout — `/readyz`
    // returning 200 in that state would let the load balancer keep sending
    // traffic to a degraded instance. Both checks are short (1s read timeout)
    // and run concurrently so an unhealthy dependency doesn't drag the probe
    // out past the orchestrator's own timeout window.
    let db_fut = async {
        state
            .db
            .ping()
            .await
            .map_err(|e| tracing::warn!(error = %e, "db ping failed"))
            .is_ok()
    };
    let redis_fut = async {
        let mut conn = state.jobs.redis.clone();
        let res: Result<String, _> = redis::cmd("PING").query_async(&mut conn).await;
        res.map_err(|e| tracing::warn!(error = %e, "redis ping failed"))
            .is_ok()
    };
    let (db_ok, redis_ok) = tokio::join!(db_fut, redis_fut);

    if db_ok && redis_ok {
        let uptime = (chrono::Utc::now() - state.started_at).num_seconds();
        (
            StatusCode::OK,
            Json(Health {
                status: "ready",
                version: env!("CARGO_PKG_VERSION"),
                uptime_seconds: uptime,
                build_sha: BUILD_SHA,
                build_epoch: BUILD_EPOCH.parse().unwrap_or(0),
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "db": if db_ok { "ok" } else { "unreachable" },
                "redis": if redis_ok { "ok" } else { "unreachable" },
            })),
        )
            .into_response()
    }
}
