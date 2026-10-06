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
use crate::metadata::direct_lookup::{
    CoverageMatch, DirectLookupCtx, DirectMode, FallbackReason, SourceLookup,
};
use crate::metadata::gcd::GcdClient;
use crate::metadata::identifier::Source;
use crate::metadata::matcher::{
    self, Confidence, IssueQueryFacts, Score, SeriesQueryFacts, Thresholds,
};
use crate::metadata::metron::MetronClient;
use crate::metadata::provider::{
    IssueCandidate, IssueQuery, MetadataProvider, ProviderError, SeriesCandidate, SeriesQuery,
};
use crate::metadata::provider_status::{self, PartialSearch, ProviderState, ProviderStatus};
use crate::metadata::range_map::EffectiveTarget;
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
/// ComicVine, then GCD (WP-6.1 — the tightest budget and slimmest
/// search payloads, so it runs last as the coverage backstop for
/// Golden/Silver Age and non-US runs). The M5 admin UI exposes a
/// drag-reorder of this list; for now the priority is hard-coded.
pub fn build_providers(cfg: &Config, redis: ConnectionManager) -> Vec<Arc<dyn MetadataProvider>> {
    let mut out: Vec<Arc<dyn MetadataProvider>> = Vec::new();

    // Token auth is preferred; username + password is the fallback
    // (`MetronAuth::from_config`). Either counts as "configured".
    if cfg.metron_enabled
        && let Some(client) = MetronClient::from_config(cfg, redis.clone())
    {
        out.push(Arc::new(client));
    }

    if comicvine_configured(cfg) {
        let key = cfg.comicvine_api_key.clone().unwrap_or_default();
        out.push(Arc::new(match cfg.comicvine_base_url.clone() {
            Some(base) => ComicVineClient::with_base_url(key, base, redis.clone()),
            None => ComicVineClient::new(key, redis.clone()),
        }));
    }

    if cfg.gcd_enabled
        && let Some(client) = GcdClient::from_config(cfg, redis.clone())
    {
        out.push(Arc::new(client));
    }

    out
}

fn comicvine_configured(cfg: &Config) -> bool {
    cfg.comicvine_enabled
        && cfg
            .comicvine_api_key
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
}

/// The ids [`build_providers`] would return, in the same order, without
/// building anything. Each provider owns a `reqwest` client (a rustls
/// config per build), so a caller that only needs to know *which*
/// providers are on — the per-issue enqueue, called once per issue in a
/// batch fan-out — must not construct them: three clients × a 200-issue
/// batch kept the request past the 60 s JSON timeout on a loaded host.
pub fn configured_provider_ids(cfg: &Config) -> Vec<Source> {
    let mut out = Vec::new();
    if cfg.metron_enabled && crate::metadata::metron::MetronAuth::from_config(cfg).is_some() {
        out.push(Source::Metron);
    }
    if comicvine_configured(cfg) {
        out.push(Source::ComicVine);
    }
    if cfg.gcd_enabled && crate::metadata::gcd::GcdCredentials::from_config(cfg).is_some() {
        out.push(Source::Gcd);
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
    /// Groups this run under a bulk-fetch `metadata_batch`. `None` for
    /// standalone per-entity runs.
    pub batch_id: Option<Uuid>,
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
        batch_id: Set(args.batch_id),
        // Every provider starts owed a query; the search loop flips each
        // entry as it answers / is denied (provider-complete search).
        provider_status: Set(Some(crate::metadata::provider_status::initial_status_json(
            args.providers,
        ))),
        partial_results: Set(None),
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

/// A parked run re-queued by the resume sweep: back to `queued` (so the
/// sweep doesn't re-queue it before the worker picks it up) with its
/// stash + statuses untouched.
pub async fn mark_resumed<C: ConnectionTrait>(db: &C, run_id: Uuid) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::QUEUED.to_owned());
    am.resume_after = Set(None);
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

/// Merge `patch`'s top-level keys into `metadata_run.query`. The
/// stored query starts life as the serialized [`StoredQuery`]
/// (`{"kind": "series", "name": …}`); the WP-2.8 surfaces layer small
/// notes on top of it so the Review queue / run detail can say what was
/// actually searched:
///
/// - `overrides: {name?, year?, publisher?, issue_number?}` — the
///   user-supplied query overrides (the facts themselves already hold
///   the effective values; this records *which* were overridden).
/// - `year_gate_relaxed: true` — the hard year gate emptied the list
///   and the orchestrator re-scored under the cover-pHash-aware gate.
/// - `lookup: {source, external_id, url?}` — the run was produced by a
///   direct provider lookup rather than a search.
///
/// A non-object stored query (legacy NULL) is replaced by `patch`.
pub async fn annotate_query<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    patch: serde_json::Value,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let mut merged = match row.query.clone() {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    if let serde_json::Value::Object(extra) = patch {
        merged.extend(extra);
    }
    let mut am: metadata_run::ActiveModel = row.into();
    am.query = Set(Some(serde_json::Value::Object(merged)));
    am.update(db).await?;
    Ok(())
}

/// Cover-hash resolver: candidate cover URL → 64-bit pHash (`None` on
/// any failure). Production resolves over the network through the
/// SSRF-guarded fetcher ([`crate::metadata::phash::fetch_and_hash_cover`]);
/// tests inject a lookup table so the cover-aware paths (M4 bucketing,
/// M5 alternates, the WP-2.8 relaxed year gate) are exercisable without
/// a publicly-routable image host.
pub type CoverHasher =
    Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, Option<i64>> + Send + Sync>;

/// Per-run search policy knobs that aren't matcher thresholds.
#[derive(Clone)]
pub struct SearchOpts {
    /// When the hard year gate leaves **zero** candidates, re-score the
    /// same provider results under [`YearGate::PhashAware`] (no extra
    /// provider call) and annotate the run with `year_gate_relaxed`.
    /// `false` when the user asserted the year via a query override —
    /// they told us the year, so a mismatch is a real mismatch.
    pub relax_year_gate: bool,
    /// `None` ⇒ fetch + hash candidate covers over the network.
    pub cover_hasher: Option<CoverHasher>,
    /// Answer a provider from its series' cached issue list + one cached
    /// detail fetch when the issue's provider series is known (see
    /// [`crate::metadata::direct_lookup`]); `DirectLookupCtx::mode` says
    /// whether the search is skipped (batches), still run (the match
    /// dialog) or never run (issue-level refresh). `None` ⇒ always search.
    pub direct: Option<DirectLookupCtx>,
    /// The serialized search job driving this run, stashed with the
    /// partial results when the run parks on quota so the resume sweep
    /// re-runs the *same* query (overrides, direct-lookup mode, series
    /// targets) on the owed providers. `None` for direct orchestrator
    /// callers (tests, lookups); a parked run then resumes from a fresh
    /// job built off the entity.
    pub job_payload: Option<serde_json::Value>,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            relax_year_gate: true,
            cover_hasher: None,
            direct: None,
            job_payload: None,
        }
    }
}

