//! Auto-resume parked metadata runs (refine-bulk-metadata M5, reworked for
//! the provider-complete search).
//!
//! A run parks at `status='awaiting_quota'` whenever **any** provider it
//! owes a query was denied by its local quota bucket. The candidates the
//! other providers already produced are stashed on the run
//! (`metadata_run.partial_results`, see
//! [`crate::metadata::provider_status`]). This once-a-minute scheduler tick
//! picks up runs whose `resume_after` has passed, checks that every provider
//! the run still owes has budget again, and re-queues the run's own stashed
//! search job on the **same** run id — the orchestrator then asks only the
//! owed providers and merges into the stash. Nothing is re-spent on the
//! providers that already answered.
//!
//! Pacing: a run is resumed only when all of its owed providers have
//! budget (so it can't bounce straight back into a denial on one of them),
//! and the tick is capped so a large backlog drains gradually.

use crate::jobs::metadata_search::{ResumeOutcome, resume_parked_run};
use crate::metadata::identifier::Source;
use crate::metadata::orchestrator;
use crate::metadata::provider_status;
use crate::state::AppState;
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use std::collections::HashMap;

/// Max runs re-enqueued per tick.
const RESUME_CAP: u64 = 50;

/// Re-enqueue due `awaiting_quota` runs whose owed providers have budget.
/// Returns the count resumed.
pub async fn run(state: &AppState) -> usize {
    use entity::metadata_run;

    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    if providers.is_empty() {
        return 0;
    }
    // One quota snapshot per provider per tick. `None` for a window means
    // unknown/unmetered → available; a failed snapshot is treated as
    // available too (the bucket still gates the actual call).
    let mut has_budget: HashMap<Source, bool> = HashMap::new();
    for p in &providers {
        let ok = match p.quota().await {
            Ok(snap) => {
                snap.remaining_hour.map(|n| n > 0).unwrap_or(true)
                    && snap.remaining_day.map(|n| n > 0).unwrap_or(true)
            }
            Err(_) => true,
        };
        has_budget.insert(p.id(), ok);
    }
    if !has_budget.values().any(|ok| *ok) {
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
        // Every provider this run still owes must have budget; a provider
        // no longer configured can't be asked and doesn't block the rest.
        let owed = provider_status::owed_sources(&provider_status::for_run(&parked));
        let ready = owed
            .iter()
            .all(|s| has_budget.get(s).copied().unwrap_or(true));
        if !ready {
            continue;
        }
        match resume_parked_run(state, &parked).await {
            Ok(ResumeOutcome::Resumed | ResumeOutcome::Replaced(_)) => resumed += 1,
            Ok(ResumeOutcome::InFlight) => {}
            Err(e) => {
                tracing::warn!(run_id = %parked.id, error = %e, "metadata resume: re-enqueue failed");
            }
        }
    }
    resumed
}
