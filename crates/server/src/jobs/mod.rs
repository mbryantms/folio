//! Background job runtime — apalis 0.7 over Redis.
//!
//! Library Scanner v1, Milestone 2 (spec §3, §3.2, §4.8).
//!
//! **Retry / orphan policy** (WP-1.6 ops hygiene) is explicit, not
//! inherited from library defaults: every queue is built by [`storage`]
//! from [`storage_config`], which pins `reenqueue_orphaned_after` to
//! [`JOB_REENQUEUE_ORPHANED_AFTER_SECS`] and keeps the namespace apalis
//! derives from the job type (so queued jobs survive an upgrade). The
//! per-job attempt budget is [`JOB_MAX_ATTEMPTS`]; see its doc for why it
//! is a mirrored constant rather than a setter.
//!
//! Two responsibilities:
//!   1. Own the [`apalis_redis::RedisStorage`] for each typed queue.
//!   2. Implement the **library-scoped coalescing** the spec requires
//!      (§3.2): only one full-library scan in flight at a time; subsequent
//!      triggers are coalesced into a single queued slot.
//!
//! Coalescing uses three Redis keys per library:
//!   - `scan:in_flight:<library_id>` — set while a scan job is executing
//!   - `scan:queued:<library_id>`    — bool flag; set by the coalescer when
//!     a trigger arrived during an in-flight scan
//!   - `scan:scan_id:<library_id>`   — current scan run id (for read-back)
//!
//! Watcher-scoped scans (WP-3.1) ride the same three keys, so an event
//! storm collapses into one in-flight scan plus at most one queued
//! follow-up. Two extra keys carry the follow-up's scope:
//!   - `scan:queued:<library_id>:scoped` — set when every trigger that
//!     queued the follow-up was scoped; a full trigger deletes it (full wins)
//!   - `scan:queued:<library_id>:dirs`   — Redis SET, union of the touched
//!     directories of every scoped trigger that coalesced
//!
//! Workers run inside the same tokio runtime as the HTTP server. There is no
//! separate worker binary; this matches the single-binary deploy story.

use crate::state::AppState;
use apalis_redis::{Config as RedisConfig, RedisStorage, connect};
use entity::scan_run::{ActiveModel as ScanRunAM, Entity as ScanRunEntity};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

pub mod archive_edit;
pub mod archive_transforms;
pub mod backfill;
pub mod backup_prune;
pub mod close_dangling_sessions;
pub mod hard_purge;
pub mod metadata_apply;
pub mod metadata_resume;
pub mod metadata_search;
pub mod metrics_layer;
pub mod orphan_sweep;
pub mod post_scan;
pub mod prune_auth_sessions;
pub mod rewrite_sidecars;
pub mod scan;
pub mod scan_series;
pub mod scheduler;

/// Owned by [`AppState`]. Cloneable handle into the apalis storages and a
/// raw Redis connection (used for the coalescing keys, which live outside
/// apalis's queue model).
#[derive(Clone)]
pub struct JobRuntime {
    pub db: DatabaseConnection,
    pub scan_storage: RedisStorage<scan::Job>,
    pub scan_series_storage: RedisStorage<scan_series::Job>,
    pub post_scan_thumbs_storage: RedisStorage<post_scan::ThumbsJob>,
    pub post_scan_search_storage: RedisStorage<post_scan::SearchJob>,
    pub post_scan_dictionary_storage: RedisStorage<post_scan::DictionaryJob>,
    pub metadata_search_series_storage: RedisStorage<metadata_search::SearchSeriesJob>,
    pub metadata_search_issue_storage: RedisStorage<metadata_search::SearchIssueJob>,
    pub metadata_apply_series_storage: RedisStorage<metadata_apply::ApplySeriesJob>,
    pub metadata_apply_issue_storage: RedisStorage<metadata_apply::ApplyIssueJob>,
    pub rewrite_issue_sidecars_storage: RedisStorage<rewrite_sidecars::RewriteIssueSidecarsJob>,
    pub archive_edit_storage: RedisStorage<archive_edit::ArchiveEditJob>,
    pub backfill_storage: RedisStorage<backfill::BackfillJob>,
    pub redis: ConnectionManager,
}

