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
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, Statement};
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
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
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
    /// This test's lease on a Redis logical DB from the shared instance (see
    /// [`SharedRedis`]). Held for the test's lifetime; `Drop` returns the index
    /// to the in-process pool (plain `cargo test` path) or is a no-op (nextest,
    /// where the slot index is owned by nextest).
    _redis_lease: RedisLease,
    /// Shared-Postgres base URL + this test's clone DB name, so `Drop` can drop
    /// the clone (see [`drop_test_db`]).
    pg_base: String,
    db_name: String,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        // Drop the clone DB so databases + their lingering connections don't
        // accumulate on the shared server across a binary's tests.
        drop_test_db(self.pg_base.clone(), self.db_name.clone());
    }
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
    /// `Some` for the per-process testcontainer (the local `cargo test`
    /// fallback); `None` when pointed at an external shared server (CI / the
    /// `cargo nextest` path). Held for the process lifetime so the container
    /// outlives every test.
    _container: Option<ContainerAsync<Postgres>>,
    /// Server base URL without a database path, e.g.
    /// `postgres://comic:comic@localhost:5432`. Maintenance/template/clone URLs
    /// are `{base}/<db>`.
    base: String,
}

static SHARED_PG: tokio::sync::OnceCell<SharedPg> = tokio::sync::OnceCell::const_new();

/// Advisory-lock key serializing template creation across processes. nextest
/// runs process-per-test, so many processes race on the shared server's
/// template; this makes only the first build it.
const TEMPLATE_LOCK_KEY: i64 = 559_038_737;

#[expect(
    clippy::print_stderr,
    reason = "fatal harness error before logging exists"
)]
async fn shared_pg() -> &'static SharedPg {
    SHARED_PG
        .get_or_init(|| async {
            // A failure here must NOT bubble as a panic: tokio's `OnceCell`
            // re-runs the init closure after a panic, so every test in the
            // process would retry the whole setup — storming the log and piling
            // up resources (which is how Phase 1 first took down CI). Fail the
            // process hard, once, with a readable message instead.
            match build_shared_pg().await {
                Ok(pg) => pg,
                Err(e) => {
                    eprintln!("FATAL: shared test Postgres init failed: {e}");
                    std::process::exit(1);
                }
            }
        })
        .await
}

