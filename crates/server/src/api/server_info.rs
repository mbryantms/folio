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
    /// Human-readable build version. `git describe --tags --always --dirty`
    /// at build time, falling back to `"dev"` when the build script
    /// couldn't shell to git (e.g. inside Docker without `.git`). Common
    /// shapes:
    ///   - `"v0.1.8"`             — clean checkout on a release tag
    ///   - `"v0.1.8-3-gabcd1234"` — 3 commits past v0.1.8
    ///   - `"v0.1.8-dirty"`       — release tag with uncommitted changes
    ///   - `"abcd1234"`           — no tags exist (bare SHA fallback)
    ///   - `"dev"`                — build script couldn't reach git
    ///
    /// UI links this to the GitHub release page when it starts with `v`
    /// and has no `-` suffix (i.e. a clean tag), and renders verbatim
    /// otherwise.
    pub version: &'static str,
    /// 12-char short SHA for display. Read from `COMIC_BUILD_SHA`
    /// (env override or build.rs `git rev-parse HEAD`). Falls back
    /// to `"unknown"`.
    pub build_sha: &'static str,
    /// 40-char full SHA used to construct stable commit URLs. Falls
    /// back to `"unknown"` when the build script couldn't reach git.
    pub build_sha_full: &'static str,
    /// Browse URL for the repo this binary was built from. Auto-detected
    /// from `git config --get remote.origin.url` at build time and
    /// normalized to `https://host/owner/repo` (strips `.git`, converts
    /// SSH → HTTPS). CI passes `COMIC_BUILD_REPO_URL` directly so Docker
    /// builds without `.git` still get the link. `None` when no remote
    /// was detected and no override was passed.
    pub repo_url: Option<&'static str>,
    /// Unix-seconds at build time (UTC). UI renders as
    /// `Built — N hours ago`. `None` when the build script couldn't
    /// resolve the system clock (shouldn't happen in practice).
    pub build_epoch: Option<i64>,
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
    // `Cargo.toml` declares `version = "0.0.0"` and the release ritual
    // uses git tags only. The displayed version comes from
    // `git describe --tags --always --dirty` captured by build.rs into
    // `COMIC_BUILD_TAG`. Falls back to `"dev"` for builds where the
    // build script couldn't reach git (e.g. Docker without `.git`).
    let version: &'static str = option_env!("COMIC_BUILD_TAG").unwrap_or("dev");
    let build_sha: &'static str = option_env!("COMIC_BUILD_SHA").unwrap_or("unknown");
    let build_sha_full: &'static str = option_env!("COMIC_BUILD_SHA_FULL").unwrap_or("unknown");
    // `COMIC_BUILD_REPO_URL` is the empty string when no remote was
    // detected and no env override was passed. Surface that as `None`
    // so the UI doesn't render a broken link.
    let repo_url: Option<&'static str> =
        option_env!("COMIC_BUILD_REPO_URL").filter(|s| !s.is_empty());
    let build_epoch: Option<i64> =
        option_env!("COMIC_BUILD_EPOCH").and_then(|s| s.parse::<i64>().ok());
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
        build_sha_full,
        repo_url,
        build_epoch,
        uptime_secs,
        postgres_ok,
        redis_ok,
        scheduler_running: true,
        watchers_enabled,
    })
    .into_response()
}