impl JobRuntime {
    /// Connect to Redis and build the typed storages. Fails fast if Redis is
    /// unreachable — the spec treats Redis as a hard dependency post-Milestone-2.
    pub async fn new(redis_url: &str, db: DatabaseConnection) -> anyhow::Result<Self> {
        let conn = connect(redis_url).await?;
        let scan_storage = storage::<scan::Job>(conn.clone());
        let scan_series_storage = storage::<scan_series::Job>(conn.clone());
        let post_scan_thumbs_storage = storage::<post_scan::ThumbsJob>(conn.clone());
        let post_scan_search_storage = storage::<post_scan::SearchJob>(conn.clone());
        let post_scan_dictionary_storage = storage::<post_scan::DictionaryJob>(conn.clone());
        let metadata_search_series_storage =
            storage::<metadata_search::SearchSeriesJob>(conn.clone());
        let metadata_search_issue_storage =
            storage::<metadata_search::SearchIssueJob>(conn.clone());
        let metadata_apply_series_storage = storage::<metadata_apply::ApplySeriesJob>(conn.clone());
        let metadata_apply_issue_storage = storage::<metadata_apply::ApplyIssueJob>(conn.clone());
        let rewrite_issue_sidecars_storage =
            storage::<rewrite_sidecars::RewriteIssueSidecarsJob>(conn.clone());
        let archive_edit_storage = storage::<archive_edit::ArchiveEditJob>(conn.clone());
        let backfill_storage = storage::<backfill::BackfillJob>(conn.clone());
        Ok(Self {
            db,
            scan_storage,
            scan_series_storage,
            post_scan_thumbs_storage,
            post_scan_search_storage,
            post_scan_dictionary_storage,
            metadata_search_series_storage,
            metadata_search_issue_storage,
            metadata_apply_series_storage,
            metadata_apply_issue_storage,
            rewrite_issue_sidecars_storage,
            archive_edit_storage,
            backfill_storage,
            redis: conn,
        })
    }

    /// Coalesce a full-library scan request (spec §3.2).
    ///
    /// Returns the scan run id that the caller should report. Behavior:
    /// - No scan running for this library → enqueue + return new id
    /// - Scan running, none queued → mark queued, return the in-flight id
    /// - Scan running, already queued → return the in-flight id (no-op)
    pub async fn coalesce_scan(
        &self,
        library_id: Uuid,
        force: bool,
    ) -> anyhow::Result<CoalesceOutcome> {
        self.coalesce_scan_inner(library_id, force, None, None)
            .await
    }

    /// Coalesce a watcher-triggered, directory-scoped scan (WP-3.1).
    ///
    /// Shares the full-library in-flight / queued keys so an event storm
    /// coalesces into one scan: if nothing is running the scoped job is
    /// enqueued; if a scan is running the directories are unioned into the
    /// queued follow-up (which stays a *full* scan when a full trigger also
    /// queued it). Never forces — the scan reuses the size+mtime fast paths.
    /// A scope larger than [`MAX_WATCH_SCOPE_DIRS`] degrades to a non-forced
    /// full scan so the job payload stays bounded.
    pub async fn coalesce_watch_scan(
        &self,
        library_id: Uuid,
        dirs: Vec<String>,
    ) -> anyhow::Result<CoalesceOutcome> {
        let scope = (!dirs.is_empty() && dirs.len() <= MAX_WATCH_SCOPE_DIRS).then_some(dirs);
        self.coalesce_scan_inner(library_id, false, None, scope)
            .await
    }

    /// Like [`Self::coalesce_scan`], but stamps `batch_id` on the
    /// scan_run row when one is newly enqueued (observability-split M6 —
    /// "Scan all" batch grouping). A *coalesced* request (a scan was already
    /// in flight) does not adopt the batch: the in-flight run belongs to
    /// whatever first triggered it.
    pub async fn coalesce_scan_with_batch(
        &self,
        library_id: Uuid,
        force: bool,
        batch_id: Uuid,
    ) -> anyhow::Result<CoalesceOutcome> {
        self.coalesce_scan_inner(library_id, force, Some(batch_id), None)
            .await
    }