/// Provision the shared Postgres for this process and ensure the migrated
/// template exists. Two modes:
///   - `COMIC_TEST_PG_URL` set → an **external** shared server (CI service
///     container, or a local test PG). Required for `cargo nextest`, which runs
///     process-per-test: a per-process container would degenerate to per-test.
///   - unset → boot a per-process **testcontainer** (the `cargo test` default;
///     no external services needed for a plain local run).
///
/// Fallible so [`shared_pg`] can fail fast instead of panic-and-retry.
async fn build_shared_pg() -> Result<SharedPg, String> {
    if let Ok(url) = std::env::var("COMIC_TEST_PG_URL") {
        let base = url
            .rsplit_once('/')
            .map(|(b, _db)| b.to_owned())
            .ok_or_else(|| format!("COMIC_TEST_PG_URL needs a /<db> path: {url}"))?;
        ensure_template(&base).await?;
        return Ok(SharedPg {
            _container: None,
            base,
        });
    }

    // Pin to 18-alpine; the testcontainers-modules default of 11-alpine predates
    // STORED generated columns (Postgres 12+) used by the search migration, and
    // the prod compose stack is on 18 anyway. Raise max_connections (every
    // per-test clone shares this one server) and drop durability — pointless for
    // ephemeral test data, and it speeds migrations + writes.
    let build = || {
        Postgres::default()
            .with_db_name("comic_reader_test")
            .with_user("comic")
            .with_password("comic")
            .with_tag("18-alpine")
            .with_cmd([
                "postgres",
                "-c",
                "max_connections=200",
                "-c",
                "fsync=off",
                "-c",
                "synchronous_commit=off",
                "-c",
                "full_page_writes=off",
            ])
    };
    // Container start is the step most likely to flake on a loaded CI runner;
    // retry a few times before giving up.
    let mut container = None;
    let mut last_err = String::from("no attempts");
    for attempt in 1..=3 {
        match build().start().await {
            Ok(c) => {
                container = Some(c);
                break;
            }
            Err(e) => {
                last_err = format!("postgres start (attempt {attempt}/3): {e}");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
    let container = container.ok_or(last_err)?;
    let host = container
        .get_host()
        .await
        .map_err(|e| format!("pg host: {e}"))?
        .to_string();
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .map_err(|e| format!("pg port: {e}"))?;
    let base = format!("postgres://comic:comic@{host}:{port}");
    ensure_template(&base).await?;
    Ok(SharedPg {
        _container: Some(container),
        base,
    })
}

/// Create + migrate [`TEMPLATE_DB`] if absent, serialized across processes by a
/// Postgres advisory lock. Holding the lock across the existence check + build
/// means "exists" reliably implies "fully migrated" for the next process.
async fn ensure_template(base: &str) -> Result<(), String> {
    let admin = admin_conn(base).await;
    admin
        .execute_unprepared(&format!("SELECT pg_advisory_lock({TEMPLATE_LOCK_KEY})"))
        .await
        .map_err(|e| format!("advisory lock: {e}"))?;
    let result = build_template_locked(base, &admin).await;
    // Release the lock + connection regardless of outcome.
    let _ = admin
        .execute_unprepared(&format!("SELECT pg_advisory_unlock({TEMPLATE_LOCK_KEY})"))
        .await;
    let _ = admin.close().await;
    result
}

async fn build_template_locked(
    base: &str,
    admin: &sea_orm::DatabaseConnection,
) -> Result<(), String> {
    let exists = admin
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT 1 FROM pg_database WHERE datname = '{TEMPLATE_DB}'"),
        ))
        .await
        .map_err(|e| format!("template existence check: {e}"))?
        .is_some();
    if exists {
        return Ok(());
    }
    admin
        .execute_unprepared(&format!("CREATE DATABASE {TEMPLATE_DB}"))
        .await
        .map_err(|e| format!("create template db: {e}"))?;
    // sqlx_logging off: migration SQL logging is noise, and libtest dumps
    // captured output on failure — keep it small so a real error stays legible.
    let mut tmpl_opts = ConnectOptions::new(format!("{base}/{TEMPLATE_DB}"));
    tmpl_opts.sqlx_logging(false);
    let tmpl = Database::connect(tmpl_opts)
        .await
        .map_err(|e| format!("connect template: {e}"))?;
    use migration::MigratorTrait;
    let migrated = migration::Migrator::up(&tmpl, None)
        .await
        .map_err(|e| format!("migrate template: {e}"));
    let _ = tmpl.close().await;
    if let Err(e) = migrated {
        // Drop the half-built template so a later run rebuilds it cleanly.
        let _ = admin
            .execute_unprepared(&format!(
                "DROP DATABASE IF EXISTS {TEMPLATE_DB} WITH (FORCE)"
            ))
            .await;
        return Err(e);
    }
    Ok(())
}

/// A single-connection, short-lived pool on the maintenance database, for
/// issuing `CREATE`/`DROP DATABASE` + advisory locks (DDL can't run in a
/// transaction — `execute_unprepared` runs it in autocommit). Caller closes it.
async fn admin_conn(base: &str) -> sea_orm::DatabaseConnection {
    let mut opts = ConnectOptions::new(format!("{base}/comic_reader_test"));
    opts.max_connections(1)
        .min_connections(0)
        .sqlx_logging(false);
    Database::connect(opts).await.expect("connect admin")
}

/// Clone a fresh, already-migrated database off [`TEMPLATE_DB`] for one test.
/// Returns the new database NAME (the caller builds the URL). The clone is done
/// on a fresh connection bound to the calling test's runtime (see [`shared_pg`]
/// for why a shared pool can't be reused across tests). The database is dropped
/// in [`TestApp`]'s `Drop` — see [`drop_test_db`] — so neither databases nor
/// their lingering backend connections accumulate on the shared server within a
/// binary (which would otherwise exhaust `max_connections` on a big test file).
async fn clone_test_db(pg: &SharedPg) -> String {
    let db_name = format!("t_{}", Uuid::now_v7().simple());
    let admin = admin_conn(&pg.base).await;
    admin
        .execute_unprepared(&format!(
            "CREATE DATABASE \"{db_name}\" TEMPLATE {TEMPLATE_DB}"
        ))
        .await
        .expect("clone test db from template");
    admin.close().await.ok();
    db_name
}

/// Drop a per-test clone database, terminating any backends still attached
/// (`WITH (FORCE)`, Postgres 13+) so connections don't pile up on the shared
/// server. Runs on its own current-thread runtime in a dedicated OS thread so
/// it works from `Drop` regardless of any ambient runtime's state.
fn drop_test_db(base: String, db_name: String) {
    let _ = std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async {
            // Fallible (no `.expect`): a connect hiccup during teardown should
            // leak the clone DB, not panic + spam backtraces.
            if let Ok(admin) = Database::connect({
                let mut opts = ConnectOptions::new(format!("{base}/comic_reader_test"));
                opts.max_connections(1)
                    .min_connections(0)
                    .sqlx_logging(false);
                opts
            })
            .await
            {
                let _ = admin
                    .execute_unprepared(&format!(
                        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
                    ))
                    .await;
                let _ = admin.close().await;
            }
        });
    })
    .join();
}

