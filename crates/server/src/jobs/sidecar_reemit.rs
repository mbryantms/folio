//! `SidecarReemitJob` — rewrite the ComicInfo / MetronInfo sidecars of every
//! issue in one writeback library from the current database (audit 2026-11
//! A7, decision D-6).
//!
//! Archives written by an older composer keep the older XML until something
//! touches them again. This job walks the library and pushes, per issue, the
//! same "compose from the database" rewrite a manual edit pushes
//! ([`crate::metadata::manual_writeback::enqueue_reemit_issue`]): an empty
//! provider payload, `post_apply = None`. Every archive write still goes
//! through [`RewriteIssueSidecarsJob`] and `archive::rewrite_policy`; this
//! module never opens an archive.
//!
//! ## Throttle
//!
//! The job is a self-re-enqueueing **tick** on its own queue
//! (`sidecar_reemit`, concurrency 1). Each tick pushes at most
//! [`TICK_BATCH`] rewrites and never lets the run have more than
//! [`MAX_OUTSTANDING`] rewrites queued or running at once. Each rewrite job
//! carries the inline XML (several KB), so queueing a 10k-issue library in
//! one go would park tens of MB in Redis and put every interactive
//! apply/edit behind the whole library; with the cap a user's edit waits
//! behind at most [`MAX_OUTSTANDING`] rewrites on the two sidecar workers.
//! A throttled or draining tick sleeps [`TICK_PAUSE`] before re-pushing
//! itself; an unthrottled one re-pushes at once (the 500 ms worker poll
//! paces it).
//!
//! ## Rescans
//!
//! Rewrites are pushed with `skip_rescan = true`. The walk is ordered by
//! `(series_id, issue id)`, and each series carries a pending counter: one
//! per queued rewrite, plus a guard the walk holds while it is still inside
//! that series. Whoever drops the counter to zero (the last rewrite to
//! finish, or the walk leaving the series) queues **one** series-scoped
//! rescan, and only if at least one archive of that series was rewritten —
//! the series-scope apply pattern, never an unscoped library scan.
//!
//! ## State
//!
//! Run state lives in Redis next to the queue (`sidecar_reemit:run:<id>`),
//! so a restart resumes from the persisted cursor (apalis re-enqueues the
//! orphaned tick). Finished runs expire after [`FINISHED_RUN_TTL_SECS`].
//! One run per library: `sidecar_reemit:active:<library_id>` names it while
//! it runs (TTL refreshed every tick, so a chain that died frees the library
//! after [`ACTIVE_TTL_SECS`]).
//!
//! Archives the sidecar path refuses (`sidecar_refusal`: CBR / CB7 without
//! conversion, unknown formats) are skipped and counted, never queued.
//! Turning either writeback flag off stops the run at its next tick.

use std::collections::HashMap;
use std::time::Duration;

use apalis::prelude::*;
use chrono::Utc;
use entity::{issue, library, series};
use redis::AsyncCommands;
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::jobs::rewrite_sidecars::sidecar_refusal;
use crate::metadata::manual_writeback::{self, Actor, SeriesContext};
use crate::state::AppState;

/// Most rewrites one tick queues.
pub const TICK_BATCH: i64 = 32;
/// Most rewrites of one run queued or running at once.
pub const MAX_OUTSTANDING: i64 = 32;
/// Pause before a throttled or draining tick re-enqueues itself.
pub const TICK_PAUSE: Duration = Duration::from_secs(2);
/// The library's active-run marker lives this long past the last tick.
pub const ACTIVE_TTL_SECS: u64 = 15 * 60;
/// How long a finished run stays readable (background work, library card).
pub const FINISHED_RUN_TTL_SECS: i64 = 7 * 24 * 3600;
/// Consecutive ticks that must see the rewrite queue empty while the run
/// still counts outstanding rewrites before those are written off as lost.
const LOST_AFTER_IDLE_TICKS: i64 = 2;

