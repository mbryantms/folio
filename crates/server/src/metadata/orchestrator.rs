//! Cross-provider search orchestration + run/candidate persistence.
//!
//! Functions here are the single audited surface that fans out a
//! search query across every enabled provider, scores results with
//! [`crate::metadata::matcher`], and writes both the `metadata_run`
//! row + per-candidate rows. The apalis SearchSeries / SearchIssue
//! jobs in [`crate::jobs::metadata_search`] call into this module —
//! the same entry points are reachable from any sync caller (the M5
//! bulk-refresh UI fan-out, a hypothetical CLI driver) without
//! re-implementing the lifecycle.
//!
//! Provider fan-out is sequential by design — the per-provider
//! velocity caps + Redis token buckets already throttle concurrent
//! calls within a process, and the per-provider quota is shared
//! across requests anyway. A parallel fan-out gains nothing on the
//! happy path and risks burst-deny on bucket exhaustion.
//!
//! The orchestrator never reaches into the matcher's score floors —
//! the operator-tunable `metadata.auto_apply_threshold` (M5 setting,
//! defaults to 95) is plumbed in as a parameter so the auto-apply
//! routing in M4 reads the same number the matcher used to bucket.

use crate::config::Config;
use crate::metadata::comicvine::ComicVineClient;
use crate::metadata::identifier::Source;
use crate::metadata::matcher::{
    self, Confidence, IssueQueryFacts, Score, SeriesQueryFacts,
};
use crate::metadata::metron::MetronClient;
use crate::metadata::provider::{
    IssueCandidate, IssueQuery, MetadataProvider, ProviderError, SeriesCandidate, SeriesQuery,
};
use chrono::Utc;
use entity::{metadata_run, metadata_run_candidate};
use redis::aio::ConnectionManager;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// ───────── status strings (single source of truth) ─────────

pub mod status {
    pub const QUEUED: &str = "queued";
    pub const SEARCHING: &str = "searching";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
    pub const AWAITING_QUOTA: &str = "awaiting_quota";
}

pub mod trigger_kind {
    pub const MANUAL: &str = "manual";
    pub const WEEKLY_REFRESH: &str = "weekly_refresh";
    pub const SCANNER: &str = "scanner";
    pub const BULK_ACTION: &str = "bulk_action";
}

pub mod scope {
    pub const SERIES: &str = "series";
    pub const ISSUE: &str = "issue";
    pub const LIBRARY: &str = "library";
    pub const BULK_REFRESH: &str = "bulk_refresh";
}

// ───────── provider factory ─────────

/// Build the configured providers in priority order. Providers whose
/// master toggle is off OR credentials are missing are skipped — the
/// orchestrator never speaks to a disabled provider.
///
/// Priority: Metron first (richer + native cross-source IDs), then
/// ComicVine. The M5 admin UI exposes a drag-reorder of this list;
/// for now the priority is hard-coded.
pub fn build_providers(
    cfg: &Config,
    redis: ConnectionManager,
) -> Vec<Arc<dyn MetadataProvider>> {
    let mut out: Vec<Arc<dyn MetadataProvider>> = Vec::new();

    let metron_user_set = cfg
        .metron_username
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let metron_pass_set = cfg
        .metron_password
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if cfg.metron_enabled && metron_user_set && metron_pass_set {
        let username = cfg.metron_username.clone().unwrap_or_default();
        let password = cfg.metron_password.clone().unwrap_or_default();
        out.push(Arc::new(MetronClient::new(&username, &password, redis.clone())));
    }

    let cv_key_set = cfg
        .comicvine_api_key
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if cfg.comicvine_enabled && cv_key_set {
        let key = cfg.comicvine_api_key.clone().unwrap_or_default();
        out.push(Arc::new(ComicVineClient::new(key, redis.clone())));
    }

    out
}

// ───────── stored query payload ─────────

/// What the polling endpoint renders ("Searching ‹Saga (2012)›
/// across ComicVine + Metron…") — serialized into
/// `metadata_run.query` at run start so the UI is independent of the
/// (mutable) source entity row.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredQuery {
    Series(SeriesQueryFacts),
    Issue(IssueQueryFacts),
}

// ───────── run lifecycle ─────────

#[derive(Clone, Debug)]
pub struct StartRunArgs<'a> {
    pub scope: &'static str,
    pub scope_entity_id: Option<String>,
    pub library_id: Option<Uuid>,
    pub triggered_by: Option<Uuid>,
    pub trigger_kind: &'static str,
    pub providers: &'a [Source],
    pub query: StoredQuery,
}

