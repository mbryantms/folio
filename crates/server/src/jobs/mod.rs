//! Background job runtime — apalis 1.0 over Redis.
//!
//! Library Scanner v1, Milestone 2 (spec §3, §3.2, §4.8).
//!
//! **Retry / orphan policy** (WP-1.6 ops hygiene) is explicit, not
//! inherited from library defaults: every queue is built by [`storage`]
//! from [`storage_config`], which pins the orphan window to
//! [`JOB_REENQUEUE_ORPHANED_AFTER_SECS`] (heartbeat × missed heartbeats)
//! and names the queue after the job type, exactly as apalis-redis 0.7
//! derived its namespace, so every Redis key keeps its 0.7 name. The
//! per-job attempt budget is [`JOB_MAX_ATTEMPTS`], enforced by
//! [`with_retry_budget`] around every handler; see its doc for why the
//! backend cannot do it for us.
//!
//! **Upgrading from apalis-redis 0.7** (folio ≤ 0.61): the key *names*
//! survive but the stored payload shape does not (0.7 wrote the whole
//! `Request` as JSON; 1.0 writes the bare args plus a `{queue}:meta:{id}`
//! hash). [`JobRuntime::purge_legacy_queue_state`] runs at boot and drops
//! any queue that still carries a 0.7 `:consumers` set, with a warning —
//! every job type is re-derivable (scans re-trigger, thumbnail marks
//! re-seed, metadata runs re-queue), so nothing is lost but the backlog.
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
//! `scan:in_flight` is a lease held by one scan run id: claimed with
//! `SET NX`, kept alive by the running scan, released or handed to the
//! queued follow-up only by its holder. See [`scan_lease`].
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
//!
//! **Pickup latency.** apalis 1.0 does not poll Redis on its own: after a
//! fetch comes back empty the worker parks its waker, and the only built-in
//! wake-up is the heartbeat timer ([`JOB_HEARTBEAT_SECS`], 30 s). apalis-redis
//! 0.7 polled every 100 ms, so the 1.0 upgrade (#1254) silently turned every
//! queue hop into a 0–30 s wait — a metadata apply (apply → sidecar rewrite →
//! scoped rescan → thumbnails) took a minute of mostly idle time. Every worker
//! is therefore built with `poll_with_interval(`[`JOB_POLL_INTERVAL`]`)`, which
//! wakes it on a short timer independent of the heartbeat. The interval is a
//! trade between pickup latency and idle Redis traffic (one `fetch_next` per
//! worker per tick); 500 ms × ~17 workers is a few dozen cheap commands a
//! second. `job_worker_loop.rs` pins the latency.

use crate::state::AppState;
use apalis::prelude::{AbortError, Attempt, BoxDynError, Data};
use apalis_redis::{Config as RedisConfig, RedisStorage, connect};
use entity::scan_run::{
    ActiveModel as ScanRunAM, Entity as ScanRunEntity, ScanRunKind, ScanRunState,
};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