    async fn coalesce_scan_inner(
        &self,
        library_id: Uuid,
        force: bool,
        batch_id: Option<Uuid>,
        scope: Option<Vec<String>>,
    ) -> anyhow::Result<CoalesceOutcome> {
        let mut conn = self.redis.clone();
        let in_flight_key = in_flight_key(library_id);
        let queued_key = queued_key(library_id);
        let scan_id_key = scan_id_key(library_id);

        let in_flight: Option<String> = conn.get(&in_flight_key).await?;
        if let Some(_existing) = in_flight {
            // A scan is running. Mark another one queued and return the
            // running id so the caller can advertise a stable scan_id.
            let already_queued: Option<String> = conn.get(&queued_key).await?;
            let scoped_key = queued_scoped_key(library_id);
            let dirs_key = queued_dirs_key(library_id);
            match &scope {
                Some(dirs) => {
                    // Scoped follow-up only if nothing (or only other scoped
                    // triggers) queued it; a full queued follow-up stays full.
                    let scoped_marker: Option<String> = conn.get(&scoped_key).await?;
                    if already_queued.is_none() || scoped_marker.is_some() {
                        let _: () = conn
                            .set_ex(&scoped_key, "1", SCAN_COALESCE_TTL_SECS)
                            .await?;
                        let _: () = conn.sadd(&dirs_key, dirs).await?;
                        let _: () = conn
                            .expire(&dirs_key, SCAN_COALESCE_TTL_SECS as i64)
                            .await?;
                    }
                }
                None => {
                    // Full trigger: the follow-up must cover the whole library.
                    let _: () = conn.del(&scoped_key).await?;
                    let _: () = conn.del(&dirs_key).await?;
                }
            }
            let _: () = conn
                .set_ex(&queued_key, "1", SCAN_COALESCE_TTL_SECS)
                .await?;
            // Persist the queued-force flag so the post-completion re-enqueue
            // honors the strongest request.
            if force {
                let _: () = conn
                    .set_ex(format!("{queued_key}:force"), "1", SCAN_COALESCE_TTL_SECS)
                    .await?;
            }
            let scan_id: Option<String> = conn.get(&scan_id_key).await?;
            let scan_id = scan_id
                .and_then(|s| Uuid::parse_str(&s).ok())
                .unwrap_or_else(Uuid::now_v7);
            return Ok(CoalesceOutcome::Coalesced { scan_id });
        }

        // No scan in flight — claim it (TTL'd; see SCAN_COALESCE_TTL_SECS).
        let scan_id = Uuid::now_v7();
        let _: () = conn
            .set_ex(&scan_id_key, scan_id.to_string(), SCAN_COALESCE_TTL_SECS)
            .await?;
        let _: () = conn
            .set_ex(&in_flight_key, scan_id.to_string(), SCAN_COALESCE_TTL_SECS)
            .await?;
        self.insert_queued_scan_run(library_id, scan_id, "library", None, None, batch_id)
            .await?;
        // Push to apalis last; if push fails the in_flight key is stale, but
        // the next request will see no apalis backlog and overwrite cleanly.
        use apalis::prelude::Storage;
        let mut storage = self.scan_storage.clone();
        storage
            .push(scan::Job {
                library_id,
                scan_run_id: scan_id,
                force,
                scope,
            })
            .await?;
        Ok(CoalesceOutcome::Enqueued { scan_id })
    }

    /// Called by the worker after a scan job finishes. Clears the in-flight
    /// marker; if a queued trigger arrived during the scan, re-enqueues.
    pub async fn release_scan(&self, library_id: Uuid) -> anyhow::Result<()> {
        let mut conn = self.redis.clone();
        let in_flight_key = in_flight_key(library_id);
        let queued_key = queued_key(library_id);
        let queued_force_key = format!("{queued_key}:force");
        let scan_id_key = scan_id_key(library_id);
        let scoped_key = queued_scoped_key(library_id);
        let dirs_key = queued_dirs_key(library_id);

        let _: () = conn.del(&in_flight_key).await?;

        let queued: Option<String> = conn.get(&queued_key).await?;
        if queued.is_some() {
            let force_flag: Option<String> = conn.get(&queued_force_key).await?;
            let scoped_marker: Option<String> = conn.get(&scoped_key).await?;
            let scope = if scoped_marker.is_some() {
                let mut dirs: Vec<String> = conn.smembers(&dirs_key).await?;
                dirs.sort();
                (!dirs.is_empty() && dirs.len() <= MAX_WATCH_SCOPE_DIRS).then_some(dirs)
            } else {
                None
            };
            let _: () = conn.del(&queued_key).await?;
            let _: () = conn.del(&queued_force_key).await?;
            let _: () = conn.del(&scoped_key).await?;
            let _: () = conn.del(&dirs_key).await?;
            let new_id = Uuid::now_v7();
            let _: () = conn
                .set_ex(&scan_id_key, new_id.to_string(), SCAN_COALESCE_TTL_SECS)
                .await?;
            let _: () = conn
                .set_ex(&in_flight_key, new_id.to_string(), SCAN_COALESCE_TTL_SECS)
                .await?;
            self.insert_queued_scan_run(library_id, new_id, "library", None, None, None)
                .await?;
            use apalis::prelude::Storage;
            let mut storage = self.scan_storage.clone();
            storage
                .push(scan::Job {
                    library_id,
                    scan_run_id: new_id,
                    force: force_flag.is_some(),
                    scope,
                })
                .await?;
        }
        Ok(())
    }

