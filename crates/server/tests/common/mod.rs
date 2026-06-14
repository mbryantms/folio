//! Test harness shared across integration tests.
//!
//! Spins up a Postgres container via testcontainers, runs migrations, builds the
//! Axum router, and returns a tower::ServiceExt-compatible app for direct invocation.

// Each integration test file links a separate copy of this module, so cargo's
// dead-code analysis sees fields/methods that THIS test happens not to use as
// "unused" even though peer tests rely on them. Same for `unreachable_pub`:
// the test binary doesn't re-export, but the `pub` is needed for tests to read
// the fields.
#![allow(dead_code, unreachable_pub)]

pub mod seed;

use axum::Router;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use sea_orm::{ConnectOptions, ConnectionTrait, Database};
use server::{
    app,
    config::{AuthMode, Config},
    email::MockSender,
    jobs::JobRuntime,
    observability::{LogReloadHandle, LogRingBuffer},
    secrets::Secrets,
    state::AppState,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;
use tracing_subscriber::{EnvFilter, reload};
use uuid::Uuid;

/// One Prometheus recorder per test process — `install_recorder` errors on
/// second call. Stored in a OnceLock so concurrent tests share the handle.
fn prometheus_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("install prometheus recorder")
        })
        .clone()
}

pub struct TestApp {
    pub router: Router,
    pub db_url: String,
    state: AppState,
    /// Cloned handle on the [`MockSender`] AppState carries — tests assert
    /// against `app.email.outbox()`.
    pub email: MockSender,
    /// TempDir is held to keep `/data/secrets/` alive for the duration of the test.
    pub _data_dir: tempfile::TempDir,
    /// Per-test Redis container. Postgres is shared per process (see
    /// [`SharedPg`]); Redis stays per-test — its boot is cheap and isolating
    /// its many key consumers (apalis, ws-tickets, in-flight dedup) would need
    /// invasive per-test namespacing.
    pub _redis: ContainerAsync<Redis>,
}

impl TestApp {
    /// Clone the AppState — useful for tests that exercise modules below the
    /// HTTP layer (the scanner, jobs, library/identity, etc.).
    pub fn state(&self) -> AppState {
        self.state.clone()
    }
}

/// Name of the migrated template database every per-test database is cloned
/// from. Built once per process by [`shared_pg`].
const TEMPLATE_DB: &str = "comic_template";

/// One Postgres container per test *process*, shared by every `TestApp` that
/// process spawns. Booting a container and running all migrations per test was
/// the dominant cost of the integration suite (≈1095 spawns × 93 migrations).
/// Instead we migrate once into [`TEMPLATE_DB`] and clone it per test with
/// `CREATE DATABASE … TEMPLATE` (a fast Postgres file-copy).
///
/// As a bonus, the shared [`OnceCell`] also serializes the single Postgres
/// start before any per-test Redis start, so it subsumes the old
/// first-start watchdog gate: testcontainers' process-global `conquer_once`
/// watchdog `Lazy` (a blocking once-cell with an init-race bug in
/// conquer-once 0.4.0 → `unreachable!()` panic) is warmed uncontended by that
/// first Postgres boot, before any thread reaches a Redis start.
struct SharedPg {
    /// Held for the process lifetime so the container outlives every test.
    _container: ContainerAsync<Postgres>,
    host: String,
    port: u16,
}

static SHARED_PG: tokio::sync::OnceCell<SharedPg> = tokio::sync::OnceCell::const_new();

