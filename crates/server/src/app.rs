//! Router assembly + listener.

use crate::api;
use crate::auth;
use crate::config::Config;
use crate::middleware::nonce;
use crate::middleware::request_context::{self, TrustedProxies};
use crate::middleware::security_headers::{self, CspTemplate};
use crate::observability::ObservabilityHandles;
use crate::state::AppState;
use axum::Router;
use sea_orm::{ColumnTrait, ConnectOptions, Database, EntityTrait, PaginatorTrait, QueryFilter};
use std::sync::Arc;
use std::time::Duration;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing::Level;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

/// OpenAPI document metadata (info, etc.). All paths and component schemas
/// are discovered automatically via the `utoipa-axum` router composition in
/// [`build_openapi_router`] — there is no manual `paths(...)` or
/// `components(schemas(...))` list to keep in sync. Adding a new handler with
/// `#[utoipa::path]` and registering it via `routes!()` is sufficient; the
/// spec picks it up on the next `just openapi` run.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Comic Reader API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Self-hostable comic reader. See comic-reader-spec.md."
    ),
)]
pub struct ApiDoc;

/// Compose every routed module into a single [`OpenApiRouter`].
///
/// State-free — the result is bound to a concrete [`AppState`] at serve time
/// via `.with_state(state)`. This is the one place new modules get wired
/// into both the axum router AND the OpenAPI spec; there is no separate
/// `paths(...)` list to maintain.
///
/// Group layout follows the rust-public-origin v0.2.1 design:
///
///   * `bare` — routes external clients hit directly (operator healthchecks,
///     form-action POSTs the Next sign-in submits to, OIDC callbacks, OPDS
///     clients, page-byte streams, WebSocket upgrades). None share path
///     shapes with Next.js HTML pages.
///   * `api`  — JSON the web app reaches via `apiFetch` (which prepends
///     `/api/`). Mounted via `nest("/api", api)`.
///
/// `auth::local::routes()` is intentionally merged into BOTH groups — it
/// covers both the bare form-action POSTs and the cookie-API endpoints the
/// web app calls; duplicate routing is harmless, but the spec must include
/// each operation only once or `openapi-typescript` emits duplicate
/// identifiers. The bare-group registration is wrapped via
/// `OpenApiRouter::from(axum::Router::from(...))` so its routes are live
/// but its spec contribution is dropped; the `api`-group registration is
/// the canonical spec entry.
pub fn build_openapi_router() -> OpenApiRouter<AppState> {
    let bare = OpenApiRouter::<AppState>::new()
        .merge(api::health::routes())
        .merge(api::csp::routes())
        // Modules below don't have `#[utoipa::path]` on their handlers
        // (XML feeds, binary streams, WS upgrades, browser-defined bodies).
        // Wrap each as an OpenApiRouter so it composes into the chain;
        // their routes still work, they just don't appear in the spec.
        .merge(OpenApiRouter::from(api::meta::routes()))
        // auth::local and auth::ws_ticket: routes-only in `bare` (canonical
        // spec entry lives in the `api` group below).
        .merge(OpenApiRouter::from(axum::Router::from(auth::local::routes())))
        .merge(OpenApiRouter::from(auth::oidc::routes()))
        .merge(OpenApiRouter::from(axum::Router::from(auth::ws_ticket::routes())))
        .merge(OpenApiRouter::from(api::ws_scan_events::routes()))
        .merge(OpenApiRouter::from(api::page_bytes::routes()))
        .merge(OpenApiRouter::from(api::thumbnails::routes()))
        .merge(OpenApiRouter::from(api::opds::routes()))
        .merge(OpenApiRouter::from(api::opds_v2::routes()))
        .merge(OpenApiRouter::from(api::komga_compat::routes()))
        .merge(OpenApiRouter::from(api::opds_progression::routes()));

    let api = OpenApiRouter::<AppState>::new()
        .merge(auth::local::routes())
        .merge(auth::ws_ticket::routes())
        .merge(api::account::routes())
        .merge(api::libraries::routes())
        .merge(api::health_issues::routes())
        .merge(api::reconcile::routes())
        .merge(api::scan_runs::routes())
        .merge(api::admin_queue::routes())
        .merge(api::admin_thumbs::routes())
        .merge(api::admin_users::routes())
        .merge(api::audit::routes())
        .merge(api::series::routes())
        .merge(api::issues::routes())
        .merge(api::people::routes())
        .merge(api::creators::routes())
        .merge(api::progress::routes())
        .merge(api::rails::routes())
        .merge(api::next_up::routes())
        .merge(api::ratings::routes())
        .merge(api::reading_sessions::routes())
        .merge(api::reading_log::routes())
        .merge(api::log_widgets::routes())
        .merge(api::admin_stats::routes())
        .merge(api::saved_views::routes())
        .merge(api::sidebar_layout::routes())
        .merge(api::cbl_lists::routes())
        .merge(api::collections::routes())
        .merge(api::pages::routes())
        .merge(api::issue_ocr::routes())
        .merge(api::admin_ocr::routes())
        .merge(api::markers::routes())
        .merge(api::filter_options::routes())
        .merge(api::admin_logs::routes())
        .merge(api::admin_fs::routes())
        .merge(api::admin_activity::routes())
        .merge(api::auth_config::routes())
        .merge(api::server_info::routes())
        .merge(api::server_releases::routes())
        .merge(api::sessions::routes())
        .merge(api::app_passwords::routes())
        .merge(api::admin_settings::routes())
        .merge(api::admin_email::routes())
        .merge(api::admin_metadata::routes())
        .merge(api::metadata_search::routes())
        .merge(api::external_ids::routes())
        .merge(api::covers::routes());

    bare.nest("/api", api)
}

pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    let (_router, mut spec) = build_openapi_router().split_for_parts();
    // Carry the `info` block (title/version/description) over from the
    // metadata-only ApiDoc derive — `OpenApiRouter` defaults to an empty
    // info block.
    spec.info = ApiDoc::openapi().info;
    spec
}

pub async fn serve(mut cfg: Config, handles: ObservabilityHandles) -> anyhow::Result<()> {
    let mut db_opts = ConnectOptions::new(cfg.database_url.clone());
    db_opts
        .max_connections(30)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .sqlx_logging(false);
    let db = Database::connect(db_opts).await?;

    if cfg.auto_migrate {
        use migration::MigratorTrait;
        tracing::info!("running migrations");
        migration::Migrator::up(&db, None).await?;
    }

    let secrets = crate::secrets::Secrets::load(&cfg.data_path)?;

    // Refuse to boot when a freshly-generated pepper or settings-
    // encryption key would silently invalidate existing DB rows. The
    // canonical trigger is `COMIC_DATA_PATH` pointing at a different
    // directory than the one used in a prior deployment — the new
    // secrets/ dir doesn't exist, so the loader generates a fresh
    // pepper, every login fails with "invalid credentials", and
    // every encrypted app_setting row throws AEAD seal/open errors.
    // The operator-facing error message names the misconfiguration
    // and the recovery paths instead of leaving them to discover the
    // pepper regeneration via log spelunking.
    if secrets.load_report.pepper_regenerated {
        let count = entity::user::Entity::find()
            .filter(entity::user::Column::PasswordHash.is_not_null())
            .count(&db)
            .await
            .unwrap_or(0);
        if count > 0 {
            anyhow::bail!(
                "refusing to boot: pepper at {dir}/secrets/pepper was freshly \
                 generated, but the database has {count} user row(s) with \
                 existing password_hash values. Every local login will fail \
                 because the pepper is mixed into argon2 verification. This \
                 almost always means COMIC_DATA_PATH ({dir:?}) points at the \
                 wrong directory. Fix one of: \
                 (1) point COMIC_DATA_PATH at the directory that owns the \
                 original `secrets/` subdir (typically `/data` in the shipped \
                 compose.prod.yml); \
                 (2) restore secrets/ from backup; \
                 (3) if this is a clean re-install and data loss is \
                 acknowledged, DELETE FROM users; first.",
                dir = cfg.data_path.display(),
                count = count,
            );
        }
    }
    if secrets.load_report.settings_key_regenerated {
        let count = entity::app_setting::Entity::find()
            .filter(entity::app_setting::Column::IsSecret.eq(true))
            .count(&db)
            .await
            .unwrap_or(0);
        if count > 0 {
            anyhow::bail!(
                "refusing to boot: settings-encryption.key at \
                 {dir}/secrets/settings-encryption.key was freshly generated, \
                 but the database has {count} sealed-secret app_setting \
                 row(s) (SMTP password, OIDC client_secret, etc.). Their \
                 ciphertext cannot be decrypted with a new key. Same root \
                 cause as the pepper case above — fix COMIC_DATA_PATH or \
                 restore the original key. As a last resort: \
                 DELETE FROM app_setting WHERE is_secret = true; \
                 to clear the unreadable rows and re-enter the secrets via \
                 /admin/email and /admin/auth.",
                dir = cfg.data_path.display(),
                count = count,
            );
        }
    }

    // Capture the env-only Config before applying the DB overlay. The
    // baseline lives in AppState and powers the
    // rebuild-from-scratch path inside `PATCH /admin/settings` so
    // deleting a DB row reverts to the env value.
    let baseline = cfg.clone();

    // First-boot bootstrap: copy env-set SMTP + auth values into
    // `app_setting` when no sentinel row (smtp.host / auth.mode) exists
    // yet. Lets an existing `compose.prod.yml` deployment upgrade in
    // place without losing config — the admin UI sees the env-derived
    // values and the operator can edit from there. Runs before
    // `overlay_db` so the overlay re-reads what we just wrote (the
    // WARN-on-collision logic suppresses warnings when env value == DB
    // value, so this isn't noisy).
    if let Err(e) = crate::settings::bootstrap::seed_smtp_from_env(&db, &cfg, &secrets).await {
        tracing::warn!(error = %e, "SMTP env bootstrap failed; falling back to env-only");
    }
    if let Err(e) = crate::settings::bootstrap::seed_auth_from_env(&db, &cfg, &secrets).await {
        tracing::warn!(error = %e, "auth env bootstrap failed; falling back to env-only");
    }
    if let Err(e) =
        crate::settings::bootstrap::seed_tokens_and_diagnostics_from_env(&db, &cfg).await
    {
        tracing::warn!(error = %e, "tokens/diagnostics env bootstrap failed; falling back to env-only");
    }
    if let Err(e) = crate::settings::bootstrap::seed_operational_from_env(&db, &cfg).await {
        tracing::warn!(error = %e, "operational env bootstrap failed; falling back to env-only");
    }

    // Apply DB-stored setting overrides on top of the env-loaded config.
    // Failures here fall back to the env-only Config so the server stays
    // bootable when a malformed DB row exists.
    if let Err(e) = cfg.overlay_db(&db, &secrets).await {
        tracing::warn!(error = %e, "app_setting overlay failed; using env-only Config");
    }

    // Deprecation WARNs (since=2026-Q3): env vars in moved-to-DB blocks
    // still work as boot-time defaults, but operators should migrate to
    // the admin UI for live edits. See docs/dev/runtime-configuration.md.
    if std::env::var("COMIC_SMTP_HOST")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
    {
        tracing::warn!(
            target: "comic.deprecation",
            since = "2026-Q3",
            replace_with = "/admin/email",
            "COMIC_SMTP_* env vars are deprecated; DB value wins once a row exists for each key"
        );
    }
    if std::env::var("COMIC_AUTH_MODE").is_ok()
        || std::env::var("COMIC_OIDC_ISSUER").is_ok()
        || std::env::var("COMIC_LOCAL_REGISTRATION_OPEN").is_ok()
    {
        tracing::warn!(
            target: "comic.deprecation",
            since = "2026-Q3",
            replace_with = "/admin/auth",
            "COMIC_AUTH_MODE / COMIC_OIDC_* / COMIC_LOCAL_* env vars are deprecated"
        );
    }
    if std::env::var("COMIC_JWT_ACCESS_TTL").is_ok()
        || std::env::var("COMIC_JWT_REFRESH_TTL").is_ok()
        || std::env::var("COMIC_RATE_LIMIT_ENABLED").is_ok()
        || std::env::var("COMIC_LOG_LEVEL").is_ok()
    {
        tracing::warn!(
            target: "comic.deprecation",
            since = "2026-Q3",
            replace_with = "/admin/auth + /admin/server",
            "COMIC_JWT_*, COMIC_RATE_LIMIT_ENABLED, COMIC_LOG_LEVEL env vars are deprecated"
        );
    }
    if std::env::var("COMIC_ZIP_LRU_CAPACITY").is_ok()
        || std::env::var("COMIC_SCAN_WORKER_COUNT").is_ok()
        || std::env::var("COMIC_POST_SCAN_WORKER_COUNT").is_ok()
        || std::env::var("COMIC_SCAN_BATCH_SIZE").is_ok()
        || std::env::var("COMIC_SCAN_HASH_BUFFER_KB").is_ok()
        || std::env::var("COMIC_ARCHIVE_WORK_PARALLEL").is_ok()
        || std::env::var("COMIC_THUMB_INLINE_PARALLEL").is_ok()
    {
        tracing::warn!(
            target: "comic.deprecation",
            since = "2026-Q3",
            replace_with = "/admin/server",
            "COMIC_ZIP_LRU_CAPACITY / COMIC_*_WORKER_COUNT / COMIC_SCAN_BATCH_SIZE / \
             COMIC_SCAN_HASH_BUFFER_KB / COMIC_ARCHIVE_WORK_PARALLEL / COMIC_THUMB_INLINE_PARALLEL \
             env vars are deprecated; values apply on next restart"
        );
    }

    let jobs = crate::jobs::JobRuntime::new(&cfg.redis_url, db.clone()).await?;

    let email = crate::email::build(&cfg)?;

    let bind = cfg.bind_addr;
    let state = AppState::new(
        cfg,
        baseline,
        db,
        secrets,
        handles.prometheus,
        handles.log_buffer,
        handles.log_reload,
        jobs,
        email,
    );

    // Thumbnail/phash catchup is intentionally NOT auto-run at boot.
    // Queued work is user-directed only: a scan (`enqueue_post_scan_*`)
    // or an explicit admin action (the thumbnails "Generate missing"
    // button, `/admin/metadata/phash-backfill`,
    // `/admin/metadata/variant-cover-backfill`). Auto-enqueueing every
    // phash-missing issue at boot flooded the queue (~9k jobs) on a
    // large library; the operator now decides when that work runs.

    // Archive-rewrite startup sweep (M0 of metadata-sidecar-writeback-1.0):
    // walk every `library.root_path` and remove orphan `.tmp` siblings
    // older than 10 min. Crashed rewrites leave these behind; without the
    // sweep they accumulate forever. Skipped libraries with read errors
    // (mount went away, perm issue) — logged inside `walk_and_remove`.
    {
        use entity::library;
        use sea_orm::EntityTrait;
        if let Ok(libs) = library::Entity::find().all(&state.db).await {
            let roots: Vec<std::path::PathBuf> = libs
                .into_iter()
                .map(|l| std::path::PathBuf::from(l.root_path))
                .collect();
            let removed = crate::archive_rewrite::startup_cleanup(
                roots,
                std::time::Duration::from_secs(10 * 60),
            );
            if removed > 0 {
                tracing::info!(removed, "archive_rewrite startup: cleaned orphan .tmp");
            }
        }
    }

    // Shared shutdown token: HTTP server and apalis monitor both
    // observe cancellation, so a single SIGTERM drains both cleanly.
    // M4 of code-quality-cleanup-1.0 — before this, the apalis monitor
    // was a detached spawn dropped on HTTP shutdown, abandoning any
    // in-flight worker tasks mid-write.
    let shutdown = tokio_util::sync::CancellationToken::new();

    // Spawn the apalis monitor; retain the JoinHandle so we can wait
    // on its graceful drain after the HTTP server returns.
    let jobs_handle = state.jobs.clone();
    let jobs_state = state.clone();
    let jobs_shutdown = shutdown.clone();
    let monitor_handle = tokio::spawn(async move {
        jobs_handle.run(jobs_state, jobs_shutdown).await;
    });

    // Cron scheduler — best-effort. Failure here doesn't block startup.
    match crate::jobs::scheduler::start(state.clone()).await {
        Ok(scheduler) => {
            state.set_scheduler(scheduler).await;
        }
        Err(e) => tracing::error!(error = %e, "scheduler failed to start"),
    }

    // pHash + variant-cover backfills are NOT auto-run at boot (see the
    // catchup note above). The matcher falls back to text-only for
    // not-yet-hashed covers until the operator runs
    // `/admin/metadata/phash-backfill` (or a scan recomputes them);
    // hotlinked variants localize on re-apply or via
    // `/admin/metadata/variant-cover-backfill`.

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %bind, "listening");

    let http_shutdown = shutdown.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        http_shutdown.cancel();
    })
    .await?;

    // HTTP server has stopped accepting; wait for the apalis monitor
    // to finish draining its workers (graceful) before returning.
    if let Err(e) = monitor_handle.await {
        tracing::warn!(error = %e, "apalis monitor task failed to join cleanly");
    }
    tracing::info!("apalis monitor drained; shutdown complete");

    Ok(())
}

