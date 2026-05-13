//! Shared application state (`Arc<AppState>` cloned into every handler).

use crate::config::Config;
use crate::email::{Email, EmailSender, EmailStatus};
use crate::jobs::JobRuntime;
use crate::library::events::Broadcaster;
use crate::library::zip_lru::ZipLru;
use crate::observability::{LogRingBuffer, LogReloadHandle};
use crate::secrets::Secrets;
use arc_swap::ArcSwap;
use metrics_exporter_prometheus::PrometheusHandle;
use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_cron_scheduler::JobScheduler;

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
    pub db: DatabaseConnection,
    pub secrets: Secrets,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub zip_lru: ZipLru,
    pub prometheus: PrometheusHandle,
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
    /// Process-local cache from a thumbnail request key to the exact file that
    /// satisfied it. This avoids extension probing on hot image requests.
    pub thumb_path_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
    /// Global cap on blocking archive work shared by scans and thumbnail
    /// workers. Queue concurrency controls scheduling; this controls actual
    /// filesystem/archive pressure.
    pub archive_work_semaphore: Arc<Semaphore>,
    /// Live cron scheduler handle. Stored after startup so library schedule
    /// changes can register/replace scan jobs without a server restart.
    pub scheduler: Arc<Mutex<Option<JobScheduler>>>,
    pub library_scan_job_ids: Arc<Mutex<HashMap<uuid::Uuid, uuid::Uuid>>>,
}

impl AppState {
    // Every parameter is a logically-distinct dependency assembled in
    // `app::serve`; the parameter list mirrors `Inner`. Bundling into a
    // builder buys little vs. the noise of routing each name through
    // it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: Config,
        baseline: Config,
        db: DatabaseConnection,
        secrets: Secrets,
        prometheus: PrometheusHandle,
        log_buffer: LogRingBuffer,
        log_reload: LogReloadHandle,
        jobs: JobRuntime,
        email: Arc<dyn EmailSender>,
    ) -> Self {
        let zip_lru = ZipLru::new(cfg.zip_lru_capacity);
        let thumb_inline_parallel = cfg.thumb_inline_parallel.max(1);
        let thumb_inline_semaphore = Arc::new(Semaphore::new(thumb_inline_parallel));
        let archive_work_parallel = cfg.archive_work_parallel.max(1);
        let archive_work_semaphore = Arc::new(Semaphore::new(archive_work_parallel));
        let thumb_job_inflight = Arc::new(Mutex::new(HashSet::new()));
        let thumb_path_cache = Arc::new(Mutex::new(HashMap::new()));
        let scheduler = Arc::new(Mutex::new(None));
        let library_scan_job_ids = Arc::new(Mutex::new(HashMap::new()));
        let initial_status = EmailStatus::from_sender(email.as_ref());
        Self(Arc::new(Inner {
            cfg: ArcSwap::from_pointee(cfg),
            cfg_baseline: Arc::new(baseline),
            db,
            secrets,
            started_at: chrono::Utc::now(),
            zip_lru,
            prometheus,
            log_buffer,
            log_reload,
            jobs,
            email: std::sync::Mutex::new(email),
            email_status: Arc::new(RwLock::new(initial_status)),
            events: Broadcaster::new(),
            thumb_inline_semaphore,
            thumb_job_inflight,
            thumb_path_cache,
            archive_work_semaphore,
            scheduler,
            library_scan_job_ids,
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

    pub async fn cached_thumb_path(&self, key: &str) -> Option<PathBuf> {
        self.thumb_path_cache.lock().await.get(key).cloned()
    }

    pub async fn cache_thumb_path(&self, key: String, path: PathBuf) {
        self.thumb_path_cache.lock().await.insert(key, path);
    }

    pub async fn uncache_thumb_path(&self, key: &str) {
        self.thumb_path_cache.lock().await.remove(key);
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
