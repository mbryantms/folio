//! `GET /admin/server/info` — version, uptime, dependency pings, scheduler /
//! watcher status. Used by the M6c dashboard's "service status" tile and by
//! the M6e `/admin/server` page when it lands.
//!
//! Admin-only. The endpoint runs two cheap probes per request (postgres
//! `SELECT 1`, redis `PING`) — fine because the dashboard polls at human
//! cadences, not real-time.

use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use entity::library;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Statement};
use serde::Serialize;

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/server/info", get(info))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServerInfoView {
    /// `CARGO_PKG_VERSION` baked in at build time.
    pub version: &'static str,
    /// Optional `BUILD_SHA` env var captured at build time, else `"dev"`.
    pub build_sha: &'static str,
    /// Seconds since `AppState` was constructed (process start).
    pub uptime_secs: i64,
    pub postgres_ok: bool,
    pub redis_ok: bool,
    /// True once the cron scheduler has been wired (we always start it at
    /// boot; if it failed, that was logged but we still serve traffic). The
    /// proper "is the scheduler ticking" check would walk the JobScheduler;
    /// for now just report whether the AppState was built (i.e. the server
    /// is up).
    pub scheduler_running: bool,
    /// Count of libraries with `file_watch_enabled = true`. The scanner v1
    /// codebase exposes the flag; the in-process watcher is wired separately.
    pub watchers_enabled: i64,
}

#[utoipa::path(
    get,
    path = "/admin/server/info",
    responses(
        (status = 200, body = ServerInfoView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn info(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let version: &'static str = env!("CARGO_PKG_VERSION");
    let build_sha: &'static str = option_env!("BUILD_SHA").unwrap_or("dev");
    let uptime_secs = (chrono::Utc::now() - app.started_at).num_seconds().max(0);

    // Postgres: a trivial round-trip is cheaper than `pg_isready` and gives
    // us the same signal — does the connection pool have a working session?
    let postgres_ok = app
        .db
        .execute(Statement::from_string(
            app.db.get_database_backend(),
            "SELECT 1".to_string(),
        ))
        .await
        .is_ok();

    // Redis: a real PING beats `client.is_open()` (which only checks the
    // local handle).
    let redis_ok = {
        let mut conn = app.jobs.redis.clone();
        let result: redis::RedisResult<String> = redis::cmd("PING").query_async(&mut conn).await;
        match result {
            Ok(reply) => reply.eq_ignore_ascii_case("PONG"),
            Err(_) => false,
        }
    };

    let watchers_enabled = library::Entity::find()
        .filter(library::Column::FileWatchEnabled.eq(true))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;

    Json(ServerInfoView {
        version,
        build_sha,
        uptime_secs,
        postgres_ok,
        redis_ok,
        scheduler_running: true,
        watchers_enabled,
    })
    .into_response()
}
