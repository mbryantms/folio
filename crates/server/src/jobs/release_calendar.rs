//! Release-calendar work (`upcoming::calendar`, upcoming-releases 1.0).
//!
//! - **List pass**: one queue job, concurrency 1. It is pushed by the
//!   daily cron, by the enrich sweep when a pass is due or stopped
//!   part-way, and by the admin "Sync now", all through [`enqueue`],
//!   whose Redis `SET NX` key is held from the push until the pass
//!   returns. So at most one pass is queued or running at a time (on
//!   every replica: there is no leader lease, see
//!   `docs/install/scaling.md`), and a trigger that arrives mid-pass
//!   adds nothing.
//! - **Enrich sweep**: a cron closure once a minute ([`sweep`]),
//!   guarded against overlapping itself. It stands aside while a list
//!   pass is stopped part-way: both draw on the same per-minute Metron
//!   budget, and the listing goes first.
//!
//! The handler returns `Ok` after a failed pass on purpose. The failure
//! is recorded on `release_calendar_state` with a backoff, and the sweep
//! queues the next attempt once it has run out. Returning `Err` would
//! have apalis retry at once, straight into the same failure and the
//! same provider budget.

use apalis::prelude::*;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::upcoming::calendar::{self, PassOutcome};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseCalendarSyncJob {
    /// Who asked: `scheduled`, `sweep` or `manual`. Shown in the logs.
    pub trigger: String,
}

/// Lifetime of the queued-or-running key: longer than a pass can sit in
/// the queue and run, short enough that a crashed worker only blocks
/// re-queueing briefly.
const QUEUED_TTL_SECS: u64 = 30 * 60;
const QUEUED_KEY: &str = "release_calendar:sync:queued";
/// The sweep's overlap guard; a tick is at most ten requests.
const SWEEP_LOCK_KEY: &str = "release_calendar:enrich:lock";
const SWEEP_LOCK_TTL_SECS: u64 = 4 * 60;

/// `SET key 1 NX EX ttl`. `Ok(true)` when this caller took the key.
async fn claim(state: &AppState, key: &str, ttl_secs: u64) -> Result<bool, redis::RedisError> {
    let mut conn = state.jobs.redis.clone();
    redis::cmd("SET")
        .arg(key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async::<Option<String>>(&mut conn)
        .await
        .map(|r| r.is_some())
}

async fn release(state: &AppState, key: &str) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del(key).await;
}

/// Queue a list pass unless one is already queued or running. Returns
/// `true` when a job was pushed. Best-effort: a Redis error is logged and
/// reported as `false` (the next sweep tick queues it).
pub async fn enqueue(state: &AppState, trigger: &str) -> bool {
    match claim(state, QUEUED_KEY, QUEUED_TTL_SECS).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(
                trigger,
                "release calendar: a list pass is already queued or running"
            );
            return false;
        }
        Err(e) => {
            tracing::warn!(trigger, error = %e, "release calendar: dedupe key failed");
            return false;
        }
    }
    let mut storage = state.jobs.release_calendar_storage.clone();
    match storage
        .push(ReleaseCalendarSyncJob {
            trigger: trigger.to_owned(),
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            release(state, QUEUED_KEY).await;
            tracing::error!(trigger, error = %e, "release calendar: enqueue failed");
            false
        }
    }
}

pub async fn handle(job: ReleaseCalendarSyncJob, state: Data<AppState>) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();
    process(&state, &job.trigger).await;
    // Released only now: a trigger that arrived mid-pass would have
    // queued a second pass with nothing left to do. A pass that stopped
    // part-way is queued again by the next sweep tick.
    release(&state, QUEUED_KEY).await;
    Ok(())
}

/// Run one list pass and log what it did. Public so tests drive it
/// without a worker.
pub async fn process(state: &AppState, trigger: &str) -> Option<PassOutcome> {
    match calendar::list_pass(state).await {
        Ok(outcome) => {
            match &outcome {
                PassOutcome::Finished {
                    weeks,
                    issues,
                    removed,
                    requests,
                } => tracing::info!(
                    trigger,
                    weeks,
                    issues,
                    removed,
                    requests,
                    "release calendar: list pass finished"
                ),
                PassOutcome::Paused {
                    reason,
                    next_week,
                    requests,
                } => tracing::info!(
                    trigger,
                    reason = ?reason,
                    next_week = %next_week,
                    requests,
                    "release calendar: list pass paused; the sweep resumes it"
                ),
                PassOutcome::Failed { error } => {
                    tracing::warn!(trigger, error, "release calendar: list pass failed");
                }
                PassOutcome::BackingOff { until } => {
                    tracing::debug!(trigger, until = %until, "release calendar: backing off");
                }
                PassOutcome::Disabled => {
                    tracing::debug!(trigger, "release calendar: disabled");
                }
            }
            Some(outcome)
        }
        Err(e) => {
            tracing::error!(trigger, error = %e, "release calendar: list pass hit a database error");
            None
        }
    }
}

/// One enrich tick, then queue a list pass if one is due. Called by the
/// five-minute cron. No-op while the calendar is off.
pub async fn sweep(state: &AppState) {
    if !calendar::ready(state) {
        return;
    }
    let row = match calendar::load_state(&state.db).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, "release calendar: state read failed");
            return;
        }
    };
    // A list pass in flight or stopped part-way keeps the Metron budget:
    // the enrich tick stands aside and the sweep only makes sure the pass
    // is queued. Cover colours are image requests, not API requests, so
    // they are read either way.
    let pass_idle = row.pass_next_week.is_none();
    match claim(state, SWEEP_LOCK_KEY, SWEEP_LOCK_TTL_SECS).await {
        Ok(true) => {
            if pass_idle {
                match calendar::enrich(state).await {
                    Ok(out) if out.fetched > 0 || out.paused => tracing::info!(
                        fetched = out.fetched,
                        paused = out.paused,
                        "release calendar: enrich tick"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "release calendar: enrich tick failed"),
                }
            }
            match calendar::read_cover_tones(state).await {
                Ok(0) => {}
                Ok(read) => tracing::info!(read, "release calendar: cover colours read"),
                Err(e) => {
                    tracing::error!(error = %e, "release calendar: cover colour tick failed");
                }
            }
            release(state, SWEEP_LOCK_KEY).await;
        }
        // Another tick (or another replica) is still working.
        Ok(false) if pass_idle => return,
        Ok(false) => {}
        Err(e) if pass_idle => {
            tracing::warn!(error = %e, "release calendar: sweep lock failed");
            return;
        }
        Err(e) => tracing::warn!(error = %e, "release calendar: sweep lock failed"),
    }
    if calendar::pass_due(&row, chrono::Utc::now().fixed_offset()) {
        enqueue(state, "sweep").await;
    }
}