    /// Spawn workers for every queue, watching `shutdown` for a clean
    /// drain signal. Returns when `shutdown.cancelled()` resolves AND
    /// the apalis monitor finishes its graceful-shutdown handshake
    /// (drains in-flight jobs up to the apalis-default timeout).
    ///
    /// **Panic safety.** Monitor errors (or future panics) restart the
    /// monitor with an exponential backoff capped at 30s. The outer
    /// cancellation token still breaks the loop on shutdown, so a
    /// SIGTERM mid-restart still terminates cleanly. Replaces the
    /// `.expect("apalis monitor crashed")` from before code-quality-
    /// cleanup M4 which would have killed the worker process on the
    /// first transient redis blip.
    pub async fn run(self, state: AppState, shutdown: tokio_util::sync::CancellationToken) {
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            if shutdown.is_cancelled() {
                tracing::info!("apalis monitor: shutdown observed, exiting run loop");
                return;
            }

            let outcome = self
                .clone()
                .run_monitor_once(state.clone(), &shutdown)
                .await;
            if shutdown.is_cancelled() {
                tracing::info!("apalis monitor: shutdown observed after run; exiting");
                return;
            }
            match outcome {
                Ok(()) => {
                    // Monitor returned Ok without shutdown being signalled —
                    // unusual; treat the same as transient error and restart.
                    tracing::warn!("apalis monitor exited unexpectedly; restarting");
                }
                Err(e) => {
                    tracing::error!(error = %e, "apalis monitor exited with error; restarting");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = shutdown.cancelled() => {
                    tracing::info!("apalis monitor: shutdown observed during backoff; exiting");
                    return;
                }
            }
            // Exponential backoff capped at 30s.
            delay = (delay * 2).min(std::time::Duration::from_secs(30));
        }
    }

    /// Run the apalis monitor once. Separated from [`run`] so the
    /// panic-restart loop can call back in cleanly. `shutdown` is
    /// forwarded as the `run_with_signal` cancellation future.
    async fn run_monitor_once(
        self,
        state: AppState,
        shutdown: &tokio_util::sync::CancellationToken,
    ) -> std::io::Result<()> {
        use apalis::prelude::*;

        let scan_concurrency = state.cfg().scan_worker_count;
        let post_concurrency = state.cfg().post_scan_worker_count;

        let scan_worker = WorkerBuilder::new("scan")
            .concurrency(scan_concurrency)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("scan"))
            .backend(self.scan_storage.clone())
            .build_fn(scan::handle);

