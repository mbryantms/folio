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
use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use sea_orm::{ColumnTrait, ConnectOptions, Database, EntityTrait, PaginatorTrait, QueryFilter};
use std::sync::Arc;
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
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
        title = "Folio API",
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
///
/// **Per-group layers (WP-1.6 ops hygiene):** the `api` group is wrapped
/// by [`with_json_layers`] — a [`TimeoutLayer`] ([`JSON_ROUTE_TIMEOUT`])
/// and a [`CompressionLayer`] — *before* it is nested under `/api`. The
/// `bare` group is deliberately left unwrapped: it carries the
/// page-byte / thumbnail / OPDS-PSE / archive-download streams (which a
/// handler-latency timeout must never cut short and which compression
/// must never re-encode — Range + `Content-Length` semantics depend on
/// the bytes going out verbatim) and the `/ws/*` upgrades. The
/// `Router::fallback` proxy in [`router`] sits outside both groups, so
/// its WebSocket passthrough is untouched too.
///
/// **Long-body routes (E8):** the staged page-image upload the web app
/// sends through `apiFetch` (`POST /api/uploads`, up to 32 MiB) lives in
/// a third group, `long_body`, nested under `/api` beside the JSON group
/// but wrapped by [`with_long_body_layers`] instead:
/// [`UPLOAD_ROUTE_TIMEOUT`] rather than [`JSON_ROUTE_TIMEOUT`] (the
/// timeout covers reading the body, so 60 s would 408 a big upload on a
/// slow phone link), a per-frame [`UPLOAD_BODY_IDLE_TIMEOUT`] so a client
/// that stops sending is still dropped quickly, and no compression. Every
/// outer layer in [`router`] (CSRF, auth extractors, security headers,
/// tracing, metrics) still runs on it, and it stays in the spec. The CBL
/// file import (`POST /api/me/cbl-lists/upload`) stays in the JSON group:
/// its 4 MiB cap is an eighth of the page cap, real lists are a few
/// hundred KiB, and its handler matches up to 5000 entries against the DB
/// synchronously, which the 60 s ceiling should keep bounding.
///
/// **Body limits (N-A2):** every group declares [`DEFAULT_BODY_LIMIT`]
/// explicitly. It only bounds buffering extractors, so the bare streams
/// are unaffected.
pub fn build_openapi_router() -> OpenApiRouter<AppState> {
    build_openapi_router_with(RouteTimeouts::PRODUCTION)
}