pub mod activity;
pub mod archive_edit;
pub mod archive_transforms;
pub mod audit_log_retention;
pub mod backfill;
pub mod backup_prune;
pub mod busy_requeue;
pub mod close_dangling_sessions;
pub mod entity_enrich;
pub mod hard_purge;
pub mod hash_backfill;
pub mod metadata_apply;
pub mod metadata_resume;
pub mod metadata_search;
pub mod metrics_layer;
pub mod orphan_sweep;
pub mod post_scan;
pub mod provider_coverage;
pub mod prune_auth_sessions;
pub mod relationship_suggest;
pub mod rewrite_sidecars;
pub mod scan;
pub mod scan_lease;
pub mod scan_series;
pub mod scheduler;
pub mod sidecar_reemit;

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
    pub hash_backfill_storage: RedisStorage<hash_backfill::HashBackfillJob>,
    pub relationship_suggest_storage: RedisStorage<relationship_suggest::RelationshipSuggestJob>,
    pub provider_coverage_storage: RedisStorage<provider_coverage::ProviderCoverageJob>,
    pub entity_enrich_storage: RedisStorage<entity_enrich::EntityEnrichJob>,
    pub sidecar_reemit_storage: RedisStorage<sidecar_reemit::SidecarReemitJob>,
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
        let hash_backfill_storage = storage::<hash_backfill::HashBackfillJob>(conn.clone());
        let relationship_suggest_storage =
            storage::<relationship_suggest::RelationshipSuggestJob>(conn.clone());
        let provider_coverage_storage =
            storage::<provider_coverage::ProviderCoverageJob>(conn.clone());
        let entity_enrich_storage = storage::<entity_enrich::EntityEnrichJob>(conn.clone());
        let sidecar_reemit_storage = storage::<sidecar_reemit::SidecarReemitJob>(conn.clone());
        let rt = Self {
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
            hash_backfill_storage,
            relationship_suggest_storage,
            provider_coverage_storage,
            entity_enrich_storage,
            sidecar_reemit_storage,
            redis: conn,
        };
        rt.purge_legacy_queue_state().await?;
        Ok(rt)
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
        // One script claims the key (`SET NX PX`) or, when another scan holds
        // it, records the queued follow-up; see `scan_lease`.
        let token = Uuid::now_v7();
        let (claimed, current) = self
            .claim_or_queue(library_id, token, force, scope.as_deref(), true)
            .await?;
        if !claimed {
            // A scan is queued or running: the trigger was folded into its
            // follow-up. Report the current id so the caller can advertise a
            // stable scan_id.
            let scan_id = Uuid::parse_str(&current).unwrap_or_else(|_| Uuid::now_v7());
            return Ok(CoalesceOutcome::Coalesced { scan_id });
        }

        let scan_id = token;
        if let Err(e) = self
            .queue_claimed_scan(library_id, scan_id, force, batch_id, scope)
            .await
        {
            // Nothing will run under this claim: give the key back so the
            // next trigger can claim it.
            self.undo_claim(library_id, scan_id).await;
            return Err(e);
        }
        Ok(CoalesceOutcome::Enqueued { scan_id })
    }

    /// Run [`scan_lease::CLAIM_OR_QUEUE_LUA`]. With `may_claim` false the
    /// request is only recorded as the queued follow-up. Returns whether
    /// the key was claimed for `token`, and the current scan id.
    async fn claim_or_queue(
        &self,
        library_id: Uuid,
        token: Uuid,
        force: bool,
        scope: Option<&[String]>,
        may_claim: bool,
    ) -> anyhow::Result<(bool, String)> {
        let mut conn = self.redis.clone();
        let script = redis::Script::new(scan_lease::CLAIM_OR_QUEUE_LUA);
        let mut invocation = script.prepare_invoke();
        for key in coalesce_keys(library_id) {
            invocation.key(key);
        }
        invocation
            .arg(token.to_string())
            .arg(scan_lease::ttl_ms(scan_lease::SCAN_QUEUED_TTL))
            .arg(if force { "1" } else { "0" })
            .arg(if scope.is_some() { "1" } else { "0" })
            .arg(if may_claim { "claim" } else { "queue" });
        if let Some(dirs) = scope {
            for d in dirs {
                invocation.arg(d);
            }
        }
        let (claimed, current): (String, String) = invocation.invoke_async(&mut conn).await?;
        Ok((claimed == "1", current))
    }

    /// Hand on the request of a scan run that was superseded (another run
    /// holds the library's key): it becomes that holder's queued follow-up,
    /// with force OR-ed and a full request widening a scoped one, or a new
    /// run when the key has become free. The superseded run itself is
    /// already closed; its id is never reused, so this happens once per run.
    pub async fn requeue_superseded_request(
        &self,
        library_id: Uuid,
        force: bool,
        scope: Option<Vec<String>>,
    ) -> anyhow::Result<CoalesceOutcome> {
        self.coalesce_scan_inner(library_id, force, None, scope)
            .await
    }

    /// Insert the `queued` run row and push the scan job for a claim.
    async fn queue_claimed_scan(
        &self,
        library_id: Uuid,
        scan_id: Uuid,
        force: bool,
        batch_id: Option<Uuid>,
        scope: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        self.insert_queued_scan_run(
            library_id,
            scan_id,
            ScanRunKind::Library,
            None,
            None,
            batch_id,
        )
        .await?;
        use apalis::prelude::TaskSink;
        let mut storage = self.scan_storage.clone();
        storage
            .push(scan::Job {
                library_id,
                scan_run_id: scan_id,
                force,
                scope,
            })
            .await?;
        Ok(())
    }

    /// Compare-and-delete the key for a claim whose job was never queued.
    async fn undo_claim(&self, library_id: Uuid, token: Uuid) {
        let mut conn = self.redis.clone();
        let res: redis::RedisResult<i64> = redis::Script::new(scan_lease::COMPARE_AND_DELETE_LUA)
            .key(in_flight_key(library_id))
            .arg(token.to_string())
            .invoke_async(&mut conn)
            .await;
        if let Err(e) = res {
            tracing::warn!(
                %library_id,
                scan_run_id = %token,
                error = %e,
                "scan coalescing: could not undo a claim; the key expires on its own",
            );
        }
    }

    /// The scan run id holding `library_id`'s coalescing key, if any.
    pub async fn scan_in_flight(&self, library_id: Uuid) -> anyhow::Result<Option<Uuid>> {
        let mut conn = self.redis.clone();
        let v: Option<String> = conn.get(in_flight_key(library_id)).await?;
        Ok(v.and_then(|s| Uuid::parse_str(&s).ok()))
    }

    /// Called by the worker after the scan run `scan_run_id` finishes.
    /// Releases the coalescing key only if that run still holds it
    /// (compare-and-delete); if a trigger was queued during the scan, the key
    /// is handed to a new run id in the same script and that follow-up is
    /// enqueued. A run that no longer holds the key changes nothing: the
    /// queued flags belong to the current holder.
    pub async fn release_scan(
        &self,
        library_id: Uuid,
        scan_run_id: Uuid,
    ) -> anyhow::Result<ReleaseOutcome> {
        let mut conn = self.redis.clone();
        let new_id = Uuid::now_v7();
        let script = redis::Script::new(scan_lease::RELEASE_LUA);
        let mut invocation = script.prepare_invoke();
        for key in coalesce_keys(library_id) {
            invocation.key(key);
        }
        invocation
            .arg(scan_run_id.to_string())
            .arg(new_id.to_string())
            .arg(scan_lease::ttl_ms(scan_lease::SCAN_QUEUED_TTL));
        let out: Vec<String> = invocation.invoke_async(&mut conn).await?;
        match out.first().map(String::as_str) {
            Some("0") => Ok(ReleaseOutcome::Released),
            Some("1") => {
                let force = out.get(1).is_some_and(|f| f == "1");
                let scoped = out.get(2).is_some_and(|f| f == "1");
                let scope = if scoped {
                    let mut dirs: Vec<String> = out.iter().skip(3).cloned().collect();
                    dirs.sort();
                    (!dirs.is_empty() && dirs.len() <= MAX_WATCH_SCOPE_DIRS).then_some(dirs)
                } else {
                    None
                };
                if let Err(e) = self
                    .queue_claimed_scan(library_id, new_id, force, None, scope.clone())
                    .await
                {
                    // Give the key back, and keep the follow-up recorded so
                    // the next holder's release runs it.
                    self.undo_claim(library_id, new_id).await;
                    if let Err(re) = self
                        .claim_or_queue(library_id, new_id, force, scope.as_deref(), false)
                        .await
                    {
                        tracing::error!(%library_id, error = %re, "release_scan: could not re-record the follow-up");
                    }
                    tracing::error!(
                        %library_id,
                        error = %e,
                        "release_scan: follow-up could not be queued; kept for the next scan",
                    );
                    return Err(e);
                }
                Ok(ReleaseOutcome::HandedOver { scan_id: new_id })
            }
            _ => {
                tracing::info!(
                    %library_id,
                    %scan_run_id,
                    "release_scan: this run no longer holds the coalescing key; left it to the holder",
                );
                Ok(ReleaseOutcome::NotHolder)
            }
        }
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

            let started = std::time::Instant::now();
            let outcome = self
                .clone()
                .run_monitor_once(state.clone(), &shutdown)
                .await;
            // A monitor that ran healthily for a while earns a fresh backoff;
            // only back-to-back failures should climb toward the 30s cap.
            if started.elapsed() >= std::time::Duration::from_secs(60) {
                delay = std::time::Duration::from_secs(1);
            }
            if shutdown.is_cancelled() {
                // The monitor has drained its in-flight jobs; write the
                // cover stamps they left buffered so a restart does not
                // redo those jobs (post_scan::ThumbStampBuffer).
                post_scan::flush_done_stamps(&state).await;
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
                    post_scan::flush_done_stamps(&state).await;
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

        // Free our worker names before registering them. apalis-redis refuses
        // to register a worker whose name was seen within the orphan window,
        // so a restart inside that window used to leave those workers dead
        // (scans stuck "queued") until the next restart. Best effort: on a
        // Redis error the monitor still starts, and apalis's own orphan
        // recovery takes over once the window passes.
        match self.release_stale_workers().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                requeued = n,
                "apalis: released stale worker registrations; re-enqueued their in-flight jobs"
            ),
            Err(e) => {
                tracing::warn!(error = %e, "apalis: could not release stale worker registrations")
            }
        }

        // A worker whose future fails (a Redis outage, a registration
        // refusal) is not restarted in place: apalis would retry it in a hot
        // loop with no delay. Instead the failure stops this monitor
        // gracefully and [`Self::run`] starts a fresh one after its backoff.
        let worker_failed = shutdown.child_token();

        // apalis 1.0 registers a worker *factory*: the monitor rebuilds the
        // worker on every (re)start, so each registration captures its
        // storage + state and builds the chain inside the closure. Builder
        // order matters — `backend` must come first (the layer helpers are
        // only available once the backend type is known); the middleware
        // stack is then concurrency (outermost) → Data → metrics, and the
        // handler is wrapped by [`with_retry_budget`].
        //
        // The `attempt:` arm is for handlers that take the task's `Attempt`
        // themselves (to do last-attempt bookkeeping before the budget
        // kills the task); it must come first so the literal token is
        // tried before `$handler:expr` commits to parsing.
        macro_rules! register {
            ($monitor:expr, $name:literal, $storage:expr, $concurrency:expr, attempt: $handler:expr) => {
                register!(@build $monitor, $name, $storage, $concurrency, with_retry_budget_attempt($handler))
            };
            ($monitor:expr, $name:literal, $storage:expr, $concurrency:expr, $handler:expr) => {
                register!(@build $monitor, $name, $storage, $concurrency, with_retry_budget($handler))
            };
            (@build $monitor:expr, $name:literal, $storage:expr, $concurrency:expr, $wrapped:expr) => {{
                let storage = $storage.clone();
                let state = state.clone();
                $monitor.register(move |_runs: usize| {
                    WorkerBuilder::new($name)
                        // Short fetch timer: without a poll strategy an idle
                        // 1.0 worker only wakes on the 30 s heartbeat.
                        .backend(storage.clone().poll_with_interval(JOB_POLL_INTERVAL))
                        .concurrency($concurrency)
                        .data(state.clone())
                        .layer(
                            metrics_layer::JobMetricsLayer::new($name)
                                .with_activity(state.job_activity.clone()),
                        )
                        .build($wrapped)
                })
            }};
        }

        let monitor = Monitor::new().should_restart({
            let worker_failed = worker_failed.clone();
            move |worker, error| {
                tracing::error!(
                    worker = %worker.name(),
                    error = ?error,
                    "apalis worker failed; restarting the job monitor"
                );
                worker_failed.cancel();
                false
            }
        });
        let monitor = register!(
            monitor,
            "scan",
            self.scan_storage,
            scan_concurrency,
            scan::handle
        );
        let monitor = register!(
            monitor,
            "scan_series",
            self.scan_series_storage,
            scan_concurrency,
            scan_series::handle
        );
        let monitor = register!(
            monitor,
            "post_scan_thumbs",
            self.post_scan_thumbs_storage,
            post_concurrency,
            post_scan::handle_thumbs
        );
        let monitor = register!(
            monitor,
            "post_scan_search",
            self.post_scan_search_storage,
            post_concurrency,
            post_scan::handle_search
        );
        let monitor = register!(
            monitor,
            "post_scan_dictionary",
            self.post_scan_dictionary_storage,
            post_concurrency,
            post_scan::handle_dictionary
        );
        // Metadata search workers: concurrency=1 — the per-provider
        // velocity caps + Redis token buckets already throttle the
        // outbound calls; spinning up multiple workers only contends
        // for the same budget.
        let monitor = register!(
            monitor,
            "metadata_search_series",
            self.metadata_search_series_storage,
            1,
            metadata_search::handle_series
        );
        let monitor = register!(
            monitor,
            "metadata_search_issue",
            self.metadata_search_issue_storage,
            1,
            metadata_search::handle_issue
        );
        // Apply workers: concurrency=1 per-job-type — apply itself
        // is short (single detail fetch + DB writes), serializing
        // by entity is sufficient via the per-entity mutex inside
        // the handler.
        let monitor = register!(
            monitor,
            "metadata_apply_series",
            self.metadata_apply_series_storage,
            1,
            metadata_apply::handle_series
        );
        let monitor = register!(
            monitor,
            "metadata_apply_issue",
            self.metadata_apply_issue_storage,
            1,
            metadata_apply::handle_issue
        );
        // Sidecar writeback workers — concurrency=2 because the work
        // is mostly zip stream-copy + filesystem rename; per-issue
        // serialization is enforced by the rewrite mutex inside the
        // handler, so two workers won't race on the same archive. The
        // handler reads the task's `Attempt` so the last budgeted failure
        // marks the triggering run failed before apalis dead-letters it.
        let monitor = register!(
            monitor,
            "rewrite_issue_sidecars",
            self.rewrite_issue_sidecars_storage,
            2,
            attempt: rewrite_sidecars::handle
        );
        // Page-byte edits — concurrency=1. Re-encoding pages on a large
        // archive can be CPU-heavy; serializing keeps host load bounded
        // under a bulk fan-out (per-issue serialization is the mutex's
        // job, but we also cap total in-flight edits here).
        let monitor = register!(
            monitor,
            "archive_edit",
            self.archive_edit_storage,
            1,
            archive_edit::handle
        );
        // Backfill drains — concurrency=1. Each job loops the whole backlog;
        // serializing keeps one operator-triggered sweep at a time (and a
        // redundant re-enqueue just queues behind it and finds nothing left).
        let monitor = register!(
            monitor,
            "backfill",
            self.backfill_storage,
            1,
            backfill::handle
        );
        // First-import lazy-hash drains (WP-3.2) — concurrency=1. Hashing is
        // full-file I/O against the same disks the scanner and reader use
        // (and shares their archive-work semaphore); one library at a time.
        let monitor = register!(
            monitor,
            "hash_backfill",
            self.hash_backfill_storage,
            1,
            hash_backfill::handle
        );
        // Relationship suggestions (WP-7.2) — concurrency=1. A run is a
        // handful of set-based queries per library; serializing keeps two
        // post-scan runs from upserting the same rows concurrently.
        let monitor = register!(
            monitor,
            "relationship_suggest",
            self.relationship_suggest_storage,
            1,
            relationship_suggest::handle
        );
        // Provider coverage analyses — concurrency=1. Each run spends the
        // provider budgets (ComicVine's 200/h, GCD's 100/h); serializing
        // keeps two analyses from racing for the same buckets.
        let monitor = register!(
            monitor,
            "provider_coverage",
            self.provider_coverage_storage,
            1,
            provider_coverage::handle
        );
        // Entity profile fetches — concurrency=1 for the same reason.
        let monitor = register!(
            monitor,
            "entity_enrich",
            self.entity_enrich_storage,
            1,
            entity_enrich::handle
        );
        // Library-wide sidecar re-emit ticks — concurrency=1. A tick only
        // composes and queues rewrites (bounded by the run's own throttle);
        // the archive work happens on the sidecar rewrite workers above.
        let monitor = register!(
            monitor,
            "sidecar_reemit",
            self.sidecar_reemit_storage,
            1,
            sidecar_reemit::handle
        );

        // Process shutdown, or a failed worker (`worker_failed` is a child
        // of `shutdown`, so it fires for both).
        let shutdown_fut = {
            let token = worker_failed.clone();
            async move {
                token.cancelled().await;
                Ok(())
            }
        };
        monitor
            // Bound the graceful drain (OPS-3, JOBS-3): without this the monitor
            // waits indefinitely for an in-flight job, so a SIGTERM during a
            // 30-minute scan wouldn't return until the scan finished — past the
            // container's grace period, which then SIGKILLs. Cap the wait so the
            // process exits cleanly; an abandoned job is re-enqueued by apalis's
            // orphan recovery ([`JOB_REENQUEUE_ORPHANED_AFTER_SECS`]) on the next boot.
            .shutdown_timeout(std::time::Duration::from_secs(JOB_SHUTDOWN_TIMEOUT_SECS))
            .run_with_signal(shutdown_fut)
            .await
            .map_err(std::io::Error::other)
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
        let run_kind = match kind {
            scan_series::JobKind::Series => ScanRunKind::Series,
            scan_series::JobKind::Issue => ScanRunKind::Issue,
        };
        let issue_for_run = issue_id
            .clone()
            .filter(|_| matches!(kind, scan_series::JobKind::Issue));
        self.insert_queued_scan_run(
            library_id,
            scan_id,
            run_kind,
            Some(series_id),
            issue_for_run.clone(),
            None,
        )
        .await?;
        let _: () = conn
            .set_ex(&key, scan_id.to_string(), SCAN_QUEUED_TTL_SECS)
            .await?;
        let _: () = conn
            .set_ex(&scan_id_key, scan_id.to_string(), SCAN_QUEUED_TTL_SECS)
            .await?;

        use apalis::prelude::TaskSink;
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
        kind: ScanRunKind,
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
            state: Set(ScanRunState::Queued),
            started_at: Set(now),
            ended_at: Set(None),
            stats: Set(serde_json::to_value(
                crate::library::scanner::ScanStats::default(),
            )?),
            error: Set(None),
            kind: Set(kind),
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

