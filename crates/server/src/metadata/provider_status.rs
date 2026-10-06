//! Per-provider search bookkeeping — the provider-complete search.
//!
//! A metadata search asks every configured provider. Before this module a
//! provider whose local quota bucket was empty was simply skipped: the run
//! finalized with whatever the others returned, nothing recorded the gap,
//! and a strong single-provider match could auto-apply with one provider's
//! ids. Now each run carries one [`ProviderStatus`] per provider:
//!
//! - `pending` — not asked yet (the run was just started / queued).
//! - `answered` — searched; `candidates` is how many it produced (`0` is a
//!   genuine "no match", not a gap).
//! - `quota` — the local bucket denied the call. The provider is *owed* a
//!   query: the run parks `awaiting_quota` with the candidates the other
//!   providers produced stashed in [`PartialSearch`], and the resume sweep
//!   ([`crate::jobs::metadata_resume`]) re-runs only the owed providers on
//!   the same run once their buckets refill.
//! - `failed` — a hard provider error (transport / 5xx). Not retried; the
//!   run finalizes and the gap is flagged (the auto-apply gate refuses a
//!   run that isn't fully answered, so the operator sees it in Review).
//!
//! `metadata_run.provider_status` survives finalize so the Review queue and
//! the match dialog can say whether a match covers every provider.

use crate::metadata::direct_lookup::SourceLookup;
use crate::metadata::identifier::Source;
use crate::metadata::orchestrator::RankedCandidate;
use entity::metadata_run;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    Pending,
    Answered,
    Quota,
    Failed,
}

/// One provider's state within a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderStatus {
    /// `"comicvine"` | `"metron"` | `"gcd"`.
    #[schema(value_type = String)]
    pub source: Source,
    pub state: ProviderState,
    /// Candidates this provider contributed (after pre-filter + scoring).
    /// Meaningful once `answered`.
    #[serde(default)]
    pub candidates: u32,
    /// `quota` only: the bucket's suggested wait when it denied the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    /// `failed` only: the provider error, for the run detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProviderStatus {
    pub fn pending(source: Source) -> Self {
        Self {
            source,
            state: ProviderState::Pending,
            candidates: 0,
            retry_after_secs: None,
            error: None,
        }
    }

    /// Still owed a query: never asked, or denied by quota.
    pub fn is_owed(&self) -> bool {
        matches!(self.state, ProviderState::Pending | ProviderState::Quota)
    }

    pub fn answered(&mut self, candidates: usize) {
        self.state = ProviderState::Answered;
        self.candidates = candidates as u32;
        self.retry_after_secs = None;
        self.error = None;
    }

    pub fn quota_denied(&mut self, retry_after_secs: u64) {
        self.state = ProviderState::Quota;
        self.retry_after_secs = Some(retry_after_secs);
    }

    pub fn failed(&mut self, error: &str) {
        self.state = ProviderState::Failed;
        self.error = Some(error.to_owned());
    }
}

/// One `pending` entry per provider, in search order.
pub fn initial(providers: &[Source]) -> Vec<ProviderStatus> {
    providers
        .iter()
        .map(|s| ProviderStatus::pending(*s))
        .collect()
}

/// [`initial`] as the JSON the `metadata_run` row stores.
pub fn initial_status_json(providers: &[Source]) -> serde_json::Value {
    serde_json::to_value(initial(providers)).unwrap_or(serde_json::Value::Array(Vec::new()))
}

/// Parse a run's stored `provider_status`. `None` for rows that predate
/// the column (and lookup runs, which never ran the matcher).
pub fn parse(stored: Option<&serde_json::Value>) -> Option<Vec<ProviderStatus>> {
    stored.and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// A run's statuses, or — for a legacy row without them — the run's
/// provider list all `pending` (so an old parked run still resumes every
/// provider).
pub fn for_run(run: &metadata_run::Model) -> Vec<ProviderStatus> {
    parse(run.provider_status.as_ref()).unwrap_or_else(|| {
        run.providers
            .iter()
            .filter_map(|s| s.parse::<Source>().ok())
            .map(ProviderStatus::pending)
            .collect()
    })
}

/// Providers still owed a query (`pending` / `quota`).
pub fn owed_sources(statuses: &[ProviderStatus]) -> Vec<Source> {
    statuses
        .iter()
        .filter(|s| s.is_owed())
        .map(|s| s.source)
        .collect()
}

/// Every provider the run was started with answered. `false` when any is
/// owed or failed — the auto-apply gate's definition of "complete".
pub fn all_answered(statuses: &[ProviderStatus]) -> bool {
    !statuses.is_empty() && statuses.iter().all(|s| s.state == ProviderState::Answered)
}

/// Providers that answered with at least one candidate.
pub fn matched_sources(statuses: &[ProviderStatus]) -> Vec<Source> {
    statuses
        .iter()
        .filter(|s| s.state == ProviderState::Answered && s.candidates > 0)
        .map(|s| s.source)
        .collect()
}

/// What a parked (`awaiting_quota`) run has so far: the ranked candidates
/// the answering providers produced, the batch lookup notes, the year-gate
/// flag, and the search job itself so the resume re-runs the *same* query
/// (overrides, direct-lookup mode, series targets) on the owed providers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PartialSearch {
    pub ranked: Vec<RankedCandidate>,
    #[serde(default)]
    pub lookups: Vec<SourceLookup>,
    #[serde(default)]
    pub year_gate_relaxed: bool,
    /// The serialized `SearchSeriesJob` / `SearchIssueJob` (by run scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<serde_json::Value>,
}

/// The stash of a parked run, if any.
pub fn parse_partial(stored: Option<&serde_json::Value>) -> Option<PartialSearch> {
    stored.and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Park a run on quota: status `awaiting_quota`, the owed providers'
/// statuses, the stash, and when to try again.
pub async fn park_awaiting_quota<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    statuses: &[ProviderStatus],
    partial: &PartialSearch,
    resume_after: chrono::DateTime<chrono::Utc>,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let status_json = serde_json::to_value(statuses)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize provider_status: {e}")))?;
    let partial_json = serde_json::to_value(partial)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize partial_results: {e}")))?;
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(crate::metadata::orchestrator::status::AWAITING_QUOTA.to_owned());
    am.resume_after = Set(Some(resume_after.into()));
    am.provider_status = Set(Some(status_json));
    am.partial_results = Set(Some(partial_json));
    am.update(db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owed_and_complete_track_state() {
        let mut s = initial(&[Source::ComicVine, Source::Metron]);
        assert_eq!(owed_sources(&s), vec![Source::ComicVine, Source::Metron]);
        assert!(!all_answered(&s));
        s[0].answered(3);
        s[1].quota_denied(120);
        assert_eq!(owed_sources(&s), vec![Source::Metron]);
        assert!(!all_answered(&s));
        assert_eq!(matched_sources(&s), vec![Source::ComicVine]);
        s[1].answered(0);
        assert!(owed_sources(&s).is_empty());
        assert!(all_answered(&s));
        assert_eq!(matched_sources(&s), vec![Source::ComicVine]);
        s[1].failed("boom");
        assert!(!all_answered(&s));
        assert!(owed_sources(&s).is_empty());
    }

    #[test]
    fn json_round_trip_keeps_source_names() {
        let s = initial(&[Source::Gcd]);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v[0]["source"], "gcd");
        assert_eq!(v[0]["state"], "pending");
        let back = parse(Some(&v)).unwrap();
        assert_eq!(back, s);
        assert!(parse(None).is_none());
    }
}
