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

use axum::Router;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use sea_orm::Database;
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
    pub _pg: ContainerAsync<Postgres>,
    pub _redis: ContainerAsync<Redis>,
}

impl TestApp {
    /// Clone the AppState — useful for tests that exercise modules below the
    /// HTTP layer (the scanner, jobs, library/identity, etc.).
    pub fn state(&self) -> AppState {
        self.state.clone()
    }
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
        })
        .await
    }

    async fn spawn_inner(opts: SpawnOpts) -> Self {
        // Pin to 17-alpine; the testcontainers-modules default of 11-alpine
        // predates STORED generated columns (Postgres 12+) used by the search
        // migration, and the prod compose stack is on 17 anyway.
        let pg = Postgres::default()
            .with_db_name("comic_reader_test")
            .with_user("comic")
            .with_password("comic")
            .with_tag("17-alpine")
            .start()
            .await
            .expect("postgres start");

        let host = pg.get_host().await.expect("pg host");
        let port = pg.get_host_port_ipv4(5432).await.expect("pg port");
        let db_url = format!("postgres://comic:comic@{}:{}/comic_reader_test", host, port);

        let db = Database::connect(&db_url).await.expect("connect db");
        use migration::MigratorTrait;
        migration::Migrator::up(&db, None)
            .await
            .expect("run migrations");

        // Redis (apalis backend) — required since Library Scanner v1.
        let redis = Redis::default()
            .with_tag("7-alpine")
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
            local_registration_open: true,
            jwt_access_ttl: "15m".into(),
            jwt_refresh_ttl: "30d".into(),
            rate_limit_enabled: true,
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
            _pg: pg,
            _redis: redis,
        }
    }
}