pub mod status {
    pub const RUNNING: &str = "running";
    pub const DRAINING: &str = "draining";
    pub const COMPLETED: &str = "completed";
    pub const ABORTED: &str = "aborted";
}

/// One tick of a run. The payload is only the handle: the cursor and the
/// counters live in the run's Redis hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidecarReemitJob {
    pub run_id: Uuid,
    pub library_id: Uuid,
}

/// Carried on each [`RewriteIssueSidecarsJob`] the run queues so the
/// rewrite reports back ([`on_rewrite_finished`]).
///
/// [`RewriteIssueSidecarsJob`]: crate::jobs::rewrite_sidecars::RewriteIssueSidecarsJob
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReemitTag {
    pub run_id: Uuid,
    pub library_id: Uuid,
    pub series_id: Uuid,
}

/// How a tagged rewrite ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteEnd {
    Rewritten,
    /// Refused at job time (writeback turned off, format gate, issue gone).
    Refused,
    /// Gave up after the retry budget (or the busy-lock budget).
    Failed,
}

fn run_key(run: Uuid) -> String {
    format!("sidecar_reemit:run:{run}")
}
fn pending_key(run: Uuid) -> String {
    format!("sidecar_reemit:run:{run}:pending")
}
fn rewrote_key(run: Uuid) -> String {
    format!("sidecar_reemit:run:{run}:rewrote")
}
fn active_key(library: Uuid) -> String {
    format!("sidecar_reemit:active:{library}")
}
const RUNS_INDEX: &str = "sidecar_reemit:runs";

/// A run as stored, decoded for the API.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSnapshot {
    pub run_id: Uuid,
    pub library_id: Uuid,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Active, non-removed issues when the run started.
    pub total: i64,
    /// Rewrites queued.
    pub enqueued: i64,
    /// Archives the sidecar path refused (skipped before queueing, or
    /// refused by the rewrite job itself).
    pub refused: i64,
    /// Issues that disappeared between the walk and the compose.
    pub gone: i64,
    /// Rewrites that landed.
    pub rewritten: i64,
    /// Rewrites that gave up (or were lost with their queue).
    pub failed: i64,
    /// Rewrites queued or running right now.
    pub outstanding: i64,
    /// Why an aborted run stopped.
    pub note: Option<String>,
}

impl RunSnapshot {
    fn from_hash(run_id: Uuid, h: &HashMap<String, String>) -> Option<Self> {
        let library_id = h.get("library_id")?.parse().ok()?;
        let int = |k: &str| h.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        Some(Self {
            run_id,
            library_id,
            status: h.get("status").cloned().unwrap_or_default(),
            started_at: h.get("started_at").cloned().unwrap_or_default(),
            finished_at: h.get("finished_at").cloned().filter(|s| !s.is_empty()),
            total: int("total"),
            enqueued: int("enqueued"),
            refused: int("refused"),
            gone: int("gone"),
            rewritten: int("rewritten"),
            failed: int("failed"),
            outstanding: int("outstanding").max(0),
            note: h.get("note").cloned().filter(|s| !s.is_empty()),
        })
    }

    pub fn is_live(&self) -> bool {
        self.status == status::RUNNING || self.status == status::DRAINING
    }
}