        let scan_series_worker = WorkerBuilder::new("scan_series")
            .concurrency(scan_concurrency)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("scan_series"))
            .backend(self.scan_series_storage.clone())
            .build_fn(scan_series::handle);

        let thumbs_worker = WorkerBuilder::new("post_scan_thumbs")
            .concurrency(post_concurrency)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("post_scan_thumbs"))
            .backend(self.post_scan_thumbs_storage.clone())
            .build_fn(post_scan::handle_thumbs);

        let search_worker = WorkerBuilder::new("post_scan_search")
            .concurrency(post_concurrency)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("post_scan_search"))
            .backend(self.post_scan_search_storage.clone())
            .build_fn(post_scan::handle_search);

        let dictionary_worker = WorkerBuilder::new("post_scan_dictionary")
            .concurrency(post_concurrency)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("post_scan_dictionary"))
            .backend(self.post_scan_dictionary_storage.clone())
            .build_fn(post_scan::handle_dictionary);

        // Metadata search workers: concurrency=1 — the per-provider
        // velocity caps + Redis token buckets already throttle the
        // outbound calls; spinning up multiple workers only contends
        // for the same budget.
        let metadata_series_worker = WorkerBuilder::new("metadata_search_series")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new(
                "metadata_search_series",
            ))
            .backend(self.metadata_search_series_storage.clone())
            .build_fn(metadata_search::handle_series);

        let metadata_issue_worker = WorkerBuilder::new("metadata_search_issue")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("metadata_search_issue"))
            .backend(self.metadata_search_issue_storage.clone())
            .build_fn(metadata_search::handle_issue);

        // Apply workers: concurrency=1 per-job-type — apply itself
        // is short (single detail fetch + DB writes), serializing
        // by entity is sufficient via the per-entity mutex inside
        // the handler.
        let metadata_apply_series_worker = WorkerBuilder::new("metadata_apply_series")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("metadata_apply_series"))
            .backend(self.metadata_apply_series_storage.clone())
            .build_fn(metadata_apply::handle_series);

        let metadata_apply_issue_worker = WorkerBuilder::new("metadata_apply_issue")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("metadata_apply_issue"))
            .backend(self.metadata_apply_issue_storage.clone())
            .build_fn(metadata_apply::handle_issue);

        // Sidecar writeback workers — concurrency=2 because the work
        // is mostly zip stream-copy + filesystem rename; per-issue
        // serialization is enforced by the rewrite mutex inside the
        // handler, so two workers won't race on the same archive.
        let rewrite_issue_sidecars_worker = WorkerBuilder::new("rewrite_issue_sidecars")
            .concurrency(2)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new(
                "rewrite_issue_sidecars",
            ))
            .backend(self.rewrite_issue_sidecars_storage.clone())
            .build_fn(rewrite_sidecars::handle);

        // Page-byte edits — concurrency=1. Re-encoding pages on a large
        // archive can be CPU-heavy; serializing keeps host load bounded
        // under a bulk fan-out (per-issue serialization is the mutex's
        // job, but we also cap total in-flight edits here).
        let archive_edit_worker = WorkerBuilder::new("archive_edit")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("archive_edit"))
            .backend(self.archive_edit_storage.clone())
            .build_fn(archive_edit::handle);

        // Backfill drains — concurrency=1. Each job loops the whole backlog;
        // serializing keeps one operator-triggered sweep at a time (and a
        // redundant re-enqueue just queues behind it and finds nothing left).
        let backfill_worker = WorkerBuilder::new("backfill")
            .concurrency(1)
            .data(state.clone())
            .layer(metrics_layer::JobMetricsLayer::new("backfill"))
            .backend(self.backfill_storage.clone())
            .build_fn(backfill::handle);

        let shutdown_fut = {
            let token = shutdown.clone();
            async move {
                token.cancelled().await;
                Ok(())
            }
        };
        Monitor::new()
            .register(scan_worker)
            .register(scan_series_worker)
            .register(thumbs_worker)
            .register(search_worker)
            .register(dictionary_worker)
            .register(metadata_series_worker)
            .register(metadata_issue_worker)
            .register(metadata_apply_series_worker)
            .register(metadata_apply_issue_worker)
            .register(rewrite_issue_sidecars_worker)
            .register(archive_edit_worker)
            .register(backfill_worker)
            // Bound the graceful drain (OPS-3, JOBS-3): without this the monitor
            // waits indefinitely for an in-flight job, so a SIGTERM during a
            // 30-minute scan wouldn't return until the scan finished — past the
            // container's grace period, which then SIGKILLs. Cap the wait so the
            // process exits cleanly; an abandoned job is re-enqueued by apalis's
            // orphan recovery ([`JOB_REENQUEUE_ORPHANED_AFTER_SECS`]) on the next boot.
            .shutdown_timeout(std::time::Duration::from_secs(JOB_SHUTDOWN_TIMEOUT_SECS))
            .run_with_signal(shutdown_fut)
            .await
    }

    /// Coalesce a series- or issue-scoped scan request. Unlike full-library
    /// scans, repeated scoped clicks while the same target is queued/running
    /// join the existing run instead of scheduling a follow-up pass.
    pub async fn coalesce_scoped_scan(
        &self,
        library_id: Uuid,
        series_id: Uuid,
        folder_path: Option<String>,
        kind: scan_series::JobKind,
        issue_id: Option<String>,
        force: bool,
    ) -> anyhow::Result<CoalesceOutcome> {
        let mut conn = self.redis.clone();
        let key = scoped_in_flight_key(library_id, series_id, kind, issue_id.as_deref());
        let scan_id_key = scoped_scan_id_key(library_id, series_id, kind, issue_id.as_deref());

        let existing: Option<String> = conn.get(&key).await?;
        if existing.is_some() {
            let scan_id: Option<String> = conn.get(&scan_id_key).await?;
            let scan_id = scan_id
                .and_then(|s| Uuid::parse_str(&s).ok())
                .unwrap_or_else(Uuid::now_v7);
            return Ok(CoalesceOutcome::Coalesced { scan_id });
        }

        let scan_id = Uuid::now_v7();
        let kind_str = match kind {
            scan_series::JobKind::Series => "series",
            scan_series::JobKind::Issue => "issue",
        };
        let issue_for_run = issue_id
            .clone()
            .filter(|_| matches!(kind, scan_series::JobKind::Issue));
        self.insert_queued_scan_run(
            library_id,
            scan_id,
            kind_str,
            Some(series_id),
            issue_for_run.clone(),
            None,
        )
        .await?;
        let _: () = conn
            .set_ex(&key, scan_id.to_string(), SCAN_COALESCE_TTL_SECS)
            .await?;
        let _: () = conn
            .set_ex(&scan_id_key, scan_id.to_string(), SCAN_COALESCE_TTL_SECS)
            .await?;

        use apalis::prelude::Storage;
        let mut storage = self.scan_series_storage.clone();
        storage
            .push(scan_series::Job {
                library_id,
                series_id: Some(series_id),
                folder_path,
                kind: Some(kind),
                issue_id,
                force,
                scan_run_id: Some(scan_id),
            })
            .await?;
        Ok(CoalesceOutcome::Enqueued { scan_id })
    }

    pub async fn release_scoped_scan(
        &self,
        library_id: Uuid,
        series_id: Uuid,
        kind: scan_series::JobKind,
        issue_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut conn = self.redis.clone();
        let key = scoped_in_flight_key(library_id, series_id, kind, issue_id);
        let scan_id_key = scoped_scan_id_key(library_id, series_id, kind, issue_id);
        let _: () = conn.del(&key).await?;
        let _: () = conn.del(&scan_id_key).await?;
        Ok(())
    }

    async fn insert_queued_scan_run(
        &self,
        library_id: Uuid,
        scan_id: Uuid,
        kind: &str,
        series_id: Option<Uuid>,
        issue_id: Option<String>,
        batch_id: Option<Uuid>,
    ) -> anyhow::Result<()> {
        if ScanRunEntity::find_by_id(scan_id)
            .one(&self.db)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let now = chrono::Utc::now().fixed_offset();
        ScanRunAM {
            id: Set(scan_id),
            library_id: Set(library_id),
            state: Set("queued".to_owned()),
            started_at: Set(now),
            ended_at: Set(None),
            stats: Set(serde_json::to_value(
                crate::library::scanner::ScanStats::default(),
            )?),
            error: Set(None),
            kind: Set(kind.to_owned()),
            series_id: Set(series_id),
            issue_id: Set(issue_id),
            batch_id: Set(batch_id),
        }
        .insert(&self.db)
        .await?;
        Ok(())
    }
}

