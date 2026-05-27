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
    self, Confidence, IssueQueryFacts, Score, SeriesQueryFacts, Thresholds,
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
pub fn build_providers(cfg: &Config, redis: ConnectionManager) -> Vec<Arc<dyn MetadataProvider>> {
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
        out.push(Arc::new(MetronClient::new(
            &username,
            &password,
            redis.clone(),
        )));
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

pub async fn start_run<C: ConnectionTrait>(
    db: &C,
    args: StartRunArgs<'_>,
) -> Result<Uuid, sea_orm::DbErr> {
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
        providers: Set(args
            .providers
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect()),
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

pub async fn mark_searching<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
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
            // M4: surface the raw Hamming so the review-UI tooltip can
            // explain "cover within 6 bits → HIGH" or "cover 24 bits
            // off → LOW". null when no phash was available.
            "cover_hamming": self.score.cover_hamming,
            // M5: flag whether the winning cover came from a variant.
            // Drives the dialog's "via alternate cover" badge.
            "matched_via_alternate": self.score.matched_via_alternate,
        })
    }
}

/// Apply matching-accuracy-1.0 M4's post-scoring ranking pass:
///
/// 1. **Gap-to-next-best guard**: When the two closest cover-Hamming
///    candidates are within [`matcher::MIN_SCORE_DISTANCE`] bits of
///    each other AND the winner is currently HIGH, downgrade the
///    winner to MEDIUM. Mirrors ComicTagger's `min_score_distance`
///    safeguard — when two real candidates have near-identical
///    covers we can't be confident which is right, so the user picks
///    explicitly instead of getting a one-click apply.
/// 2. **Final sort** orders by bucket priority (HIGH first), then
///    by cover Hamming ascending (lower = better match), then by
///    text `total` descending. Pre-M4 the sort was text-only — that
///    fought the cover-decides bucketing whenever a perfect-text +
///    wrong-cover candidate would rank above a worse-text +
///    perfect-cover one.
pub(crate) fn finalize_ranking(ranked: &mut [RankedCandidate]) {
    // Step 1: gap-to-next-best guard. Indexed walk so we can mutate
    // `ranked[i0].bucket` without holding an aliasing reference.
    let mut hamming_indices: Vec<usize> = (0..ranked.len())
        .filter(|&i| ranked[i].score.cover_hamming.is_some())
        .collect();
    hamming_indices.sort_by_key(|&i| ranked[i].score.cover_hamming.unwrap());
    if let (Some(&i0), Some(&i1)) = (hamming_indices.first(), hamming_indices.get(1)) {
        let d0 = ranked[i0].score.cover_hamming.unwrap();
        let d1 = ranked[i1].score.cover_hamming.unwrap();
        if ranked[i0].bucket == Confidence::High
            && d0 <= matcher::STRONG_SCORE_THRESH
            && d1.saturating_sub(d0) < matcher::MIN_SCORE_DISTANCE
        {
            tracing::debug!(
                top_hamming = d0,
                second_hamming = d1,
                "matcher gap-to-next-best: downgrading HIGH → MEDIUM (gap < 4 bits)",
            );
            ranked[i0].bucket = Confidence::Medium;
        }
    }

    // Step 2: bucket priority asc → Hamming asc (None last) → total desc.
    ranked.sort_by(|a, b| {
        let bucket_order = |c: Confidence| -> u8 {
            match c {
                Confidence::High => 0,
                Confidence::Medium => 1,
                Confidence::Low => 2,
            }
        };
        bucket_order(a.bucket)
            .cmp(&bucket_order(b.bucket))
            .then_with(|| {
                let ka = a.score.cover_hamming.unwrap_or(u32::MAX);
                let kb = b.score.cover_hamming.unwrap_or(u32::MAX);
                ka.cmp(&kb)
            })
            .then_with(|| {
                b.score
                    .total
                    .partial_cmp(&a.score.total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
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
    let scope = row.scope.clone();
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

    // Matching-accuracy-1.0 M0: stamp one outcome row alongside the
    // candidates so the dashboard can render rolling distribution
    // before any matcher tuning ships. Same transaction so the row +
    // candidates land atomically.
    crate::metadata::match_outcome::record(&tx, run_id, &scope, ranked).await?;

    tx.commit().await?;
    Ok(())
}

// ───────── pre-filter (matching-accuracy-1.0 M3) ─────────

/// Per-search filter that drops provider candidates **before** they
/// reach the scorer. Two signals:
///
/// 1. **Hard year gate** — implicit, runs whenever both
///    `facts.year` (or `facts.series_year` for issue queries) and
///    the candidate's start year are present. Drops candidates whose
///    `start_year > comic_year + 1`. Pre-M3 these scored Medium
///    because the year weight gave them partial credit on the
///    component sum; the gate now removes them outright so they
///    never compete for the top slot.
///
/// 2. **Publisher blacklist** — operator-tunable list per library
///    (`library.metadata_publisher_blacklist`). Compared
///    case-insensitively against the candidate publisher after
///    running both through [`crate::metadata::title_norm::sanitize_title`],
///    so `"DC Comics"` / `"dc comics"` / `"DC"` all match the same
///    entry.
#[derive(Clone, Debug, Default)]
pub struct PreFilter {
    pub publisher_blacklist: Vec<String>,
}

impl PreFilter {
    /// Build a `PreFilter` from a `library` row. Tolerant of bad
    /// JSON shape (returns an empty blacklist) — the column type is
    /// `JSONB NOT NULL DEFAULT '[]'` so the only way to land here
    /// with a non-array is operator-written garbage, which we soft-
    /// fail on with a debug log.
    pub fn from_library(library: &entity::library::Model) -> Self {
        let publisher_blacklist = library
            .metadata_publisher_blacklist
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            publisher_blacklist,
        }
    }
}

/// Apply the M3 pre-filter to a series-search result set. Public to
/// the crate so the orchestrator can drive it; tests in this module
/// pin the behavior.
pub(crate) fn pre_filter_series(
    candidates: Vec<SeriesCandidate>,
    facts: &SeriesQueryFacts,
    filter: &PreFilter,
) -> Vec<SeriesCandidate> {
    let blacklist_keys: Vec<String> = filter
        .publisher_blacklist
        .iter()
        .map(|s| crate::metadata::title_norm::sanitize_title(s))
        .filter(|s| !s.is_empty())
        .collect();
    candidates
        .into_iter()
        .filter(|c| {
            if let (Some(local), Some(cand)) = (facts.year, c.year)
                && cand > local + 1
            {
                return false;
            }
            if let Some(pub_name) = c.publisher.as_deref() {
                let canonical = crate::metadata::title_norm::sanitize_title(pub_name);
                if !canonical.is_empty() && blacklist_keys.iter().any(|k| k == &canonical) {
                    return false;
                }
            }
            true
        })
        .collect()
}

/// Apply the M3 pre-filter to an issue-search result set. Today this
/// only fires the year gate — `IssueCandidate` doesn't carry the
/// publisher, so the operator's blacklist is enforced at the
/// upstream series search instead.
pub(crate) fn pre_filter_issue(
    candidates: Vec<IssueCandidate>,
    facts: &IssueQueryFacts,
) -> Vec<IssueCandidate> {
    candidates
        .into_iter()
        .filter(|c| {
            if let (Some(local), Some(cand)) = (facts.series_year, c.series_year)
                && cand > local + 1
            {
                return false;
            }
            true
        })
        .collect()
}

// ───────── search execution ─────────

const SEARCH_LIMIT_PER_PROVIDER: u32 = 25;

/// Timeout for the per-candidate cover-image phash fetch.
/// Aggressive on purpose — covers are small + CDN-cached upstream,
/// and a slow CDN shouldn't stall the whole search-ranking pass.
const COVER_PHASH_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Run a series search across `providers`, score with the matcher,
/// rank, and finalize the run. Returns the ranked list (also
/// persisted to `metadata_run_candidate`).
///
/// When `local_series_id` is `Some`, the orchestrator looks up the
/// series's representative cover phash + fetches every candidate's
/// cover URL in parallel + hashes them, feeding the (local,
/// candidate) phash pair into [`matcher::score_series_with_phash`]
/// so cover-image similarity contributes to the rank. Pass `None`
/// to disable (tests + the future cross-library bulk-refresh path
/// that doesn't yet thread a series id).
///
/// metadata-providers-1.0 M9.5.
// 8 args is borderline-noisy but every one is a distinct knob; the
// natural fix is a `MatchOpts` struct that bundles thresholds +
// pre_filter + alt_cap, tracked as a follow-up.
#[allow(clippy::too_many_arguments)]
pub async fn run_series_search(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &SeriesQueryFacts,
    thresholds: Thresholds,
    pre_filter: &PreFilter,
    alternate_cover_fetch_cap: u32,
    local_series_id: Option<Uuid>,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    if let Err(e) = mark_searching(db, run_id).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }

    // Pre-fetch the local phash once; missing is fine — the scorer
    // skips the phash bonus silently and we fall back to text-only.
    let local_phash = match local_series_id {
        Some(id) => crate::metadata::phash::series_representative_phash(db, id)
            .await
            .unwrap_or(None),
        None => None,
    };

    let mut ranked = Vec::new();
    let mut surfaced_quota: Option<u64> = None;
    let mut last_error: Option<ProviderError> = None;
    let http = reqwest::Client::new();
    for p in providers {
        let q = SeriesQuery {
            name: facts.name.clone(),
            year: facts.year,
            publisher: facts.publisher.clone(),
            limit: SEARCH_LIMIT_PER_PROVIDER,
        };
        match p.search_series(&q).await {
            Ok(candidates) => {
                // M3 pre-filter: drop candidates the operator's
                // library settings + the hard year gate would reject
                // before any phash fetching or scoring runs.
                let candidates = pre_filter_series(candidates, facts, pre_filter);
                // M5: build the [primary, alternates...] URL slice
                // per candidate so the matcher can pick the min
                // Hamming across variants. When local_phash is None
                // we skip the network entirely.
                let candidate_phashes: Vec<Vec<Option<i64>>> = if local_phash.is_some() {
                    let urls_per_candidate: Vec<Vec<Option<&str>>> = candidates
                        .iter()
                        .map(|c| {
                            cover_urls_for_candidate(
                                c.cover_image_url.as_deref(),
                                &c.alternate_cover_urls,
                                alternate_cover_fetch_cap,
                            )
                        })
                        .collect();
                    fetch_phashes_per_candidate(&http, &urls_per_candidate).await
                } else {
                    candidates.iter().map(|_| Vec::new()).collect()
                };
                for (c, cand_phashes) in candidates.into_iter().zip(candidate_phashes) {
                    let score =
                        matcher::score_series_with_phash(facts, &c, local_phash, &cand_phashes);
                    let bucket = score.bucket(thresholds);
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

    finalize_ranking(&mut ranked);

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
///
/// `local_issue_id` enables cover-phash scoring per M9.5 — pass
/// `None` to disable.
#[allow(clippy::too_many_arguments)]
pub async fn run_issue_search(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &IssueQueryFacts,
    series_external_id_by_provider: &[(Source, String)],
    thresholds: Thresholds,
    alternate_cover_fetch_cap: u32,
    local_issue_id: Option<&str>,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    if let Err(e) = mark_searching(db, run_id).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }

    let local_phash = match local_issue_id {
        Some(id) => crate::metadata::phash::issue_phash(db, id)
            .await
            .unwrap_or(None),
        None => None,
    };

    let mut ranked = Vec::new();
    let mut surfaced_quota: Option<u64> = None;
    let mut last_error: Option<ProviderError> = None;
    let http = reqwest::Client::new();
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
                // M3 pre-filter: hard year gate (issue candidates
                // don't carry publisher, so the operator's blacklist
                // is enforced at the series-search side instead).
                let candidates = pre_filter_issue(candidates, facts);
                let candidate_phashes: Vec<Vec<Option<i64>>> = if local_phash.is_some() {
                    let urls_per_candidate: Vec<Vec<Option<&str>>> = candidates
                        .iter()
                        .map(|c| {
                            cover_urls_for_candidate(
                                c.cover_image_url.as_deref(),
                                &c.alternate_cover_urls,
                                alternate_cover_fetch_cap,
                            )
                        })
                        .collect();
                    fetch_phashes_per_candidate(&http, &urls_per_candidate).await
                } else {
                    candidates.iter().map(|_| Vec::new()).collect()
                };
                for (c, cand_phashes) in candidates.into_iter().zip(candidate_phashes) {
                    let score =
                        matcher::score_issue_with_phash(facts, &c, local_phash, &cand_phashes);
                    let bucket = score.bucket(thresholds);
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

    finalize_ranking(&mut ranked);

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

// ───────── cover-phash helpers (M9.5 + M5) ─────────

/// Build the [primary, alternates...] URL slice the matcher consumes.
/// The first slot is **always** the primary (None when the candidate
/// has no cover URL); subsequent slots are alternates capped at
/// `cap`. Caller passes the resulting Vec through
/// [`fetch_phashes_per_candidate`] for parallel hashing.
///
/// Matching-accuracy-1.0 M5.
fn cover_urls_for_candidate<'a>(
    primary: Option<&'a str>,
    alternates: &'a [String],
    cap: u32,
) -> Vec<Option<&'a str>> {
    let mut out: Vec<Option<&'a str>> = Vec::with_capacity(1 + alternates.len().min(cap as usize));
    out.push(primary);
    for url in alternates.iter().take(cap as usize) {
        out.push(Some(url.as_str()));
    }
    out
}

/// Parallel-fetch + hash every URL across every candidate. Returns
/// one `Vec<Option<i64>>` per candidate, in the same order as the
/// input — slot 0 = primary phash, slots 1.. = alternate phashes,
/// each `None` when the URL was missing / timed out / failed to
/// decode. Per-request timeout: [`COVER_PHASH_FETCH_TIMEOUT`].
///
/// Matching-accuracy-1.0 M5. Pre-M5 the orchestrator fetched a
/// single phash per candidate via `fetch_candidate_phashes`; this
/// replaces that helper.
async fn fetch_phashes_per_candidate(
    http: &reqwest::Client,
    urls_per_candidate: &[Vec<Option<&str>>],
) -> Vec<Vec<Option<i64>>> {
    use futures::future::join_all;
    // Flatten into a single batch so all fetches run in one
    // join_all (vs nested join_all, which serializes batches).
    let mut offsets: Vec<usize> = Vec::with_capacity(urls_per_candidate.len() + 1);
    offsets.push(0);
    let mut flat: Vec<Option<&str>> = Vec::new();
    for batch in urls_per_candidate {
        flat.extend_from_slice(batch);
        offsets.push(flat.len());
    }
    let futures: Vec<_> = flat
        .iter()
        .map(|maybe_url| async move {
            match maybe_url {
                Some(url) => {
                    crate::metadata::phash::fetch_and_hash_cover(
                        http,
                        url,
                        COVER_PHASH_FETCH_TIMEOUT,
                    )
                    .await
                }
                None => None,
            }
        })
        .collect();
    let flat_hashes: Vec<Option<i64>> = join_all(futures).await;
    // Slice the flat result back into per-candidate Vecs.
    let mut out: Vec<Vec<Option<i64>>> = Vec::with_capacity(urls_per_candidate.len());
    for w in offsets.windows(2) {
        out.push(flat_hashes[w[0]..w[1]].to_vec());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::matcher::Thresholds;

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

    // ────────────────────────────────────────────────────────────
    // M4 — finalize_ranking gap-to-next-best guard + sort
    // ────────────────────────────────────────────────────────────

    fn fake_candidate(
        text_total: f32,
        cover_hamming: Option<u32>,
        external_id: &str,
    ) -> RankedCandidate {
        let score = Score {
            total: text_total,
            cover_hamming,
            ..Default::default()
        };
        let bucket = score.bucket(Thresholds::default());
        RankedCandidate {
            source: Source::ComicVine,
            external_id: external_id.into(),
            score,
            bucket,
            payload: CandidatePayload::Series(SeriesCandidate {
                source: Source::ComicVine,
                external_id: external_id.into(),
                external_url: None,
                name: external_id.into(),
                year: None,
                publisher: None,
                issue_count: None,
                cover_image_url: None,
                deck: None,
                alternate_cover_urls: Vec::new(),
            }),
        }
    }

    #[test]
    fn gap_guard_downgrades_winner_when_top_two_within_distance() {
        // Two candidates at Hamming 6 and 9 — gap = 3 < MIN_SCORE_DISTANCE.
        // Both individually would bucket HIGH (≤8 / ≤16), but the
        // winner downgrades to MEDIUM because we can't be confident
        // which one is right.
        let mut ranked = vec![
            fake_candidate(50.0, Some(6), "winner"),
            fake_candidate(40.0, Some(9), "runner_up"),
        ];
        finalize_ranking(&mut ranked);

        let winner = ranked.iter().find(|r| r.external_id == "winner").unwrap();
        assert_eq!(winner.bucket, Confidence::Medium);
        let runner_up = ranked
            .iter()
            .find(|r| r.external_id == "runner_up")
            .unwrap();
        // Runner-up at Hamming 9 stays Medium (9 ≤ MIN_SCORE_THRESH=16).
        assert_eq!(runner_up.bucket, Confidence::Medium);
    }

    #[test]
    fn gap_guard_keeps_winner_high_when_top_two_are_distant() {
        // Hamming 4 + 18 — gap = 14 ≥ MIN_SCORE_DISTANCE. Winner is
        // decisively the better match; stays HIGH.
        let mut ranked = vec![
            fake_candidate(50.0, Some(4), "winner"),
            fake_candidate(40.0, Some(18), "runner_up"),
        ];
        finalize_ranking(&mut ranked);

        assert_eq!(ranked[0].external_id, "winner");
        assert_eq!(ranked[0].bucket, Confidence::High);
        // Runner-up at Hamming 18 > MIN_SCORE_THRESH = LOW.
        assert_eq!(ranked[1].external_id, "runner_up");
        assert_eq!(ranked[1].bucket, Confidence::Low);
    }

    #[test]
    fn sort_prefers_cover_match_over_perfect_text() {
        // Candidate A: perfect text (90), no cover.
        // Candidate B: low text (40), perfect cover (Hamming 0).
        // M4 invariant: cover-match wins the top slot.
        let mut ranked = vec![
            fake_candidate(90.0, None, "text_only"),
            fake_candidate(40.0, Some(0), "cover_match"),
        ];
        finalize_ranking(&mut ranked);

        assert_eq!(ranked[0].external_id, "cover_match");
        assert_eq!(ranked[0].bucket, Confidence::High);
        assert_eq!(ranked[1].external_id, "text_only");
        assert_eq!(ranked[1].bucket, Confidence::High); // 90 ≥ 80 text-only HIGH
    }

    #[test]
    fn finalize_ranking_noop_on_empty_or_single() {
        let mut empty: Vec<RankedCandidate> = vec![];
        finalize_ranking(&mut empty);
        assert!(empty.is_empty());

        let mut one = vec![fake_candidate(50.0, Some(4), "only")];
        finalize_ranking(&mut one);
        assert_eq!(one.len(), 1);
        // Single candidate at Hamming 4 stays HIGH — gap guard requires
        // a runner-up to fire.
        assert_eq!(one[0].bucket, Confidence::High);
    }

    // ────────────────────────────────────────────────────────────
    // M3 — pre-filter: hard year gate + publisher blacklist
    // ────────────────────────────────────────────────────────────

    fn series_cand(ext_id: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesCandidate {
        SeriesCandidate {
            source: Source::ComicVine,
            external_id: ext_id.into(),
            external_url: None,
            name: ext_id.into(),
            year,
            publisher: publisher.map(str::to_owned),
            issue_count: None,
            cover_image_url: None,
            deck: None,
            alternate_cover_urls: Vec::new(),
        }
    }

    fn series_facts(year: Option<i32>) -> SeriesQueryFacts {
        SeriesQueryFacts {
            name: "Saga".into(),
            year,
            publisher: None,
            volume: None,
        }
    }

    #[test]
    fn pre_filter_drops_year_too_far_in_future() {
        // local = 2012, candidate start_year = 2018 → 6 years past →
        // dropped. Pre-M3 this scored Medium (year=0 partial credit
        // didn't sink the score below threshold).
        let facts = series_facts(Some(2012));
        let candidates = vec![
            series_cand("keep", Some(2012), None),
            series_cand("drop", Some(2018), None),
        ];
        let out = pre_filter_series(candidates, &facts, &PreFilter::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "keep");
    }

    #[test]
    fn pre_filter_year_gate_allows_plus_one() {
        // ComicTagger's hard year gate is `cand > local + 1`, so
        // local=2012 / cand=2013 stays. Mylar-style "release a year
        // later than announced" doesn't get filtered.
        let facts = series_facts(Some(2012));
        let candidates = vec![series_cand("plus_one", Some(2013), None)];
        let out = pre_filter_series(candidates, &facts, &PreFilter::default());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn pre_filter_year_gate_inactive_when_local_year_unknown() {
        // No local year → gate doesn't fire (can't compute a delta).
        let facts = series_facts(None);
        let candidates = vec![series_cand("future", Some(2099), None)];
        let out = pre_filter_series(candidates, &facts, &PreFilter::default());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn pre_filter_drops_blacklisted_publisher() {
        let facts = series_facts(Some(2012));
        let filter = PreFilter {
            publisher_blacklist: vec!["DC Comics".into()],
        };
        let candidates = vec![
            series_cand("image", Some(2012), Some("Image Comics")),
            series_cand("dc", Some(2012), Some("DC Comics")),
        ];
        let out = pre_filter_series(candidates, &facts, &filter);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "image");
    }

    #[test]
    fn pre_filter_blacklist_is_case_insensitive_and_sanitized() {
        let facts = series_facts(Some(2012));
        // Operator wrote "DC". Candidate publisher is "dc comics".
        // After sanitize_title both keys reduce to substrings, but
        // exact key equality is what we compare — "dc" vs "dc comics"
        // are NOT equal. Operator must list the full canonical form.
        // Confirm the asymmetry so we don't accidentally over-match.
        let filter_partial = PreFilter {
            publisher_blacklist: vec!["DC".into()],
        };
        let candidates = vec![series_cand("dc", Some(2012), Some("DC Comics"))];
        let out = pre_filter_series(candidates.clone(), &facts, &filter_partial);
        assert_eq!(out.len(), 1, "partial-key shouldn't accidentally match");

        // Same publisher, blacklist with mismatched casing → match.
        let filter_full = PreFilter {
            publisher_blacklist: vec!["dc comics".into()],
        };
        let out = pre_filter_series(candidates, &facts, &filter_full);
        assert_eq!(out.len(), 0, "lowercase blacklist matches sanitized form");
    }

    // ────────────────────────────────────────────────────────────
    // M5 — cover-urls cap helper
    // ────────────────────────────────────────────────────────────

    #[test]
    fn cover_urls_includes_primary_and_caps_alternates() {
        let alts: Vec<String> = (0..5).map(|i| format!("alt-{i}")).collect();
        // cap=3 → primary + 3 alternates = 4 slots.
        let urls = cover_urls_for_candidate(Some("primary"), &alts, 3);
        assert_eq!(urls.len(), 4);
        assert_eq!(urls[0], Some("primary"));
        assert_eq!(urls[1], Some("alt-0"));
        assert_eq!(urls[2], Some("alt-1"));
        assert_eq!(urls[3], Some("alt-2"));
    }

    #[test]
    fn cover_urls_cap_zero_emits_primary_only() {
        let alts: Vec<String> = vec!["alt".into()];
        let urls = cover_urls_for_candidate(Some("primary"), &alts, 0);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], Some("primary"));
    }

    #[test]
    fn cover_urls_primary_none_preserves_slot() {
        // When the candidate has no primary cover, slot 0 is None
        // so the matcher's index-0-is-primary convention stays
        // intact — phash[0] simply ends up None.
        let alts: Vec<String> = vec!["alt-a".into(), "alt-b".into()];
        let urls = cover_urls_for_candidate(None, &alts, 3);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], None);
        assert_eq!(urls[1], Some("alt-a"));
        assert_eq!(urls[2], Some("alt-b"));
    }

    #[test]
    fn pre_filter_issue_runs_year_gate_only() {
        let facts = IssueQueryFacts {
            series_name: "Saga".into(),
            series_year: Some(2012),
            publisher: None,
            volume: None,
            issue_number: "1".into(),
        };
        let candidates = vec![
            IssueCandidate {
                source: Source::ComicVine,
                external_id: "keep".into(),
                external_url: None,
                issue_number: Some("1".into()),
                name: None,
                cover_date: None,
                series_name: Some("Saga".into()),
                series_year: Some(2012),
                series_external_id: None,
                cover_image_url: None,
                alternate_cover_urls: Vec::new(),
            },
            IssueCandidate {
                source: Source::ComicVine,
                external_id: "drop_future".into(),
                external_url: None,
                issue_number: Some("1".into()),
                name: None,
                cover_date: None,
                series_name: Some("Saga".into()),
                series_year: Some(2099),
                series_external_id: None,
                cover_image_url: None,
                alternate_cover_urls: Vec::new(),
            },
        ];
        let out = pre_filter_issue(candidates, &facts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "keep");
    }
}
