//! Direct lookups for batch issue searches, sourced from series coverage.
//!
//! A metadata batch ("Fetch metadata → All issues / Only missing or
//! partial", a selection or a saved view) runs one issue search per local
//! issue per enabled provider. When the issue's provider series is known —
//! the series-level `external_ids` row or a covering
//! `series_provider_range`, folded by
//! [`range_map::fold_targets`](crate::metadata::range_map) — the provider
//! issue can usually be read straight off that series' issue list
//! ([`coverage::provider_issues_with`], cached 24 h) by number and cover
//! date ([`coverage::lookup_provider_issue`]). The orchestrator then fetches
//! that one issue's detail through the shared `metadata_cache` row (the
//! same row the apply later reads) instead of searching, and scores it with
//! the ordinary matcher, so a wrong mapping is still caught.
//!
//! This module holds the list half (which provider issue id, or why not)
//! and the per-source record the run stores under
//! [`QUERY_KEY`] for the batch header's direct-vs-search counts. The
//! detail fetch + scoring live in
//! [`orchestrator::run_issue_search_with`](crate::metadata::orchestrator).

use crate::metadata::coverage::{self, DateMatch, IssueListLookup};
use crate::metadata::identifier::Source;
use crate::metadata::provider::{IssueListOpts, MetadataProvider, ProviderError};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

/// Key on `metadata_run.query` holding the per-source [`SourceLookup`]
/// list of a run that was allowed to use direct lookups.
pub const QUERY_KEY: &str = "coverage_lookups";

/// How long a provider series whose issue list couldn't be used (fetch
/// error, or a page-capped partial listing that isn't cached) is skipped
/// by later issues of the batch. Without it every issue would re-request
/// the listing before falling back to its search.
pub const LIST_MISS_TTL_SECS: u64 = 3600;

/// Per-run context that turns direct lookups on. Batch children get
/// [`DirectMode::Replace`]; the single-issue search from the match dialog
/// gets [`DirectMode::Additive`] (the coverage candidate *and* the search,
/// so the operator still sees alternatives and compare mode can default
/// to the coverage-assigned issue); the opt-in issue-level refresh gets
/// [`DirectMode::Only`].
#[derive(Clone)]
pub struct DirectLookupCtx {
    /// Redis for the issue-list cache ([`coverage::provider_issues_with`]).
    pub redis: ConnectionManager,
    /// Local cover month; the year is `IssueQueryFacts::issue_year`.
    pub cover_month: Option<i32>,
    /// What a direct hit / miss does to the provider's search.
    pub mode: DirectMode,
}

/// How a run combines a provider's direct lookup with its search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectMode {
    /// A direct hit replaces the search; a miss falls back to it (metadata
    /// batches).
    #[default]
    Replace,
    /// A direct hit is added and the search still runs (the match dialog:
    /// alternatives stay visible; the compare view picks the coverage
    /// candidate per provider).
    Additive,
    /// A miss is recorded and nothing is searched (issue-level library /
    /// weekly refresh: covered issues only, bounded per provider).
    Only,
}

/// Whether a provider answered a batch issue from coverage or a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LookupPath {
    /// The provider issue came from the series' issue list; no search.
    Direct,
    /// Today's search (narrowed when the provider series is known).
    Search,
}

/// Why a batch issue fell back from a direct lookup to a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// No series-level id or covering range for this provider.
    NoTarget,
    /// The provider can't list a series' issues, or the listing failed.
    ListUnavailable,
    /// The provider series doesn't list this issue number.
    NotListed,
    /// Listed, but the cover date conflicts (a same-numbered issue of
    /// another run).
    DateConflict,
    /// The issue detail fetch failed (quota, network, 404).
    DetailUnavailable,
    /// The matcher rejected the looked-up issue: the year gate dropped
    /// it, or its cover disagrees with the local cover (bucket low with a
    /// cover comparison).
    RejectedByMatcher,
}

/// One provider's path for one batch issue, stored on the run under
/// [`QUERY_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLookup {
    pub source: Source,
    pub path: LookupPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackReason>,
    /// The provider issue a direct lookup used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_issue_id: Option<String>,
}

impl SourceLookup {
    pub fn direct(source: Source, provider_issue_id: String) -> Self {
        Self {
            source,
            path: LookupPath::Direct,
            fallback: None,
            provider_issue_id: Some(provider_issue_id),
        }
    }

    pub fn search(source: Source, why: FallbackReason) -> Self {
        Self {
            source,
            path: LookupPath::Search,
            fallback: Some(why),
            provider_issue_id: None,
        }
    }
}