impl std::fmt::Debug for SearchOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchOpts")
            .field("relax_year_gate", &self.relax_year_gate)
            .field(
                "cover_hasher",
                &self.cover_hasher.as_ref().map(|_| "<injected>"),
            )
            .field("direct", &self.direct.is_some())
            .field("job_payload", &self.job_payload.is_some())
            .finish()
    }
}

// ───────── ranked candidate ─────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidatePayload {
    Series(SeriesCandidate),
    Issue(IssueCandidate),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub source: Source,
    pub external_id: String,
    pub score: Score,
    pub bucket: Confidence,
    pub payload: CandidatePayload,
    /// Set when a batch direct lookup produced this candidate from the
    /// provider series' issue list instead of a search. Informational —
    /// the bucket is still the matcher's.
    pub coverage: Option<CoverageMatch>,
}

impl RankedCandidate {
    fn score_breakdown_json(&self) -> serde_json::Value {
        let mut v = self.score_breakdown_base();
        // Batch direct lookup: which provider series' issue list supplied
        // this candidate and why (number + cover date). Absent otherwise.
        if let (Some(c), serde_json::Value::Object(m)) = (&self.coverage, &mut v)
            && let Ok(note) = serde_json::to_value(c)
        {
            m.insert("coverage".into(), note);
        }
        v
    }

    fn score_breakdown_base(&self) -> serde_json::Value {
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
            // WP-5.6: format penalty (0 or -FORMAT_MISMATCH_PENALTY)
            // + whether it capped the bucket at MEDIUM.
            "format": self.score.format,
            "format_mismatch": self.score.format_mismatch,
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
    statuses: Option<&[ProviderStatus]>,
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
    // Provider-complete search: the per-provider statuses survive
    // finalize (the Review queue flags a run some provider didn't
    // answer); the quota stash is spent.
    if let Some(statuses) = statuses {
        let status_json = serde_json::to_value(statuses)
            .map_err(|e| sea_orm::DbErr::Custom(format!("serialize provider_status: {e}")))?;
        am.provider_status = Set(Some(status_json));
    }
    am.partial_results = Set(None);
    am.update(&tx).await?;

    // Matching-accuracy-1.0 M0: stamp one outcome row alongside the
    // candidates so the dashboard can render rolling distribution
    // before any matcher tuning ships. Same transaction so the row +
    // candidates land atomically.
    crate::metadata::match_outcome::record(&tx, run_id, &scope, ranked).await?;

    tx.commit().await?;
    Ok(())
}

/// Finalize a **lookup** run (WP-2.8): persist exactly one candidate
/// that bypassed scoring — the record the user pointed at by URL / id —
/// as `bucket=high`, score 100, with `score_breakdown.lookup = true` so
/// the dialog tooltip can explain the score. Deliberately does **not**
/// write a `metadata_match_outcome` row: the matcher didn't run, so a
/// `single_good` outcome here would inflate the dashboard's accuracy
/// distribution.
pub async fn finalize_lookup_run(
    db: &DatabaseConnection,
    run_id: Uuid,
    candidate: &RankedCandidate,
) -> Result<(), sea_orm::DbErr> {
    let tx = db.begin().await?;
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(&tx).await? else {
        tx.rollback().await?;
        return Ok(());
    };
    let payload_json = serde_json::to_value(&candidate.payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize candidate: {e}")))?;
    let mut breakdown = candidate.score_breakdown_json();
    if let serde_json::Value::Object(m) = &mut breakdown {
        m.insert("lookup".into(), serde_json::Value::Bool(true));
    }
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set(candidate.source.as_str().to_owned()),
        external_id: Set(candidate.external_id.clone()),
        bucket: Set(Confidence::High.as_str().to_owned()),
        score: Set(candidate.score.total),
        score_breakdown: Set(breakdown),
        candidate: Set(payload_json),
        applied_at: Set(None),
    }
    .insert(&tx)
    .await?;
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::COMPLETED.to_owned());
    am.finished_at = Set(Some(Utc::now().into()));
    am.items_total = Set(1);
    am.items_matched_high = Set(1);
    am.items_matched_medium = Set(0);
    am.items_matched_low = Set(0);
    am.items_no_match = Set(0);
    am.update(&tx).await?;
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
    pre_filter_series_with_gate(candidates, facts.year, filter, true)
}

