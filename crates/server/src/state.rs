//! Shared application state (`Arc<AppState>` cloned into every handler).

use crate::auth::jwt::JwtKeys;
use crate::config::Config;
use crate::email::{Email, EmailSender, EmailStatus};
use crate::jobs::JobRuntime;
use crate::library::events::Broadcaster;
use crate::library::zip_lru::ZipLru;
use crate::observability::{LogReloadHandle, LogRingBuffer};
use crate::secrets::Secrets;
use arc_swap::ArcSwap;
use lru::LruCache;
use metrics_exporter_prometheus::PrometheusHandle;
use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_cron_scheduler::JobScheduler;

/// Capacity of the thumbnail-path LRU. Each entry is a small string key → path;
/// a few thousand covers the working set of an active browsing session while
/// bounding memory regardless of library size.
const THUMB_PATH_CACHE_CAP: usize = 4096;

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    /// Live config snapshot. Read via [`AppState::cfg`] which returns an
    /// owned `Arc<Config>`. Replaced atomically by the runtime-settings
    /// admin API (`PATCH /admin/settings`, milestone M2 onward of the
    /// runtime-config-admin plan).
    pub cfg: ArcSwap<Config>,
    /// Env-only snapshot from boot — the value `Config::load()` produced
    /// before `overlay_db` was applied. Used by `PATCH /admin/settings`
    /// to rebuild from scratch each save, so deleting a DB row falls
    /// back to the env value rather than retaining the stale overlay.
    /// Immutable at runtime; replacing requires a restart.
    pub cfg_baseline: Arc<Config>,
    /// Effective config at boot (env baseline + DB overlay), captured once.
    /// Unlike [`Inner::cfg`] this never changes after startup, so it's the
    /// reference the restart-pending banner diffs the live `cfg` against for
    /// boot-only settings (worker pools, ZIP LRU, the metadata cron) — those
    /// keep running on the boot value even after a PATCH updates `cfg`.
    pub cfg_boot: Arc<Config>,
    pub db: DatabaseConnection,
    pub secrets: Secrets,
    /// JWT signer/verifier built once at boot from the Ed25519 secret +
    /// public_url, rather than re-derived per request (PERF-9). `public_url` is
    /// env/infra (not runtime-editable), so the boot value is stable for the
    /// process. Used by the auth extractor (verify) and the sign-in / OIDC
    /// callback (issue).
    pub jwt_keys: Arc<JwtKeys>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub zip_lru: ZipLru,
    /// Caches `SHA-256(app-password) → row id` so repeat OPDS/Bearer auths skip
    /// the argon2 scan (PERF-1). See [`crate::auth::app_password::verify`].
    pub app_password_cache: crate::auth::app_password::AppPasswordCache,
    pub prometheus: PrometheusHandle,
    /// Process/runtime gauge sampler (`folio_process_*`). The `/metrics`
    /// handler calls `.collect()` per scrape before rendering.
    pub process_metrics: metrics_process::Collector,
    /// In-process structured-log ring buffer (M6d). Source for
    /// `GET /admin/logs`. Always populated regardless of OTLP routing.
    pub log_buffer: LogRingBuffer,
    /// Handle for swapping the live `EnvFilter` directive on
    /// `observability.log_level` changes. Live-reload added in M4 of
    /// the runtime-config-admin plan.
    pub log_reload: LogReloadHandle,
    pub jobs: JobRuntime,
    /// Outbound transactional email (verify-email, password-reset, etc.).
    /// `Noop` when SMTP is unconfigured, `LettreSender` otherwise, or
    /// `MockSender` in tests. See `crate::email::build`.
    ///
    /// Replaced by `PATCH /admin/settings` when an `smtp.*` key changes
    /// (M2 of the runtime-config-admin plan). Wrapped in a `std::sync::Mutex`
    /// rather than `ArcSwap` because `arc-swap` requires `T: Sized` and
    /// `dyn EmailSender` is unsized — the lock is only held to clone the
    /// `Arc`, never across an await. Read with [`AppState::email`] or
    /// [`AppState::send_email`].
    pub email: std::sync::Mutex<Arc<dyn EmailSender>>,
    /// Last-result probe surfaced by `GET /admin/email/status`. Updated
    /// on every successful or failed [`AppState::send_email`] call.
    pub email_status: Arc<RwLock<EmailStatus>>,
    pub events: Broadcaster,
    /// Shared `reqwest::Client` used by `upstream::proxy` to forward
    /// requests to the Next.js SSR server. Built once at startup so
    /// connection pooling kicks in; the per-request timeout is set on
    /// each call rather than baked into the client. Redirects are
    /// *not* followed — the proxy passes redirects through to the
    /// originating client verbatim. See
    /// `~/.claude/plans/rust-public-origin-1.0.md` for context.
    pub web_proxy_client: reqwest::Client,
    /// Global cap on concurrent on-demand thumbnail generations. The
    /// post-scan worker pre-generates everything for already-scanned
    /// libraries; this semaphore only kicks in when the HTTP handler hits a
    /// missing thumb (freshly-added issue, mid-scan reader, race) and
    /// prevents a frantic page-strip open from saturating the encoder pool.
    pub thumb_inline_semaphore: Arc<Semaphore>,
    /// Process-local dedupe for issue-level thumbnail catchup jobs. Redis may
    /// still contain jobs from a previous process, but this prevents one page
    /// strip burst from pushing the same issue dozens of times.
    pub thumb_job_inflight: Arc<Mutex<HashSet<String>>>,
    /// The subset of [`Self::thumb_job_inflight`] a worker is executing
    /// right now (same keys), so status surfaces can tell "running" from
    /// "queued". Held by a [`ThumbRunningGuard`] for the job's duration.
    pub thumb_job_running: Arc<std::sync::Mutex<HashSet<String>>>,
    /// When recent thumbnail jobs finished executing — the sliding window
    /// behind [`AppState::thumbs_per_min`]. Process-local, like the marks.
    pub thumb_throughput: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Instant>>>,
    /// Process-local cache from a thumbnail request key to the exact file that
    /// satisfied it, avoiding extension probing on hot image requests. Bounded
    /// LRU (PERF-9): the previous unbounded `HashMap` grew one entry per
    /// (issue, variant, page) ever served and was never evicted; a `std::sync`
    /// mutex replaces the global async lock since the critical section is a
    /// trivial map op held across no await.
    pub thumb_path_cache: Arc<std::sync::Mutex<LruCache<String, PathBuf>>>,
    /// Global cap on blocking archive work shared by scans and thumbnail
    /// workers. Queue concurrency controls scheduling; this controls actual
    /// filesystem/archive pressure.
    pub archive_work_semaphore: Arc<Semaphore>,
    /// Live cron scheduler handle. Stored after startup so library schedule
    /// changes can register/replace scan jobs without a server restart.
    pub scheduler: Arc<Mutex<Option<JobScheduler>>>,
    pub library_scan_job_ids: Arc<Mutex<HashMap<uuid::Uuid, uuid::Uuid>>>,
    /// Server-side cache for `GET /admin/server/latest-release`. Single
    /// in-flight fetch + 1-hour TTL so N admins polling don't each
    /// trigger a GitHub API call. `None` slot before the first fetch;
    /// `Some((when, payload))` afterwards where `payload` may itself
    /// be `None` (last fetch errored). See
    /// [`crate::api::server_releases`].
    pub latest_release_cache: Arc<Mutex<crate::api::server_releases::ReleaseCache>>,
    /// File-watcher registry (WP-3.1): per-library mode + last trigger, the
    /// running watcher handles, and the supervisor nudge. Populated by
    /// [`crate::library::watcher::spawn_supervisor`] (started in `app::serve`;
    /// tests drive watchers directly).
    pub watchers: Arc<crate::library::watcher::WatcherRegistry>,
    /// WP-7.4: per-series "similar series" neighbour cache (unfiltered;
    /// ACL applied per request). Invalidated by scans, metadata applies
    /// and manual metadata edits. See [`crate::similarity`].
    pub similarity: Arc<crate::similarity::SimilarityCache>,
    /// Issues with a lazy page-hash backfill running (WP-8.4), so a
    /// burst of archive opens starts at most one task per issue. See
    /// [`crate::reading::page_hash_backfill`].
    pub page_hash_backfill_inflight: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl AppState {
    // Every parameter is a logically-distinct dependency assembled in
    // `app::serve`; the parameter list mirrors `Inner`. Bundling into a
    // builder buys little vs. the noise of routing each name through
    // it.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        cfg: Config,
        baseline: Config,
        db: DatabaseConnection,
        secrets: Secrets,
        prometheus: PrometheusHandle,
        process_metrics: metrics_process::Collector,
        log_buffer: LogRingBuffer,
        log_reload: LogReloadHandle,
        jobs: JobRuntime,
        email: Arc<dyn EmailSender>,
    ) -> Self {
        // Snapshot the effective boot config before `cfg` is moved into the
        // live `ArcSwap` below — the restart-pending banner diffs against it.
        let cfg_boot = Arc::new(cfg.clone());
        let zip_lru = ZipLru::new(cfg.zip_lru_capacity, cfg.archive_limits());
        let app_password_cache = crate::auth::app_password::AppPasswordCache::new();
        // Build the JWT keys once (PERF-9). from_secret is effectively
        // infallible (byte wrapping + key construction); a failure here means a
        // broken boot secret, so panicking at startup is correct.
        let jwt_keys = Arc::new(
            JwtKeys::from_secret(&secrets.jwt_ed25519, &cfg.public_url)
                .expect("build JWT keys from boot secret + public_url"),
        );
        let thumb_inline_parallel = cfg.thumb_inline_parallel.max(1);
        let thumb_inline_semaphore = Arc::new(Semaphore::new(thumb_inline_parallel));
        let archive_work_parallel = cfg.archive_work_parallel.max(1);
        let archive_work_semaphore = Arc::new(Semaphore::new(archive_work_parallel));
        let thumb_job_inflight = Arc::new(Mutex::new(HashSet::new()));
        let thumb_job_running = Arc::new(std::sync::Mutex::new(HashSet::new()));
        let thumb_throughput = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        let thumb_path_cache = Arc::new(std::sync::Mutex::new(LruCache::new(
            NonZeroUsize::new(THUMB_PATH_CACHE_CAP).expect("nonzero"),
        )));
        let scheduler = Arc::new(Mutex::new(None));
        let library_scan_job_ids = Arc::new(Mutex::new(HashMap::new()));
        let initial_status = EmailStatus::from_sender(email.as_ref());
        // Build the proxy client once. `redirect::Policy::none()` so a 3xx
        // from Next is forwarded to the originating client unchanged.
        // `pool_idle_timeout` keeps keepalive connections warm for
        // chained SSR requests (Next's RSC pipeline often emits several
        // sub-requests for a single page load).
        let web_proxy_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("build proxy client");
        Self(Arc::new(Inner {
            cfg: ArcSwap::from_pointee(cfg),
            cfg_baseline: Arc::new(baseline),
            cfg_boot,
            db,
            secrets,
            jwt_keys,
            started_at: chrono::Utc::now(),
            zip_lru,
            app_password_cache,
            prometheus,
            process_metrics,
            log_buffer,
            log_reload,
            jobs,
            email: std::sync::Mutex::new(email),
            email_status: Arc::new(RwLock::new(initial_status)),
            events: Broadcaster::new(),
            web_proxy_client,
            thumb_inline_semaphore,
            thumb_job_inflight,
            thumb_job_running,
            thumb_throughput,
            thumb_path_cache,
            archive_work_semaphore,
            scheduler,
            library_scan_job_ids,
            latest_release_cache: Arc::new(Mutex::new(
                crate::api::server_releases::ReleaseCache::default(),
            )),
            watchers: Arc::new(crate::library::watcher::WatcherRegistry::new()),
            similarity: Arc::new(crate::similarity::SimilarityCache::new()),
            page_hash_backfill_inflight: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }))
    }

    /// Snapshot of the current [`Config`]. Cheap (`Arc` clone). Use this in
    /// handlers and downstream call sites instead of holding a long-lived
    /// reference, so that runtime settings changes are picked up on the
    /// next request without forcing a server restart.
    pub fn cfg(&self) -> Arc<Config> {
        self.0.cfg.load_full()
    }

    /// Env-only baseline captured at boot. Use this when rebuilding a
    /// fresh Config + overlay (e.g. inside `PATCH /admin/settings`) so a
    /// deleted DB row reverts to the env value rather than retaining the
    /// previous overlay state.
    pub fn cfg_baseline(&self) -> Arc<Config> {
        self.0.cfg_baseline.clone()
    }

    /// Effective config captured at boot. Diff the live [`Self::cfg`] against
    /// this to find boot-only settings that changed since startup and need a
    /// restart to take effect (see `api::server_info::restart_pending`).
    pub fn cfg_boot(&self) -> Arc<Config> {
        self.0.cfg_boot.clone()
    }

    /// Atomically replace the live config. Returns the previous snapshot
    /// (mostly useful for diff logging). Caller is responsible for any
    /// side-effects (rebuilding the email sender, swapping the OIDC
    /// provider registry, etc.) — those land in later milestones.
    pub fn replace_cfg(&self, cfg: Config) -> Arc<Config> {
        self.0.cfg.swap(Arc::new(cfg))
    }

    /// Snapshot of the current email sender. Cheap (`Arc` clone). Prefer
    /// [`Self::send_email`] when sending — it records `last_send_*` in
    /// `email_status` for the `/admin/email/status` probe.
    pub fn email(&self) -> Arc<dyn EmailSender> {
        self.0.email.lock().expect("email mutex poisoned").clone()
    }

    /// Replace the live email sender. Called from `PATCH /admin/settings`
    /// when an `smtp.*` key changed. Also updates `email_status.configured`
    /// so the status probe reflects the new wiring without waiting for a
    /// send.
    pub async fn replace_email(&self, sender: Arc<dyn EmailSender>) {
        let configured = sender.is_configured();
        {
            let mut guard = self.0.email.lock().expect("email mutex poisoned");
            *guard = sender;
        }
        // Preserve last-send history; only the configured flag tracks
        // the new sender shape until a real send updates the rest.
        let mut guard = self.0.email_status.write().await;
        guard.configured = configured;
    }

    /// Send a transactional email and record the result in `email_status`.
    /// Use this in preference to `email().send(...)` so the
    /// `/admin/email/status` probe stays in sync with actual outbound
    /// activity.
    pub async fn send_email(&self, email: Email) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        let sender = self.email();
        let result = sender.send(email).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let mut guard = self.0.email_status.write().await;
        guard.last_send_at = Some(chrono::Utc::now());
        guard.last_send_ok = Some(result.is_ok());
        guard.last_duration_ms = Some(elapsed_ms);
        guard.last_error = result.as_ref().err().map(|e| e.to_string());
        result
    }

    pub async fn try_mark_thumb_job_queued(&self, key: String) -> bool {
        self.thumb_job_inflight.lock().await.insert(key)
    }

    pub async fn unmark_thumb_job_queued(&self, key: &str) {
        self.thumb_job_inflight.lock().await.remove(key);
    }

    pub async fn clear_thumb_job_marks(&self) {
        self.thumb_job_inflight.lock().await.clear();
    }

    pub async fn thumb_job_keys(&self) -> HashSet<String> {
        self.thumb_job_inflight.lock().await.clone()
    }

    /// Mark a thumbnail job as executing until the returned guard drops.
    pub fn mark_thumb_job_running(&self, key: String) -> ThumbRunningGuard {
        self.thumb_job_running
            .lock()
            .expect("thumb_job_running poisoned")
            .insert(key.clone());
        ThumbRunningGuard {
            set: self.thumb_job_running.clone(),
            throughput: self.thumb_throughput.clone(),
            key,
        }
    }

    /// Thumbnail jobs finished per minute over the last
    /// [`THUMB_RATE_WINDOW`], or `None` with too few samples to say (idle,
    /// or the drain only just started).
    pub fn thumbs_per_min(&self) -> Option<f64> {
        let now = std::time::Instant::now();
        let mut window = self
            .thumb_throughput
            .lock()
            .expect("thumb_throughput poisoned");
        while window
            .front()
            .is_some_and(|t| now.duration_since(*t) > THUMB_RATE_WINDOW)
        {
            window.pop_front();
        }
        thumb_rate_per_min(window.len(), window.front().map(|t| now.duration_since(*t)))
    }

    pub fn thumb_job_running_keys(&self) -> HashSet<String> {
        self.thumb_job_running
            .lock()
            .expect("thumb_job_running poisoned")
            .clone()
    }

    pub fn cached_thumb_path(&self, key: &str) -> Option<PathBuf> {
        // `LruCache::get` marks recency, so it needs &mut — the std Mutex gives
        // it. No await is held across the lock.
        self.thumb_path_cache
            .lock()
            .expect("thumb_path_cache poisoned")
            .get(key)
            .cloned()
    }

    pub fn cache_thumb_path(&self, key: String, path: PathBuf) {
        self.thumb_path_cache
            .lock()
            .expect("thumb_path_cache poisoned")
            .put(key, path);
    }

    pub fn uncache_thumb_path(&self, key: &str) {
        self.thumb_path_cache
            .lock()
            .expect("thumb_path_cache poisoned")
            .pop(key);
    }

    pub async fn set_scheduler(&self, scheduler: JobScheduler) {
        *self.scheduler.lock().await = Some(scheduler);
    }
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Removes a thumbnail job from [`AppState::thumb_job_running`] on drop, so
/// every exit path of the handler (including a panic) clears it.
pub struct ThumbRunningGuard {
    set: Arc<std::sync::Mutex<HashSet<String>>>,
    throughput: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Instant>>>,
    key: String,
}