/// Provenance a direct-lookup candidate carries into its
/// `score_breakdown.coverage` (the dialog's "matched by series coverage"
/// note). Informational only: the bucket is the matcher's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageMatch {
    /// Provider series whose issue list supplied the issue.
    pub provider_series_id: String,
    /// `true` when that series came from a `series_provider_range` row.
    pub via_range: bool,
    /// How the listed cover date compared with the local one.
    pub date: DateMatch,
    /// Human reason, e.g. "matched by series coverage (number + cover date)".
    pub reason: String,
}

impl CoverageMatch {
    pub fn new(provider_series_id: String, via_range: bool, date: DateMatch) -> Self {
        let reason = if date.is_dated() {
            "matched by series coverage (number + cover date)"
        } else {
            "matched by series coverage (number; no cover date to compare)"
        };
        Self {
            provider_series_id,
            via_range,
            date,
            reason: reason.to_owned(),
        }
    }
}

fn list_miss_key(source: Source, provider_series_id: &str) -> String {
    format!(
        "metadata:direct_lookup:list_miss:v1:{}:{provider_series_id}",
        source.as_str()
    )
}

async fn list_marked_unusable(redis: &ConnectionManager, source: Source, id: &str) -> bool {
    let mut conn = redis.clone();
    conn.exists(list_miss_key(source, id))
        .await
        .unwrap_or(false)
}

async fn mark_list_unusable(redis: &ConnectionManager, source: Source, id: &str) {
    let mut conn = redis.clone();
    let _: Result<(), _> = conn
        .set_ex(list_miss_key(source, id), 1, LIST_MISS_TTL_SECS)
        .await;
}

/// Resolve a local issue to a provider issue id through the provider
/// series' cached issue list. `canonical` is the canonical issue number
/// (after the annual rewrite); `year` / `month` the local cover date.
pub async fn resolve_issue_id(
    ctx: &DirectLookupCtx,
    provider: &dyn MetadataProvider,
    provider_series_id: &str,
    canonical: &str,
    year: Option<i32>,
) -> Result<(String, DateMatch), FallbackReason> {
    let source = provider.id();
    if !provider.lists_series_issues()
        || list_marked_unusable(&ctx.redis, source, provider_series_id).await
    {
        return Err(FallbackReason::ListUnavailable);
    }
    // GCD dates only the overview pages holding the hinted numbers; the
    // others ignore the hint.
    let opts = IssueListOpts {
        date_hint: vec![canonical.to_owned()],
        max_pages: 0,
    };
    let list = match coverage::provider_issues_with(&ctx.redis, provider, provider_series_id, &opts)
        .await
    {
        Ok(list) => list,
        Err(e) => {
            // A quota denial says nothing about the series: don't skip it
            // for the rest of the batch.
            if !matches!(e, ProviderError::QuotaExceeded { .. }) {
                mark_list_unusable(&ctx.redis, source, provider_series_id).await;
            }
            tracing::debug!(
                source = source.as_str(),
                provider_series_id,
                error = %e,
                "direct lookup: issue list unavailable; searching"
            );
            return Err(FallbackReason::ListUnavailable);
        }
    };
    if !list.complete {
        // Not cached (a later caller may have a bigger budget), so every
        // issue would pay for the pages again — use it this once.
        mark_list_unusable(&ctx.redis, source, provider_series_id).await;
    }
    match coverage::lookup_provider_issue(&list, canonical, year, ctx.cover_month) {
        IssueListLookup::Found { issue, date } => match &issue.external_id {
            Some(id) => Ok((id.clone(), date)),
            None => Err(FallbackReason::ListUnavailable),
        },
        IssueListLookup::NotListed if !list.complete => Err(FallbackReason::ListUnavailable),
        IssueListLookup::NotListed => Err(FallbackReason::NotListed),
        IssueListLookup::DateConflict => Err(FallbackReason::DateConflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_lookup_round_trips_compactly() {
        let direct = SourceLookup::direct(Source::ComicVine, "44623".into());
        let v = serde_json::to_value(&direct).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"source": "comicvine", "path": "direct", "provider_issue_id": "44623"})
        );
        let search = SourceLookup::search(Source::Gcd, FallbackReason::NoTarget);
        let v = serde_json::to_value(&search).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"source": "gcd", "path": "search", "fallback": "no_target"})
        );
        assert_eq!(serde_json::from_value::<SourceLookup>(v).unwrap(), search);
    }

    #[test]
    fn coverage_reason_says_whether_a_date_confirmed_it() {
        assert_eq!(
            CoverageMatch::new("1".into(), false, DateMatch::Confirmed).reason,
            "matched by series coverage (number + cover date)"
        );
        assert!(
            CoverageMatch::new("1".into(), false, DateMatch::Unknown)
                .reason
                .contains("no cover date")
        );
    }
}