/// [`pre_filter_series`] with the year gate switchable. The publisher
/// blacklist always applies — it's operator policy, not a heuristic —
/// but the WP-2.8 relaxed retry re-runs the filter with `year_gate =
/// false` and lets [`score_series_candidates`] decide per candidate
/// whether the cover vouches for the year mismatch.
fn pre_filter_series_with_gate(
    candidates: Vec<SeriesCandidate>,
    local_year: Option<i32>,
    filter: &PreFilter,
    year_gate: bool,
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
            if year_gate && !year_ok(local_year, c.year) {
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
///
/// `local_year` is the baseline the candidate's series year is gated
/// against. It is normally the parent series start year, but for an
/// issue routed through a `series_provider_range` mapping it is the
/// mapped sub-series' `declared_year` — so a legacy-renumbered relaunch
/// (e.g. a 2012 sub-series of a 2001 run) isn't dropped against the
/// wrong baseline. See [`run_issue_search`].
pub(crate) fn pre_filter_issue(
    candidates: Vec<IssueCandidate>,
    local_year: Option<i32>,
) -> Vec<IssueCandidate> {
    candidates
        .into_iter()
        .filter(|c| year_ok(local_year, c.series_year))
        .collect()
}

/// The keep predicate behind the year gate (series + issue): a
/// candidate survives unless its (series) start year runs more than one
/// year past the local baseline. Missing on either side ⇒ keep (no
/// signal to gate on).
fn year_ok(local_year: Option<i32>, candidate_year: Option<i32>) -> bool {
    match (local_year, candidate_year) {
        (Some(local), Some(cand)) => cand <= local + 1,
        _ => true,
    }
}

/// Year-gate policy for [`score_issue_candidates`].
#[derive(Copy, Clone)]
enum YearGate {
    /// Drop a candidate before scoring when its series year exceeds the
    /// baseline + 1. The default for the narrowed primary search — keeps
    /// the matcher's golden behaviour intact.
    Hard(Option<i32>),
    /// Score first; keep a year-mismatched candidate only when its cover
    /// pHash confirms the match (cover present + buckets MEDIUM-or-better
    /// on the M4 Hamming ladder). Used on the broad fallback so a
    /// cover-confirmed relaunch survives the year gate.
    PhashAware(Option<i32>),
}

/// Series-shape sibling of [`score_issue_candidates`]: apply the
/// operator pre-filter (+ the hard year gate when `gate` is
/// [`YearGate::Hard`]), fetch cover pHashes, score, and — under
/// [`YearGate::PhashAware`] — keep a year-mismatched candidate only when
/// its cover confirms the match. Shared by the primary pass and the
/// WP-2.8 relaxed retry so the two can't drift.
#[allow(clippy::too_many_arguments)]
async fn score_series_candidates(
    db: &DatabaseConnection,
    http: &reqwest::Client,
    hasher: Option<&CoverHasher>,
    facts: &SeriesQueryFacts,
    candidates: Vec<SeriesCandidate>,
    pre_filter: &PreFilter,
    local_phash: Option<i64>,
    alternate_cover_fetch_cap: u32,
    thresholds: Thresholds,
    gate: YearGate,
) -> Vec<RankedCandidate> {
    let (year, hard) = match gate {
        YearGate::Hard(y) => (y, true),
        YearGate::PhashAware(y) => (y, false),
    };
    let candidates = if hard {
        pre_filter_series(candidates, facts, pre_filter)
    } else {
        pre_filter_series_with_gate(candidates, year, pre_filter, false)
    };
    if candidates.is_empty() {
        return Vec::new();
    }
    // M5: build the [primary, alternates...] URL slice per candidate so
    // the matcher can pick the min Hamming across variants. When
    // local_phash is None we skip the network entirely.
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
        fetch_phashes_per_candidate(db, http, hasher, &urls_per_candidate).await
    } else {
        candidates.iter().map(|_| Vec::new()).collect()
    };
    let mut out = Vec::new();
    for (c, cand_phashes) in candidates.into_iter().zip(candidate_phashes) {
        let score = matcher::score_series_with_phash(facts, &c, local_phash, &cand_phashes);
        let bucket = score.bucket(thresholds);
        if !hard && !year_ok(year, c.year) {
            let cover_confirmed =
                score.cover_hamming.is_some() && !matches!(bucket, Confidence::Low);
            if !cover_confirmed {
                continue;
            }
        }
        out.push(RankedCandidate {
            source: c.source,
            external_id: c.external_id.clone(),
            score,
            bucket,
            coverage: None,
            payload: CandidatePayload::Series(c),
        });
    }
    out
}

/// Fetch candidate cover pHashes, score each issue candidate against the
/// local facts, apply the year gate per `gate`, and return ranked rows.
/// Shared by the narrowed primary search and the broad fallback so the
/// phash-fetch + scoring logic lives in one place.
#[allow(clippy::too_many_arguments)]
async fn score_issue_candidates(
    db: &DatabaseConnection,
    http: &reqwest::Client,
    hasher: Option<&CoverHasher>,
    facts: &IssueQueryFacts,
    candidates: Vec<IssueCandidate>,
    local_phash: Option<i64>,
    alternate_cover_fetch_cap: u32,
    thresholds: Thresholds,
    gate: YearGate,
) -> Vec<RankedCandidate> {
    // The hard gate is cheapest applied before any cover fetch.
    let candidates = match gate {
        YearGate::Hard(year) => pre_filter_issue(candidates, year),
        YearGate::PhashAware(_) => candidates,
    };
    if candidates.is_empty() {
        return Vec::new();
    }
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
        fetch_phashes_per_candidate(db, http, hasher, &urls_per_candidate).await
    } else {
        candidates.iter().map(|_| Vec::new()).collect()
    };

    let mut out = Vec::new();
    for (c, cand_phashes) in candidates.into_iter().zip(candidate_phashes) {
        let score = matcher::score_issue_with_phash(facts, &c, local_phash, &cand_phashes);
        let bucket = score.bucket(thresholds);
        // PhashAware fallback: a year-mismatched candidate survives only
        // when the cover confirms it. Reusing `bucket` means the M5
        // alternate-cover ceiling is honoured for free.
        if let YearGate::PhashAware(year) = gate
            && !year_ok(year, c.series_year)
        {
            let cover_confirmed =
                score.cover_hamming.is_some() && !matches!(bucket, Confidence::Low);
            if !cover_confirmed {
                continue;
            }
        }
        out.push(RankedCandidate {
            source: c.source,
            external_id: c.external_id.clone(),
            score,
            bucket,
            coverage: None,
            payload: CandidatePayload::Issue(c),
        });
    }
    out
}