/// [`build_openapi_router`] with explicit per-group deadlines. Only the
/// integration tests pass anything but [`RouteTimeouts::PRODUCTION`], to
/// prove which group a route lands in without wall-clocking 60 s.
pub fn build_openapi_router_with(timeouts: RouteTimeouts) -> OpenApiRouter<AppState> {
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
        // Bare (un-prefixed) browser navigation: `/issues/{id}` → 303 to the
        // canonical slug URL. Has a `#[utoipa::path]`, so merged unwrapped to
        // land in the spec.
        .merge(api::issue_permalink::routes())
        .merge(OpenApiRouter::from(api::page_bytes::routes()))
        .merge(OpenApiRouter::from(api::thumbnails::routes()))
        .merge(OpenApiRouter::from(api::opds::routes()))
        .merge(OpenApiRouter::from(api::opds_v2::routes()))
        .merge(OpenApiRouter::from(api::komga_compat::routes()))
        .merge(OpenApiRouter::from(api::opds_progression::routes()))
        // Applied before the `api` group is nested, so it covers the bare
        // routes only; the `api` group carries its own copy inside
        // `with_json_layers`.
        .layer(axum::middleware::map_response(envelope_payload_too_large))
        .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT));

    let api = OpenApiRouter::<AppState>::new()
        .merge(auth::local::routes())
        .merge(auth::ws_ticket::routes())
        .merge(api::account::routes())
        .merge(api::account_export::routes())
        .merge(api::holdings_export::routes())
        .merge(api::libraries::routes())
        .merge(api::health_issues::routes())
        .merge(api::reconcile::routes())
        .merge(api::duplicates::routes())
        .merge(api::scan_runs::routes())
        .merge(api::scan_batches::routes())
        .merge(api::library_events::routes())
        .merge(api::library_hash_backfill::routes())
        .merge(api::library_stats::routes())
        .merge(api::admin_queue::routes())
        .merge(api::background_work::routes())
        .merge(api::admin_thumbs::routes())
        .merge(api::admin_users::routes())
        .merge(api::audit::routes())
        .merge(api::series::routes())
        .merge(api::series_similar::routes())
        .merge(api::series_reader_prefs::routes())
        .merge(api::series_completion_prefs::routes())
        .merge(api::issues::routes())
        .merge(api::appearances::routes())
        .merge(api::archive_edit::routes())
        .merge(api::people::routes())
        .merge(api::creators::routes())
        .merge(api::characters::routes())
        .merge(api::teams::routes())
        .merge(api::locations::routes())
        .merge(api::arcs::routes())
        .merge(api::publishers::routes())
        .merge(api::progress::routes())
        .merge(api::rails::routes())
        .merge(api::next_up::routes())
        .merge(api::cover_pick::routes())
        .merge(api::ratings::routes())
        .merge(api::issue_notes::routes())
        .merge(api::reading_sessions::routes())
        .merge(api::reading_log::routes())
        .merge(api::log_widgets::routes())
        .merge(api::admin_stats::routes())
        .merge(api::saved_views::routes())
        .merge(api::sidebar_layout::routes())
        .merge(api::cbl_lists::routes())
        .merge(api::collections::routes())
        .merge(api::pages::routes())
        .merge(api::view_groups::routes())
        .merge(api::issue_ocr::routes())
        .merge(api::admin_ocr::routes())
        .merge(api::markers::routes())
        .merge(api::markers_export::routes())
        .merge(api::bookmarks_feed::routes())
        .merge(api::page_overrides::routes())
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
        .merge(api::admin_metadata_coverage::routes())
        .merge(api::metadata_search::routes())
        .merge(api::external_ids::routes())
        .merge(api::provider_ranges::routes())
        .merge(api::provider_coverage::routes())
        .merge(api::series_refresh::routes())
        .merge(api::series_metadata_tab::routes())
        .merge(api::series_relationships::routes())
        .merge(api::series_external_relationships::routes())
        .merge(api::series_same_universe::routes())
        .merge(api::relationship_suggestions::routes())
        .merge(api::covers::routes());

    // Long-body uploads: the only `/api` routes outside `with_json_layers`.
    // Add a route here only when its request body can legitimately take
    // longer than `JSON_ROUTE_TIMEOUT` to arrive and the handler does little
    // beyond receiving it (the whole handler runs under the long deadline).
    let long_body = OpenApiRouter::<AppState>::new().merge(api::archive_edit::upload_routes());

    bare.nest(
        "/api",
        with_json_layers(api, timeouts.json).merge(with_long_body_layers(
            long_body,
            timeouts.long_body,
            timeouts.long_body_idle,
        )),
    )
}

/// Per-group request deadlines for [`build_openapi_router_with`] /
/// [`router_with_timeouts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteTimeouts {
    /// The JSON `api` group ([`with_json_layers`]).
    pub json: Duration,
    /// The `long_body` upload group's whole-request deadline
    /// ([`with_long_body_layers`]).
    pub long_body: Duration,
    /// The `long_body` group's longest pause between request-body frames.
    pub long_body_idle: Duration,
}

impl RouteTimeouts {
    /// What the served router uses.
    pub const PRODUCTION: Self = Self {
        json: JSON_ROUTE_TIMEOUT,
        long_body: UPLOAD_ROUTE_TIMEOUT,
        long_body_idle: UPLOAD_BODY_IDLE_TIMEOUT,
    };
}