/// Outcome of [`JobRuntime::release_scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// The run held the key and nothing was queued; the key is gone.
    Released,
    /// The run held the key and a follow-up was queued; the key now holds
    /// the follow-up's run id and its job is enqueued.
    HandedOver { scan_id: Uuid },
    /// Another run holds the key; nothing was changed.
    NotHolder,
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

/// TTL on the series- and issue-scoped scan coalescing keys (OPS-2), the
/// same as the library key's queued lifetime
/// ([`scan_lease::SCAN_QUEUED_TTL`]). Without an expiry, a process crash
/// between the claim and the job push leaves the key set forever; the boot
/// sweep ([`JobRuntime::clear_stale_scan_keys`]) clears them on restart. The
/// full-library key is a lease with a short running lifetime; see
/// [`scan_lease`].
const SCAN_QUEUED_TTL_SECS: u64 = scan_lease::SCAN_QUEUED_TTL.as_secs();

/// Attempts a job whose handler returns `Err` gets before it is moved to the
/// queue's `{queue}:dead` set (surfaced by [`JobRuntime::dead_letter_counts`]).
/// Handlers that return `Ok` on a recorded failure (the scan handlers, most
/// soft-fails) never consume this budget — see `jobs::scan::handle`.
///
/// apalis-redis 1.0 reschedules a `Failed` task until a cut-off hardcoded in
/// its `handle_results.lua` (25 attempts); the per-task `max_attempts` it
/// stores is never consulted. [`with_retry_budget`] therefore wraps every
/// handler: a failure on attempt `JOB_MAX_ATTEMPTS` is reported as an
/// [`AbortError`], which apalis marks `Killed` and the backend dead-letters
/// at once. The `retry_budget_kills_on_last_attempt` test pins the wrapper.
pub const JOB_MAX_ATTEMPTS: usize = 5;