/// Why [`start`] did not start a run.
#[derive(Debug)]
pub enum StartError {
    /// The library is missing a writeback flag.
    NotWriteback,
    /// A run for this library is still live.
    InProgress(Uuid),
    Other(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for StartError {
    fn from(e: E) -> Self {
        Self::Other(e.into())
    }
}

fn writeback_on(lib: &library::Model) -> bool {
    lib.allow_archive_writeback && lib.metadata_writeback_enabled
}

fn live_issues(library_id: Uuid) -> sea_orm::Select<issue::Entity> {
    issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
}

/// Claim the library, record the run and queue its first tick.
pub async fn start(
    state: &AppState,
    lib: &library::Model,
    actor: &Actor,
) -> Result<RunSnapshot, StartError> {
    if !writeback_on(lib) {
        return Err(StartError::NotWriteback);
    }
    let mut conn = state.jobs.redis.clone();
    let run_id = Uuid::now_v7();
    let claimed: Option<String> = redis::cmd("SET")
        .arg(active_key(lib.id))
        .arg(run_id.to_string())
        .arg("NX")
        .arg("EX")
        .arg(ACTIVE_TTL_SECS)
        .query_async(&mut conn)
        .await?;
    if claimed.is_none() {
        let holder: Option<String> = conn.get(active_key(lib.id)).await?;
        let holder = holder.and_then(|s| s.parse().ok()).unwrap_or(Uuid::nil());
        return Err(StartError::InProgress(holder));
    }

    let started = async {
        // A run whose chain died keeps reading `running` after its claim
        // expired; the new run supersedes it.
        if let Some(stale) = latest_for_library(state, lib.id).await?
            && stale.is_live()
        {
            finish(
                state,
                &stale,
                status::ABORTED,
                Some("superseded by a newer run"),
            )
            .await?;
        }
        let total = live_issues(lib.id).count(&state.db).await? as i64;
        let now = Utc::now();
        let fields: Vec<(&str, String)> = vec![
            ("library_id", lib.id.to_string()),
            ("status", status::RUNNING.to_owned()),
            ("started_at", now.to_rfc3339()),
            ("total", total.to_string()),
            (
                "actor_id",
                actor.id.map(|u| u.to_string()).unwrap_or_default(),
            ),
            ("actor_ip", actor.ip.clone().unwrap_or_default()),
            ("actor_ua", actor.user_agent.clone().unwrap_or_default()),
        ];
        let _: () = conn.hset_multiple(run_key(run_id), &fields).await?;
        let cutoff = now.timestamp_millis() - FINISHED_RUN_TTL_SECS * 1000;
        let _: () = redis::pipe()
            .zadd(RUNS_INDEX, run_id.to_string(), now.timestamp_millis())
            .ignore()
            .zrembyscore(RUNS_INDEX, "-inf", cutoff)
            .ignore()
            .query_async(&mut conn)
            .await?;
        let mut storage = state.jobs.sidecar_reemit_storage.clone();
        storage
            .push(SidecarReemitJob {
                run_id,
                library_id: lib.id,
            })
            .await
            .map_err(|e| anyhow::anyhow!("sidecar re-emit enqueue failed: {e}"))?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = started {
        // Free the library and drop the half-written run.
        let _: Result<(), _> = redis::pipe()
            .del(active_key(lib.id))
            .ignore()
            .del(run_key(run_id))
            .ignore()
            .zrem(RUNS_INDEX, run_id.to_string())
            .ignore()
            .query_async(&mut conn)
            .await;
        return Err(StartError::Other(e));
    }
    load_run(state, run_id)
        .await?
        .ok_or_else(|| StartError::Other(anyhow::anyhow!("run {run_id} vanished after start")))
}

/// One run, or `None` once it expired (or a queue clear removed it).
pub async fn load_run(state: &AppState, run_id: Uuid) -> anyhow::Result<Option<RunSnapshot>> {
    let mut conn = state.jobs.redis.clone();
    let h: HashMap<String, String> = conn.hgetall(run_key(run_id)).await?;
    Ok(RunSnapshot::from_hash(run_id, &h))
}

/// The most recent runs (newest first), live and finished, still readable.
pub async fn recent_runs(state: &AppState, limit: isize) -> anyhow::Result<Vec<RunSnapshot>> {
    let mut conn = state.jobs.redis.clone();
    let ids: Vec<String> = conn.zrevrange(RUNS_INDEX, 0, limit - 1).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(run_id) = id.parse::<Uuid>() else {
            continue;
        };
        if let Some(run) = load_run(state, run_id).await? {
            out.push(run);
        }
    }
    Ok(out)
}

/// The newest readable run of one library.
pub async fn latest_for_library(
    state: &AppState,
    library_id: Uuid,
) -> anyhow::Result<Option<RunSnapshot>> {
    Ok(recent_runs(state, 50)
        .await?
        .into_iter()
        .find(|r| r.library_id == library_id))
}

/// What one [`tick`] left for the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// More to queue right now; re-enqueue without a pause.
    Continue,
    /// Throttled or draining; re-enqueue after [`TICK_PAUSE`].
    Wait,
    /// The run is over (completed, aborted, or gone).
    Done,
}

/// apalis entry point: one tick, then re-enqueue the next while the run
/// lives. A tick error returns `Err` so apalis retries this tick (the
/// cursor is persisted per issue, so a retry resumes where it stopped).
pub async fn handle(job: SidecarReemitJob, state: Data<AppState>) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();
    let outcome = tick(&state, job.run_id).await.map_err(|e| {
        tracing::warn!(run_id = %job.run_id, error = %e, "sidecar re-emit: tick failed");
        BoxDynError::from(e.to_string())
    })?;
    match outcome {
        TickOutcome::Done => return Ok(()),
        TickOutcome::Wait => tokio::time::sleep(TICK_PAUSE).await,
        TickOutcome::Continue => {}
    }
    let mut storage = state.jobs.sidecar_reemit_storage.clone();
    storage
        .push(job.clone())
        .await
        .map_err(|e| BoxDynError::from(format!("sidecar re-emit: next tick push failed: {e}")))?;
    Ok(())
}