async fn shared_pg() -> &'static SharedPg {
    SHARED_PG
        .get_or_init(|| async {
            // Pin to 18-alpine; the testcontainers-modules default of 11-alpine
            // predates STORED generated columns (Postgres 12+) used by the
            // search migration, and the prod compose stack is on 18 anyway.
            // Every per-test clone now shares this server's connection budget,
            // so raise max_connections; durability is pointless for ephemeral
            // test data, and turning it off speeds migrations + writes.
            let container = Postgres::default()
                .with_db_name("comic_reader_test")
                .with_user("comic")
                .with_password("comic")
                .with_tag("18-alpine")
                .with_cmd([
                    "postgres",
                    "-c",
                    "max_connections=500",
                    "-c",
                    "fsync=off",
                    "-c",
                    "synchronous_commit=off",
                    "-c",
                    "full_page_writes=off",
                ])
                .start()
                .await
                .expect("postgres start");
            let host = container.get_host().await.expect("pg host").to_string();
            let port = container.get_host_port_ipv4(5432).await.expect("pg port");

            // Build the migrated template once. Both connections here live on
            // (and are closed within) this OnceCell init — do NOT keep a
            // long-lived pool in the static: each `#[tokio::test]` runs on its
            // own tokio runtime, and an sqlx pool's background tasks die with
            // the runtime that created it, so a later test would hang acquiring
            // from it. Per-test DDL gets a fresh connection in `clone_test_db`.
            let admin = admin_conn(&host, port).await;
            // Postgres refuses to clone a template that has live connections, so
            // migrate it on a dedicated connection and close that before any
            // test clones it.
            admin
                .execute_unprepared(&format!("CREATE DATABASE {TEMPLATE_DB}"))
                .await
                .expect("create template db");
            let tmpl_url = format!("postgres://comic:comic@{host}:{port}/{TEMPLATE_DB}");
            let tmpl = Database::connect(&tmpl_url)
                .await
                .expect("connect template");
            use migration::MigratorTrait;
            migration::Migrator::up(&tmpl, None)
                .await
                .expect("migrate template");
            tmpl.close().await.expect("close template connection");
            admin.close().await.expect("close admin connection");

            SharedPg {
                _container: container,
                host,
                port,
            }
        })
        .await
}

/// A single-connection, short-lived pool on the maintenance database, for
/// issuing `CREATE DATABASE` (which can't run inside a transaction —
/// `execute_unprepared` runs it in autocommit). Caller closes it.
async fn admin_conn(host: &str, port: u16) -> sea_orm::DatabaseConnection {
    let url = format!("postgres://comic:comic@{host}:{port}/comic_reader_test");
    let mut opts = ConnectOptions::new(url);
    opts.max_connections(1)
        .min_connections(0)
        .sqlx_logging(false);
    Database::connect(opts).await.expect("connect admin")
}

/// Clone a fresh, already-migrated database off [`TEMPLATE_DB`] for one test.
/// Returns its connection URL. The clone is done on a fresh connection bound to
/// the calling test's runtime (see [`shared_pg`] for why a shared pool can't be
/// reused across tests). The database is dropped implicitly when the shared
/// container dies at process exit.
async fn clone_test_db(pg: &SharedPg) -> String {
    let db_name = format!("t_{}", Uuid::now_v7().simple());
    let admin = admin_conn(&pg.host, pg.port).await;
    admin
        .execute_unprepared(&format!(
            "CREATE DATABASE \"{db_name}\" TEMPLATE {TEMPLATE_DB}"
        ))
        .await
        .expect("clone test db from template");
    admin.close().await.ok();
    format!("postgres://comic:comic@{}:{}/{}", pg.host, pg.port, db_name)
}

