//! Entity profile fetches (`metadata::entity_profile`): one job per
//! character / team / location / story arc, queued lazily by the first
//! landing-page view or an admin's Refresh. Concurrency 1 — each job
//! spends the provider budgets (ComicVine's pooled `entities` resource,
//! Metron's per-minute bucket).

use apalis::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::metadata::entity_profile::{self, ProfileKind};
use crate::state::AppState;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityEnrichJob {
    pub kind: ProfileKind,
    pub entity_id: Uuid,
}

/// A profile whose fetch was queued longer ago than this and never
/// completed is queued again on the next page view (a crashed worker, a
/// provider that was out of quota).
pub const RETRY_AFTER_SECS: i64 = 30 * 60;

/// Stamp the profile row as requested and push the job. `force` clears a
/// recorded failure so the page reports "pending" instead of the old
/// error while the fetch runs.
pub async fn enqueue(
    state: &AppState,
    kind: ProfileKind,
    entity_id: Uuid,
    force: bool,
) -> anyhow::Result<()> {
    entity_profile::mark_requested(&state.db, kind.entity_type(), entity_id, force).await?;
    let mut storage = state.jobs.entity_enrich_storage.clone();
    storage
        .push(EntityEnrichJob { kind, entity_id })
        .await
        .map_err(|e| anyhow::anyhow!("enqueue entity enrich: {e}"))?;
    Ok(())
}

pub async fn handle(job: EntityEnrichJob, state: Data<AppState>) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();
    if let Err(e) = process(&state, job.kind, job.entity_id).await {
        // Recorded on the profile row where it can be; not retried (a
        // retry would spend the provider budget again for the same
        // failure — the next page view after RETRY_AFTER_SECS queues it).
        tracing::error!(
            entity_type = kind_str(job.kind),
            entity_id = %job.entity_id,
            error = %e,
            "entity enrich: job failed"
        );
    }
    Ok(())
}

fn kind_str(kind: ProfileKind) -> &'static str {
    kind.entity_type()
}

/// Run one fetch. Public so tests drive it without a worker.
pub async fn process(state: &AppState, kind: ProfileKind, entity_id: Uuid) -> anyhow::Result<()> {
    entity_profile::enrich(state, kind, entity_id).await
}