/// Advance a run by one step. Public so tests can drive a run without the
/// worker's pacing.
pub async fn tick(state: &AppState, run_id: Uuid) -> anyhow::Result<TickOutcome> {
    let mut conn = state.jobs.redis.clone();
    let h: HashMap<String, String> = conn.hgetall(run_key(run_id)).await?;
    let Some(run) = RunSnapshot::from_hash(run_id, &h) else {
        // Expired or cleared with the metadata queues: nothing to do.
        return Ok(TickOutcome::Done);
    };
    if !run.is_live() {
        return Ok(TickOutcome::Done);
    }
    // Refresh the library claim, and keep the run's keys alive for a week
    // past its last tick (a chain that died still ages out).
    let _: () = redis::pipe()
        .set_ex(
            active_key(run.library_id),
            run_id.to_string(),
            ACTIVE_TTL_SECS,
        )
        .ignore()
        .expire(run_key(run_id), FINISHED_RUN_TTL_SECS)
        .ignore()
        .expire(pending_key(run_id), FINISHED_RUN_TTL_SECS)
        .ignore()
        .query_async(&mut conn)
        .await?;

    let lib = library::Entity::find_by_id(run.library_id)
        .one(&state.db)
        .await?;
    let Some(lib) = lib.filter(writeback_on) else {
        finish(
            state,
            &run,
            status::ABORTED,
            Some("library writeback was turned off (or the library was deleted)"),
        )
        .await?;
        return Ok(TickOutcome::Done);
    };

    // Rewrites the run is still waiting on, but whose queue is empty: they
    // were cleared or dead-lettered without reporting back. Write them off
    // so the throttle (and the drain) cannot wait forever.
    let mut outstanding = run.outstanding;
    if outstanding > 0 {
        let queue_empty = state
            .jobs
            .queue_counts()
            .await?
            .iter()
            .find(|c| c.queue == "rewrite_issue_sidecars")
            .is_none_or(|c| c.outstanding() == 0);
        if queue_empty {
            let idle: i64 = conn.hincr(run_key(run_id), "idle_ticks", 1).await?;
            if idle >= LOST_AFTER_IDLE_TICKS {
                tracing::warn!(run_id = %run_id, lost = outstanding, "sidecar re-emit: rewrites lost with their queue; counting them failed");
                let _: () = redis::pipe()
                    .hincr(run_key(run_id), "failed", outstanding)
                    .ignore()
                    .hset(run_key(run_id), "outstanding", 0)
                    .ignore()
                    .hset(run_key(run_id), "idle_ticks", 0)
                    .ignore()
                    .query_async(&mut conn)
                    .await?;
                outstanding = 0;
            }
        } else {
            let _: () = conn.hset(run_key(run_id), "idle_ticks", 0).await?;
        }
    }

    if run.status == status::DRAINING {
        if outstanding <= 0 {
            finish(state, &run, status::COMPLETED, None).await?;
            return Ok(TickOutcome::Done);
        }
        return Ok(TickOutcome::Wait);
    }

    let budget = (MAX_OUTSTANDING - outstanding).min(TICK_BATCH);
    if budget <= 0 {
        return Ok(TickOutcome::Wait);
    }
    walk(state, &run, &lib, budget as u64).await
}