/// Outcome of [`JobRuntime::coalesce_scan`].
#[derive(Debug, Clone, Copy)]
pub enum CoalesceOutcome {
    /// New job pushed to the queue.
    Enqueued { scan_id: Uuid },
    /// Existing in-flight scan returned; queued flag set if needed.
    Coalesced { scan_id: Uuid },
}

impl CoalesceOutcome {
    pub fn scan_id(self) -> Uuid {
        match self {
            Self::Enqueued { scan_id } | Self::Coalesced { scan_id } => scan_id,
        }
    }
    pub fn was_coalesced(self) -> bool {
        matches!(self, Self::Coalesced { .. })
    }
}

/// TTL on the scan-coalescing Redis keys (OPS-2). Without an expiry, a process
/// crash between claiming `in_flight` and pushing the apalis job — or a
/// retry-exhausted scan that never calls `release_scan` — leaves the key set
/// forever, so every later trigger coalesces into a phantom run and the
/// library's scans wedge permanently. The TTL is a self-healing backstop for a
/// box that's never restarted; the boot sweep ([`JobRuntime::clear_stale_scan_keys`])
/// clears them immediately on the common recovery path. Generous (6h) so it
/// never expires under a legitimately long scan — and if it somehow did, the
/// worst case is a redundant, idempotent re-scan, not data loss.
const SCAN_COALESCE_TTL_SECS: u64 = 6 * 60 * 60;

/// Attempts apalis-redis makes on a job whose handler returns `Err` before
/// moving it to the queue's `{namespace}:dead` set (surfaced by
/// [`JobRuntime::dead_letter_counts`]). Handlers that return `Ok` on a
/// recorded failure (the scan handlers, most soft-fails) never consume this
/// budget — see `jobs::scan::handle`.
///
/// apalis-redis 0.7 stores the budget in the per-job [`apalis_redis::RedisContext`]
/// stamped at push time (`max_attempts`, default 5) and exposes no setter —
/// it is *not* part of [`apalis_redis::Config`]. This constant mirrors that
/// default so call sites and comments have one name to point at; the
/// `job_retry_budget_matches_apalis_default` test fails the build if an
/// apalis bump changes the library value, at which point the choice is
/// deliberate: either update this constant or stamp the context by hand.
pub const JOB_MAX_ATTEMPTS: usize = 5;

/// Seconds a job may sit in a worker's in-flight set without a heartbeat
/// before apalis's orphan sweep re-enqueues it. Covers a process that died
/// mid-job (SIGKILL past the [`JOB_SHUTDOWN_TIMEOUT_SECS`] drain, OOM, host
/// crash): the sweep — run at worker start and then on an interval — sees
/// the stale lock and hands the job to a live worker. Must comfortably
/// exceed the worker `keep_alive` heartbeat (30s default) so a
/// slow-but-alive worker is never mistaken for a dead one.
pub const JOB_REENQUEUE_ORPHANED_AFTER_SECS: u64 = 300;

/// The [`RedisConfig`] every queue is built with. Namespace is the job's
/// `type_name` — identical to what `RedisStorage::new` derives, so the Redis
/// keys (`{namespace}:active`, `:dead`, …) of jobs queued by an older build
/// are still the keys the new build polls. Changing the namespace would
/// silently orphan every queued job across an upgrade.
pub fn storage_config<T>() -> RedisConfig {
    RedisConfig::default()
        .set_namespace(std::any::type_name::<T>())
        .set_reenqueue_orphaned_after(std::time::Duration::from_secs(
            JOB_REENQUEUE_ORPHANED_AFTER_SECS,
        ))
}