/// Knobs flipped per test variant. Keeps `spawn_inner` from growing a
/// long boolean parameter list.
#[derive(Default, Clone)]
pub struct SpawnOpts {
    pub smtp_on: bool,
    /// When `Some`, configures the OIDC issuer + dummy client credentials
    /// and sets `auth_mode = Both` so the OIDC handlers respond. Used by
    /// `tests/oidc.rs` with a wiremock mock OP.
    pub oidc_issuer: Option<String>,
    /// Mirrors `COMIC_OIDC_TRUST_UNVERIFIED_EMAIL`.
    pub oidc_trust_unverified_email: bool,
    /// Mirrors `COMIC_OIDC_LINK_LOCAL_BY_VERIFIED_EMAIL` — auto-link an OIDC
    /// identity onto a matching local account on first verified-email login.
    pub oidc_link_local_by_verified_email: bool,
    /// Override the library root path. Default of `/tmp/library` is a
    /// bogus path that the scanner / fs-list handlers reject as missing
    /// — pass `Some(real_dir)` when the test needs the handler to read
    /// actual on-disk children.
    pub library_root: Option<PathBuf>,
    /// Override the Next.js SSR upstream URL the `upstream::proxy`
    /// fallback forwards to. Defaults to a guaranteed-dead address;
    /// tests covering the fallback (see `tests/fallback_proxy.rs`)
    /// point it at a wiremock instance.
    pub web_upstream_url: Option<String>,
    /// ComicVine API key. Some(_) wires the value into `Config`; tests
    /// pair this with [`comicvine_enabled`] to exercise the M1 admin
    /// surface.
    pub comicvine_api_key: Option<String>,
    /// Master toggle for ComicVine integration (M1).
    pub comicvine_enabled: bool,
    /// Metron credentials (M2). `username` + `password` must both be set
    /// for the client to construct.
    pub metron_username: Option<String>,
    pub metron_password: Option<String>,
    pub metron_enabled: bool,
    /// When `Some`, gates `GET /metrics` behind this bearer token
    /// (`COMIC_METRICS_TOKEN`). `None` (default) leaves it open.
    pub metrics_token: Option<String>,
}

impl TestApp {
    /// Same as [`spawn`] but flips `cfg.smtp_host` / `smtp_from` to non-empty
    /// values so the `register` handler takes the `pending_verification`
    /// branch and the recovery endpoints exercise the "SMTP configured"
    /// code path. The actual transport is still the in-memory
    /// [`MockSender`].
    pub async fn spawn_with_smtp() -> Self {
        Self::spawn_inner(SpawnOpts {
            smtp_on: true,
            ..SpawnOpts::default()
        })
        .await
    }

    pub async fn spawn() -> Self {
        Self::spawn_inner(SpawnOpts::default()).await
    }