/// Queue up to `budget` rewrites from the cursor on.
async fn walk(
    state: &AppState,
    run: &RunSnapshot,
    lib: &library::Model,
    budget: u64,
) -> anyhow::Result<TickOutcome> {
    let run_id = run.run_id;
    let mut conn = state.jobs.redis.clone();
    let cursor: (Option<String>, Option<String>, Option<String>) = redis::pipe()
        .hget(run_key(run_id), "cursor_series")
        .hget(run_key(run_id), "cursor_issue")
        .hget(run_key(run_id), "guard_series")
        .query_async(&mut conn)
        .await?;
    let cursor_series: Option<Uuid> = cursor.0.and_then(|s| s.parse().ok());
    let cursor_issue = cursor.1.unwrap_or_default();
    let mut guard: Option<Uuid> = cursor.2.and_then(|s| s.parse().ok());

    // Keyset page over (series_id, id), projecting only what the walk
    // needs; the composer loads one full row at a time.
    let mut page = live_issues(lib.id)
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::SeriesId)
        .column(issue::Column::FilePath);
    if let Some(cs) = cursor_series {
        page = page.filter(
            Condition::any().add(issue::Column::SeriesId.gt(cs)).add(
                Condition::all()
                    .add(issue::Column::SeriesId.eq(cs))
                    .add(issue::Column::Id.gt(cursor_issue)),
            ),
        );
    }
    let rows: Vec<(String, Uuid, String)> = page
        .order_by_asc(issue::Column::SeriesId)
        .order_by_asc(issue::Column::Id)
        .limit(budget)
        .into_tuple()
        .all(&state.db)
        .await?;

    let actor = run_actor(state, run_id).await?;
    let mut series_ctx: Option<SeriesContext> = None;
    for (issue_id, series_id, file_path) in &rows {
        if guard != Some(*series_id) {
            if let Some(prev) = guard {
                release_series(state, run_id, run.library_id, prev).await?;
            }
            let _: () = redis::pipe()
                .hincr(pending_key(run_id), series_id.to_string(), 1)
                .ignore()
                .hset(run_key(run_id), "guard_series", series_id.to_string())
                .ignore()
                .query_async(&mut conn)
                .await?;
            guard = Some(*series_id);
            series_ctx = None;
        }
        if sidecar_refusal(lib, file_path).is_some() {
            let _: () = conn.hincr(run_key(run_id), "refused", 1).await?;
        } else {
            if series_ctx.is_none() {
                series_ctx = match series::Entity::find_by_id(*series_id)
                    .one(&state.db)
                    .await?
                {
                    Some(row) => Some(SeriesContext::load(state, &row).await?),
                    None => None,
                };
            }
            let tag = ReemitTag {
                run_id,
                library_id: run.library_id,
                series_id: *series_id,
            };
            // Count the rewrite before it is queued, so a fast worker can
            // never report back below zero.
            let _: () = redis::pipe()
                .hincr(pending_key(run_id), series_id.to_string(), 1)
                .ignore()
                .hincr(run_key(run_id), "outstanding", 1)
                .ignore()
                .query_async(&mut conn)
                .await?;
            let queued = match &series_ctx {
                Some(ctx) => {
                    manual_writeback::enqueue_reemit_issue(state, issue_id, ctx, &actor, tag).await
                }
                None => Ok(false),
            };
            match queued {
                Ok(true) => {
                    let _: () = conn.hincr(run_key(run_id), "enqueued", 1).await?;
                }
                Ok(false) => {
                    undo_count(&mut conn, run_id, *series_id).await?;
                    let _: () = conn.hincr(run_key(run_id), "gone", 1).await?;
                }
                Err(e) => {
                    undo_count(&mut conn, run_id, *series_id).await?;
                    return Err(e);
                }
            }
        }
        let _: () = redis::pipe()
            .hset(run_key(run_id), "cursor_series", series_id.to_string())
            .ignore()
            .hset(run_key(run_id), "cursor_issue", issue_id)
            .ignore()
            .query_async(&mut conn)
            .await?;
    }

    if (rows.len() as u64) < budget {
        // The walk reached the end of the library.
        if let Some(prev) = guard {
            release_series(state, run_id, run.library_id, prev).await?;
            let _: () = conn.hdel(run_key(run_id), "guard_series").await?;
        }
        let _: () = conn
            .hset(run_key(run_id), "status", status::DRAINING)
            .await?;
        return Ok(TickOutcome::Wait);
    }
    Ok(TickOutcome::Continue)
}