/// One Redis instance per test *process*, shared by every `TestApp` that
/// process spawns — the Redis analogue of [`SharedPg`]. Previously every test
/// booted its own Redis container (~600 container create/start/healthcheck/stop
/// cycles per CI run), which dominated the suite once Postgres was shared. Now
/// each test gets an isolated **logical DB** on one shared instance.
///
/// Two provisioning modes (mirrors [`build_shared_pg`]):
///   - `COMIC_REDIS_URL` set → an **external** shared Redis (CI service
///     container, or `just test-rust-fast`'s throwaway). Required for nextest,
///     which runs process-per-test: a per-process container would degenerate to
///     per-test.
///   - unset → boot a per-process **testcontainer** (the plain `cargo test`
///     fallback; no external services needed).
///
/// Per-test isolation = a distinct Redis logical DB (`redis://host:port/<n>`),
/// `FLUSHDB`-ed on acquire. Index selection differs by run mode (see
/// [`acquire_redis_db`]). Requires `test-threads <= db_count` so concurrent
/// tests never share an index.
struct SharedRedis {
    /// `Some` for the per-process testcontainer; `None` for an external server.
    _container: Option<ContainerAsync<Redis>>,
    /// `redis://host:port` with no `/<db>` path.
    base: String,
    /// `databases` from `CONFIG GET` (default 16). Bounds the index space.
    db_count: u32,
    /// In-process index pool for the plain `cargo test` path (single process,
    /// libtest thread concurrency). `None` under nextest, where each test is its
    /// own process and uses `NEXTEST_TEST_GLOBAL_SLOT` for a cross-process-safe
    /// index — an in-process pool couldn't coordinate across those processes.
    pool: Option<RedisDbPool>,
}

/// Counting semaphore (caps concurrent leases to `db_count`) + a free-list that
/// hands out the actual distinct index. The semaphore guarantees the free-list
/// is non-empty whenever a permit is held.
struct RedisDbPool {
    sem: Arc<Semaphore>,
    free: Arc<StdMutex<Vec<u32>>>,
}

/// A held Redis logical-DB index. Returned to the pool on drop (plain mode);
/// a no-op under nextest. Kept alive by [`TestApp`] for the test's lifetime.
struct RedisLease {
    idx: u32,
    /// `Some((permit, free))` in plain mode; `None` under nextest.
    ret: Option<(OwnedSemaphorePermit, Arc<StdMutex<Vec<u32>>>)>,
}

impl Drop for RedisLease {
    fn drop(&mut self) {
        if let Some((permit, free)) = self.ret.take() {
            // Return the index BEFORE releasing the permit so a waiter that
            // acquires the freed permit always finds an index to pop.
            free.lock()
                .expect("redis free-list poisoned")
                .push(self.idx);
            drop(permit);
        }
    }
}

static SHARED_REDIS: tokio::sync::OnceCell<SharedRedis> = tokio::sync::OnceCell::const_new();

#[expect(
    clippy::print_stderr,
    reason = "fatal harness error before logging exists"
)]
async fn shared_redis() -> &'static SharedRedis {
    SHARED_REDIS
        .get_or_init(|| async {
            match build_shared_redis().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("FATAL: shared test Redis init failed: {e}");
                    std::process::exit(1);
                }
            }
        })
        .await
}