/// Build one typed queue on a shared connection with [`storage_config`].
fn storage<T>(conn: ConnectionManager) -> RedisStorage<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    RedisStorage::new_with_config(conn, storage_config::<T>())
}

/// Max seconds the apalis monitor waits for in-flight jobs to drain on shutdown
/// before exiting anyway (OPS-3). Kept under a typical container stop grace
/// period (~30s) so the process exits on its own rather than being SIGKILLed
/// mid-drain; abandoned jobs are recovered by apalis's orphan re-enqueue.
const JOB_SHUTDOWN_TIMEOUT_SECS: u64 = 25;

fn in_flight_key(library_id: Uuid) -> String {
    format!("scan:in_flight:{library_id}")
}
fn queued_key(library_id: Uuid) -> String {
    format!("scan:queued:{library_id}")
}
fn scan_id_key(library_id: Uuid) -> String {
    format!("scan:scan_id:{library_id}")
}
fn queued_scoped_key(library_id: Uuid) -> String {
    format!("scan:queued:{library_id}:scoped")
}
fn queued_dirs_key(library_id: Uuid) -> String {
    format!("scan:queued:{library_id}:dirs")
}

/// Largest directory scope a watcher-triggered scan carries (WP-3.1). A
/// bigger change set (a whole-library reshuffle) is cheaper and simpler as a
/// non-forced full scan, which still skips unchanged folders by mtime.
pub const MAX_WATCH_SCOPE_DIRS: usize = 1000;
fn scoped_in_flight_key(
    library_id: Uuid,
    series_id: Uuid,
    kind: scan_series::JobKind,
    issue_id: Option<&str>,
) -> String {
    match kind {
        scan_series::JobKind::Series => format!("scan:in_flight:{library_id}:series:{series_id}"),
        scan_series::JobKind::Issue => format!(
            "scan:in_flight:{library_id}:issue:{series_id}:{}",
            issue_id.unwrap_or("")
        ),
    }
}
fn scoped_scan_id_key(
    library_id: Uuid,
    series_id: Uuid,
    kind: scan_series::JobKind,
    issue_id: Option<&str>,
) -> String {
    match kind {
        scan_series::JobKind::Series => format!("scan:scan_id:{library_id}:series:{series_id}"),
        scan_series::JobKind::Issue => format!(
            "scan:scan_id:{library_id}:issue:{series_id}:{}",
            issue_id.unwrap_or("")
        ),
    }
}

impl JobRuntime {
    /// Best-effort delete of every Redis key referencing `library_id`.
    /// Called on library deletion so a stale "in-flight" marker can't keep
    /// future scans from coalescing correctly. Errors are returned but
    /// callers typically log + continue, since the rest of the deletion
    /// (DB rows + on-disk files) is the source of truth.
    pub async fn purge_scan_keys(&self, library_id: Uuid) -> anyhow::Result<()> {
        let mut conn = self.redis.clone();
        let in_flight_key = in_flight_key(library_id);
        let queued_key = queued_key(library_id);
        let queued_force_key = format!("{queued_key}:force");
        let scan_id_key = scan_id_key(library_id);
        let _: () = conn.del(&in_flight_key).await?;
        let _: () = conn.del(&queued_key).await?;
        let _: () = conn.del(&queued_force_key).await?;
        let _: () = conn.del(queued_scoped_key(library_id)).await?;
        let _: () = conn.del(queued_dirs_key(library_id)).await?;
        let _: () = conn.del(&scan_id_key).await?;
        Ok(())
    }

    /// Boot-time sweep of every scan-coalescing key left over from a previous
    /// process (OPS-2). A crash between claiming `in_flight` and pushing the
    /// apalis job leaves the key set with no job that will ever clear it,
    /// wedging the library's scans permanently; on restart this removes the
    /// orphans so coalescing starts clean. Safe because nothing is actually
    /// scanning at boot — any in-flight scan died with the previous process,
    /// and apalis re-enqueues its orphaned job independently. Matches only the
    /// three folio coalescing prefixes (covers library + scoped + force keys),
    /// never apalis's own `scan` queue keys. Returns the number removed.
    pub async fn clear_stale_scan_keys(&self) -> anyhow::Result<usize> {
        let mut conn = self.redis.clone();
        let mut total = 0usize;
        for pattern in ["scan:in_flight:*", "scan:scan_id:*", "scan:queued:*"] {
            total += delete_matching_scan_keys(&mut conn, pattern).await?;
        }
        Ok(total)
    }