/// Take back the count of a rewrite that was never queued.
async fn undo_count(
    conn: &mut redis::aio::ConnectionManager,
    run_id: Uuid,
    series_id: Uuid,
) -> redis::RedisResult<()> {
    redis::pipe()
        .hincr(pending_key(run_id), series_id.to_string(), -1)
        .ignore()
        .hincr(run_key(run_id), "outstanding", -1)
        .ignore()
        .query_async(conn)
        .await
}

async fn run_actor(state: &AppState, run_id: Uuid) -> anyhow::Result<Actor> {
    let mut conn = state.jobs.redis.clone();
    let (id, ip, ua): (Option<String>, Option<String>, Option<String>) = redis::pipe()
        .hget(run_key(run_id), "actor_id")
        .hget(run_key(run_id), "actor_ip")
        .hget(run_key(run_id), "actor_ua")
        .query_async(&mut conn)
        .await?;
    let non_empty = |s: Option<String>| s.filter(|v| !v.is_empty());
    Ok(Actor {
        id: non_empty(id).and_then(|s| s.parse().ok()),
        ip: non_empty(ip),
        user_agent: non_empty(ua),
    })
}

/// Drop one hold on `series`; the hold that reaches zero queues the
/// series-scoped rescan when any of its archives was rewritten.
async fn release_series(
    state: &AppState,
    run_id: Uuid,
    library_id: Uuid,
    series_id: Uuid,
) -> anyhow::Result<()> {
    let mut conn = state.jobs.redis.clone();
    let left: i64 = conn
        .hincr(pending_key(run_id), series_id.to_string(), -1)
        .await?;
    if left != 0 {
        return Ok(());
    }
    let _: () = conn
        .hdel(pending_key(run_id), series_id.to_string())
        .await?;
    rescan_if_rewritten(state, run_id, library_id, series_id).await
}

async fn rescan_if_rewritten(
    state: &AppState,
    run_id: Uuid,
    library_id: Uuid,
    series_id: Uuid,
) -> anyhow::Result<()> {
    let mut conn = state.jobs.redis.clone();
    let rewrote: i64 = conn
        .srem(rewrote_key(run_id), series_id.to_string())
        .await?;
    if rewrote == 0 {
        return Ok(());
    }
    // Best-effort, as on the series-scope apply: the XML already landed
    // and the next scheduled scan ingests it if this enqueue fails.
    if let Err(e) = state
        .jobs
        .coalesce_scoped_scan(
            library_id,
            series_id,
            None,
            crate::jobs::scan_series::JobKind::Series,
            None,
            true,
        )
        .await
    {
        tracing::error!(series_id = %series_id, error = %e, "sidecar re-emit: series rescan enqueue failed");
    }
    Ok(())
}