async fn build_shared_redis() -> Result<SharedRedis, String> {
    let under_nextest = std::env::var("NEXTEST_TEST_GLOBAL_SLOT").is_ok();
    let (container, base) = if let Ok(url) = std::env::var("COMIC_REDIS_URL") {
        (None, redis_base(&url))
    } else {
        // Plain `cargo test`: boot one Redis per process. `--databases 256`
        // gives the in-process pool plenty of headroom for libtest concurrency.
        let mut container = None;
        let mut last_err = String::from("no attempts");
        for attempt in 1..=3 {
            let build = Redis::default().with_tag("8-alpine").with_cmd([
                "redis-server",
                "--databases",
                "256",
            ]);
            match build.start().await {
                Ok(c) => {
                    container = Some(c);
                    break;
                }
                Err(e) => {
                    last_err = format!("redis start (attempt {attempt}/3): {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
        let container = container.ok_or(last_err)?;
        let host = container
            .get_host()
            .await
            .map_err(|e| format!("redis host: {e}"))?
            .to_string();
        let port = container
            .get_host_port_ipv4(6379)
            .await
            .map_err(|e| format!("redis port: {e}"))?;
        (Some(container), format!("redis://{host}:{port}"))
    };

    let db_count = redis_db_count(&base).await.unwrap_or(16);
    // Pool only off-nextest (single process). Seed indices 0..db_count.
    let pool = if under_nextest {
        None
    } else {
        let n = db_count as usize;
        Some(RedisDbPool {
            sem: Arc::new(Semaphore::new(n)),
            free: Arc::new(StdMutex::new((0..db_count).collect())),
        })
    };
    Ok(SharedRedis {
        _container: container,
        base,
        db_count,
        pool,
    })
}

/// Strip any `/<db>` (and query) suffix off a redis URL, leaving `redis://host:port`.
fn redis_base(url: &str) -> String {
    // redis://host:port[/db]. Find the host[:port] segment after the scheme.
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split(['/', '?']).next().unwrap_or(rest);
            format!("{scheme}://{authority}")
        }
        None => url.trim_end_matches('/').to_owned(),
    }
}

async fn redis_db_count(base: &str) -> Option<u32> {
    let client = redis::Client::open(format!("{base}/0")).ok()?;
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    // `CONFIG GET databases` → ["databases", "16"].
    let kv: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("databases")
        .query_async(&mut conn)
        .await
        .ok()?;
    kv.get(1).and_then(|s| s.parse().ok())
}

/// Acquire an isolated Redis logical DB for one test and return its URL +
/// the lease that frees it. Under nextest the index is the per-test global
/// slot (unique across the concurrently-running processes that share the
/// external Redis); otherwise it comes from the in-process pool.
async fn acquire_redis_db(redis: &'static SharedRedis) -> (String, RedisLease) {
    let (idx, ret) = match (&redis.pool, std::env::var("NEXTEST_TEST_GLOBAL_SLOT")) {
        (Some(pool), _) => {
            let permit = pool
                .sem
                .clone()
                .acquire_owned()
                .await
                .expect("redis db semaphore");
            let idx = pool
                .free
                .lock()
                .expect("redis free-list poisoned")
                .pop()
                .expect("free redis db (semaphore guarantees availability)");
            (idx, Some((permit, pool.free.clone())))
        }
        (None, Ok(slot)) => {
            let slot: u32 = slot.parse().unwrap_or(0);
            (slot % redis.db_count, None)
        }
        // No pool and no slot: shouldn't happen (pool is built whenever not
        // under nextest), but fall back to DB 0 rather than panic.
        (None, Err(_)) => (0, None),
    };
    let url = format!("{}/{}", redis.base, idx);
    // Clean slate: the previous occupant of this index/slot left apalis +
    // coalescing keys behind.
    if let Ok(client) = redis::Client::open(url.clone())
        && let Ok(mut conn) = client.get_multiplexed_async_connection().await
    {
        let _: redis::RedisResult<()> = redis::cmd("FLUSHDB").query_async(&mut conn).await;
    }
    (url, RedisLease { idx, ret })
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
        let db_name = clone_test_db(pg).await;
        let db_url = format!("{}/{}", pg.base, db_name);

        // Cap the per-test pool: every test now shares one server's connection
        // budget, so don't let each open the sea-orm default fan-out. 6 keeps
        // the request/assert shape of these tests comfortable while leaving the
        // 100-connection Postgres service headroom at 12-way nextest
        // oversubscription (12 × 6 = 72); the realistic peak is ~2 conns/test.
        let mut db_opts = ConnectOptions::new(db_url.clone());
        db_opts.max_connections(6).sqlx_logging(false);
        let db = Database::connect(db_opts).await.expect("connect test db");

        // Redis (apalis backend) — required since Library Scanner v1. One
        // shared instance per process; this test gets an isolated logical DB
        // (CI-speed Phase 3) instead of its own container.
        let redis = shared_redis().await;
        let (redis_url, redis_lease) = acquire_redis_db(redis).await;

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
            _redis_lease: redis_lease,
            pg_base: pg.base.clone(),
            db_name,
        }
    }
}