/// Handler-latency budget for the JSON `api` group. tower-http's
/// [`TimeoutLayer`] bounds the time until the response *head* is
/// produced (not response-body streaming); that includes reading the
/// request body, which a buffering handler does before it answers. So it
/// caps a handler that is stuck on a lock / a slow query / an
/// unresponsive metadata provider, and answers `408 Request Timeout`
/// instead of holding the connection open indefinitely. Long-running
/// work in this group is already dispatched to apalis jobs and returns
/// `202` immediately (scans, deep-validate, metadata search/apply,
/// archive edits), so 60 s is a ceiling nothing healthy approaches. The
/// page-image upload, whose body alone can take longer, runs under
/// [`UPLOAD_ROUTE_TIMEOUT`] instead.
pub const JSON_ROUTE_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-request deadline for the `long_body` upload group (E8): the
/// page-image upload (`POST /api/uploads`, 32 MiB + framing). The timeout
/// covers receiving the body, and the owner reads on phones over LAN and
/// sometimes remotely: a full 32 MiB page needs about 0.45 Mbit/s to land
/// in 10 minutes, against about 4.5 Mbit/s under the 60 s JSON deadline.
pub const UPLOAD_ROUTE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Longest pause between request-body frames on the `long_body` group. A
/// slow link keeps delivering frames, so it never trips this; a client
/// that goes silent mid-body is dropped after a minute (the same hold the
/// JSON deadline gave before E8) instead of holding the connection, the
/// handler task and the `.part` file for the whole
/// [`UPLOAD_ROUTE_TIMEOUT`]. The upload handler answers it with `408`.
pub const UPLOAD_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Request-body limit for every buffered extractor (`Json`, `Bytes`,
/// `String`, `Form`, `Multipart`) on every route group: 2 MiB, the value
/// axum applied implicitly before it was declared (N-A2). JSON bodies in
/// this API are small; the two multipart imports that need more raise it
/// on their own sub-routers (`POST /uploads` 32 MiB + framing in
/// `api::archive_edit`, `POST /me/cbl-lists/upload` 4 MiB + framing in
/// `api::cbl_lists`). Over-limit bodies answer 413 with the canonical
/// envelope (see [`envelope_payload_too_large`]). Streaming handlers that
/// never buffer — page bytes, the upstream proxy (capped separately by
/// `upstream::MAX_PROXY_BODY_BYTES`) — are unaffected; `/ws/*` frames are
/// bounded by `api::ws_scan_events::{WS_MAX_MESSAGE_BYTES,
/// WS_MAX_FRAME_BYTES}`.
pub const DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Rewrites axum's plain-text `413 Payload Too Large` body-limit rejection
/// (`Failed to buffer the request body: length limit exceeded`) into the
/// canonical `{"error": {...}}` envelope, code `too_large`. A 413 a handler
/// already built through `api::error` / `api::respond` is JSON and passes
/// through untouched.
async fn envelope_payload_too_large(resp: axum::response::Response) -> axum::response::Response {
    if resp.status() != axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        return resp;
    }
    let is_json = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if is_json {
        return resp;
    }
    api::respond(
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        shared::error::ApiErrorCode::TooLarge,
        "request body exceeds the size limit",
    )
}

/// Wrap a JSON route group with the WP-1.6 tower layers plus the explicit
/// [`DEFAULT_BODY_LIMIT`] (N-A2).
///
/// Order: `.layer(A).layer(B)` makes `B` the outer layer, so requests
/// pass **timeout → compression → body limit → 413 envelope → handler**.
/// The body limit only inserts a request extension that extractors read,
/// so a per-route `DefaultBodyLimit` layer further in (the multipart
/// imports) overrides it. The timeout is outermost so
/// a `408` is produced by the layer that owns the deadline and never
/// waits on compression; compression sits directly on the handler so
/// it sees the handler's own headers.
///
/// Compression uses tower-http's `DefaultPredicate`: responses under
/// 32 bytes, `image/*` (except SVG), gRPC and `text/event-stream` are
/// never encoded, and a compressed response gets `Vary:
/// Accept-Encoding` appended. A `304 Not Modified` has an empty body,
/// so it passes through untouched and the handler-computed `ETag`
/// round-trips unchanged — conditional GETs on JSON keep working.
/// `Accept-Encoding` is honoured as sent (gzip / zstd per the enabled
/// tower-http features); a client that sends none gets identity.
///
/// Generic over the state so the unit test can exercise the exact layer
/// stack on a stateless router with a tiny timeout.
pub fn with_json_layers<S>(router: OpenApiRouter<S>, timeout: Duration) -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    with_body_limit_layers(router)
        .layer(CompressionLayer::new())
        .layer(request_timeout(timeout))
}