pub fn router(state: AppState) -> Router {
    let request_id_header = axum::http::HeaderName::from_static("x-request-id");
    // Snapshot config at router-build time: CspTemplate + TrustedProxies
    // are built once and live for the lifetime of the process. They don't
    // hot-reload yet (deferred to a later milestone of the
    // runtime-config-admin plan). The CSP itself is templated per-request
    // by `security_headers::set_headers` so the per-request nonce can be
    // slotted in.
    let cfg = state.cfg();
    let csp_template = Arc::new(CspTemplate::new(&cfg));
    let trusted_proxies = TrustedProxies::from_config(&cfg.trusted_proxies);

    // Single source of truth: `build_openapi_router` composes every routed
    // module. `split_for_parts` discards the OpenApi side here — that
    // surface is exposed via [`openapi_spec`] / the `--emit-openapi` flag.
    let (api_router, _) = build_openapi_router().split_for_parts();

    api_router
        // Catch-all fallback: anything no explicit route matched is
        // forwarded to the configured Next.js SSR upstream. Layers
        // below wrap the fallback the same way they wrap any explicit
        // route, so security_headers / set_context / TraceLayer / CSRF
        // all run on proxied requests too.
        .fallback(crate::upstream::proxy)
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf))
        // Order matters: outermost wraps innermost. `set_context` needs to run
        // before handlers so `Request::extensions::get::<RequestContext>()`
        // works inside extractors and handlers.
        .layer(axum::middleware::from_fn_with_state(
            trusted_proxies,
            request_context::set_context,
        ))
        .layer(axum::middleware::from_fn_with_state(
            csp_template,
            security_headers::set_headers,
        ))
        // Nonce runs outside `security_headers` so the CSP builder can
        // read the per-request nonce from the request extensions, and
        // outside the `upstream::proxy` fallback so the proxy can
        // forward the nonce to Next.js as `x-csp-nonce`. Stays inside
        // `TraceLayer` because trace spans don't need it and including
        // it would put a fresh random in every log line.
        .layer(axum::middleware::from_fn(nonce::set_nonce))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(
            request_id_header.clone(),
            MakeRequestUuid,
        ))
        // Custom request span: emit `path` only, never `uri`. The default
        // `TraceLayer::new_for_http()` captures the full URI including the
        // query string, which would leak credentials into the trace log if
        // anything ever submitted them as GET params (e.g. an unprotected
        // form falling through to its native handler). The auth-hardening
        // M9 audit found exactly this regression in the wild — fixing it
        // here is defense-in-depth so any future surface that mis-handles
        // credentials in a query string doesn't also poison journald.
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                tracing::span!(
                    Level::INFO,
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                )
            }),
        )
        .with_state(state)
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received; draining");
}