// ───────── search execution ─────────

const SEARCH_LIMIT_PER_PROVIDER: u32 = 25;

/// Timeout for the per-candidate cover-image phash fetch.
/// Aggressive on purpose — covers are small + CDN-cached upstream,
/// and a slow CDN shouldn't stall the whole search-ranking pass.
const COVER_PHASH_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Max concurrent cover fetches per search run (PERF-9). A run can produce ~100
/// candidate-variant URLs; bounding the fan-out keeps the provider CDN + local
/// decode pool from being hammered in one burst.
const COVER_FETCH_CONCURRENCY: usize = 16;

/// Shared `reqwest::Client` for cover fetches, built once so connection pooling
/// is reused across search runs instead of discarded with a per-run client
/// (PERF-9). SSRF-safe by construction — DNS answers and redirect hops are
/// vetted inside the client (WP-2.9) — and the phash fetch consults the
/// `metadata_cover_hash` cache before touching it. `reqwest::Client` is
/// `Arc` inside, so cloning is cheap.
fn cover_http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            crate::util::ssrf::shared_public_client(
                crate::build_info::USER_AGENT_COVER,
                COVER_PHASH_FETCH_TIMEOUT,
                2,
            )
        })
        .clone()
}

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
    run_series_search_with(
        db,
        run_id,
        providers,
        facts,
        thresholds,
        pre_filter,
        alternate_cover_fetch_cap,
        local_series_id,
        SearchOpts::default(),
    )
    .await
}

/// [`run_series_search`] with explicit [`SearchOpts`]. The job handlers
/// call this so a user-asserted year override can pin the hard gate.
#[allow(clippy::too_many_arguments)]
pub async fn run_series_search_with(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &SeriesQueryFacts,
    thresholds: Thresholds,
    pre_filter: &PreFilter,
    alternate_cover_fetch_cap: u32,
    local_series_id: Option<Uuid>,
    opts: SearchOpts,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    // Provider-complete search: what this run already has (a resumed
    // run carries the answering providers' candidates + statuses).
    let SearchBook {
        mut statuses,
        mut ranked,
        lookups,
        mut year_gate_relaxed,
    } = load_search_book(db, run_id, providers).await?;
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

    let http = cover_http_client();
    for p in providers {
        // Only providers still owed a query (a resumed run skips the
        // ones whose candidates are already in `ranked`).
        let Some(si) = statuses
            .iter()
            .position(|st| st.source == p.id() && st.is_owed())
        else {
            continue;
        };
        let mut outcome = ProviderOutcome::Answered;
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
                let raw = candidates;
                let mut produced = score_series_candidates(
                    db,
                    &http,
                    opts.cover_hasher.as_ref(),
                    facts,
                    raw.clone(),
                    pre_filter,
                    local_phash,
                    alternate_cover_fetch_cap,
                    thresholds,
                    YearGate::Hard(facts.year),
                )
                .await;
                // WP-2.8 year-gate escape: the provider *did* return
                // candidates but the hard gate dropped every one (the
                // classic "folder year is ahead of the real volume"
                // case). Re-score the same results under the
                // cover-aware gate — no extra provider call — so a
                // cover-confirmed candidate survives. Only meaningful
                // when a local cover hash exists; without one nothing
                // could confirm anything, so we don't claim to have
                // relaxed. Skipped entirely when the user asserted the
                // year via an override.
                if produced.is_empty()
                    && opts.relax_year_gate
                    && local_phash.is_some()
                    && facts.year.is_some()
                    && !raw.is_empty()
                {
                    produced = score_series_candidates(
                        db,
                        &http,
                        opts.cover_hasher.as_ref(),
                        facts,
                        raw,
                        pre_filter,
                        local_phash,
                        alternate_cover_fetch_cap,
                        thresholds,
                        YearGate::PhashAware(facts.year),
                    )
                    .await;
                    if !produced.is_empty() {
                        year_gate_relaxed = true;
                    }
                }
                ranked.extend(produced);
            }
            Err(ProviderError::QuotaExceeded { retry_after_secs }) => {
                tracing::info!(
                    provider = p.id().as_str(),
                    retry_after_secs,
                    "metadata search: provider out of quota; run will park and resume it"
                );
                outcome = ProviderOutcome::Quota(retry_after_secs);
            }
            Err(e) => {
                tracing::warn!(
                    provider = p.id().as_str(),
                    error = %e,
                    "metadata search: provider returned error; falling through"
                );
                outcome = ProviderOutcome::Failed(e.to_string());
            }
        }
        record_outcome(&mut statuses, si, outcome, &ranked);
    }

    settle_search(
        db,
        run_id,
        &opts,
        SearchBook {
            statuses,
            ranked,
            lookups,
            year_gate_relaxed,
        },
    )
    .await
}

/// WP-5.6: provider query shape for a local annual. `"Annual 1"` in
/// series `"X-Men"` → `("X-Men Annual", "1")`; a local series already
/// named `"... Annual"` keeps its name. `None` for non-annual numbers.
pub(crate) fn annual_query_rewrite(
    series_name: &str,
    issue_number: &str,
) -> Option<(String, String)> {
    let inner = crate::metadata::title_norm::strip_annual_prefix(issue_number)?;
    if inner.is_empty() {
        return None;
    }
    let number = crate::metadata::matcher::canonical_issue_number(inner);
    let name = if crate::metadata::title_norm::has_annual_token(series_name) {
        series_name.to_owned()
    } else {
        format!("{} Annual", series_name.trim())
    };
    Some((name, number))
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
    series_targets: &[EffectiveTarget],
    thresholds: Thresholds,
    alternate_cover_fetch_cap: u32,
    local_issue_id: Option<&str>,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    run_issue_search_with(
        db,
        run_id,
        providers,
        facts,
        series_targets,
        thresholds,
        alternate_cover_fetch_cap,
        local_issue_id,
        SearchOpts::default(),
    )
    .await
}