/// Close a run: stamp it, queue the rescans still owed, free the library
/// and let the keys expire.
async fn finish(
    state: &AppState,
    run: &RunSnapshot,
    final_status: &str,
    note: Option<&str>,
) -> anyhow::Result<()> {
    let run_id = run.run_id;
    let mut conn = state.jobs.redis.clone();
    let _: () = redis::pipe()
        .hset(run_key(run_id), "status", final_status)
        .ignore()
        .hset(run_key(run_id), "finished_at", Utc::now().to_rfc3339())
        .ignore()
        .hset(run_key(run_id), "note", note.unwrap_or_default())
        .ignore()
        .query_async(&mut conn)
        .await?;
    // Series whose count never reached zero (lost rewrites, an abort
    // mid-series) still get their rescan if anything was written.
    let owed: HashMap<String, i64> = conn.hgetall(pending_key(run_id)).await?;
    for series in owed.keys() {
        if let Ok(series_id) = series.parse::<Uuid>() {
            rescan_if_rewritten(state, run_id, run.library_id, series_id).await?;
        }
    }
    let leftover: Vec<String> = conn.smembers(rewrote_key(run_id)).await?;
    for series in leftover {
        if let Ok(series_id) = series.parse::<Uuid>() {
            rescan_if_rewritten(state, run_id, run.library_id, series_id).await?;
        }
    }
    let _: () = conn.del(pending_key(run_id)).await?;
    let holder: Option<String> = conn.get(active_key(run.library_id)).await?;
    if holder.as_deref() == Some(run_id.to_string().as_str()) {
        let _: () = conn.del(active_key(run.library_id)).await?;
    }
    let _: () = conn.expire(run_key(run_id), FINISHED_RUN_TTL_SECS).await?;
    tracing::info!(run_id = %run_id, library_id = %run.library_id, status = final_status, "sidecar re-emit finished");
    Ok(())
}

/// Called by the rewrite job when a tagged rewrite reaches a terminal
/// state. Best-effort: a Redis failure here only logs (the archive write
/// already happened or already failed).
pub async fn on_rewrite_finished(state: &AppState, tag: &ReemitTag, end: RewriteEnd) {
    if let Err(e) = record_rewrite_end(state, tag, end).await {
        tracing::warn!(run_id = %tag.run_id, error = %e, "sidecar re-emit: progress update failed");
    }
}

async fn record_rewrite_end(
    state: &AppState,
    tag: &ReemitTag,
    end: RewriteEnd,
) -> anyhow::Result<()> {
    let mut conn = state.jobs.redis.clone();
    // A run removed by a queue clear (or expired) takes no updates; the
    // hash must not be resurrected field by field.
    let run_status: Option<String> = conn.hget(run_key(tag.run_id), "status").await?;
    let Some(run_status) = run_status else {
        return Ok(());
    };
    let live = run_status == status::RUNNING || run_status == status::DRAINING;
    let field = match end {
        RewriteEnd::Rewritten => "rewritten",
        RewriteEnd::Refused => "refused",
        RewriteEnd::Failed => "failed",
    };
    let mut pipe = redis::pipe();
    pipe.hincr(run_key(tag.run_id), field, 1).ignore();
    if end == RewriteEnd::Rewritten {
        pipe.sadd(rewrote_key(tag.run_id), tag.series_id.to_string())
            .ignore()
            .expire(rewrote_key(tag.run_id), FINISHED_RUN_TTL_SECS)
            .ignore();
    }
    if !live {
        // A straggler after the run closed: count it, nothing else (the
        // close already queued the rescans and dropped the counters).
        let _: () = pipe.query_async(&mut conn).await?;
        return Ok(());
    }
    pipe.hincr(run_key(tag.run_id), "outstanding", -1);
    let (left,): (i64,) = pipe.query_async(&mut conn).await?;
    if left < 0 {
        // A dead-letter retry of an already-counted rewrite.
        let _: () = conn.hset(run_key(tag.run_id), "outstanding", 0).await?;
    }
    release_series(state, tag.run_id, tag.library_id, tag.series_id).await
}
