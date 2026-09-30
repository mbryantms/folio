//! `GET /admin/server/info` — version, uptime, dependency pings, scheduler /
//! watcher status. Used by the M6c dashboard's "service status" tile and by
//! the M6e `/admin/server` page when it lands.
//!
//! Admin-only. The endpoint runs two cheap probes per request (postgres
//! `SELECT 1`, redis `PING`) — fine because the dashboard polls at human
//! cadences, not real-time.

use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use entity::library;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Statement,
};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::RequireAdmin;
use crate::library::watcher::{WatchMode, WatcherStatus};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(info))
        .routes(routes!(restart_pending))
        .routes(routes!(watchers))
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
    /// Count of libraries with `file_watch_enabled = true`.
    pub watchers_enabled: i64,
    /// Count of libraries whose file watcher is actually running (inotify
    /// or poll mode). Lower than `watchers_enabled` when a watcher couldn't
    /// start — `GET /admin/server/watchers` says why. (WP-3.1)
    pub watchers_running: i64,
}

#[utoipa::path(
    operation_id = "server_info_info",    get,
    path = "/admin/server/info",
    responses(
        (status = 200, body = ServerInfoView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
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
        .execute_raw(Statement::from_string(
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

    let watchers_running = app
        .watchers
        .statuses()
        .values()
        .filter(|s| s.mode != WatchMode::Disabled)
        .count() as i64;

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
        watchers_running,
    })
    .into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RestartPendingItem {
    /// Setting registry key, e.g. `workers.scan_count`.
    pub key: String,
    /// Value the running process booted with (display string).
    pub boot_value: String,
    /// Value currently persisted — what the admin form shows and what the
    /// next restart will pick up (display string).
    pub current_value: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RestartPendingView {
    /// Boot-only settings whose persisted value now differs from the value
    /// the process is actually running. Empty when nothing needs a restart.
    pub pending: Vec<RestartPendingItem>,
}

/// `GET /admin/server/restart-pending` — boot-only settings (worker pools,
/// ZIP LRU, the metadata weekly-refresh cron) whose persisted value has been
/// changed since startup and so won't take effect until the server restarts.
///
/// Diffs the boot-time `Config` snapshot ([`AppState::cfg_boot`]) against the
/// live one. Read-only; no audit row (allow-listed in `audit-check`).
#[utoipa::path(
    operation_id = "server_info_restart_pending",    get,
    path = "/admin/server/restart-pending",
    responses(
        (status = 200, body = RestartPendingView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn restart_pending(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let boot = app.cfg_boot();
    let current = app.cfg();
    let pending = crate::settings::registry::RESTART_REQUIRED_KEYS
        .iter()
        .filter_map(|&key| {
            let boot_value = crate::config::restart_setting_value(&boot, key)?;
            let current_value = crate::config::restart_setting_value(&current, key)?;
            if boot_value == current_value {
                return None;
            }
            Some(RestartPendingItem {
                key: key.to_string(),
                boot_value: display_value(&boot_value),
                current_value: display_value(&current_value),
            })
        })
        .collect::<Vec<_>>();

    Json(RestartPendingView { pending }).into_response()
}

/// Render a setting JSON value as a bare display string — a number as its
/// digits, a string without surrounding quotes.
fn display_value(v: &serde_json::Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryWatcherView {
    pub library_id: uuid::Uuid,
    pub library_name: String,
    pub library_slug: String,
    /// The library's `file_watch_enabled` toggle.
    pub file_watch_enabled: bool,
    pub watcher: WatcherStatus,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WatchersView {
    /// Every library, alphabetical. Bounded by the (small) library count,
    /// so not paginated.
    pub libraries: Vec<LibraryWatcherView>,
    /// Effective `scanner.watch_debounce_secs`.
    pub debounce_secs: u64,
    /// Effective `scanner.watch_poll_interval_secs`.
    pub poll_interval_secs: u64,
    /// `COMIC_WATCH_FORCE_POLL` — every watched library polls.
    pub force_poll: bool,
}

/// `GET /admin/server/watchers` — per-library file-watcher mode (inotify /
/// poll / disabled), filesystem, and last trigger (WP-3.1). Feeds the admin
/// scan dashboard. Read-only; no audit row (allow-listed in `audit-check`).
#[utoipa::path(
    operation_id = "server_info_watchers",    get,
    path = "/admin/server/watchers",
    responses(
        (status = 200, body = WatchersView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn watchers(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let libs = match library::Entity::find()
        .order_by_asc(library::Column::Name)
        .all(&app.db)
        .await
    {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "watchers: library query failed");
            return crate::api::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            );
        }
    };
    let statuses = app.watchers.statuses();
    let libraries = libs
        .into_iter()
        .map(|l| {
            let watcher = statuses.get(&l.id).cloned().unwrap_or_else(|| {
                crate::library::watcher::not_running_status(l.file_watch_enabled)
            });
            LibraryWatcherView {
                library_id: l.id,
                library_name: l.name,
                library_slug: l.slug,
                file_watch_enabled: l.file_watch_enabled,
                watcher,
            }
        })
        .collect();
    let cfg = app.cfg();
    Json(WatchersView {
        libraries,
        debounce_secs: cfg.watch_debounce_secs,
        poll_interval_secs: cfg.watch_poll_interval_secs,
        force_poll: cfg.watch_force_poll,
    })
    .into_response()
}