/// apalis-redis's own retry cut-off is a literal in its `handle_results.lua`;
/// the budget here must stay below it or [`with_retry_budget`] never fires
/// first and the backend's 25-attempt loop takes over.
const APALIS_REDIS_HARDCODED_RETRY_CUTOFF: usize = 25;
const _: () = assert!(
    JOB_MAX_ATTEMPTS >= 1 && JOB_MAX_ATTEMPTS < APALIS_REDIS_HARDCODED_RETRY_CUTOFF,
    "JOB_MAX_ATTEMPTS must sit below apalis-redis's hardcoded 25-attempt cut-off"
);

/// Wrap a `(job, Data<AppState>)` handler with the [`JOB_MAX_ATTEMPTS`]
/// budget: an `Err` on the last budgeted attempt becomes an [`AbortError`]
/// (task `Killed` → dead set, no further reschedule); earlier failures pass
/// through unchanged so apalis-redis retries them. `Attempt` is the task's
/// own counter, already incremented for the running attempt (1-based).
pub fn with_retry_budget<Args, D, H, Fut>(
    handler: H,
) -> impl FnMut(Args, Data<D>, Attempt) -> futures::future::BoxFuture<'static, Result<(), BoxDynError>>
+ Clone
+ Send
+ 'static
where
    Args: Send + 'static,
    D: Clone + Send + Sync + 'static,
    H: Fn(Args, Data<D>) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Result<(), BoxDynError>> + Send + 'static,
{
    with_retry_budget_attempt(move |job, state, _attempt: Attempt| handler(job, state))
}

