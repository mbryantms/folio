//! Auto-resume parked metadata runs (refine-bulk-metadata M5).
//!
//! When every enabled provider is out of quota mid-search, the orchestrator
//! parks the run at `status='awaiting_quota'` with a `resume_after`. Nothing
//! re-drove those runs before — the user had to re-trigger. This sweep (a
//! once-a-minute scheduler tick) picks up runs whose window has passed and
//! re-enqueues them through the normal coalesce-gated path, reusing each run's
//! stored entity + `batch_id` so a parked bulk-fetch finishes on its own.
//!
//! Pacing: the tick is gated on a cheap provider quota snapshot (skip entirely
//! when no provider has budget) and capped per run, so a large backlog drains
//! gradually rather than bursting back into another denial.

use crate::metadata::orchestrator;
use crate::state::AppState;
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

/// Max runs re-enqueued per tick.
const RESUME_CAP: u64 = 50;

/// Re-enqueue due `awaiting_quota` runs. Returns the count resumed.
pub async fn run(state: &AppState) -> usize {
    use entity::metadata_run;

    // Budget pre-check — if no enabled provider has any remaining budget,
    // skip the whole tick (the buckets would just deny again).
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    if providers.is_empty() {
        return 0;
    }
    let mut has_budget = false;
    for p in &providers {
        match p.quota().await {
            Ok(snap) => {
                // `None` = unknown/unmetered → treat as available.
                let hour_ok = snap.remaining_hour.map(|n| n > 0).unwrap_or(true);
                let day_ok = snap.remaining_day.map(|n| n > 0).unwrap_or(true);
                if hour_ok && day_ok {
                    has_budget = true;
                    break;
                }
            }
            // Snapshot failed → be optimistic; the bucket still gates the call.
            Err(_) => {
                has_budget = true;
                break;
            }
        }
    }
    if !has_budget {
        return 0;
    }

    let now = Utc::now().fixed_offset();
    let due = metadata_run::Entity::find()
        .filter(metadata_run::Column::Status.eq(orchestrator::status::AWAITING_QUOTA))
        .filter(metadata_run::Column::ResumeAfter.lte(now))
        .order_by_asc(metadata_run::Column::ResumeAfter)
        .limit(RESUME_CAP)
        .all(&state.db)
        .await
        .unwrap_or_default();

    let mut resumed = 0usize;
    for parked in due {
        let kind = match parked.trigger_kind.as_str() {
            "weekly_refresh" => orchestrator::trigger_kind::WEEKLY_REFRESH,
            "scanner" => orchestrator::trigger_kind::SCANNER,
            "bulk_action" => orchestrator::trigger_kind::BULK_ACTION,
            _ => orchestrator::trigger_kind::MANUAL,
        };
        let outcome = match parked.scope.as_str() {
            "series" => match parked
                .scope_entity_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                Some(series_id) => crate::jobs::metadata_search::enqueue_series_search(
                    state,
                    series_id,
                    parked.triggered_by,
                    kind,
                    parked.batch_id,
                )
                .await
                .ok(),
                None => None,
            },
            "issue" => match parked.scope_entity_id.clone() {
                Some(issue_id) => crate::jobs::metadata_search::enqueue_issue_search(
                    state,
                    &issue_id,
                    parked.triggered_by,
                    kind,
                    parked.batch_id,
                )
                .await
                .ok(),
                None => None,
            },
            _ => None,
        };

        if let Some(o) = outcome {
            // Drop the parked row so it doesn't double-count in batch
            // aggregates — the fresh run carries the same batch_id. Guard
            // against the (rare) coalesce-onto-self case.
            if o.run_id != parked.id {
                let _ = metadata_run::Entity::delete_by_id(parked.id)
                    .exec(&state.db)
                    .await;
            }
            resumed += 1;
        }
    }
    resumed
}
