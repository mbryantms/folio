//! `RelationshipSuggestJob` — runs the WP-7.2 relationship suggestion
//! engine ([`crate::relationships::suggestions`]) for one library.
//!
//! Enqueued at the end of every library scan that changed something (full,
//! watcher-scoped, or series-scoped — see `library::scanner`), and on demand
//! from `POST /api/admin/relationship-suggestions/run`. The job is bounded
//! regardless of what triggered it: every evidence query is set-based and
//! library-scoped, and at most
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
//! `generated`) whenever the run inserted or changed suggestions.

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
    if let Err(e) = run(&state.db, library_id).await {
        // Logged, not retried: the next scan re-runs it, and a retry storm
        // on a persistent SQL error would only repeat the failure.
        tracing::error!(library_id = %library_id, error = %e, "relationship suggest: run failed");
    }
    Ok(())
}

/// Generate suggestions for `library_id` and record the `library_events`
/// row. Public so tests and the on-demand path can run it without a worker.
/// A library that no longer exists is a no-op.
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
    let report = suggestions::generate_for_library(db, library_id).await?;
    if report.inserted + report.updated > 0 {
        event_log::record(
            db,
            NewEvent::new(
                library_id,
                Category::Series,
                Action::Generated,
                Severity::Info,
                format!(
                    "Relationship suggestions: {} new, {} updated{}",
                    report.inserted,
                    report.updated,
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