/// [`with_retry_budget`] for a handler that wants the task's own
/// [`Attempt`] — a job that must record "this was the last try" itself
/// (mark a metadata run failed, write the library event) before the
/// wrapper turns the error into an [`AbortError`]. `attempt.current() >=
/// JOB_MAX_ATTEMPTS` inside the handler is the same test the wrapper
/// applies afterwards.
pub fn with_retry_budget_attempt<Args, D, H, Fut>(
    handler: H,
) -> impl FnMut(Args, Data<D>, Attempt) -> futures::future::BoxFuture<'static, Result<(), BoxDynError>>
+ Clone
+ Send
+ 'static
where
    Args: Send + 'static,
    D: Clone + Send + Sync + 'static,
    H: Fn(Args, Data<D>, Attempt) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Result<(), BoxDynError>> + Send + 'static,
{
    move |job, state, attempt| {
        let fut = handler(job, state, attempt.clone());
        Box::pin(async move {
            match fut.await {
                Ok(()) => Ok(()),
                Err(e) if attempt.current() >= JOB_MAX_ATTEMPTS => {
                    Err(Box::new(AbortError::new(e)) as BoxDynError)
                }
                Err(e) => Err(e),
            }
        })
    }
}

/// Seconds a job may sit in a worker's in-flight set without a heartbeat
/// before apalis's orphan sweep re-enqueues it. Covers a process that died
/// mid-job (SIGKILL past the [`JOB_SHUTDOWN_TIMEOUT_SECS`] drain, OOM, host
/// crash): the sweep — run at worker start and then on an interval — sees
/// the stale lock and hands the job to a live worker. Must comfortably
/// exceed the worker heartbeat ([`JOB_HEARTBEAT_SECS`]) so a slow-but-alive
/// worker is never mistaken for a dead one. apalis-redis 1.0 expresses the
/// window as `heartbeat_interval × missed_heartbeats`, so the two constants
/// must divide evenly (checked at compile time).
pub const JOB_REENQUEUE_ORPHANED_AFTER_SECS: u64 = 300;

/// Worker heartbeat period — how often a running worker re-stamps its entry
/// in the queue's `{queue}:workers` set. **Not** the fetch cadence — see
/// [`JOB_POLL_INTERVAL`].
pub const JOB_HEARTBEAT_SECS: u64 = 30;

/// How often an idle worker asks Redis for new tasks. apalis 1.0 workers
/// park after an empty fetch and would otherwise wait for the next heartbeat
/// (30 s) — see the module docs. Keep well under [`JOB_HEARTBEAT_SECS`]; the
/// unit test pins the relationship.
pub const JOB_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

const _: () = assert!(
    JOB_REENQUEUE_ORPHANED_AFTER_SECS.is_multiple_of(JOB_HEARTBEAT_SECS)
        && JOB_REENQUEUE_ORPHANED_AFTER_SECS > JOB_HEARTBEAT_SECS,
    "orphan window must be a whole number (> 1) of heartbeats"
);

/// The [`RedisConfig`] every queue is built with. The queue name is the
/// job's `type_name` — exactly the namespace apalis-redis 0.7 derived in
/// `RedisStorage::new`, so the Redis keys (`{queue}:active`, `:dead`, …) keep
/// the names every dashboard, test and operator runbook already knows.
/// Changing it would silently orphan every queued job across an upgrade.
pub fn storage_config<T>() -> RedisConfig {
    RedisConfig::default()
        .queue(std::any::type_name::<T>())
        .heartbeat_interval(std::time::Duration::from_secs(JOB_HEARTBEAT_SECS))
        .missed_heartbeats((JOB_REENQUEUE_ORPHANED_AFTER_SECS / JOB_HEARTBEAT_SECS) as usize)
}

/// Build one typed queue on a shared connection with [`storage_config`].
fn storage<T>(conn: ConnectionManager) -> RedisStorage<T> {
    RedisStorage::new(conn).with_config(storage_config::<T>())
}

/// [`JobRuntime::release_stale_workers`]: apalis-redis's
/// `reenqueue_orphaned.lua` for one named worker, minus the "has it
/// expired" check. KEYS: workers zset, active list, signal list, in-flight
/// set prefix. ARGV[1]: worker name. Returns the jobs re-enqueued.
const RELEASE_WORKER_LUA: &str = r#"
local inflight = KEYS[4] .. ":" .. ARGV[1]
local jobs = redis.call("smembers", inflight)
local n = #jobs
if n > 0 then
    redis.call("rpush", KEYS[2], unpack(jobs))
    redis.call("del", inflight)
    redis.call("del", KEYS[3])
    redis.call("lpush", KEYS[3], 1)