/// Wrap the `long_body` upload group: the same body-limit floor and 413
/// envelope as [`with_json_layers`] (each upload raises its own limit
/// further in), every request-body frame bounded by `idle`
/// ([`UPLOAD_BODY_IDLE_TIMEOUT`] in production) and the whole request by
/// `timeout` ([`UPLOAD_ROUTE_TIMEOUT`]), and deliberately no
/// [`CompressionLayer`]: the responses are a few bytes of JSON, so there
/// is nothing worth encoding.
pub fn with_long_body_layers<S>(
    router: OpenApiRouter<S>,
    timeout: Duration,
    idle: Duration,
) -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    with_body_limit_layers(router)
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                // `TimeoutBody` restarts its clock on every frame, so this
                // bounds a stall, not the transfer. Its `TimeoutError`
                // reaches the handler through the body extractor.
                let req = req.map(|body| {
                    axum::body::Body::new(tower_http::timeout::TimeoutBody::new(idle, body))
                });
                next.run(req)
            },
        ))
        .layer(request_timeout(timeout))
}

/// The layers every `/api` group shares, innermost first: the 413 envelope
/// and the explicit [`DEFAULT_BODY_LIMIT`].
fn with_body_limit_layers<S>(router: OpenApiRouter<S>) -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(axum::middleware::map_response(envelope_payload_too_large))
        .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT))
}