    /// Count dead-lettered jobs per queue (OPS-3 follow-up). apalis moves a job
    /// to its `{namespace}:dead` set after it exhausts [`JOB_MAX_ATTEMPTS`]; nothing
    /// surfaced these before, so a permanently-failing job vanished silently.
    ///
    /// The dead set is a Redis ZSET, so each is counted with `ZCARD`. The key is
    /// resolved from the storage's own `Config::dead_jobs_set()` rather than
    /// hardcoded, so it stays correct across apalis versions (the dead key is
    /// `{type_name}:dead` and the ZSET shape are unchanged from 0.7 through the
    /// 1.0 release candidates). Returns `(queue_label, count)` for every queue.
    pub async fn dead_letter_counts(&self) -> redis::RedisResult<Vec<(&'static str, i64)>> {
        let keys: [(&'static str, String); 12] = [
            ("scan", self.scan_storage.get_config().dead_jobs_set()),
            (
                "scan_series",
                self.scan_series_storage.get_config().dead_jobs_set(),
            ),
            (
                "post_scan_thumbs",
                self.post_scan_thumbs_storage.get_config().dead_jobs_set(),
            ),
            (
                "post_scan_search",
                self.post_scan_search_storage.get_config().dead_jobs_set(),
            ),
            (
                "post_scan_dictionary",
                self.post_scan_dictionary_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "metadata_search_series",
                self.metadata_search_series_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "metadata_search_issue",
                self.metadata_search_issue_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "metadata_apply_series",
                self.metadata_apply_series_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "metadata_apply_issue",
                self.metadata_apply_issue_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "rewrite_issue_sidecars",
                self.rewrite_issue_sidecars_storage
                    .get_config()
                    .dead_jobs_set(),
            ),
            (
                "archive_edit",
                self.archive_edit_storage.get_config().dead_jobs_set(),
            ),
            (
                "backfill",
                self.backfill_storage.get_config().dead_jobs_set(),
            ),
        ];
        let mut conn = self.redis.clone();
        let mut out = Vec::with_capacity(keys.len());
        for (label, key) in keys {
            let count: i64 = conn.zcard(&key).await?;
            out.push((label, count));
        }
        Ok(out)
    }
}

/// SCAN + DEL every key matching `pattern`. Cursor-based (never `KEYS`) so it
/// stays non-blocking on a large keyspace; deletes in 500-key batches.
async fn delete_matching_scan_keys(
    conn: &mut ConnectionManager,
    pattern: &str,
) -> redis::RedisResult<usize> {
    let mut cursor = 0_u64;
    let mut keys = Vec::<String>::new();
    loop {
        let (next, mut batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(1000)
            .query_async(&mut *conn)
            .await?;
        keys.append(&mut batch);
        cursor = next;
        if cursor == 0 {
            break;
        }
    }
    let mut deleted = 0usize;
    for chunk in keys.chunks(500) {
        let n: usize = redis::cmd("DEL").arg(chunk).query_async(&mut *conn).await?;
        deleted += n;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The namespace must stay exactly what `RedisStorage::new` derives
    /// (`type_name::<T>()`): it is the Redis key prefix for every queue, so a
    /// drift here would orphan jobs queued by the previous build on upgrade.
    #[test]
    fn storage_config_keeps_type_name_namespace() {
        let cfg = storage_config::<scan::Job>();
        assert_eq!(
            cfg.get_namespace(),
            std::any::type_name::<scan::Job>(),
            "namespace must match apalis-redis's RedisStorage::new derivation"
        );
        assert!(
            cfg.get_namespace().ends_with("jobs::scan::Job"),
            "type_name shape changed: {}",
            cfg.get_namespace()
        );
    }

    #[test]
    fn storage_config_pins_reenqueue_orphaned_after() {
        let cfg = storage_config::<post_scan::ThumbsJob>();
        assert_eq!(
            cfg.reenqueue_orphaned_after(),
            std::time::Duration::from_secs(JOB_REENQUEUE_ORPHANED_AFTER_SECS)
        );
        // Must exceed the keep-alive heartbeat or a live-but-slow worker's
        // job gets re-enqueued underneath it.
        assert!(cfg.reenqueue_orphaned_after() > *cfg.get_keep_alive());
    }

    /// `JOB_MAX_ATTEMPTS` mirrors the private `RedisContext::max_attempts`
    /// default apalis-redis stamps on every pushed job (no public setter in
    /// 0.7). Read it back through serde so a dependency bump that changes
    /// the library default fails here instead of silently changing the
    /// retry budget in production.
    #[test]
    fn job_retry_budget_matches_apalis_default() {
        let ctx = serde_json::to_value(apalis_redis::RedisContext::default())
            .expect("RedisContext serializes");
        let max_attempts = ctx
            .get("max_attempts")
            .and_then(serde_json::Value::as_u64)
            .expect("RedisContext carries max_attempts");
        assert_eq!(
            usize::try_from(max_attempts).expect("fits usize"),
            JOB_MAX_ATTEMPTS,
            "apalis-redis changed its default attempt budget; update JOB_MAX_ATTEMPTS deliberately"
        );
    }
}