/// [`run_issue_search`] with explicit [`SearchOpts`].
#[allow(clippy::too_many_arguments)]
pub async fn run_issue_search_with(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
    facts: &IssueQueryFacts,
    series_targets: &[EffectiveTarget],
    thresholds: Thresholds,
    alternate_cover_fetch_cap: u32,
    local_issue_id: Option<&str>,
    opts: SearchOpts,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    // Provider-complete search: what this run already has (a resumed
    // run carries the answering providers' candidates + statuses).
    let SearchBook {
        mut statuses,
        mut ranked,
        mut lookups,
        mut year_gate_relaxed,
    } = load_search_book(db, run_id, providers).await?;
    if let Err(e) = mark_searching(db, run_id).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }

    let local_phash = match local_issue_id {
        Some(id) => crate::metadata::phash::issue_phash(db, id)
            .await
            .unwrap_or(None),
        None => None,
    };

    // `cover_year` is the year on the issue's cover (e.g. 2026 for an
    // issue cover-dated Jan 2026). Distinct from `series_year`, the
    // series *start* year. Metron filters its /api/issue/ endpoint by
    // cover_year; CV ignores it, so the fix is Metron-specific by
    // design. Captured once and reused for the narrowed + fallback query.
    //
    // WP-5.6: a local "Annual N" lives in its own "<Series> Annual"
    // series on both ComicVine and Metron, numbered plain "N". Query
    // that shape instead of asking the parent series for an issue
    // literally numbered "Annual N" (which neither provider has).
    let annual = annual_query_rewrite(&facts.series_name, &facts.issue_number);
    let (query_series_name, query_issue_number) = annual
        .clone()
        .unwrap_or_else(|| (facts.series_name.clone(), facts.issue_number.clone()));
    let issue_query = |series_external_id: Option<String>| IssueQuery {
        series_external_id,
        series_name: Some(query_series_name.clone()),
        series_year: facts.series_year,
        issue_number: query_issue_number.clone(),
        cover_year: facts.issue_year,
        limit: SEARCH_LIMIT_PER_PROVIDER,
    };

    let http = cover_http_client();
    for p in providers {
        // Only providers still owed a query (a resumed run skips the
        // ones whose candidates are already in `ranked`).
        let Some(si) = statuses
            .iter()
            .position(|st| st.source == p.id() && st.is_owed())
        else {
            continue;
        };
        let mut outcome = ProviderOutcome::Answered;
        'provider: {
            // Effective provider target for this issue: a covering
            // `series_provider_range` mapping wins, else the series-level
            // external id default (see `metadata::range_map`).
            //
            // WP-5.6: for an annual only a *range* target can point at the
            // annual series; the series-level default is the parent run,
            // which never carries the annual, so don't narrow to it.
            let target = series_targets
                .iter()
                .find(|t| t.source == p.id() && (annual.is_none() || t.via_range));
            let narrow_id = target.map(|t| t.provider_series_id.clone());
            // Gate the candidate year against the mapped sub-series year
            // when a range supplies one; otherwise the parent series year.
            // An annual series starts after its parent, so gate an annual
            // against its own cover year when known.
            let default_gate_year = if annual.is_some() {
                facts.issue_year.or(facts.series_year)
            } else {
                facts.series_year
            };
            let gate_year = target.and_then(|t| t.declared_year).or(default_gate_year);
            // When we narrowed to a known provider series we trust the
            // mapping and gate hard on the year. When we DIDN'T (this
            // provider has no series-level id or range for the issue), the
            // primary search is itself a broad discovery query — a divergent
            // issue (e.g. a legacy-renumbered #601 that only Metron's "FF
            // (2012)" series carries) would otherwise be year-gated out
            // before its cover is ever compared. Use the cover-pHash-aware
            // gate there so a cover-confirmed candidate survives the year
            // mismatch even with no mapping configured.
            let primary_gate = if narrow_id.is_some() {
                YearGate::Hard(gate_year)
            } else {
                YearGate::PhashAware(gate_year)
            };

            // ── direct lookup via series coverage ──
            // The provider series is known and lists this number with an
            // agreeing cover date: fetch that issue's detail (cached, and the
            // same row the apply reads) and score it. `Replace` (batches)
            // skips the search on a hit; `Additive` (the match dialog) keeps
            // searching for alternatives; `Only` (issue-level refresh) never
            // searches. A miss falls through to the search below, unchanged,
            // except under `Only`.
            if let Some(ctx) = opts.direct.as_ref() {
                match direct_issue_candidate(
                    db,
                    &http,
                    &opts,
                    ctx,
                    p.as_ref(),
                    target,
                    &query_issue_number,
                    facts,
                    local_phash,
                    alternate_cover_fetch_cap,
                    thresholds,
                    gate_year,
                )
                .await
                {
                    Ok((produced, rec)) => {
                        lookups.push(rec);
                        for rc in produced {
                            if !ranked.iter().any(|x: &RankedCandidate| {
                                x.source == rc.source && x.external_id == rc.external_id
                            }) {
                                ranked.push(rc);
                            }
                        }
                        if ctx.mode != DirectMode::Additive {
                            break 'provider;
                        }
                    }
                    Err(rec) => {
                        lookups.push(rec);
                        if ctx.mode == DirectMode::Only {
                            break 'provider;
                        }
                    }
                }
            }

            // ── primary search (narrowed to the provider series when known) ──
            let primary = match p.search_issue(&issue_query(narrow_id.clone())).await {
                Ok(candidates) => {
                    let raw = candidates;
                    let mut scored = score_issue_candidates(
                        db,
                        &http,
                        opts.cover_hasher.as_ref(),
                        facts,
                        raw.clone(),
                        local_phash,
                        alternate_cover_fetch_cap,
                        thresholds,
                        primary_gate,
                    )
                    .await;
                    // WP-2.8 year-gate escape on the *narrowed* pass: the
                    // user pinned this provider series, the provider
                    // returned issues for it, and the hard gate threw them
                    // all away — almost always a wrong local year rather
                    // than a wrong series. Re-score the same results under
                    // the cover-aware gate before falling through to the
                    // broad search. See `run_series_search_with` for the
                    // guard rationale.
                    if scored.is_empty()
                        && matches!(primary_gate, YearGate::Hard(_))
                        && opts.relax_year_gate
                        && local_phash.is_some()
                        && gate_year.is_some()
                        && !raw.is_empty()
                    {
                        scored = score_issue_candidates(
                            db,
                            &http,
                            opts.cover_hasher.as_ref(),
                            facts,
                            raw,
                            local_phash,
                            alternate_cover_fetch_cap,
                            thresholds,
                            YearGate::PhashAware(gate_year),
                        )
                        .await;
                        if !scored.is_empty() {
                            year_gate_relaxed = true;
                        }
                    }
                    scored
                }
                Err(ProviderError::QuotaExceeded { retry_after_secs }) => {
                    tracing::info!(
                        provider = p.id().as_str(),
                        retry_after_secs,
                        "metadata search: provider out of quota; run will park and resume it"
                    );
                    outcome = ProviderOutcome::Quota(retry_after_secs);
                    break 'provider;
                }
                Err(e) => {
                    tracing::warn!(
                        provider = p.id().as_str(),
                        error = %e,
                        "metadata search: provider returned error; falling through"
                    );
                    outcome = ProviderOutcome::Failed(e.to_string());
                    break 'provider;
                }
            };

            // ── broad fallback for provider series-boundary divergence ──
            // We narrowed to the local series' provider id but found nothing.
            // The issue may belong to a *different* provider series of this
            // source (a split / legacy-renumbered run, e.g. Fantastic Four
            // #600–611 in a "FF (2012)" Metron series). Re-search by
            // name+number and keep candidates the cover confirms even when
            // the year gate would otherwise drop the relaunch.
            let mut produced = primary;
            if produced.is_empty() && narrow_id.is_some() {
                match p.search_issue(&issue_query(None)).await {
                    Ok(candidates) => {
                        produced = score_issue_candidates(
                            db,
                            &http,
                            opts.cover_hasher.as_ref(),
                            facts,
                            candidates,
                            local_phash,
                            alternate_cover_fetch_cap,
                            thresholds,
                            YearGate::PhashAware(gate_year),
                        )
                        .await;
                    }
                    Err(ProviderError::QuotaExceeded { retry_after_secs }) => {
                        tracing::info!(
                            provider = p.id().as_str(),
                            retry_after_secs,
                            "metadata search: provider out of quota on fallback; run will park and resume it"
                        );
                        outcome = ProviderOutcome::Quota(retry_after_secs);
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = p.id().as_str(),
                            error = %e,
                            "metadata search: provider fallback error; falling through"
                        );
                        outcome = ProviderOutcome::Failed(e.to_string());
                    }
                }
            }

            // Dedup by (source, external_id) — the fallback can resurface a
            // candidate the narrowed pass already produced.
            for rc in produced {
                if !ranked.iter().any(|x: &RankedCandidate| {
                    x.source == rc.source && x.external_id == rc.external_id
                }) {
                    ranked.push(rc);
                }
            }
        }
        record_outcome(&mut statuses, si, outcome, &ranked);
    }

    settle_search(
        db,
        run_id,
        &opts,
        SearchBook {
            statuses,
            ranked,
            lookups,
            year_gate_relaxed,
        },
    )
    .await
}