/// Sliding window the thumbnail rate is measured over.
pub const THUMB_RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);

/// Finished-job samples kept; bounds the window on a very fast drain.
const THUMB_RATE_MAX_SAMPLES: usize = 20_000;

/// Rate from `samples` jobs finished since `oldest_age` ago. Needs a few
/// samples spread over a few seconds, otherwise one burst reads as a huge
/// rate and the ETA built on it is nonsense.
pub(crate) fn thumb_rate_per_min(
    samples: usize,
    oldest_age: Option<std::time::Duration>,
) -> Option<f64> {
    let span = oldest_age?.as_secs_f64();
    if samples < 5 || span < 5.0 {
        return None;
    }
    Some(samples as f64 / span * 60.0)
}

impl Drop for ThumbRunningGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.key);
        }
        if let Ok(mut window) = self.throughput.lock() {
            if window.len() >= THUMB_RATE_MAX_SAMPLES {
                window.pop_front();
            }
            window.push_back(std::time::Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::thumb_rate_per_min;
    use std::time::Duration;

    #[test]
    fn thumb_rate_needs_enough_samples_over_enough_time() {
        assert_eq!(thumb_rate_per_min(0, None), None);
        // One burst: plenty of samples but no time base.
        assert_eq!(thumb_rate_per_min(50, Some(Duration::from_secs(1))), None);
        // A couple of jobs over a long span says nothing either.
        assert_eq!(thumb_rate_per_min(2, Some(Duration::from_secs(60))), None);
        // 30 jobs over 60s = 30/min.
        let rate = thumb_rate_per_min(30, Some(Duration::from_secs(60))).unwrap();
        assert!((rate - 30.0).abs() < 1e-9);
    }
}
