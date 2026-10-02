//! `RelationshipSuggestJob` — runs the WP-7.2 relationship suggestion
//! engine ([`crate::relationships::suggestions`]) for one library.
//!
//! Enqueued at the end of every library scan that changed something (full,
//! watcher-scoped, or series-scoped — see `library::scanner`), and on demand
//! from `POST /api/admin/relationship-suggestions/run`. The job is bounded
//! regardless of what triggered it: every evidence query is set-based and
//! starts from this library's series (the WP-8.2 edition sources look up
//! targets in other libraries by key), and at most
//! [`MAX_SUGGESTIONS_PER_RUN`](crate::relationships::suggestions::MAX_SUGGESTIONS_PER_RUN)
//! rows are written per run.
//!
//! **Dedupe.** [`enqueue`] claims `relsuggest:queued:<library_id>` with
//! `SET NX EX`; a second trigger while one is queued is a no-op. The handler
//! clears the key when it *starts*, so a scan that finishes while a run is
//! in progress queues exactly one follow-up. The TTL bounds a stale key left
//! by a crash. The worker runs at `concurrency(1)`.
//!
//! **Observability.** A `relationship_suggest` tracing span with the run
//! counts, plus a `library_events` row (category `series`, action
//! `generated`) whenever the run inserted, changed or staled suggestions.

use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::relationships::suggestions::{self, RunReport};
use crate::state::AppState;
use apalis::prelude::*;
use redis::AsyncCommands;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelationshipSuggestJob {
    pub library_id: Uuid,
}

/// Lifetime of the enqueue-dedupe key. Long enough to cover a queue backlog,
/// short enough that a crash only suppresses re-enqueues briefly (a stale
/// key costs at most one skipped post-scan run).
const QUEUED_TTL_SECS: u64 = 30 * 60;

fn queued_key(library_id: Uuid) -> String {
    format!("relsuggest:queued:{library_id}")
}

/// Push a run for `library_id` unless one is already queued. Returns `true`
/// when a job was pushed. Best-effort: a Redis error is logged and reported
/// as `false` (the next scan or an on-demand run catches up).
pub async fn enqueue(state: &AppState, library_id: Uuid) -> bool {
    let mut conn = state.jobs.redis.clone();
    let key = queued_key(library_id);
    let claimed: Result<bool, _> = redis::cmd("SET")
        .arg(&key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(QUEUED_TTL_SECS)
        .query_async::<Option<String>>(&mut conn)
        .await
        .map(|r| r.is_some());
    match claimed {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(library_id = %library_id, "relationship suggest: already queued");
            return false;
        }
        Err(e) => {
            tracing::warn!(library_id = %library_id, error = %e, "relationship suggest: dedupe key failed");
            return false;
        }
    }
    let mut storage = state.jobs.relationship_suggest_storage.clone();
    match storage.push(RelationshipSuggestJob { library_id }).await {
        Ok(_) => true,
        Err(e) => {
            let _: Result<(), _> = conn.del(&key).await;
            tracing::error!(library_id = %library_id, error = %e, "relationship suggest: enqueue failed");
            false
        }
    }
}

pub async fn handle(job: RelationshipSuggestJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    let library_id = job.library_id;
    // Clear first: triggers that arrive while this run is in progress must
    // queue a follow-up (they may have changed the evidence).
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del(queued_key(library_id)).await;
    if let Err(e) = run_with_state(&state, library_id).await {
        // Logged, not retried: the next scan re-runs it, and a retry storm
        // on a persistent SQL error would only repeat the failure.
        tracing::error!(library_id = %library_id, error = %e, "relationship suggest: run failed");
    }
    Ok(())
}

/// [`run`], then drop the similar-series cache when the promotion pass
/// turned external links into series relationships (WP-8.2: the promotion
/// code only has a connection, so the job does it). What the worker runs.
pub async fn run_with_state(state: &AppState, library_id: Uuid) -> anyhow::Result<RunReport> {
    let report = run(&state.db, library_id).await?;
    if report.promoted_pairs > 0 {
        state.similarity.invalidate_all();
    }
    Ok(report)
}

/// Generate suggestions for `library_id` and record the `library_events`
/// row. Public so tests can run it without a worker. A library that no
/// longer exists is a no-op. Doesn't touch the similar-series cache (no
/// `AppState`): [`run_with_state`] does, from `report.promoted_pairs`.
pub async fn run(db: &DatabaseConnection, library_id: Uuid) -> anyhow::Result<RunReport> {
    use sea_orm::EntityTrait;
    if entity::library::Entity::find_by_id(library_id)
        .one(db)
        .await?
        .is_none()
    {
        return Ok(RunReport {
            library_id,
            ..Default::default()
        });
    }
    // WP-7.8: resolve external links and label-only reprints whose target
    // was scanned in / matched since the last run, before the sources read
    // them. Best-effort: a failure only delays promotion to the next run.
    let mut promoted_pairs = 0;
    match crate::relationships::external::promote_library(db, library_id).await {
        Ok(p) if p.promoted + p.unmarked > 0 || p.reprints_resolved > 0 => {
            promoted_pairs = p.pairs_created;
            tracing::info!(
                library_id = %library_id,
                promoted = p.promoted,
                pairs_created = p.pairs_created,
                unmarked = p.unmarked,
                reprints_resolved = p.reprints_resolved,
                "relationship suggest: external links promoted"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(library_id = %library_id, error = %e,
                "relationship suggest: external-link promotion failed");
        }
    }
    let mut report = suggestions::generate_for_library(db, library_id).await?;
    report.promoted_pairs = promoted_pairs;
    if report.inserted + report.updated + report.marked_stale > 0 {
        event_log::record(
            db,
            NewEvent::new(
                library_id,
                Category::Series,
                Action::Generated,
                Severity::Info,
                format!(
                    "Relationship suggestions: {} new, {} updated, {} stale{}",
                    report.inserted,
                    report.updated,
                    report.marked_stale,
                    if report.capped > 0 {
                        format!(
                            " ({} over the per-run cap of {})",
                            report.capped,
                            suggestions::MAX_SUGGESTIONS_PER_RUN
                        )
                    } else {
                        String::new()
                    }
                ),
            )
            .detail(serde_json::json!({
                "kind": "relationship_suggestions",
                "report": report,
            })),
        )
        .await;
    }
    Ok(report)
}