pub async fn start_run<C: ConnectionTrait>(db: &C, args: StartRunArgs<'_>) -> Result<Uuid, sea_orm::DbErr> {
    let id = Uuid::now_v7();
    let now = Utc::now();
    let query_json = serde_json::to_value(&args.query)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize query: {e}")))?;
    let am = metadata_run::ActiveModel {
        id: Set(id),
        scope: Set(args.scope.to_owned()),
        scope_entity_id: Set(args.scope_entity_id),
        library_id: Set(args.library_id),
        triggered_by: Set(args.triggered_by),
        trigger_kind: Set(args.trigger_kind.to_owned()),
        providers: Set(args.providers.iter().map(|p| p.as_str().to_owned()).collect()),
        status: Set(status::QUEUED.to_owned()),
        started_at: Set(now.into()),
        finished_at: Set(None),
        items_total: Set(0),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(Some(query_json)),
    };
    am.insert(db).await?;
    Ok(id)
}

pub async fn mark_searching<C: ConnectionTrait>(db: &C, run_id: Uuid) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::SEARCHING.to_owned());
    am.update(db).await?;
    Ok(())
}

pub async fn fail_run<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    error: &str,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::FAILED.to_owned());
    am.finished_at = Set(Some(Utc::now().into()));
    am.error_summary = Set(Some(error.to_owned()));
    am.update(db).await?;
    Ok(())
}

pub async fn mark_awaiting_quota<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    resume_after: chrono::DateTime<chrono::Utc>,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::AWAITING_QUOTA.to_owned());
    am.resume_after = Set(Some(resume_after.into()));
    am.update(db).await?;
    Ok(())
}

// ───────── ranked candidate ─────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidatePayload {
    Series(SeriesCandidate),
    Issue(IssueCandidate),
}

#[derive(Clone, Debug)]
pub struct RankedCandidate {
    pub source: Source,
    pub external_id: String,
    pub score: Score,
    pub bucket: Confidence,
    pub payload: CandidatePayload,
}

impl RankedCandidate {
    fn score_breakdown_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.score.name,
            "year": self.score.year,
            "publisher": self.score.publisher,
            "issue_number": self.score.issue_number,
            "volume": self.score.volume,
        })
    }
}

/// Finalize a run by persisting the ranked candidates + flipping the
/// run row to `completed`. Single transaction so a partial write
/// can't leave the run in `searching` with half its candidates.
pub async fn finalize_run(
    db: &DatabaseConnection,
    run_id: Uuid,
    ranked: &[RankedCandidate],
) -> Result<(), sea_orm::DbErr> {
    let tx = db.begin().await?;
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(&tx).await? else {
        tx.rollback().await?;
        return Ok(());
    };
    let mut high = 0;
    let mut medium = 0;
    let mut low = 0;
    for (i, r) in ranked.iter().enumerate() {
        match r.bucket {
            Confidence::High => high += 1,
            Confidence::Medium => medium += 1,
            Confidence::Low => low += 1,
        }
        let payload_json = serde_json::to_value(&r.payload)
            .map_err(|e| sea_orm::DbErr::Custom(format!("serialize candidate: {e}")))?;
        let am = metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(i as i32),
            source: Set(r.source.as_str().to_owned()),
            external_id: Set(r.external_id.clone()),
            bucket: Set(r.bucket.as_str().to_owned()),
            score: Set(r.score.total),
            score_breakdown: Set(r.score_breakdown_json()),
            candidate: Set(payload_json),
            applied_at: Set(None),
            dismissed_at: Set(None),
        };
        am.insert(&tx).await?;
    }
    let total = ranked.len() as i32;
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::COMPLETED.to_owned());
    am.finished_at = Set(Some(Utc::now().into()));
    am.items_total = Set(total);
    am.items_matched_high = Set(high);
    am.items_matched_medium = Set(medium);
    am.items_matched_low = Set(low);
    am.items_no_match = Set(if total == 0 { 1 } else { 0 });
    am.update(&tx).await?;
    tx.commit().await?;
    Ok(())
}

// ───────── search execution ─────────

const SEARCH_LIMIT_PER_PROVIDER: u32 = 25;

/// Run a series search across `providers`, score with the matcher,
/// rank, and finalize the run. Returns the ranked list (also
/// persisted to `metadata_run_candidate`).
pub async fn run_series_search(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &SeriesQueryFacts,
    high_threshold: f32,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    if let Err(e) = mark_searching(db, run_id).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }

    let mut ranked = Vec::new();
    let mut surfaced_quota: Option<u64> = None;
    let mut last_error: Option<ProviderError> = None;
    for p in providers {
        let q = SeriesQuery {
            name: facts.name.clone(),
            year: facts.year,
            publisher: facts.publisher.clone(),
            limit: SEARCH_LIMIT_PER_PROVIDER,
        };
        match p.search_series(&q).await {
            Ok(candidates) => {
                for c in candidates {
                    let score = matcher::score_series(facts, &c);
                    let bucket = score.bucket(high_threshold);
                    ranked.push(RankedCandidate {
                        source: c.source,
                        external_id: c.external_id.clone(),
                        score,
                        bucket,
                        payload: CandidatePayload::Series(c),
                    });
                }
            }
            Err(ProviderError::QuotaExceeded { retry_after_secs }) => {
                surfaced_quota = Some(retry_after_secs);
                tracing::info!(
                    provider = p.id().as_str(),
                    retry_after_secs,
                    "metadata search: provider out of quota; falling through"
                );
            }
            Err(e) => {
                tracing::warn!(
                    provider = p.id().as_str(),
                    error = %e,
                    "metadata search: provider returned error; falling through"
                );
                last_error = Some(e);
            }
        }
    }

    ranked.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // If *every* enabled provider was quota-exhausted, surface that
    // as `awaiting_quota` instead of `completed-with-no-results` so
    // the M5 UI can render the right state + the operator dashboard
    // can flag the budget pressure.
    if ranked.is_empty() && surfaced_quota.is_some() {
        let resume = Utc::now() + chrono::Duration::seconds(surfaced_quota.unwrap_or(60) as i64);
        if let Err(e) = mark_awaiting_quota(db, run_id, resume).await {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        return Err(ProviderError::QuotaExceeded {
            retry_after_secs: surfaced_quota.unwrap_or(60),
        });
    }

    // If every provider returned a hard error AND we got zero
    // candidates, fail the run loudly. A single provider failing
    // while the other succeeds finalizes normally.
    if ranked.is_empty()
        && let Some(err) = last_error
    {
        if let Err(e) = fail_run(db, run_id, &err.to_string()).await {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        return Err(err);
    }

    if let Err(e) = finalize_run(db, run_id, &ranked).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }
    Ok(ranked)
}

