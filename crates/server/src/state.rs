//! Shared application state (`Arc<AppState>` cloned into every handler).

use crate::config::Config;
use crate::email::EmailSender;
use crate::jobs::JobRuntime;
use crate::library::events::Broadcaster;
use crate::library::zip_lru::ZipLru;
use crate::observability::LogRingBuffer;
use crate::secrets::Secrets;
use metrics_exporter_prometheus::PrometheusHandle;
use sea_orm::DatabaseConnection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio_cron_scheduler::JobScheduler;

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub cfg: Config,
    pub db: DatabaseConnection,
    pub secrets: Secrets,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub zip_lru: ZipLru,
    pub prometheus: PrometheusHandle,
    /// In-process structured-log ring buffer (M6d). Source for
    /// `GET /admin/logs`. Always populated regardless of OTLP routing.
    pub log_buffer: LogRingBuffer,
    pub jobs: JobRuntime,
    /// Outbound transactional email (verify-email, password-reset, etc.).
    /// `Noop` when SMTP is unconfigured, `LettreSender` otherwise, or
    /// `MockSender` in tests. See `crate::email::build`.
    pub email: Arc<dyn EmailSender>,
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
    pub fn new(
        cfg: Config,
        db: DatabaseConnection,
        secrets: Secrets,
        prometheus: PrometheusHandle,
        log_buffer: LogRingBuffer,
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
        Self(Arc::new(Inner {
            cfg,
            db,
            secrets,
            started_at: chrono::Utc::now(),
            zip_lru,
            prometheus,
            log_buffer,
            jobs,
            email,
            events: Broadcaster::new(),
            thumb_inline_semaphore,
            thumb_job_inflight,
            thumb_path_cache,
            archive_work_semaphore,
            scheduler,
            library_scan_job_ids,
        }))
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
