//! Background job runtime — apalis 0.6 over Redis.
//!
//! Library Scanner v1, Milestone 2 (spec §3, §3.2, §4.8).
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
//! Workers run inside the same tokio runtime as the HTTP server. There is no
//! separate worker binary; this matches the single-binary deploy story.

use crate::state::AppState;
use apalis_redis::{RedisStorage, connect};
use entity::scan_run::{ActiveModel as ScanRunAM, Entity as ScanRunEntity};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

pub mod close_dangling_sessions;
pub mod orphan_sweep;
pub mod post_scan;
pub mod prune_auth_sessions;
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
    pub redis: ConnectionManager,
}

impl JobRuntime {
    /// Connect to Redis and build the typed storages. Fails fast if Redis is
    /// unreachable — the spec treats Redis as a hard dependency post-Milestone-2.
    pub async fn new(redis_url: &str, db: DatabaseConnection) -> anyhow::Result<Self> {
        let conn = connect(redis_url).await?;
        let scan_storage = RedisStorage::<scan::Job>::new(conn.clone());
        let scan_series_storage = RedisStorage::<scan_series::Job>::new(conn.clone());
        let post_scan_thumbs_storage = RedisStorage::<post_scan::ThumbsJob>::new(conn.clone());
        let post_scan_search_storage = RedisStorage::<post_scan::SearchJob>::new(conn.clone());
        let post_scan_dictionary_storage =
            RedisStorage::<post_scan::DictionaryJob>::new(conn.clone());
        Ok(Self {
            db,
            scan_storage,
            scan_series_storage,
            post_scan_thumbs_storage,
            post_scan_search_storage,
            post_scan_dictionary_storage,
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
        let mut conn = self.redis.clone();
        let in_flight_key = in_flight_key(library_id);
        let queued_key = queued_key(library_id);
        let scan_id_key = scan_id_key(library_id);

        let in_flight: Option<String> = conn.get(&in_flight_key).await?;
        if let Some(_existing) = in_flight {
            // A scan is running. Mark another one queued and return the
            // running id so the caller can advertise a stable scan_id.
            let _: () = conn.set(&queued_key, "1").await?;
            // Persist the queued-force flag so the post-completion re-enqueue
            // honors the strongest request.
            if force {
                let _: () = conn.set(format!("{queued_key}:force"), "1").await?;
            }
            let scan_id: Option<String> = conn.get(&scan_id_key).await?;
            let scan_id = scan_id
                .and_then(|s| Uuid::parse_str(&s).ok())
                .unwrap_or_else(Uuid::now_v7);
            return Ok(CoalesceOutcome::Coalesced { scan_id });
        }

        // No scan in flight — claim it.
        let scan_id = Uuid::now_v7();
        let _: () = conn.set(&scan_id_key, scan_id.to_string()).await?;
        let _: () = conn.set(&in_flight_key, scan_id.to_string()).await?;
        self.insert_queued_scan_run(library_id, scan_id, "library", None, None)
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

        let _: () = conn.del(&in_flight_key).await?;

        let queued: Option<String> = conn.get(&queued_key).await?;
        if queued.is_some() {
            let force_flag: Option<String> = conn.get(&queued_force_key).await?;
            let _: () = conn.del(&queued_key).await?;
            let _: () = conn.del(&queued_force_key).await?;
            let new_id = Uuid::now_v7();
            let _: () = conn.set(&scan_id_key, new_id.to_string()).await?;
            let _: () = conn.set(&in_flight_key, new_id.to_string()).await?;
            self.insert_queued_scan_run(library_id, new_id, "library", None, None)
                .await?;
            use apalis::prelude::Storage;
            let mut storage = self.scan_storage.clone();
            storage
                .push(scan::Job {
                    library_id,
                    scan_run_id: new_id,
                    force: force_flag.is_some(),
                })
                .await?;
        }
        Ok(())
    }

    /// Spawn workers for every queue. Returns when any worker exits — callers
    /// should keep this future alive for the server's lifetime.
    pub async fn run(self, state: AppState) {
        use apalis::prelude::*;

        let scan_concurrency = state.cfg.scan_worker_count;
        let post_concurrency = state.cfg.post_scan_worker_count;

        let scan_worker = WorkerBuilder::new("scan")
            .concurrency(scan_concurrency)
            .data(state.clone())
            .backend(self.scan_storage.clone())
            .build_fn(scan::handle);

        let scan_series_worker = WorkerBuilder::new("scan_series")
            .concurrency(scan_concurrency)
            .data(state.clone())
            .backend(self.scan_series_storage.clone())
            .build_fn(scan_series::handle);

        let thumbs_worker = WorkerBuilder::new("post_scan_thumbs")
            .concurrency(post_concurrency)
            .data(state.clone())
            .backend(self.post_scan_thumbs_storage.clone())
            .build_fn(post_scan::handle_thumbs);

        let search_worker = WorkerBuilder::new("post_scan_search")
            .concurrency(post_concurrency)
            .data(state.clone())
            .backend(self.post_scan_search_storage.clone())
            .build_fn(post_scan::handle_search);

        let dictionary_worker = WorkerBuilder::new("post_scan_dictionary")
            .concurrency(post_concurrency)
            .data(state.clone())
            .backend(self.post_scan_dictionary_storage.clone())
            .build_fn(post_scan::handle_dictionary);

        Monitor::new()
            .register(scan_worker)
            .register(scan_series_worker)
            .register(thumbs_worker)
            .register(search_worker)
            .register(dictionary_worker)
            .run()
            .await
            .expect("apalis monitor crashed");
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
        )
        .await?;
        let _: () = conn.set(&key, scan_id.to_string()).await?;
        let _: () = conn.set(&scan_id_key, scan_id.to_string()).await?;

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

fn in_flight_key(library_id: Uuid) -> String {
    format!("scan:in_flight:{library_id}")
}
fn queued_key(library_id: Uuid) -> String {
    format!("scan:queued:{library_id}")
}
fn scan_id_key(library_id: Uuid) -> String {
    format!("scan:scan_id:{library_id}")
}
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
        let _: () = conn.del(&scan_id_key).await?;
        Ok(())
    }
}