// ───────── provider-complete search bookkeeping ─────────

/// What a search loop starts from and ends with: the per-provider
/// statuses, the ranked candidates so far, the batch lookup notes and the
/// year-gate flag. A fresh run starts empty with every provider `pending`;
/// a run resumed from `awaiting_quota` starts from its stash.
struct SearchBook {
    statuses: Vec<ProviderStatus>,
    ranked: Vec<RankedCandidate>,
    lookups: Vec<SourceLookup>,
    year_gate_relaxed: bool,
}

/// One provider's turn in a search loop.
enum ProviderOutcome {
    Answered,
    Quota(u64),
    Failed(String),
}

fn record_outcome(
    statuses: &mut [ProviderStatus],
    idx: usize,
    outcome: ProviderOutcome,
    ranked: &[RankedCandidate],
) {
    let source = statuses[idx].source;
    match outcome {
        ProviderOutcome::Answered => {
            let n = ranked.iter().filter(|c| c.source == source).count();
            statuses[idx].answered(n);
        }
        ProviderOutcome::Quota(retry) => statuses[idx].quota_denied(retry),
        ProviderOutcome::Failed(err) => statuses[idx].failed(&err),
    }
}

/// Load the run's statuses (legacy rows → every provider `pending`; any
/// provider in `providers` the row doesn't know is added `pending`) and,
/// when the run is parked, its stash.
async fn load_search_book(
    db: &DatabaseConnection,
    run_id: Uuid,
    providers: &[Arc<dyn MetadataProvider>],
) -> Result<SearchBook, ProviderError> {
    let run = fetch_run(db, run_id)
        .await
        .map_err(|e| ProviderError::Transport(format!("db: {e}")))?;
    // A finalized run is never searched again on the same id (a resume
    // job that raced the worker, say): finalizing twice would overwrite
    // its candidates with an empty pass.
    if let Some(r) = run.as_ref()
        && matches!(r.status.as_str(), status::COMPLETED | status::FAILED)
    {
        return Err(ProviderError::Transport(format!(
            "run {run_id} already finalized ({})",
            r.status
        )));
    }
    let mut statuses = run
        .as_ref()
        .map(provider_status::for_run)
        .unwrap_or_default();
    for p in providers {
        if !statuses.iter().any(|st| st.source == p.id()) {
            statuses.push(ProviderStatus::pending(p.id()));
        }
    }
    // A provider the run still owes but that is no longer configured
    // can't be asked: record it as failed so the run finalizes flagged
    // instead of parking forever on a bucket nobody refills.
    for st in statuses.iter_mut() {
        if st.is_owed() && !providers.iter().any(|p| p.id() == st.source) {
            st.failed("provider no longer configured");
        }
    }
    // The stash exists only while parked (finalize clears it); the run
    // may already be `queued` again by the resume sweep.
    let partial = run
        .as_ref()
        .and_then(|r| provider_status::parse_partial(r.partial_results.as_ref()))
        .unwrap_or_default();
    Ok(SearchBook {
        statuses,
        ranked: partial.ranked,
        lookups: partial.lookups,
        year_gate_relaxed: partial.year_gate_relaxed,
    })
}