end
redis.call("zrem", KEYS[1], ARGV[1])
return n
"#;

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
/// The six library coalescing keys in the order the `scan_lease` scripts
/// take them: in_flight, scan_id, queued, queued:force, queued:scoped,
/// queued:dirs.
fn coalesce_keys(library_id: Uuid) -> [String; 6] {
    let queued = queued_key(library_id);
    let queued_force = format!("{queued}:force");
    [
        in_flight_key(library_id),
        scan_id_key(library_id),
        queued,
        queued_force,
        queued_scoped_key(library_id),
        queued_dirs_key(library_id),
    ]
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

    /// Release the apalis worker registrations this process is about to
    /// claim, re-enqueueing whatever jobs they still held.
    ///
    /// Each queue has one worker, named after the queue, and apalis-redis
    /// keys its liveness (`{queue}:workers` zset) and in-flight set
    /// (`{queue}:inflight:{name}`) by that name. Any registration found
    /// before our monitor starts is left over from a previous process (or a
    /// previous monitor in this one) and is dead, but apalis only treats it
    /// as dead once the orphan window passes; until then it refuses to
    /// register a worker with that name. This does what apalis's orphan
    /// recovery would (move the in-flight jobs back onto `{queue}:active`,
    /// wake fetchers, drop the registration) without waiting.
    ///
    /// Assumes one server process per Redis namespace, which the scan
    /// coalescing keys ([`Self::clear_stale_scan_keys`]) already do. Returns
    /// the number of jobs re-enqueued.
    pub async fn release_stale_workers(&self) -> redis::RedisResult<usize> {
        let script = redis::Script::new(RELEASE_WORKER_LUA);
        let mut conn = self.redis.clone();
        let mut total = 0usize;
        for (name, cfg) in self.queue_configs() {
            let n: usize = script
                .key(cfg.workers_set())
                .key(cfg.active_jobs_list())
                .key(cfg.signal_list())
                .key(cfg.inflight_jobs_set())
                .arg(name)
                .invoke_async(&mut conn)
                .await?;
            total += n;
        }
        Ok(total)
    }

    /// Jobs waiting for a worker — what apalis 0.7's `Storage::len()`
    /// reported (apalis 1.0 has no equivalent). apalis-redis 1.0 pushes
    /// every task into `{queue}:scheduled` stamped `run_at = now`, and a
    /// worker's fetch promotes the due ones onto `{queue}:active`, so a job
    /// is "waiting" in either place once its `run_at` has passed; a
    /// retry-delayed job (future `run_at`) is not. Excludes in-flight jobs;
    /// see [`Self::queue_counts`] for the full split.
    pub async fn waiting_len<T>(&self, storage: &RedisStorage<T>) -> redis::RedisResult<i64> {
        let cfg = storage.get_config();
        let mut conn = self.redis.clone();
        let now = unix_now_secs();
        let (active, due): (i64, i64) = redis::pipe()
            .llen(cfg.active_jobs_list())
            .zcount(cfg.scheduled_jobs_set(), "-inf", now)
            .query_async(&mut conn)
            .await?;
        Ok(active + due)
    }

    /// Every unfinished job of one queue: waiting, retry-delayed, and held
    /// by a worker — [`Self::queue_counts`]'s `outstanding()` for a single
    /// queue, without reading the other sixteen.
    pub async fn queue_outstanding<T>(&self, storage: &RedisStorage<T>) -> redis::RedisResult<i64> {
        let cfg = storage.get_config();
        let mut conn = self.redis.clone();
        let (active, scheduled, workers): (i64, i64, Vec<String>) = redis::pipe()
            .llen(cfg.active_jobs_list())
            .zcard(cfg.scheduled_jobs_set())
            .zrange(cfg.workers_set(), 0, -1)
            .query_async(&mut conn)
            .await?;
        let mut held = 0;
        if !workers.is_empty() {
            let mut pipe = redis::pipe();
            for w in &workers {
                pipe.scard(inflight_set_key(cfg, w));
            }
            let sizes: Vec<i64> = pipe.query_async(&mut conn).await?;
            held = sizes.iter().sum();
        }
        Ok(active + scheduled + held)
    }

    /// Boot-time sweep of queue state written by apalis-redis 0.7 (folio ≤
    /// 0.61). The key names are unchanged, but 0.7 stored each job as a
    /// whole `Request` JSON and kept per-worker sets in a `{queue}:consumers`
    /// ZSET; 1.0 stores the bare args plus a `{queue}:meta:{id}` hash and
    /// tracks workers in `{queue}:workers`. A 0.7 payload fetched by a 1.0
    /// worker cannot decode, so any queue that still carries the 0.7-only
    /// `:consumers` key is dropped wholesale, with a warning naming it.
    /// Every job type is re-derivable (scans re-trigger, thumbnail marks
    /// re-seed from the DB, metadata runs re-queue), so the only loss is the
    /// backlog itself. A no-op once every queue is in the 1.0 shape. Returns
    /// the number of keys removed.
    pub async fn purge_legacy_queue_state(&self) -> redis::RedisResult<usize> {
        let mut conn = self.redis.clone();
        let mut total = 0usize;
        for (label, cfg) in self.queue_configs() {
            let legacy_marker = format!("{}:consumers", cfg.queue.as_ref());
            let legacy: bool = conn.exists(&legacy_marker).await?;
            if !legacy {
                continue;
            }
            let pattern = format!("{}:*", cfg.queue.as_ref());
            let removed = delete_matching_scan_keys(&mut conn, &pattern).await?;
            tracing::warn!(
                queue = label,
                keys = removed,
                "dropped apalis-redis 0.7 queue state left by a previous build; \
                 its queued jobs will not run — re-trigger scans or metadata runs as needed"
            );
            total += removed;
        }
        Ok(total)
    }

    /// Every apalis queue, by its admin label, with the storage config that
    /// names its Redis keys. One list so the depth, dead-letter and gauge
    /// readers can never disagree about which queues exist.
    pub(crate) fn queue_configs(&self) -> [(&'static str, &RedisConfig); QUEUE_COUNT] {
        [
            ("scan", self.scan_storage.get_config()),
            ("scan_series", self.scan_series_storage.get_config()),
            (
                "post_scan_thumbs",
                self.post_scan_thumbs_storage.get_config(),
            ),
            (
                "post_scan_search",
                self.post_scan_search_storage.get_config(),
            ),
            (
                "post_scan_dictionary",
                self.post_scan_dictionary_storage.get_config(),
            ),
            (
                "metadata_search_series",
                self.metadata_search_series_storage.get_config(),
            ),
            (
                "metadata_search_issue",
                self.metadata_search_issue_storage.get_config(),
            ),
            (
                "metadata_apply_series",
                self.metadata_apply_series_storage.get_config(),
            ),
            (
                "metadata_apply_issue",
                self.metadata_apply_issue_storage.get_config(),
            ),
            (
                "rewrite_issue_sidecars",
                self.rewrite_issue_sidecars_storage.get_config(),
            ),
            ("archive_edit", self.archive_edit_storage.get_config()),
            ("backfill", self.backfill_storage.get_config()),
            ("hash_backfill", self.hash_backfill_storage.get_config()),
            (
                "relationship_suggest",
                self.relationship_suggest_storage.get_config(),
            ),
            (
                "provider_coverage",
                self.provider_coverage_storage.get_config(),
            ),
            ("entity_enrich", self.entity_enrich_storage.get_config()),
            ("sidecar_reemit", self.sidecar_reemit_storage.get_config()),
        ]
    }

    /// Count dead-lettered jobs per queue (OPS-3 follow-up). apalis moves a job
    /// to its `{queue}:dead` set after it exhausts [`JOB_MAX_ATTEMPTS`]; nothing
    /// surfaced these before, so a permanently-failing job vanished silently.
    ///
    /// The dead set is a plain Redis SET in apalis-redis 1.0 (0.7 scored a
    /// ZSET by kill time; the time now lives in `{queue}:meta:{id}`), so each
    /// is counted with `SCARD`. The key is resolved from the storage's own
    /// `Config::dead_jobs_set()` rather than hardcoded. Returns
    /// `(queue_label, count)` for every queue.
    pub async fn dead_letter_counts(&self) -> redis::RedisResult<Vec<(&'static str, i64)>> {
        let mut conn = self.redis.clone();
        let mut out = Vec::with_capacity(QUEUE_COUNT);
        for (label, cfg) in self.queue_configs() {
            let count: i64 = conn.scard(cfg.dead_jobs_set()).await?;
            out.push((label, count));
        }
        Ok(out)
    }

    /// Outstanding (not yet finished) jobs per queue, split by where apalis
    /// holds them. Two pipelined round-trips for all queues.
    ///
    /// apalis-redis keeps a job in exactly one of three places until it
    /// finishes: the `{queue}:active` LIST (waiting for a worker), the
    /// `{queue}:scheduled` ZSET (waiting out a retry delay), or one worker's
    /// `{queue}:inflight:{worker}` SET (fetched by that worker — executing,
    /// or in its prefetch buffer). The in-flight sets are found through the
    /// `{queue}:workers` ZSET, whose members are the worker names (scored by
    /// last heartbeat); a dead process's set stays counted until the orphan
    /// sweep hands its jobs back to `active`.
    pub async fn queue_counts(&self) -> redis::RedisResult<Vec<QueueCount>> {
        let queues = self.queue_configs();
        let mut conn = self.redis.clone();

        let mut pipe = redis::pipe();
        for (_, cfg) in &queues {
            pipe.llen(cfg.active_jobs_list())
                .zcard(cfg.scheduled_jobs_set())
                .zrange(cfg.workers_set(), 0, -1);
        }
        let firsts: Vec<redis::Value> = pipe.query_async(&mut conn).await?;
        let mut firsts = firsts.into_iter();

        let mut out = Vec::with_capacity(QUEUE_COUNT);
        let mut inflight_sets: Vec<Vec<String>> = Vec::with_capacity(QUEUE_COUNT);
        for (label, cfg) in &queues {
            let (Some(waiting), Some(scheduled), Some(members)) =
                (firsts.next(), firsts.next(), firsts.next())
            else {
                break;
            };
            out.push(QueueCount {
                queue: label,
                waiting: redis::from_redis_value(waiting)?,
                scheduled: redis::from_redis_value(scheduled)?,
                in_flight: 0,
            });
            let workers: Vec<String> = redis::from_redis_value(members)?;
            inflight_sets.push(workers.iter().map(|w| inflight_set_key(cfg, w)).collect());
        }

        let mut pipe = redis::pipe();
        let mut any = false;
        for set in inflight_sets.iter().flatten() {
            pipe.scard(set);
            any = true;
        }
        if any {
            let sizes: Vec<i64> = pipe.query_async(&mut conn).await?;
            let mut sizes = sizes.into_iter();
            for (count, sets) in out.iter_mut().zip(&inflight_sets) {
                count.in_flight = sizes.by_ref().take(sets.len()).sum();
            }
        }
        Ok(out)
    }

    /// Lane totals for the named queues, in the order given (unknown labels
    /// are skipped): `waiting` = the `active` list plus `scheduled` entries
    /// already due (apalis-redis 1.0 pushes every new job into `scheduled`
    /// stamped `run_at = now`; a fetch promotes it), `scheduled` = entries
    /// with a future `run_at` (retry-delayed), `running` = the worker
    /// in-flight sets. Two pipelined round-trips however many queues.
    pub async fn lane_counts(
        &self,
        labels: &[&str],
    ) -> redis::RedisResult<Vec<(&'static str, activity::LaneCounts)>> {
        let all = self.queue_configs();
        let queues: Vec<(&'static str, &RedisConfig)> = labels
            .iter()
            .filter_map(|l| all.iter().find(|(q, _)| q == l).copied())
            .collect();
        if queues.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.redis.clone();
        let now = unix_now_secs();
        let mut pipe = redis::pipe();
        for (_, cfg) in &queues {
            pipe.llen(cfg.active_jobs_list())
                .zcount(cfg.scheduled_jobs_set(), "-inf", now)
                .zcount(cfg.scheduled_jobs_set(), format!("({now}"), "+inf")
                .zrange(cfg.workers_set(), 0, -1);
        }
        type Row = (i64, i64, i64, Vec<String>);
        let rows: Vec<Row> = {
            let flat: Vec<redis::Value> = pipe.query_async(&mut conn).await?;
            let mut it = flat.into_iter();
            let mut rows = Vec::with_capacity(queues.len());
            while let (Some(a), Some(d), Some(f), Some(w)) =
                (it.next(), it.next(), it.next(), it.next())
            {
                rows.push((
                    redis::from_redis_value(a)?,
                    redis::from_redis_value(d)?,
                    redis::from_redis_value(f)?,
                    redis::from_redis_value(w)?,
                ));
            }
            rows
        };
        let mut pipe = redis::pipe();
        let mut any = false;
        for ((_, cfg), (_, _, _, workers)) in queues.iter().zip(&rows) {
            for w in workers {
                pipe.scard(inflight_set_key(cfg, w));
                any = true;
            }
        }
        let sizes: Vec<i64> = if any {
            pipe.query_async(&mut conn).await?
        } else {
            Vec::new()
        };
        let mut sizes = sizes.into_iter();
        Ok(queues
            .iter()
            .zip(rows)
            .map(|((label, _), (active, due, future, workers))| {
                (
                    *label,
                    activity::LaneCounts {
                        waiting: active + due,
                        scheduled: future,
                        running: sizes.by_ref().take(workers.len()).sum(),
                    },
                )
            })
            .collect())
    }

    /// The thumbnail jobs Redis still holds (waiting, retry-delayed or
    /// fetched by a worker), decoded from the queue's job-data hash. Read
    /// once at boot to re-seed the process-local dedupe set
    /// ([`AppState::thumb_job_inflight`](crate::state::AppState)), which
    /// otherwise forgets every job queued by the previous process: the
    /// per-library "queued" counts read 0 while thousands of jobs drain.
    /// Undecodable payloads are skipped.
    pub async fn outstanding_thumb_jobs(&self) -> redis::RedisResult<Vec<post_scan::ThumbsJob>> {
        let cfg = self.post_scan_thumbs_storage.get_config();
        let mut conn = self.redis.clone();
        let mut ids: Vec<String> = conn.lrange(cfg.active_jobs_list(), 0, -1).await?;
        let scheduled: Vec<String> = conn.zrange(cfg.scheduled_jobs_set(), 0, -1).await?;
        ids.extend(scheduled);
        let workers: Vec<String> = conn.zrange(cfg.workers_set(), 0, -1).await?;
        for worker in workers {
            let held: Vec<String> = conn.smembers(inflight_set_key(cfg, &worker)).await?;
            ids.extend(held);
        }
        let data_hash = cfg.job_data_hash();
        let mut jobs = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(1000) {
            let payloads: Vec<Option<Vec<u8>>> = redis::cmd("HMGET")
                .arg(&data_hash)
                .arg(chunk)
                .query_async(&mut conn)
                .await?;
            jobs.extend(
                payloads
                    .into_iter()
                    .flatten()
                    .filter_map(|bytes| decode_job_args(&bytes)),
            );
        }
        Ok(jobs)
    }
}

/// Number of apalis queues [`JobRuntime`] owns.
pub const QUEUE_COUNT: usize = 17;

/// One queue's outstanding jobs — see [`JobRuntime::queue_counts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueCount {
    pub queue: &'static str,
    /// In the `active` list: no worker has fetched it yet.
    pub waiting: i64,
    /// In the `scheduled` set: waiting out a retry delay.
    pub scheduled: i64,
    /// In a worker's in-flight set: executing or prefetched.
    pub in_flight: i64,
}

impl QueueCount {
    /// Everything not finished.
    pub fn outstanding(&self) -> i64 {
        self.waiting + self.scheduled + self.in_flight
    }
}

/// The job arguments of one stored apalis payload. apalis-redis 1.0 stores
/// the bare args as JSON (`JsonCodec`) in `{queue}:data`; the attempt
/// counter, status and result live in the `{queue}:meta:{id}` hash.
fn decode_job_args<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::from_slice(bytes).ok()
}