fn request_timeout(timeout: Duration) -> TimeoutLayer {
    TimeoutLayer::with_status_code(axum::http::StatusCode::REQUEST_TIMEOUT, timeout)
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
        // Post-audit T1.2: `COMIC_DB_MAX_CONNECTIONS` (default 30).
        .max_connections(cfg.db_max_connections)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        // OBS-4: keep normal per-statement logging off (far too noisy), but
        // surface pathological queries — sea-orm/sqlx emit a WARN tracing event
        // for any statement slower than the threshold, which lands in the JSON
        // log stream + the /admin/logs ring buffer. No hard `statement_timeout`:
        // that would abort long scanner/reconcile queries.
        .sqlx_logging(true)
        .sqlx_logging_level(log::LevelFilter::Off)
        .sqlx_slow_statements_logging_settings(log::LevelFilter::Warn, Duration::from_millis(500));
    let db = Database::connect(db_opts).await?;

    // Refuse to boot on a Postgres older than 17 *before* migrations run:
    // the search migration needs 17+ and would otherwise fail part-way
    // through, leaving a half-applied schema behind an opaque SQL error.
    // The check names the found version and the minimum so the operator
    // knows exactly what to upgrade. See `crate::pg_version`.
    crate::pg_version::assert_supported(&db).await?;

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
    // L-1 (WP-6.3): a pepper rotation is the one *intended* way to get a
    // fresh pepper next to existing hashes — the operator moved the old one
    // to `secrets/pepper.previous`, so verifies fall back to it and rehash.
    if secrets.load_report.pepper_previous_loaded {
        tracing::warn!(
            regenerated = secrets.load_report.pepper_regenerated,
            "argon2 pepper rotation in progress: secrets/pepper.previous is loaded; \
             passwords and app passwords verified against it are rehashed under the \
             current pepper on next use. Delete pepper.previous once the rotation \
             window is over (see docs/install/secrets-backup.md)."
        );
    }
    if secrets.load_report.pepper_regenerated && !secrets.load_report.pepper_previous_loaded {
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
    if let Err(e) = crate::settings::bootstrap::seed_audit_log_retention_from_env(&db, &cfg).await {
        tracing::warn!(error = %e, "audit-log retention env bootstrap failed; falling back to env-only");
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
        || std::env::var("COMIC_SCAN_FOLDER_CONCURRENCY").is_ok()
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
            "COMIC_ZIP_LRU_CAPACITY / COMIC_*_WORKER_COUNT / COMIC_SCAN_FOLDER_CONCURRENCY / \
             COMIC_SCAN_BATCH_SIZE / COMIC_SCAN_HASH_BUFFER_KB / COMIC_ARCHIVE_WORK_PARALLEL / \
             COMIC_THUMB_INLINE_PARALLEL env vars are deprecated; values apply on next restart"
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
        handles.process,
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

    // Scan-coalescing key boot sweep (OPS-2): clear any `scan:{in_flight,
    // scan_id,queued}:*` keys orphaned by a previous process crash so a library
    // whose scan died between claiming the key and pushing the apalis job isn't
    // wedged forever. Nothing is scanning yet at boot, so this is always safe.
    match state.jobs.clear_stale_scan_keys().await {
        Ok(n) if n > 0 => {
            tracing::info!(removed = n, "scan boot sweep: cleared stale coalesce keys")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "scan boot sweep failed"),
    }

    // Re-seed the process-local thumbnail-job marks from the jobs Redis
    // still holds, before any worker starts. Without it a restart mid-drain
    // makes every per-library "queued" count read 0 (and lets the same
    // issue be queued twice) until the leftover jobs finish.
    match state.jobs.outstanding_thumb_jobs().await {
        Ok(jobs) => {
            let n = jobs.len();
            for job in jobs {
                state.try_mark_thumb_job_queued(job.dedupe_key()).await;
            }
            if n > 0 {
                tracing::info!(
                    jobs = n,
                    "thumbnail queue: re-seeded marks for leftover jobs"
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "thumbnail queue: leftover-job read failed"),
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

    // File watcher supervisor (WP-3.1): one inotify watcher or directory-mtime
    // poller per library with `file_watch_enabled`, kept in sync with the
    // library table. Stopped by the shared shutdown token.
    let watcher_handle = crate::library::watcher::spawn_supervisor(state.clone(), shutdown.clone());

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
    if let Err(e) = watcher_handle.await {
        tracing::warn!(error = %e, "file watcher supervisor failed to join cleanly");
    }
    tracing::info!("apalis monitor drained; shutdown complete");

    Ok(())
}

pub fn router(state: AppState) -> Router {
    router_with_timeouts(state, RouteTimeouts::PRODUCTION)
}

/// [`router`] with explicit per-group deadlines; see
/// [`build_openapi_router_with`].
pub fn router_with_timeouts(state: AppState, timeouts: RouteTimeouts) -> Router {
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
    let (api_router, _) = build_openapi_router_with(timeouts).split_for_parts();

    api_router
        // Catch-all fallback: anything no explicit route matched is
        // forwarded to the configured Next.js SSR upstream. Layers
        // below wrap the fallback the same way they wrap any explicit
        // route, so security_headers / set_context / TraceLayer / CSRF
        // all run on proxied requests too.
        .fallback(crate::upstream::proxy)
        // OBS-3: innermost layer, so a panic in any handler (or the proxy
        // fallback) is caught here, logged via tracing, and converted to the
        // canonical error envelope. The response then flows back out through
        // security_headers / metrics like any other. Without this, a handler
        // panic escapes to stderr and drops the connection — invisible to the
        // JSON log stream, the /admin/logs ring buffer, and the client.
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(handle_panic))
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf))
        // M-1 (security audit) / WP-6.3: explicit deny-by-default CORS.
        // Rust is the single public origin — the web app, its JSON API,
        // OPDS, and the Komga/KOReader shims are all served from it — so no
        // cross-origin browser access is ever granted. `deny_all_cors()`
        // configures no `allow_origin` / `allow_credentials`, which means:
        // simple requests pass through untouched (no `Access-Control-*`
        // headers, so a cross-origin page can't read the response), and any
        // OPTIONS preflight is answered here with an empty 200 that carries
        // no `Access-Control-Allow-Origin` (the browser then refuses the
        // actual request). Non-browser clients (OPDS readers, KOReader,
        // Komga-shaped apps) never consult CORS, so they're unaffected. It
        // sits outside CSRF so a preflight never reaches the CSRF check.
        // Regression guards: `tests/http_layers.rs::cors_*`.
        .layer(deny_all_cors())
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
                // OBS-1: fold the request id into the span so every child log
                // event inherits it (the RingLayer walks span fields), making a
                // client-reported `x-request-id` searchable in /admin/logs. The
                // header is set by the outer `SetRequestIdLayer`, so it's
                // already present by the time this span is built.
                let request_id = req
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                tracing::span!(
                    Level::INFO,
                    "http",
                    method = %req.method(),
                    path = %req.uri().path(),
                    request_id = %request_id,
                )
            }),
        )
        // Outermost: HTTP RED metrics. Sees the client-observed status (after
        // every other layer has run on the response) and reads `MatchedPath`
        // for a bounded `route` label. Emits `folio_http_*`.
        .layer(axum::middleware::from_fn(
            crate::middleware::http_metrics::track,
        ))
        .with_state(state)
}