    /// Spawn with the SSR fallback pointed at the given upstream URL
    /// (typically a `wiremock::MockServer` for `tests/fallback_proxy.rs`).
    pub async fn spawn_with_web_upstream(url: impl Into<String>) -> Self {
        Self::spawn_inner(SpawnOpts {
            web_upstream_url: Some(url.into()),
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with an explicit library-root path. Use for tests that
    /// exercise filesystem-touching handlers (`/admin/fs/list`, the
    /// scanner) against a real on-disk fixture tree.
    pub async fn spawn_with_library_root(root: PathBuf) -> Self {
        Self::spawn_inner(SpawnOpts {
            library_root: Some(root),
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with OIDC pointed at the provided issuer URL. `auth_mode`
    /// becomes `Both` so the local + recovery endpoints still work for
    /// fixture setup. `trust_unverified_email` mirrors the env knob.
    pub async fn spawn_with_oidc(issuer: impl Into<String>, trust_unverified: bool) -> Self {
        Self::spawn_inner(SpawnOpts {
            smtp_on: false,
            oidc_issuer: Some(issuer.into()),
            oidc_trust_unverified_email: trust_unverified,
            library_root: None,
            web_upstream_url: None,
            comicvine_api_key: None,
            comicvine_enabled: false,
            metron_username: None,
            metron_password: None,
            metron_enabled: false,
            metrics_token: None,
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with OIDC configured and `auth.oidc.link_local_by_verified_email`
    /// turned on, so a first verified-email OIDC login auto-links onto a
    /// matching local account instead of returning `auth.email_in_use`.
    pub async fn spawn_with_oidc_link_local(issuer: impl Into<String>) -> Self {
        Self::spawn_inner(SpawnOpts {
            oidc_issuer: Some(issuer.into()),
            oidc_link_local_by_verified_email: true,
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with `GET /metrics` gated behind the given bearer token
    /// (metrics-observability M5). Used by `tests/metrics_endpoint.rs`.
    pub async fn spawn_with_metrics_token(token: impl Into<String>) -> Self {
        Self::spawn_inner(SpawnOpts {
            metrics_token: Some(token.into()),
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with ComicVine credentials pointed at a wiremock instance
    /// (metadata-providers-1.0 M1 tests).
    pub async fn spawn_with_comicvine(api_key: impl Into<String>, enabled: bool) -> Self {
        Self::spawn_inner(SpawnOpts {
            comicvine_api_key: Some(api_key.into()),
            comicvine_enabled: enabled,
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with Metron credentials (metadata-providers-1.0 M2 tests).
    pub async fn spawn_with_metron(
        username: impl Into<String>,
        password: impl Into<String>,
        enabled: bool,
    ) -> Self {
        Self::spawn_inner(SpawnOpts {
            metron_username: Some(username.into()),
            metron_password: Some(password.into()),
            metron_enabled: enabled,
            ..SpawnOpts::default()
        })
        .await
    }

    /// Spawn with BOTH ComicVine + Metron configured — needed by the
    /// composite (multi-provider) merge tests so `build_provider`
    /// returns a client for each source.
    pub async fn spawn_with_providers(
        comicvine_api_key: impl Into<String>,
        metron_username: impl Into<String>,
        metron_password: impl Into<String>,
    ) -> Self {
        Self::spawn_inner(SpawnOpts {
            comicvine_api_key: Some(comicvine_api_key.into()),
            comicvine_enabled: true,
            metron_username: Some(metron_username.into()),
            metron_password: Some(metron_password.into()),
            metron_enabled: true,
            ..SpawnOpts::default()
        })
        .await
    }

    async fn spawn_inner(opts: SpawnOpts) -> Self {
        // Clone a fresh, pre-migrated database off the shared per-process
        // container instead of booting a container + running 93 migrations.
        let pg = shared_pg().await;
        let db_url = clone_test_db(pg).await;

        // Cap the per-test pool: every test now shares one server's connection
        // budget, so don't let each open the sea-orm default fan-out. 8 is
        // ample for the request/assert shape of these tests.
        let mut db_opts = ConnectOptions::new(db_url.clone());
        db_opts.max_connections(8).sqlx_logging(false);
        let db = Database::connect(db_opts).await.expect("connect test db");

        // Redis (apalis backend) — required since Library Scanner v1.
        let redis = Redis::default()
            .with_tag("8-alpine")
            .start()
            .await
            .expect("redis start");
        let redis_host = redis.get_host().await.expect("redis host");
        let redis_port = redis.get_host_port_ipv4(6379).await.expect("redis port");
        let redis_url = format!("redis://{redis_host}:{redis_port}");

        let data_dir = tempfile::tempdir().expect("tempdir");
        let secrets = Secrets::load(data_dir.path()).expect("load secrets");

        let cfg = Config {
            database_url: db_url.clone(),
            redis_url: redis_url.clone(),
            library_path: opts
                .library_root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/tmp/library")),
            data_path: data_dir.path().to_path_buf(),
            public_url: "http://localhost:8080".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            log_level: "warn".into(),
            trusted_proxies: String::new(),
            auth_mode: if opts.oidc_issuer.is_some() {
                AuthMode::Both
            } else {
                AuthMode::Local
            },
            // Defaults to a guaranteed-dead address so any test that
            // accidentally triggers the SSR fallback fails loudly. Real
            // fallback coverage lives in `tests/fallback_proxy.rs`,
            // which overrides via `SpawnOpts::web_upstream_url`.
            web_upstream_url: opts
                .web_upstream_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:0".into()),
            oidc_issuer: opts.oidc_issuer.clone(),
            oidc_client_id: opts
                .oidc_issuer
                .as_ref()
                .map(|_| "folio-test-client".to_string()),
            oidc_client_secret: opts
                .oidc_issuer
                .as_ref()
                .map(|_| "folio-test-secret".to_string()),
            oidc_trust_unverified_email: opts.oidc_trust_unverified_email,
            oidc_link_local_by_verified_email: opts.oidc_link_local_by_verified_email,
            local_registration_open: true,
            jwt_access_ttl: "15m".into(),
            jwt_refresh_ttl: "30d".into(),
            rate_limit_enabled: true,
            check_upstream_releases: true,
            otlp_endpoint: None,
            auto_migrate: false,
            smtp_host: if opts.smtp_on {
                Some("test-inbox".into())
            } else {
                None
            },
            smtp_port: 587,
            smtp_username: None,
            smtp_password: None,
            smtp_tls: "starttls".into(),
            smtp_from: if opts.smtp_on {
                Some("folio@example.test".into())
            } else {
                None
            },
            metrics_token: opts.metrics_token.clone(),
            metrics_open: false,
            // progress-writeback-2.0 M4: OPDS client compat mode.
            // Default off — TestApp::spawn() preserves Folio identity;
            // tests that need Komga compat flip it via PATCH
            // /api/admin/settings.
            opds_panels_mode: "off".into(),
            zip_lru_capacity: 16,
            scan_worker_count: 2,
            post_scan_worker_count: 1,
            scan_batch_size: 100,
            scan_hash_buffer_kb: 64,
            archive_work_parallel: 2,
            thumb_inline_parallel: 2,
            archive_max_entries: 50_000,
            archive_max_total_bytes: 8 * 1024 * 1024 * 1024,
            archive_max_entry_bytes: 512 * 1024 * 1024,
            archive_max_ratio: 200,
            archive_max_nesting: 1,
            comicvine_api_key: opts.comicvine_api_key.clone(),
            comicvine_enabled: opts.comicvine_enabled,
            metron_username: opts.metron_username.clone(),
            metron_password: opts.metron_password.clone(),
            metron_enabled: opts.metron_enabled,
            // metadata-providers-1.0 M7: weekly refresh defaults. Off
            // in tests by default — the cron isn't relevant to most
            // suites, and the scope-resolver tests pass explicit
            // values to `eligible_series_for_scope`.
            metadata_weekly_refresh_enabled: false,
            metadata_weekly_refresh_cron: "0 0 4 * * 0".into(),
            metadata_weekly_refresh_window_days: 14,
            metadata_stale_after_days: 180,
            // matching-accuracy-1.0 M1 defaults. Tests that need to
            // exercise the threshold-override path override these
            // explicitly via SpawnOpts.
            metadata_auto_apply_threshold: 80,
            metadata_match_medium_threshold: 60,
            // matching-accuracy-1.0 M5 — variant fetch cap, default 3.
            metadata_alternate_cover_fetch_cap: 3,
            metadata_merge_provider_preference: String::new(),
        };

        let jobs = JobRuntime::new(&redis_url, db.clone())
            .await
            .expect("connect redis");
        let email = MockSender::new();
        let baseline = cfg.clone();
        // Detached reload handle: the layer is never installed, so
        // `.modify(...)` will fail silently. Tests that exercise the
        // log-level swap don't depend on the global subscriber actually
        // reconfiguring — they assert the handler reaches the modify
        // call and returns 200.
        let (_filter_layer, log_reload): (_, LogReloadHandle) =
            reload::Layer::new(EnvFilter::new("info"));
        let state = AppState::new(
            cfg,
            baseline,
            db,
            secrets,
            prometheus_handle(),
            metrics_process::Collector::new("folio_"),
            LogRingBuffer::default(),
            log_reload,
            jobs,
            Arc::new(email.clone()),
        );
        let router = app::router(state.clone());

        TestApp {
            router,
            db_url,
            state,
            email,
            _data_dir: data_dir,
            _redis: redis,
        }
    }
}