/// Unix seconds, the clock apalis-redis stamps `run_at` with.
pub(crate) fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A worker's in-flight SET (`{queue}:inflight:{worker}`) — the key
/// `Config::inflight_set_for` derives, for a worker name read back from
/// the `{queue}:workers` ZSET rather than a live `WorkerContext`.
pub(crate) fn inflight_set_key(cfg: &RedisConfig, worker: &str) -> String {
    format!("{}:{worker}", cfg.inflight_jobs_set())
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

    /// The fetch timer is what bounds job pickup latency (module docs). It
    /// must stay far below the heartbeat — equal to it would reproduce the
    /// 30 s hops of the un-polled 1.0 default — and sub-second so a chained
    /// apply → rewrite → rescan still feels immediate.
    #[test]
    fn poll_interval_is_sub_second_and_below_heartbeat() {
        assert!(JOB_POLL_INTERVAL <= std::time::Duration::from_secs(1));
        assert!(JOB_POLL_INTERVAL >= std::time::Duration::from_millis(100));
        assert!(
            JOB_POLL_INTERVAL * 10 <= std::time::Duration::from_secs(JOB_HEARTBEAT_SECS),
            "poll interval must be at least an order of magnitude below the heartbeat"
        );
    }

    /// The queue name must stay exactly the job's `type_name`: it is the
    /// Redis key prefix for every queue (and what apalis-redis 0.7 derived),
    /// so a drift here would rename every key across an upgrade.
    #[test]
    fn storage_config_keeps_type_name_queue() {
        let cfg = storage_config::<scan::Job>();
        assert_eq!(
            cfg.queue.as_ref(),
            std::any::type_name::<scan::Job>(),
            "queue must match the 0.7 RedisStorage::new namespace derivation"
        );
        assert!(
            cfg.queue.as_ref().ends_with("jobs::scan::Job"),
            "type_name shape changed: {}",
            cfg.queue.as_ref()
        );
        assert_eq!(
            cfg.active_jobs_list(),
            format!("{}:active", cfg.queue.as_ref())
        );
        assert_eq!(cfg.dead_jobs_set(), format!("{}:dead", cfg.queue.as_ref()));
        assert_eq!(cfg.job_data_hash(), format!("{}:data", cfg.queue.as_ref()));
    }

    #[test]
    fn storage_config_pins_orphan_window() {
        let cfg = storage_config::<post_scan::ThumbsJob>();
        assert_eq!(
            cfg.orphaned_duration(),
            std::time::Duration::from_secs(JOB_REENQUEUE_ORPHANED_AFTER_SECS)
        );
        // Must exceed the heartbeat or a live-but-slow worker's job gets
        // re-enqueued underneath it.
        assert!(cfg.orphaned_duration() > cfg.heartbeat_interval);
        assert_eq!(
            cfg.heartbeat_interval,
            std::time::Duration::from_secs(JOB_HEARTBEAT_SECS)
        );
    }

    /// `with_retry_budget` passes early failures through (apalis reschedules
    /// them) and converts the failure on attempt `JOB_MAX_ATTEMPTS` into an
    /// `AbortError` (task killed → dead set).
    #[tokio::test]
    async fn retry_budget_kills_on_last_attempt() {
        async fn always_fails(_job: u8, _state: Data<()>) -> Result<(), BoxDynError> {
            Err("boom".into())
        }
        let mut wrapped = with_retry_budget(always_fails);

        let early = wrapped(1, Data::new(()), Attempt::new_with_value(1))
            .await
            .expect_err("handler fails");
        assert!(early.downcast_ref::<AbortError>().is_none(), "{early}");

        let last = wrapped(1, Data::new(()), Attempt::new_with_value(JOB_MAX_ATTEMPTS))
            .await
            .expect_err("handler fails");
        assert!(last.downcast_ref::<AbortError>().is_some(), "{last}");
    }
}