/// Run an issue search across `providers`. Same shape as
/// [`run_series_search`]; the issue-specific bits live in
/// [`matcher::score_issue`].
pub async fn run_issue_search(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &IssueQueryFacts,
    series_external_id_by_provider: &[(Source, String)],
    high_threshold: f32,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    if let Err(e) = mark_searching(db, run_id).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }

    let mut ranked = Vec::new();
    let mut surfaced_quota: Option<u64> = None;
    let mut last_error: Option<ProviderError> = None;
    for p in providers {
        // If we already know the provider's series id (because the
        // series was previously matched), narrow the query — saves a
        // budget slot vs the keyword search.
        let series_external_id = series_external_id_by_provider
            .iter()
            .find(|(s, _)| *s == p.id())
            .map(|(_, id)| id.clone());
        let q = IssueQuery {
            series_external_id,
            series_name: Some(facts.series_name.clone()),
            series_year: facts.series_year,
            issue_number: facts.issue_number.clone(),
            cover_year: facts.series_year,
            limit: SEARCH_LIMIT_PER_PROVIDER,
        };
        match p.search_issue(&q).await {
            Ok(candidates) => {
                for c in candidates {
                    let score = matcher::score_issue(facts, &c);
                    let bucket = score.bucket(high_threshold);
                    ranked.push(RankedCandidate {
                        source: c.source,
                        external_id: c.external_id.clone(),
                        score,
                        bucket,
                        payload: CandidatePayload::Issue(c),
                    });
                }
            }
            Err(ProviderError::QuotaExceeded { retry_after_secs }) => {
                surfaced_quota = Some(retry_after_secs);
                tracing::info!(
                    provider = p.id().as_str(),
                    retry_after_secs,
                    "metadata search: provider out of quota; falling through"
                );
            }
            Err(e) => {
                tracing::warn!(
                    provider = p.id().as_str(),
                    error = %e,
                    "metadata search: provider returned error; falling through"
                );
                last_error = Some(e);
            }
        }
    }

    ranked.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if ranked.is_empty() && surfaced_quota.is_some() {
        let resume = Utc::now() + chrono::Duration::seconds(surfaced_quota.unwrap_or(60) as i64);
        if let Err(e) = mark_awaiting_quota(db, run_id, resume).await {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        return Err(ProviderError::QuotaExceeded {
            retry_after_secs: surfaced_quota.unwrap_or(60),
        });
    }
    if ranked.is_empty()
        && let Some(err) = last_error
    {
        if let Err(e) = fail_run(db, run_id, &err.to_string()).await {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        return Err(err);
    }

    if let Err(e) = finalize_run(db, run_id, &ranked).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }
    Ok(ranked)
}

// ───────── read API for the polling endpoint ─────────

pub async fn fetch_run<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
) -> Result<Option<metadata_run::Model>, sea_orm::DbErr> {
    metadata_run::Entity::find_by_id(run_id).one(db).await
}

pub async fn fetch_candidates<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
) -> Result<Vec<metadata_run_candidate::Model>, sea_orm::DbErr> {
    metadata_run_candidate::Entity::find()
        .filter(metadata_run_candidate::Column::RunId.eq(run_id))
        .order_by_asc(metadata_run_candidate::Column::Ordinal)
        .all(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_query_round_trips() {
        let series = StoredQuery::Series(SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: Some("Image".into()),
            volume: None,
        });
        let j = serde_json::to_value(&series).unwrap();
        let back: StoredQuery = serde_json::from_value(j).unwrap();
        match back {
            StoredQuery::Series(f) => {
                assert_eq!(f.name, "Saga");
                assert_eq!(f.year, Some(2012));
            }
            _ => panic!("wrong variant"),
        }
    }
}