/// The explicit, deny-by-default CORS policy (security audit M-1). A bare
/// `CorsLayer::new()` grants nothing: no allowed origin, method, header, or
/// credentials. It exists to make the intent explicit — anyone adding a
/// cross-origin surface later has to change this function (and its tests)
/// on purpose rather than inheriting an accidental default.
pub fn deny_all_cors() -> tower_http::cors::CorsLayer {
    tower_http::cors::CorsLayer::new()
}

/// Panic handler for `CatchPanicLayer` (OBS-3). Converts a handler/middleware
/// panic into a structured `tracing::error!` (so it lands in the JSON log
/// stream + `/admin/logs`) plus the canonical error envelope, instead of the
/// default behaviour of unwinding to stderr and dropping the connection.
fn handle_panic(err: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    let detail = err
        .downcast_ref::<&'static str>()
        .map(|s| (*s).to_owned())
        .or_else(|| err.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_owned());
    tracing::error!(panic = %detail, "handler panicked");
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(shared::error::ApiError::new(
            shared::error::ApiErrorCode::Internal,
            "internal",
        )),
    )
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// OBS-3: the panic handler produces the canonical error envelope (so a
    /// caught panic reaches the client as `{"error":{"code":"internal"}}`
    /// rather than a dropped connection). The `tracing::error!` side-effect is
    /// exercised in the same call.
    #[tokio::test]
    async fn handle_panic_returns_internal_envelope() {
        let resp = handle_panic(Box::new("boom"));
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "internal");
    }

    /// WP-1.6: the exact layer stack `build_openapi_router` puts on the
    /// `api` group, driven with a tiny deadline so the test stays cheap. A
    /// handler that overruns gets `408`; one that finishes in time passes
    /// through (and is compressible — the gzip path is covered end-to-end
    /// in `tests/http_layers.rs` against the real router).
    #[tokio::test]
    async fn json_layers_time_out_slow_handlers() {
        use axum::routing::get;
        use tower::ServiceExt;

        async fn slow() -> &'static str {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "late"
        }
        async fn fast() -> &'static str {
            "ok"
        }
        let inner = OpenApiRouter::<()>::from(
            axum::Router::new()
                .route("/slow", get(slow))
                .route("/fast", get(fast)),
        );
        let (router, _) = with_json_layers(inner, Duration::from_millis(50)).split_for_parts();

        let req = |path: &str| {
            axum::http::Request::builder()
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let slow_resp = router.clone().oneshot(req("/slow")).await.unwrap();
        assert_eq!(
            slow_resp.status(),
            axum::http::StatusCode::REQUEST_TIMEOUT,
            "handler overrunning the deadline must be cut off with 408"
        );
        let fast_resp = router.oneshot(req("/fast")).await.unwrap();
        assert_eq!(fast_resp.status(), axum::http::StatusCode::OK);
    }
}