/// Close a search loop. Three ends:
///
/// 1. **Some provider is owed** (quota-denied): park the run
///    `awaiting_quota` with everything gathered so far stashed, and return
///    `QuotaExceeded` with the shortest suggested wait. The resume sweep
///    re-runs the owed providers on this run; nothing is finalized, so
///    no outcome is classified and nothing auto-applies off a partial
///    provider set.
/// 2. **No provider answered and at least one failed hard**: fail the
///    run loudly (the pre-existing rule).
/// 3. Otherwise finalize with the statuses recorded — a provider that
///    failed while others answered is flagged, not hidden.
async fn settle_search(
    db: &DatabaseConnection,
    run_id: Uuid,
    opts: &SearchOpts,
    mut book: SearchBook,
) -> Result<Vec<RankedCandidate>, ProviderError> {
    finalize_ranking(&mut book.ranked);
    if book.year_gate_relaxed {
        note_year_gate_relaxed(db, run_id).await;
    }
    if opts.direct.is_some() {
        note_coverage_lookups(db, run_id, &book.lookups).await;
    }

    let owed: Vec<&ProviderStatus> = book
        .statuses
        .iter()
        .filter(|st| st.state == ProviderState::Quota)
        .collect();
    if !owed.is_empty() {
        let retry_after_secs = owed
            .iter()
            .filter_map(|st| st.retry_after_secs)
            .min()
            .unwrap_or(60)
            .max(1);
        let resume = Utc::now() + chrono::Duration::seconds(retry_after_secs as i64);
        let partial = PartialSearch {
            ranked: book.ranked,
            lookups: book.lookups,
            year_gate_relaxed: book.year_gate_relaxed,
            job: opts.job_payload.clone(),
        };
        if let Err(e) =
            provider_status::park_awaiting_quota(db, run_id, &book.statuses, &partial, resume).await
        {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        tracing::info!(
            run_id = %run_id,
            owed = ?owed.iter().map(|st| st.source.as_str()).collect::<Vec<_>>(),
            retry_after_secs,
            "metadata search: parked awaiting quota with partial results"
        );
        return Err(ProviderError::QuotaExceeded { retry_after_secs });
    }

    let answered_any = book
        .statuses
        .iter()
        .any(|st| st.state == ProviderState::Answered);
    if !answered_any && let Some(err) = book.statuses.iter().find_map(|st| st.error.clone()) {
        if let Err(e) = fail_run(db, run_id, &err).await {
            return Err(ProviderError::Transport(format!("db: {e}")));
        }
        return Err(ProviderError::Transport(err));
    }

    if let Err(e) = finalize_run(db, run_id, &book.ranked, Some(&book.statuses)).await {
        return Err(ProviderError::Transport(format!("db: {e}")));
    }
    Ok(book.ranked)
}

/// One provider's batch direct lookup for an issue
/// ([`crate::metadata::direct_lookup`]): resolve the provider issue from
/// the target series' cached issue list, fetch its detail through the
/// shared `metadata_cache` row, and score it with the ordinary matcher
/// under the hard year gate (the mapping is trusted like a narrowed
/// search). `Err` carries the fallback reason; the caller then searches
/// exactly as before. A cover comparison that lands LOW is treated as a
/// wrong mapping and falls back too; a text-only LOW is kept — the
/// narrowed search would return the same issue with the same score.
#[allow(clippy::too_many_arguments)]
async fn direct_issue_candidate(
    db: &DatabaseConnection,
    http: &reqwest::Client,
    opts: &SearchOpts,
    ctx: &DirectLookupCtx,
    provider: &dyn MetadataProvider,
    target: Option<&EffectiveTarget>,
    query_issue_number: &str,
    facts: &IssueQueryFacts,
    local_phash: Option<i64>,
    alternate_cover_fetch_cap: u32,
    thresholds: Thresholds,
    gate_year: Option<i32>,
) -> Result<(Vec<RankedCandidate>, SourceLookup), SourceLookup> {
    let source = provider.id();
    let fallback = |why| SourceLookup::search(source, why);
    let Some(target) = target else {
        return Err(fallback(FallbackReason::NoTarget));
    };
    let canonical = matcher::canonical_issue_number(query_issue_number);
    let (issue_id, date) = crate::metadata::direct_lookup::resolve_issue_id(
        ctx,
        provider,
        &target.provider_series_id,
        &canonical,
        facts.issue_year,
    )
    .await
    .map_err(fallback)?;
    let detail = crate::metadata::apply::fetch_issue_detail_cached(db, provider, &issue_id)
        .await
        .map_err(|e| {
            tracing::debug!(
                provider = source.as_str(),
                issue_id,
                error = %e,
                "direct lookup: issue detail unavailable; searching"
            );
            fallback(FallbackReason::DetailUnavailable)
        })?;
    let candidate =
        crate::metadata::lookup::issue_candidate_from_detail(source, &issue_id, &detail);
    let mut scored = score_issue_candidates(
        db,
        http,
        opts.cover_hasher.as_ref(),
        facts,
        vec![candidate],
        local_phash,
        alternate_cover_fetch_cap,
        thresholds,
        YearGate::Hard(gate_year),
    )
    .await;
    let cover_rejected = scored
        .iter()
        .all(|c| c.bucket == Confidence::Low && c.score.cover_hamming.is_some());
    if scored.is_empty() || cover_rejected {
        return Err(fallback(FallbackReason::RejectedByMatcher));
    }
    for c in &mut scored {
        c.coverage = Some(CoverageMatch::new(
            target.provider_series_id.clone(),
            target.via_range,
            date,
        ));
    }
    Ok((scored, SourceLookup::direct(source, issue_id)))
}

/// Record each provider's direct-lookup path on the run (batch header
/// counts). Soft-fails like the other run annotations.
async fn note_coverage_lookups(db: &DatabaseConnection, run_id: Uuid, lookups: &[SourceLookup]) {
    let Ok(v) = serde_json::to_value(lookups) else {
        return;
    };
    let mut patch = serde_json::Map::new();
    patch.insert(crate::metadata::direct_lookup::QUERY_KEY.to_owned(), v);
    if let Err(e) = annotate_query(db, run_id, serde_json::Value::Object(patch)).await {
        tracing::warn!(run_id = %run_id, error = %e, "metadata search: coverage_lookups annotation failed");
    }
}

/// Record on the run that the year gate was relaxed (WP-2.8). Soft-fails
/// — the annotation is advisory UI copy, never worth failing a search.
async fn note_year_gate_relaxed(db: &DatabaseConnection, run_id: Uuid) {
    if let Err(e) = annotate_query(db, run_id, serde_json::json!({"year_gate_relaxed": true})).await
    {
        tracing::warn!(run_id = %run_id, error = %e, "metadata search: year_gate_relaxed annotation failed");
    }
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
    db: &DatabaseConnection,
    http: &reqwest::Client,
    hasher: Option<&CoverHasher>,
    urls_per_candidate: &[Vec<Option<&str>>],
) -> Vec<Vec<Option<i64>>> {
    use futures::stream::StreamExt;
    // Flatten into a single batch so all fetches share one bounded-concurrency
    // stream (vs nested join_all, which serializes batches).
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
            match (maybe_url, hasher) {
                (Some(url), Some(h)) => h((*url).to_owned()).await,
                (Some(url), None) => {
                    crate::metadata::phash::fetch_and_hash_cover(
                        db,
                        http,
                        url,
                        COVER_PHASH_FETCH_TIMEOUT,
                    )
                    .await
                }
                (None, _) => None,
            }
        })
        .collect();
    // Bound concurrency so a 25-candidate search (×4 variants ≈ 100 URLs) can't
    // fire ~100 simultaneous fetches at the provider CDN + local decode pool
    // (PERF-9). `buffered` (not `buffer_unordered`) preserves input order, which
    // the window-slicing below relies on.
    let flat_hashes: Vec<Option<i64>> = futures::stream::iter(futures)
        .buffered(COVER_FETCH_CONCURRENCY)
        .collect()
        .await;
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
    fn annual_query_rewrite_targets_the_annual_series() {
        assert_eq!(
            annual_query_rewrite("X-Men", "Annual 1"),
            Some(("X-Men Annual".into(), "1".into()))
        );
        assert_eq!(
            annual_query_rewrite("X-Men", "annual #01"),
            Some(("X-Men Annual".into(), "1".into()))
        );
        // Local series already named "... Annual" keeps its name.
        assert_eq!(
            annual_query_rewrite("X-Men Annual", "Annual 3"),
            Some(("X-Men Annual".into(), "3".into()))
        );
        assert_eq!(annual_query_rewrite("X-Men", "1"), None);
        assert_eq!(annual_query_rewrite("X-Men", "Annual"), None);
    }

    #[test]
    fn stored_query_round_trips() {
        let series = StoredQuery::Series(SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: Some("Image".into()),
            volume: None,
            format: None,
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
            coverage: None,
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
                format: None,
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
            format: None,
        }
    }

    fn series_facts(year: Option<i32>) -> SeriesQueryFacts {
        SeriesQueryFacts {
            name: "Saga".into(),
            year,
            publisher: None,
            volume: None,
            format: None,
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
            issue_year: None,
            format: None,
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
                format: None,
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
                format: None,
            },
        ];
        let out = pre_filter_issue(candidates, facts.series_year);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].external_id, "keep");
    }

    #[test]
    fn pre_filter_issue_honours_mapped_sub_series_year() {
        // A relaunch candidate (series_year 2012) is dropped against the
        // parent 2001 baseline, but kept when the gate uses the mapped
        // sub-series' declared year (2012) — the range-mapping case.
        let relaunch = || IssueCandidate {
            source: Source::Metron,
            external_id: "ff-2012-600".into(),
            external_url: None,
            issue_number: Some("600".into()),
            name: None,
            cover_date: None,
            series_name: Some("Fantastic Four".into()),
            series_year: Some(2012),
            series_external_id: None,
            cover_image_url: None,
            alternate_cover_urls: Vec::new(),
            format: None,
        };
        assert_eq!(pre_filter_issue(vec![relaunch()], Some(2001)).len(), 0);
        assert_eq!(pre_filter_issue(vec![relaunch()], Some(2012)).len(), 1);
    }
}
