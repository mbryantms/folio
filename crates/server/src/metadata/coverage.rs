//! Provider-independent series coverage.
//!
//! A local series is folder-pinned: it can hold issues that a provider
//! files under several of its own series (a 1998 volume plus a
//! legacy-renumbered `#500+` run, or two relaunches in similar
//! proportions). The split detector ([`crate::metadata::auto_split`]) is
//! anchored on one already-matched "main" provider series and only
//! searches for the gaps it leaves. Coverage analysis has no anchor:
//!
//! 1. **Candidates** per provider (at most [`MAX_CANDIDATES`]): known ids
//!    (series `external_ids`, the latest applied match, existing range
//!    rows, Metron's curated `cv_id` / `gcd_id` cross-reference, the free
//!    metadata-cache bridge), then series searches on the local name, the
//!    distinct `<Series>` values the folder's own files carry (a renamed
//!    continuation such as `Firestorm: The Nuclear Man` inside
//!    `Firestorm (2004)`) and the aliases ([`search_names`]), with no
//!    year filter, run through [`PreFilter`] (publisher
//!    blacklist + the hard year gate against the *latest* local issue
//!    year) and [`matcher::score_series`]. Issue numbers no candidate
//!    covers get up to [`MAX_GAP_SEARCHES`] issue searches, as the split
//!    detector does.
//! 2. **Issue lists** with cover dates for every candidate
//!    ([`provider_issues`], cached 24 h): ComicVine pages
//!    `/issues/?filter=volume:`, Metron pages `/api/issue/?series_id=`,
//!    GCD reads its issue index plus the overview pages that hold the
//!    local numbers.
//! 3. **Assignment** ([`compute_cover`]): a local issue is eligible for a
//!    candidate that lists its canonical number *and* whose cover date
//!    agrees ([`date_match`]: ±[`DATE_TOLERANCE_MONTHS`] months, or the
//!    year ±1 when a month is missing). Only the candidates with the best
//!    date agreement for that issue stay eligible, so a same-numbered issue
//!    of the wrong volume loses to the right one.
//! 4. **Cover**: greedy minimum set cover. The largest coverer is the
//!    provider's main series (the series-level external id); every other
//!    used series becomes contiguous range(s), cut wherever the main lists
//!    a number or a local issue belongs elsewhere
//!    ([`auto_split::split_runs`]). Annuals and other non-numeric numbers
//!    never become range bounds; one assigned to a non-main series is
//!    reported as unranged.
//! 5. **Confidence** ([`confidence`]) and the **accept** path
//!    ([`accept_provider`]): the main id goes through
//!    [`writers::set_external_id_promoting`], ranges through
//!    [`auto_split::insert_detected_range`]. User-set ids and ranges
//!    (`set_by = 'user'`) are never overwritten or deleted. Existing
//!    *automated* ranges the accepted proposal no longer supports (stale:
//!    they point at the main series, or the proposal files their issues
//!    elsewhere) are deleted by the accept once the proposal's main is the
//!    series' id for that provider ([`auto_split::is_automatic_range`]),
//!    and reported in [`AcceptOutcome::stale_ranges_removed`].
//!
//! The analysis runs as a background job
//! ([`crate::jobs::provider_coverage`]): three providers, up to eight
//! candidates each and ComicVine's 1.1 s request floor easily pass the
//! 60 s JSON route timeout. Per-provider request budgets
//! ([`request_budget`]) bound the work.
//!
//! **Reuse.** [`provider_issues`] is the public cache-then-fetch path for a
//! provider series' issue list (issue id + number + cover date). Combined
//! with [`crate::metadata::range_map::fold_targets`] (which provider series
//! a local issue belongs to after an accept), it answers "which provider
//! *issue* is this local issue" without a search — see
//! [`provider_issue_for`].

use crate::metadata::auto_split::{self, numeric_value};
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::matcher::{
    self, SeriesQueryFacts, canonical_issue_number, issue_number_compare_key,
};
use crate::metadata::orchestrator::{PreFilter, pre_filter_series};
use crate::metadata::provider::{
    IssueListOpts, IssueQuery, MetadataProvider, ProviderError, ProviderIssue,
    ProviderSeriesIssues, SeriesCandidate, SeriesQuery,
};
use crate::metadata::range_map;
use crate::metadata::writers::{self, SetBy, SetExternalIdOutcome};
use crate::state::AppState;
use chrono::{Datelike, NaiveDate};
use entity::{external_id, issue, series, series_provider_range};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Providers coverage analyses, in display order.
pub const COVERAGE_SOURCES: [Source; 3] = [Source::ComicVine, Source::Metron, Source::Gcd];

/// Candidate provider series listed per provider per analysis.
pub const MAX_CANDIDATES: usize = 8;

/// Alias searches per provider on top of the series-name search.
pub const MAX_ALIAS_SEARCHES: usize = 2;

/// Distinct `<Series>` values from the folder's own files searched in
/// addition to the series name (see [`search_names`]).
pub const MAX_FILE_NAME_SEARCHES: usize = 2;

/// Issue searches per provider for local issues no candidate covers.
pub const MAX_GAP_SEARCHES: usize = 2;

/// Cover-date tolerance when both sides carry a month.
pub const DATE_TOLERANCE_MONTHS: i32 = 6;

/// Year tolerance when either side lacks a month.
pub const YEAR_TOLERANCE: i32 = 1;

/// Minimum sanitized-name similarity for a search hit to be listed.
pub const NAME_FLOOR: f32 = 0.85;

/// How long a provider series' issue list stays cached.
pub const ISSUE_LIST_TTL_SECS: u64 = 24 * 3600;

/// Wall-clock budget for one analysis (all providers run concurrently).
/// Listing stops when it runs out; what was found is still reported.
pub const ANALYSIS_TIME_BUDGET: Duration = Duration::from_secs(240);

/// Longest provider "retry after" the analysis waits out once (Metron's
/// 20/min bucket); anything longer stops the provider as rate limited.
const MAX_QUOTA_WAIT_SECS: u64 = 65;

/// Request budget per provider per analysis — a hard bound on listing
/// pages + searches. ComicVine: 40 of its 200/h; Metron: 30 of 20/min +
/// 5,000/day (the analysis waits out the minute bucket once); GCD: 30 of
/// its 100/h.
pub fn request_budget(source: Source) -> u32 {
    match source {
        Source::ComicVine => 40,
        Source::Metron => 30,
        Source::Gcd => 30,
        _ => 0,
    }
}

/// Requests a series search costs at most (GCD walks up to two pages).
fn search_cost(source: Source) -> u32 {
    match source {
        Source::Gcd => crate::metadata::gcd::SEARCH_PAGE_CAP,
        _ => 1,
    }
}

// ───────── issue lists (public, reusable) ─────────

fn issue_list_key(source: Source, id: &str) -> String {
    format!("metadata:issue_list:v1:{}:{id}", source.as_str())
}

async fn cache_get_list(
    redis: &ConnectionManager,
    source: Source,
    id: &str,
) -> Option<ProviderSeriesIssues> {
    let mut conn = redis.clone();
    let raw: Option<String> = conn.get(issue_list_key(source, id)).await.ok().flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

async fn cache_put_list(
    redis: &ConnectionManager,
    source: Source,
    id: &str,
    list: &ProviderSeriesIssues,
) {
    let Ok(raw) = serde_json::to_string(list) else {
        return;
    };
    let mut conn = redis.clone();
    let _: Result<(), _> = conn
        .set_ex(issue_list_key(source, id), raw, ISSUE_LIST_TTL_SECS)
        .await;
}

/// A provider series' cached issue list, **cache only** (never fetches).
/// Only complete listings are ever cached, so a hit lists every number.
/// The collection report's provider manifest reads lists this way.
pub async fn cached_provider_issues(
    redis: &ConnectionManager,
    source: Source,
    provider_series_id: &str,
) -> Option<ProviderSeriesIssues> {
    cache_get_list(redis, source, provider_series_id)
        .await
        .filter(|l| l.complete)
}

/// Is a cached list good enough for `opts`? Complete lists always are;
/// a GCD list (dated per overview page) only when every hinted number it
/// lists already carries a date.
fn cached_list_serves(list: &ProviderSeriesIssues, opts: &IssueListOpts) -> bool {
    if !list.complete {
        return false;
    }
    if list.dates_complete || opts.date_hint.is_empty() {
        return true;
    }
    let hint: HashSet<String> = opts
        .date_hint
        .iter()
        .map(|n| canonical_issue_number(n))
        .collect();
    list.issues
        .iter()
        .filter(|i| hint.contains(&i.number))
        .all(|i| i.cover_date.is_some())
}

/// A provider series' issue list (issue id, canonical number, cover date
/// per distinct number), cache-then-fetch. The public entry point for any
/// module that needs to map a local issue onto a provider issue without
/// searching (`provider_issue_for`). `None` provider ⇒ the provider isn't
/// configured.
pub async fn provider_issues(
    state: &AppState,
    source: Source,
    provider_series_id: &str,
) -> Result<ProviderSeriesIssues, ProviderError> {
    let provider = crate::metadata::apply::build_provider(state, source).ok_or_else(|| {
        ProviderError::Unauthorized(format!("{} is not configured", source.label()))
    })?;
    provider_issues_with(
        &state.jobs.redis,
        &*provider,
        provider_series_id,
        &IssueListOpts::default(),
    )
    .await
}

/// [`provider_issues`] with an explicit provider and options. The returned
/// `requests` is the number of network requests spent (0 on a cache hit).
pub async fn provider_issues_with(
    redis: &ConnectionManager,
    provider: &dyn MetadataProvider,
    provider_series_id: &str,
    opts: &IssueListOpts,
) -> Result<ProviderSeriesIssues, ProviderError> {
    let source = provider.id();
    let cached = cache_get_list(redis, source, provider_series_id).await;
    if let Some(list) = &cached
        && cached_list_serves(list, opts)
    {
        let mut hit = list.clone();
        hit.requests = 0;
        return Ok(hit);
    }
    let mut fresh = provider
        .list_series_issues(provider_series_id, opts)
        .await?;
    // Keep dates an earlier (GCD) call already found for other pages.
    if let Some(old) = cached {
        let old_dates: HashMap<&str, NaiveDate> = old
            .issues
            .iter()
            .filter_map(|i| i.cover_date.map(|d| (i.number.as_str(), d)))
            .collect();
        for i in &mut fresh.issues {
            if i.cover_date.is_none() {
                i.cover_date = old_dates.get(i.number.as_str()).copied();
            }
        }
        fresh.dates_complete =
            !fresh.issues.is_empty() && fresh.issues.iter().all(|i| i.cover_date.is_some());
    }
    // A partial (page-capped) listing is not cached — a later caller with
    // a bigger budget must see the whole series.
    if fresh.complete {
        cache_put_list(redis, source, provider_series_id, &fresh).await;
    }
    Ok(fresh)
}

/// The provider issue for a local issue's canonical number in a cached /
/// fetched issue list. When several entries share the number (rare: a
/// series that restarted numbering), the one whose cover date agrees with
/// the local `year` / `month` wins.
pub fn provider_issue_for<'a>(
    list: &'a ProviderSeriesIssues,
    canonical_number: &str,
    year: Option<i32>,
    month: Option<i32>,
) -> Option<&'a ProviderIssue> {
    match lookup_provider_issue(list, canonical_number, year, month) {
        IssueListLookup::Found { issue, .. } => Some(issue),
        _ => None,
    }
}

/// What [`lookup_provider_issue`] found for a local issue in a provider
/// series' issue list. The batch direct lookup
/// ([`crate::metadata::orchestrator`]) records the miss reason per source
/// when it falls back to a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueListLookup<'a> {
    /// The same-numbered entry whose cover date agrees best (or is
    /// unknown on one side).
    Found {
        issue: &'a ProviderIssue,
        date: DateMatch,
    },
    /// No entry carries this number.
    NotListed,
    /// Every same-numbered entry's cover date conflicts with the local
    /// one — a same-numbered issue of another run, not this issue.
    DateConflict,
}

/// Classifying form of [`provider_issue_for`].
pub fn lookup_provider_issue<'a>(
    list: &'a ProviderSeriesIssues,
    canonical_number: &str,
    year: Option<i32>,
    month: Option<i32>,
) -> IssueListLookup<'a> {
    let wanted = issue_number_compare_key(canonical_number);
    let mut hits = list
        .issues
        .iter()
        .filter(|i| issue_number_compare_key(&i.number) == wanted)
        .map(|i| (i, date_match(year, month, i.cover_date)))
        .collect::<Vec<_>>();
    if hits.is_empty() {
        return IssueListLookup::NotListed;
    }
    hits.sort_by_key(|(_, d)| std::cmp::Reverse(d.rank()));
    match hits.into_iter().find(|(_, d)| *d != DateMatch::Conflict) {
        Some((issue, date)) => IssueListLookup::Found { issue, date },
        None => IssueListLookup::DateConflict,
    }
}

/// Display name + start year of a provider series from its cached issue
/// list (free). Used to label coverage-card segments for providers whose
/// series detail isn't in `metadata_cache` (GCD found via a link).
pub async fn cached_series_label(
    redis: &ConnectionManager,
    source: Source,
    provider_series_id: &str,
) -> Option<(Option<String>, Option<i32>)> {
    let list = cache_get_list(redis, source, provider_series_id).await?;
    (list.series_name.is_some() || list.year_began.is_some())
        .then_some((list.series_name, list.year_began))
}

// ───────── series-match hints (display only) ─────────
//
// "Match this series…" shows, per series candidate, how many local issues
// that provider series lists (number + cover date, the same eligibility
// rule as [`compute_cover`]). Hints never touch a candidate's score, bucket
// or rank. Each needs the candidate's issue list, so they are bounded:
// at most [`HINT_MAX_PER_REQUEST`] candidates per request (the dialog asks
// for the top three, others on demand), a per-series per-provider request
// budget of [`request_budget`] per [`HINT_BUDGET_WINDOW_SECS`] (the same
// bound as one coverage analysis), and [`HINT_TIME_BUDGET`] per request.
// Lists come through the shared 24 h issue-list cache, so a later coverage
// analysis / batch direct lookup of the applied series is free.

/// Candidates one hint request may compute.
pub const HINT_MAX_PER_REQUEST: usize = 3;

/// Window over which a series' hint requests share one request budget per
/// provider ([`request_budget`]).
pub const HINT_BUDGET_WINDOW_SECS: u64 = 3600;

/// Wall clock for one hint request (the JSON route timeout is 60 s).
pub const HINT_TIME_BUDGET: Duration = Duration::from_secs(30);

/// Missing-number runs a hint spells out ("#500–512"); the rest are counted.
pub const HINT_MAX_MISSING_RUNS: usize = 6;

/// Whether a hint was computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HintStatus {
    Computed,
    NotComputed,
}

/// Why a hint wasn't computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HintSkipReason {
    /// The provider isn't configured / enabled.
    NotConfigured,
    /// The provider can't list a series' issues.
    NotListable,
    /// This series' hint budget for the provider is spent for the hour.
    Budget,
    /// The provider's rate limit refused the listing.
    RateLimited,
    /// The request's time budget ran out first.
    TimeBudget,
    /// The listing failed (not found, transport, bad response).
    Error,
    /// The local series has no numbered issues to compare.
    NoLocalIssues,
}

/// How well one series candidate covers the local issues.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SeriesCoverageHint {
    pub source: String,
    pub external_id: String,
    pub status: HintStatus,
    pub reason: Option<HintSkipReason>,
    /// Numbered local issues compared (distinct numbers).
    pub local_total: u32,
    /// Local issues this provider series lists with a non-conflicting
    /// cover date.
    pub covered: u32,
    /// Of `covered`, how many had a cover date to confirm.
    pub date_confirmed: u32,
    /// Local issues whose number it lists but whose cover date conflicts
    /// (a same-numbered issue of another run).
    pub date_conflicts: u32,
    /// Local issues it doesn't cover (missing or conflicting).
    pub missing_count: u32,
    /// Runs of consecutive local issues it doesn't cover, e.g. "#500–512"
    /// (first [`HINT_MAX_MISSING_RUNS`]).
    pub missing_runs: Vec<String>,
    /// Distinct numbers the provider series lists.
    pub listed_count: u32,
    /// The listing stopped early (page cap); missing numbers may exist.
    pub partial: bool,
    /// Network requests this hint spent (0 = served from cache).
    pub requests: u32,
}

impl SeriesCoverageHint {
    pub fn not_computed(source: &str, external_id: &str, reason: HintSkipReason) -> Self {
        Self {
            source: source.to_owned(),
            external_id: external_id.to_owned(),
            status: HintStatus::NotComputed,
            reason: Some(reason),
            local_total: 0,
            covered: 0,
            date_confirmed: 0,
            date_conflicts: 0,
            missing_count: 0,
            missing_runs: Vec::new(),
            listed_count: 0,
            partial: false,
            requests: 0,
        }
    }
}

/// Pure: a hint from a provider series' issue list. Assignment is
/// [`compute_cover`] over this one candidate.
pub fn hint_from_list(
    source: Source,
    external_id: &str,
    local: &[LocalCovIssue],
    list: &ProviderSeriesIssues,
) -> SeriesCoverageHint {
    let cand = Candidate {
        provider_series_id: external_id.to_owned(),
        name: list.series_name.clone(),
        year: list.year_began,
        publisher: list.publisher.clone(),
        url: None,
        origin: CandidateOrigin::Search,
        strict: false,
        name_score: 0.0,
        listed_count: list.issues.len() as u32,
        listed: list
            .issues
            .iter()
            .cloned()
            .map(|mut i| {
                i.number = issue_number_compare_key(&i.number);
                i
            })
            .collect(),
        partial: !list.complete,
        part: None,
    };
    let mut cand = cand;
    apply_part_remap(&mut cand);
    let listed_keys: HashSet<&str> = cand.listed.iter().map(|i| i.number.as_str()).collect();
    let cover = compute_cover(local, std::slice::from_ref(&cand), Some(0));
    let covered = cover.assignment.iter().flatten().count() as u32;
    let date_confirmed = cover
        .assignment
        .iter()
        .flatten()
        .filter(|(_, d)| d.is_dated())
        .count() as u32;
    let date_conflicts = cover
        .uncovered
        .iter()
        .filter(|i| listed_keys.contains(local[**i].key.as_str()))
        .count() as u32;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &i in &cover.uncovered {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == i => *last = i,
            _ => runs.push((i, i)),
        }
    }
    let missing_runs = runs
        .iter()
        .take(HINT_MAX_MISSING_RUNS)
        .map(|(a, b)| {
            if a == b {
                format!("#{}", local[*a].canonical)
            } else {
                format!("#{}–{}", local[*a].canonical, local[*b].canonical)
            }
        })
        .collect();
    SeriesCoverageHint {
        source: source.as_str().to_owned(),
        external_id: external_id.to_owned(),
        status: HintStatus::Computed,
        reason: None,
        local_total: local.len() as u32,
        covered,
        date_confirmed,
        date_conflicts,
        missing_count: cover.uncovered.len() as u32,
        missing_runs,
        listed_count: list.issues.len() as u32,
        partial: !list.complete,
        requests: list.requests,
    }
}

fn hint_budget_key(series_id: Uuid, source: Source) -> String {
    format!(
        "metadata:coverage_hint:spent:v1:{series_id}:{}",
        source.as_str()
    )
}

/// Requests a series' hints may still spend on `source` this window.
pub async fn hint_budget_left(redis: &ConnectionManager, series_id: Uuid, source: Source) -> u32 {
    let mut conn = redis.clone();
    let spent: Option<u32> = conn
        .get(hint_budget_key(series_id, source))
        .await
        .ok()
        .flatten();
    request_budget(source).saturating_sub(spent.unwrap_or(0))
}

async fn hint_budget_spend(redis: &ConnectionManager, series_id: Uuid, source: Source, n: u32) {
    if n == 0 {
        return;
    }
    let key = hint_budget_key(series_id, source);
    let mut conn = redis.clone();
    let total: Result<u32, _> = conn.incr(&key, n).await;
    if matches!(total, Ok(t) if t == n) {
        let _: Result<(), _> = conn.expire(&key, HINT_BUDGET_WINDOW_SECS as i64).await;
    }
}

/// One candidate's hint: the cached issue list when it serves, else a
/// listing bounded by the series' remaining hint budget and `deadline`.
pub async fn series_hint(
    redis: &ConnectionManager,
    provider: &dyn MetadataProvider,
    series_id: Uuid,
    local: &[LocalCovIssue],
    external_id: &str,
    deadline: Instant,
) -> SeriesCoverageHint {
    let source = provider.id();
    let skip = |r| SeriesCoverageHint::not_computed(source.as_str(), external_id, r);
    if local.is_empty() {
        return skip(HintSkipReason::NoLocalIssues);
    }
    if !provider.lists_series_issues() {
        return skip(HintSkipReason::NotListable);
    }
    let hint: Vec<String> = local.iter().map(|l| l.canonical.clone()).collect();
    let mut opts = IssueListOpts {
        date_hint: hint,
        max_pages: 0,
    };
    // A cached list is free, whatever the budget.
    if let Some(list) = cache_get_list(redis, source, external_id).await
        && cached_list_serves(&list, &opts)
    {
        let mut list = list;
        list.requests = 0;
        return hint_from_list(source, external_id, local, &list);
    }
    let left = hint_budget_left(redis, series_id, source).await;
    if left == 0 {
        return skip(HintSkipReason::Budget);
    }
    let now = Instant::now();
    if now >= deadline {
        return skip(HintSkipReason::TimeBudget);
    }
    opts.max_pages = left;
    let res = tokio::time::timeout(
        deadline - now,
        provider_issues_with(redis, provider, external_id, &opts),
    )
    .await;
    match res {
        Err(_) => {
            // The provider may have answered pages before the cut-off.
            hint_budget_spend(redis, series_id, source, 1).await;
            skip(HintSkipReason::TimeBudget)
        }
        Ok(Err(ProviderError::QuotaExceeded { .. })) => skip(HintSkipReason::RateLimited),
        Ok(Err(e)) => {
            tracing::debug!(
                source = source.as_str(),
                series = external_id,
                error = %e,
                "coverage hint: listing failed"
            );
            hint_budget_spend(redis, series_id, source, 1).await;
            skip(HintSkipReason::Error)
        }
        Ok(Ok(list)) => {
            hint_budget_spend(redis, series_id, source, list.requests).await;
            hint_from_list(source, external_id, local, &list)
        }
    }
}

// ───────── local issues ─────────

/// One local active issue with a number.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCovIssue {
    /// Canonical number: the grid row key and the range bound.
    pub canonical: String,
    /// What provider listings are matched on
    /// ([`issue_number_compare_key`]: `"½"` and `"0.5"` agree).
    pub key: String,
    /// Numeric value for plain numbers; `None` for annuals / `14AU` / `½`.
    pub value: Option<f64>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    /// For an issue with no cover date: the year the series is known to
    /// start (the earliest dated sibling, else the folder year). Only a
    /// floor — a provider issue dated more than [`FALLBACK_YEAR_SLACK`]
    /// years before it is a [`DateMatch::Conflict`]; it never confirms a
    /// match. Without it an undated folder matched by number alone, and
    /// "World War III" (1952) #1–2 out-covered the 2007 one-issue parts.
    pub fallback_year: Option<i32>,
}

/// How far before the series' known start year a provider cover date may
/// fall before an undated local issue refuses it. Three years absorbs a
/// folder named for a relaunch whose first issues predate it.
pub const FALLBACK_YEAR_SLACK: i32 = 3;

impl LocalCovIssue {
    pub fn new(raw: &str, year: Option<i32>, month: Option<i32>) -> Self {
        let canonical = canonical_issue_number(raw);
        let value = numeric_value(&canonical);
        Self {
            key: issue_number_compare_key(&canonical),
            canonical,
            value,
            year,
            month: month.filter(|m| (1..=12).contains(m)),
            fallback_year: None,
        }
    }

    /// [`date_match`] for this issue, with the year floor for undated ones.
    pub fn date_match(&self, provider: Option<NaiveDate>) -> DateMatch {
        if self.year.is_none()
            && let (Some(floor), Some(d)) = (self.fallback_year, provider)
            && d.year() < floor - FALLBACK_YEAR_SLACK
        {
            return DateMatch::Conflict;
        }
        date_match(self.year, self.month, provider)
    }
}

/// A local issue the analysis leaves out: an annual / one-shot / special
/// / collected edition the scanner tagged (`issue.special_type`, from
/// ComicInfo `<Format>` or an `Annuals/` / `Specials/` / `Oneshots/`
/// subfolder). Such a file is not part of the series' run — a
/// `… Annual 001.cbz` parses to number `1`, and without this exclusion it
/// would be analyzed as the run's #1 (reported uncovered, or worse, a
/// provider series that does list a #1 proposed as a range for it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExcludedSpecial {
    pub number: Option<String>,
    /// `"Annual"` | `"OneShot"` | `"Special"` | `"TPB"`.
    pub special_type: String,
}

/// Active **main-run** local issues with a number, deduplicated by
/// canonical number, in numeric order (numbers first, then non-numeric
/// numbers in reading order). Issues the scanner tagged with a
/// `special_type` are excluded — see [`load_local_with_specials`] to also
/// get them.
pub async fn load_local<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<Vec<LocalCovIssue>, sea_orm::DbErr> {
    Ok(load_local_with_specials(db, series_id).await?.0)
}

/// [`load_local`] plus the tagged specials it left out, for the analysis
/// view ("not analyzed: 3 annuals").
pub async fn load_local_with_specials<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(Vec<LocalCovIssue>, Vec<ExcludedSpecial>), sea_orm::DbErr> {
    /// `(number_raw, year, month, special_type)`.
    type Row = (Option<String>, Option<i32>, Option<i32>, Option<String>);
    let rows: Vec<Row> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Year)
        .column(issue::Column::Month)
        .column(issue::Column::SpecialType)
        .into_tuple()
        .all(db)
        .await?;
    let mut out: Vec<LocalCovIssue> = Vec::new();
    let mut excluded: Vec<ExcludedSpecial> = Vec::new();
    for (raw, year, month, special_type) in rows {
        if let Some(kind) = special_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            excluded.push(ExcludedSpecial {
                number: raw
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                special_type: kind.to_owned(),
            });
            continue;
        }
        let Some(raw) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let li = LocalCovIssue::new(raw, year, month);
        match out.iter_mut().find(|o| o.key == li.key) {
            // Two files of one number: keep the dated one.
            Some(existing) => {
                if existing.year.is_none() {
                    *existing = li;
                }
            }
            None => out.push(li),
        }
    }
    sort_local(&mut out);
    // Undated issues take the series' known start as a year floor.
    let dated_min = out.iter().filter_map(|l| l.year).min();
    let folder_year: Option<i32> = series::Entity::find_by_id(series_id)
        .select_only()
        .column(series::Column::Year)
        .into_tuple::<Option<i32>>()
        .one(db)
        .await?
        .flatten();
    if let Some(floor) = dated_min.or(folder_year) {
        for l in out.iter_mut().filter(|l| l.year.is_none()) {
            l.fallback_year = Some(floor);
        }
    }
    Ok((out, excluded))
}

fn sort_local(local: &mut [LocalCovIssue]) {
    local.sort_by(|a, b| match (a.value, b.value) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

// ───────── dates ─────────

/// How a provider issue's cover date compares with the local issue's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DateMatch {
    /// Year and month within ±6 months.
    Confirmed,
    /// Year within ±1 (a month was missing on one side).
    Year,
    /// No date on one side — matched by number only.
    Unknown,
    /// Dates disagree: not the same issue.
    Conflict,
}

impl DateMatch {
    fn rank(self) -> u8 {
        match self {
            DateMatch::Confirmed => 3,
            DateMatch::Year => 2,
            DateMatch::Unknown => 1,
            DateMatch::Conflict => 0,
        }
    }

    pub fn is_dated(self) -> bool {
        matches!(self, DateMatch::Confirmed | DateMatch::Year)
    }
}

/// Compare a local issue's cover year / month with a provider cover date.
pub fn date_match(year: Option<i32>, month: Option<i32>, provider: Option<NaiveDate>) -> DateMatch {
    let (Some(y), Some(d)) = (year, provider) else {
        return DateMatch::Unknown;
    };
    match month.filter(|m| (1..=12).contains(m)) {
        Some(m) => {
            let local = y * 12 + m;
            let theirs = d.year() * 12 + d.month() as i32;
            if (local - theirs).abs() <= DATE_TOLERANCE_MONTHS {
                DateMatch::Confirmed
            } else {
                DateMatch::Conflict
            }
        }
        None => {
            if (y - d.year()).abs() <= YEAR_TOLERANCE {
                DateMatch::Year
            } else {
                DateMatch::Conflict
            }
        }
    }
}

// ───────── candidates ─────────

/// Where a candidate provider series came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateOrigin {
    /// A user-set series external id.
    UserLink,
    /// A provider-set series external id.
    Linked,
    /// The series' latest applied match.
    Applied,
    /// An existing `series_provider_range` row.
    Range,
    /// A curated cross-reference (Metron `cv_id` / `gcd_id`) or the free
    /// metadata-cache bridge.
    Bridge,
    /// A series search on the local name or an alias.
    Search,
    /// An issue search for a local issue no other candidate covered.
    IssueSearch,
}

impl CandidateOrigin {
    fn rank(self) -> u8 {
        match self {
            CandidateOrigin::UserLink => 6,
            CandidateOrigin::Linked => 5,
            CandidateOrigin::Applied => 4,
            CandidateOrigin::Range => 3,
            CandidateOrigin::Bridge => 2,
            CandidateOrigin::Search => 1,
            CandidateOrigin::IssueSearch => 0,
        }
    }
}

/// A listed candidate provider series. Persisted with the analysis so
/// "Accept" / "Choose series" recompute the cover without a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub provider_series_id: String,
    pub name: Option<String>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    pub origin: CandidateOrigin,
    /// Strict identity match: sanitized name equals the local name (or an
    /// alias), start year equals the local series year, publisher doesn't
    /// conflict, no format mismatch. A user link counts as strict.
    pub strict: bool,
    /// Sanitized-name similarity with the local name / aliases (0–1).
    pub name_score: f32,
    /// Distinct numbers the provider series lists.
    pub listed_count: u32,
    /// Its issues restricted to the local issues' numbers plus every
    /// numeric number inside the local numeric span (enough to cut
    /// ranges); ids + cover dates.
    pub listed: Vec<ProviderIssue>,
    /// The listing stopped early (page cap / budget).
    pub partial: bool,
    /// The provider files this as one part of a numbered split — "52 /
    /// World War III Part Four: United We Stand", a one-issue series whose
    /// only number is 1 — and the listing was re-keyed to that ordinal
    /// ([`apply_part_remap`]), so local #4 is what it covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<u32>,
}

// ───────── cover (pure) ─────────

/// Result of [`compute_cover`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cover {
    /// Index into the candidates of the main series.
    pub main: Option<usize>,
    /// Per local issue (aligned with the input): `(candidate, date match)`.
    pub assignment: Vec<Option<(usize, DateMatch)>>,
    /// Proposed ranges for non-main series, numeric order.
    pub ranges: Vec<CoverRange>,
    /// Local issues (indices) no candidate lists with an agreeing date.
    pub uncovered: Vec<usize>,
    /// Non-numeric local issues (indices) assigned to a non-main series —
    /// a range bound can't express them.
    pub unranged: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoverRange {
    pub candidate: usize,
    pub low: String,
    pub high: String,
    /// Local issue indices inside the range.
    pub issues: Vec<usize>,
}

/// Assign local issues to candidate series and pick the main + ranges.
///
/// Eligibility: a candidate that lists the issue's number with a date that
/// doesn't conflict; of those, only the ones with the best [`DateMatch`]
/// for the issue. Greedy set cover then repeatedly takes the candidate
/// eligible for the most still-unassigned issues (ties: more date-
/// confirmed issues, strict identity, name similarity, a stronger origin,
/// then the id for determinism). `forced_main` ("Choose series") is taken
/// first and is the main whatever its size; otherwise the largest coverer
/// is.
pub fn compute_cover(
    local: &[LocalCovIssue],
    candidates: &[Candidate],
    forced_main: Option<usize>,
) -> Cover {
    // Per-candidate number → best provider date lookups.
    let listings: Vec<HashMap<&str, Vec<Option<NaiveDate>>>> = candidates
        .iter()
        .map(|c| {
            let mut m: HashMap<&str, Vec<Option<NaiveDate>>> = HashMap::new();
            for i in &c.listed {
                m.entry(i.number.as_str()).or_default().push(i.cover_date);
            }
            m
        })
        .collect();

    // Eligible (candidate, match) per issue at that issue's best level.
    let eligible: Vec<Vec<(usize, DateMatch)>> = local
        .iter()
        .map(|li| {
            let mut opts: Vec<(usize, DateMatch)> = Vec::new();
            for (ci, listing) in listings.iter().enumerate() {
                let Some(dates) = listing.get(li.key.as_str()) else {
                    continue;
                };
                let best = dates
                    .iter()
                    .map(|d| li.date_match(*d))
                    .max_by_key(|m| m.rank())
                    .unwrap_or(DateMatch::Unknown);
                if best != DateMatch::Conflict {
                    opts.push((ci, best));
                }
            }
            let top = opts.iter().map(|(_, m)| m.rank()).max().unwrap_or(0);
            opts.retain(|(_, m)| m.rank() == top);
            opts
        })
        .collect();

    let mut assignment: Vec<Option<(usize, DateMatch)>> = vec![None; local.len()];
    let mut taken: Vec<usize> = Vec::new();
    let gain = |ci: usize, assignment: &[Option<(usize, DateMatch)>]| {
        let mut n = 0usize;
        let mut dated = 0usize;
        for (i, opts) in eligible.iter().enumerate() {
            if assignment[i].is_some() {
                continue;
            }
            if let Some((_, m)) = opts.iter().find(|(c, _)| *c == ci) {
                n += 1;
                if m.is_dated() {
                    dated += 1;
                }
            }
        }
        (n, dated)
    };
    let take = |ci: usize, assignment: &mut Vec<Option<(usize, DateMatch)>>| {
        for (i, opts) in eligible.iter().enumerate() {
            if assignment[i].is_none()
                && let Some((_, m)) = opts.iter().find(|(c, _)| *c == ci)
            {
                assignment[i] = Some((ci, *m));
            }
        }
    };
    if let Some(f) = forced_main.filter(|f| *f < candidates.len()) {
        take(f, &mut assignment);
        taken.push(f);
    }
    loop {
        let best = (0..candidates.len())
            .filter(|ci| !taken.contains(ci))
            .map(|ci| (ci, gain(ci, &assignment)))
            .filter(|(_, (n, _))| *n > 0)
            .max_by(|(a, (an, ad)), (b, (bn, bd))| {
                an.cmp(bn)
                    .then(ad.cmp(bd))
                    .then_with(|| tie_break(&candidates[*a], &candidates[*b]))
            });
        let Some((ci, _)) = best else { break };
        take(ci, &mut assignment);
        taken.push(ci);
    }

    // Main: forced, else the largest coverer.
    let count_of = |ci: usize| {
        let n = assignment
            .iter()
            .filter(|a| matches!(a, Some((c, _)) if *c == ci))
            .count();
        let dated = assignment
            .iter()
            .filter(|a| matches!(a, Some((c, m)) if *c == ci && m.is_dated()))
            .count();
        (n, dated)
    };
    let main = match forced_main.filter(|f| *f < candidates.len()) {
        Some(f) => Some(f),
        None => taken
            .iter()
            .copied()
            .filter(|ci| count_of(*ci).0 > 0)
            .max_by(|a, b| {
                count_of(*a)
                    .cmp(&count_of(*b))
                    .then_with(|| tie_break(&candidates[*a], &candidates[*b]))
            }),
    };

    let uncovered: Vec<usize> = (0..local.len())
        .filter(|i| assignment[*i].is_none())
        .collect();

    // Ranges for every other used series.
    let mut ranges: Vec<CoverRange> = Vec::new();
    let mut unranged: Vec<usize> = Vec::new();
    if let Some(main_ci) = main {
        let main_vals: Vec<f64> = candidates[main_ci]
            .listed
            .iter()
            .filter_map(|i| numeric_value(&i.number))
            .collect();
        let mut used: Vec<usize> = taken.iter().copied().filter(|c| *c != main_ci).collect();
        used.sort_unstable();
        for ci in used {
            let mine: Vec<usize> = (0..local.len())
                .filter(|i| matches!(assignment[*i], Some((c, _)) if c == ci))
                .collect();
            if mine.is_empty() {
                continue;
            }
            // Blockers: the main's numbers and every numeric local issue
            // not assigned to this series (assigned elsewhere or uncovered).
            let mut blockers = main_vals.clone();
            blockers.extend(
                (0..local.len())
                    .filter(|i| !mine.contains(i))
                    .filter_map(|i| local[i].value),
            );
            let mut numeric: Vec<(f64, usize)> = mine
                .iter()
                .filter_map(|i| local[*i].value.map(|v| (v, *i)))
                .collect();
            numeric.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            unranged.extend(mine.iter().copied().filter(|i| local[*i].value.is_none()));
            for run in auto_split::split_runs(numeric, &blockers) {
                let (Some(first), Some(last)) = (run.first(), run.last()) else {
                    continue;
                };
                ranges.push(CoverRange {
                    candidate: ci,
                    low: local[*first].canonical.clone(),
                    high: local[*last].canonical.clone(),
                    issues: run.clone(),
                });
            }
        }
    }
    ranges.sort_by(|a, b| {
        let va = numeric_value(&a.low).unwrap_or(f64::MAX);
        let vb = numeric_value(&b.low).unwrap_or(f64::MAX);
        va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
    });

    Cover {
        main,
        assignment,
        ranges,
        uncovered,
        unranged,
    }
}

/// Ordering among otherwise-equal candidates: strict identity, name
/// similarity, origin strength, then (reversed) id so the result is
/// deterministic. `Greater` means `a` is preferred.
fn tie_break(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    a.strict
        .cmp(&b.strict)
        .then(
            a.name_score
                .partial_cmp(&b.name_score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
        .then(a.origin.rank().cmp(&b.origin.rank()))
        .then_with(|| b.provider_series_id.cmp(&a.provider_series_id))
}

/// Confidence of a provider's proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageConfidence {
    /// The main series is a strict identity match (or the user's own
    /// link) and every assignment is date-confirmed.
    High,
    /// One of the two holds.
    Medium,
    /// Neither holds.
    Low,
    /// Nothing was assigned.
    None,
}

/// Grade a cover, with human-readable reasons.
pub fn confidence(candidates: &[Candidate], cover: &Cover) -> (CoverageConfidence, Vec<String>) {
    let Some(main) = cover.main else {
        return (
            CoverageConfidence::None,
            vec!["no provider series lists these issues".to_owned()],
        );
    };
    let mut reasons = Vec::new();
    let m = &candidates[main];
    let main_strict = m.strict;
    if main_strict {
        reasons.push(if m.origin == CandidateOrigin::UserLink {
            "main series is your linked series".to_owned()
        } else {
            "main series matches the name and start year".to_owned()
        });
    } else {
        reasons.push("main series isn't an exact name / start-year match".to_owned());
    }
    let assigned: Vec<DateMatch> = cover.assignment.iter().flatten().map(|(_, d)| *d).collect();
    let undated = assigned.iter().filter(|d| !d.is_dated()).count();
    if undated == 0 {
        reasons.push("every issue's cover date agrees".to_owned());
    } else {
        reasons.push(format!(
            "{undated} issue{} matched by number only (no cover date to compare)",
            if undated == 1 { "" } else { "s" }
        ));
    }
    let partial = cover
        .assignment
        .iter()
        .flatten()
        .any(|(c, _)| candidates[*c].partial);
    if partial {
        reasons.push("a series' issue list was only partly read".to_owned());
    }
    let all_dated = undated == 0 && !partial;
    let level = match (main_strict, all_dated) {
        (true, true) => CoverageConfidence::High,
        (true, false) | (false, true) => CoverageConfidence::Medium,
        (false, false) => CoverageConfidence::Low,
    };
    (level, reasons)
}

// ───────── analysis (network) ─────────

/// One provider's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// Candidates were listed and the cover computed.
    Analyzed,
    /// The provider isn't configured / enabled.
    NotConfigured,
    /// The provider can't list a series' issues.
    NotListable,
    /// No candidate series lists any local issue.
    NoCandidates,
    /// The request budget / time budget ran out; the cover uses what was
    /// listed.
    Partial,
    /// The provider's rate limit stopped the analysis.
    RateLimited,
    /// A provider call failed (credentials, transport).
    Error,
}

/// Persisted per-provider analysis (the job record carries it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAnalysis {
    pub source: Source,
    pub status: CoverageStatus,
    /// Network requests spent (cache hits are free).
    pub requests: u32,
    /// The bound the analysis held to ([`request_budget`]).
    pub budget: u32,
    pub candidates: Vec<Candidate>,
    pub error: Option<String>,
}

impl ProviderAnalysis {
    fn new(source: Source, status: CoverageStatus) -> Self {
        Self {
            source,
            status,
            requests: 0,
            budget: request_budget(source),
            candidates: Vec::new(),
            error: None,
        }
    }
}

/// Request accounting for one provider's analysis.
pub struct Budget {
    used: u32,
    limit: u32,
    started: Instant,
}

impl Budget {
    /// A fresh budget at [`request_budget`] for `source`.
    pub fn new(source: Source) -> Self {
        Self {
            used: 0,
            limit: request_budget(source),
            started: Instant::now(),
        }
    }
    fn left(&self) -> u32 {
        self.limit.saturating_sub(self.used)
    }
    fn timed_out(&self) -> bool {
        self.started.elapsed() >= ANALYSIS_TIME_BUDGET
    }
    fn can_spend(&self, n: u32) -> bool {
        !self.timed_out() && self.left() >= n
    }
}

/// Outcome of a provider call wrapped by [`with_quota_wait`].
enum Halt {
    RateLimited(String),
    Fatal(String),
}

/// Run `f`; on a short quota denial (Metron's minute bucket) wait it out
/// once and retry. A long denial or a credentials failure halts the
/// provider; other errors are returned for the caller to skip.
async fn with_quota_wait<T, F, Fut>(budget: &Budget, f: F) -> Result<Result<T, ProviderError>, Halt>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderError>>,
{
    match f().await {
        Err(ProviderError::QuotaExceeded { retry_after_secs })
            if retry_after_secs <= MAX_QUOTA_WAIT_SECS
                && budget.started.elapsed() + Duration::from_secs(retry_after_secs)
                    < ANALYSIS_TIME_BUDGET =>
        {
            tokio::time::sleep(Duration::from_secs(retry_after_secs.max(1))).await;
            match f().await {
                Err(e @ ProviderError::QuotaExceeded { .. }) => {
                    Err(Halt::RateLimited(e.to_string()))
                }
                Err(e @ ProviderError::Unauthorized(_)) => Err(Halt::Fatal(e.to_string())),
                other => Ok(other),
            }
        }
        Err(e @ ProviderError::QuotaExceeded { .. }) => Err(Halt::RateLimited(e.to_string())),
        Err(e @ ProviderError::Unauthorized(_)) => Err(Halt::Fatal(e.to_string())),
        other => Ok(other),
    }
}

/// Inputs shared by every provider's analysis.
pub struct SeriesFacts {
    pub series: series::Model,
    /// Main-run issues only ([`load_local`]).
    pub local: Vec<LocalCovIssue>,
    /// Tagged specials left out of the analysis, for the view.
    pub excluded_specials: Vec<ExcludedSpecial>,
    pub names: Vec<String>,
    pub ext_ids: Vec<external_id::Model>,
    pub ranges: Vec<series_provider_range::Model>,
    pub applied: Vec<(Source, String)>,
    pub pre_filter: PreFilter,
    /// Provider series a series match just applied ("Match this
    /// series…"), one per provider at most. A seeded provider lists its
    /// known ids first and skips the name searches when they already
    /// cover every local issue; the seed is the forced main of the view.
    pub seeds: Vec<CoverageSeed>,
}

/// A provider series a series apply chose, carried by a post-apply
/// coverage job ([`crate::jobs::provider_coverage`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSeed {
    pub source: Source,
    pub provider_series_id: String,
    /// The applied candidate's identity (name / start year / publisher /
    /// format) — what the strict-identity check reads when the seeded
    /// provider skips its name search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<SeriesCandidate>,
}

/// The seed for `source`, if any.
pub fn seed_for(seeds: &[CoverageSeed], source: Source) -> Option<&CoverageSeed> {
    seeds.iter().find(|s| s.source == source)
}

/// The names a provider is searched for, in search order: the local
/// series name, then the distinct `<Series>` values the folder's own
/// main-run files carry (most files first, at most
/// [`MAX_FILE_NAME_SEARCHES`]), then the series' aliases / alternate
/// names (at most [`MAX_ALIAS_SEARCHES`]). Near-duplicates (sanitized
/// similarity ≥ 0.999) collapse onto the earlier entry.
///
/// The file names matter for renamed continuations: `Firestorm (2004)`
/// holds #1–22 tagged `Firestorm` and #23–35 tagged `Firestorm: The
/// Nuclear Man`, which Metron and GCD file as a separate series. A search
/// for "Firestorm" never returns it, and a gap issue search that does is
/// dropped by [`NAME_FLOOR`] because "Firestorm: The Nuclear Man" is not
/// similar enough to "Firestorm" — unless that name is one of ours. The
/// same names feed [`SeriesFacts::best_name_score`], so the continuation
/// also counts as an exact-name candidate.
pub fn search_names(
    series_name: &str,
    alias_lists: [&serde_json::Value; 2],
    file_series_names: &[String],
) -> Vec<String> {
    let mut names: Vec<String> = vec![series_name.trim().to_owned()];
    let push = |names: &mut Vec<String>, raw: &str| -> bool {
        let a = raw.trim();
        if a.is_empty()
            || names
                .iter()
                .any(|n| matcher::name_similarity(n, a) >= 0.999)
        {
            return false;
        }
        names.push(a.to_owned());
        true
    };
    let mut added = 0;
    for f in file_series_names {
        if added >= MAX_FILE_NAME_SEARCHES {
            break;
        }
        if push(&mut names, f) {
            added += 1;
        }
    }
    let mut added = 0;
    for v in alias_lists {
        if let Some(arr) = v.as_array() {
            for a in arr.iter().filter_map(|x| x.as_str()) {
                if added >= MAX_ALIAS_SEARCHES {
                    break;
                }
                if push(&mut names, a) {
                    added += 1;
                }
            }
        }
    }
    names
}

/// Distinct ComicInfo `<Series>` values across the series' active
/// main-run issues, most frequent first. Specials are left out: an annual
/// under `Annuals/` legitimately says `<Series>Batman Annual</Series>`.
/// Provider-name searches per provider (`analyze_provider` step 1b): the
/// names a provider gives the series the folder is already linked to,
/// when those differ from every local name. "52/WW III" is ComicVine's
/// name; Metron and GCD file the same book as four one-issue series
/// called "52 / World War III Part N: …", which no local-name search
/// finds. Searching the provider's own base name does.
pub const MAX_PROVIDER_NAME_SEARCHES: usize = 2;

/// `"52 / World War III Part Four: United We Stand"` → `("52 / World War
/// III", 4)`: a series name that is one numbered part of a split, and the
/// base name shared by its siblings. Recognises `Part` / `Book` /
/// `Chapter` followed by a number, a roman numeral or a number word
/// (one…twelve), as a whole word; what follows (a colon, a dash, "of 4",
/// a subtitle) is ignored. `None` for names without such a marker.
pub fn split_part_name(name: &str) -> Option<(String, u32)> {
    const WORDS: [&str; 12] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ];
    fn roman(t: &str) -> Option<u32> {
        let mut total = 0u32;
        let mut prev = 0u32;
        for ch in t.chars().rev() {
            let v = match ch.to_ascii_uppercase() {
                'I' => 1,
                'V' => 5,
                'X' => 10,
                _ => return None,
            };
            if v < prev {
                total = total.checked_sub(v)?;
            } else {
                total += v;
                prev = v;
            }
        }
        (total > 0 && total <= 40).then_some(total)
    }
    let lower = name.to_lowercase();
    let mut best: Option<(usize, u32)> = None;
    for marker in ["part", "book", "chapter"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(marker) {
            let at = from + pos;
            from = at + marker.len();
            // Whole word: preceded by start / non-alphanumeric, followed by
            // whitespace.
            let before_ok = at == 0
                || !lower[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric());
            let rest = &lower[at + marker.len()..];
            if !before_ok || !rest.starts_with(char::is_whitespace) {
                continue;
            }
            let token: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect();
            if token.is_empty() {
                continue;
            }
            let n = token
                .parse::<u32>()
                .ok()
                .or_else(|| WORDS.iter().position(|w| *w == token).map(|i| i as u32 + 1))
                .or_else(|| roman(&token));
            if let Some(n) = n.filter(|n| *n > 0) {
                // The first marker wins ("Part One: Book of Secrets").
                if best.is_none_or(|(b, _)| at < b) {
                    best = Some((at, n));
                }
            }
        }
    }
    let (at, n) = best?;
    let base = name[..at]
        .trim_end()
        .trim_end_matches([':', '-', '–', '—', ','])
        .trim_end()
        .to_owned();
    (!base.is_empty()).then_some((base, n))
}

/// A one-issue series that is "Part N" of a split covers local issue N,
/// not local #1: re-key its listing to the ordinal and remember it. Only
/// fires when every listed number is the same single number (the part's
/// own "#1"); a part series with several issues keeps its numbers.
pub fn apply_part_remap(cand: &mut Candidate) {
    let Some((_, n)) = cand.name.as_deref().and_then(split_part_name) else {
        return;
    };
    let mut keys = cand.listed.iter().map(|i| i.number.as_str());
    let Some(first) = keys.next() else { return };
    if keys.any(|k| k != first) || first != issue_number_compare_key("1") {
        return;
    }
    let key = issue_number_compare_key(&n.to_string());
    for i in &mut cand.listed {
        i.number = key.clone();
    }
    cand.part = Some(n);
}

async fn file_series_names<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<Vec<String>, sea_orm::DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT comic_info_raw->>'series' AS name, COUNT(*) AS n \
               FROM issues \
              WHERE series_id = $1 AND state = 'active' AND removed_at IS NULL \
                AND special_type IS NULL \
                AND COALESCE(comic_info_raw->>'series', '') <> '' \
              GROUP BY 1 ORDER BY n DESC, name",
            [series_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|r| r.try_get::<String>("", "name"))
        .collect()
}

impl SeriesFacts {
    pub async fn load(state: &AppState, series_row: &series::Model) -> anyhow::Result<Self> {
        let db = &state.db;
        let (local, excluded_specials) = load_local_with_specials(db, series_row.id).await?;
        let ext_ids = external_id::Entity::find()
            .filter(external_id::Column::EntityType.eq("series"))
            .filter(external_id::Column::EntityId.eq(series_row.id.to_string()))
            .all(db)
            .await?;
        let ranges = series_provider_range::Entity::find()
            .filter(series_provider_range::Column::SeriesId.eq(series_row.id))
            .all(db)
            .await?;
        let applied = crate::metadata::series_link::applied_series_targets(db, series_row.id).await;
        let pre_filter = entity::library::Entity::find_by_id(series_row.library_id)
            .one(db)
            .await?
            .as_ref()
            .map(PreFilter::from_library)
            .unwrap_or_default();
        let file_names = file_series_names(db, series_row.id).await?;
        let names = search_names(
            &series_row.name,
            [&series_row.aliases, &series_row.alternate_names],
            &file_names,
        );
        Ok(Self {
            series: series_row.clone(),
            local,
            excluded_specials,
            names,
            ext_ids,
            ranges,
            applied,
            pre_filter,
            seeds: Vec::new(),
        })
    }

    fn query_facts(&self) -> SeriesQueryFacts {
        SeriesQueryFacts {
            name: self.series.name.clone(),
            year: self.start_year(),
            publisher: self.series.publisher.clone(),
            volume: None,
            format: self.series.series_type.clone(),
        }
    }

    /// Name similarity + strict identity of a provider series against the
    /// local series: sanitized name equal to the name or an alias, start
    /// year one of [`Self::strict_years`], publisher not conflicting, no
    /// format mismatch.
    fn identity(&self, c: &SeriesCandidate) -> (f32, bool) {
        let ns = self.best_name_score(&c.name);
        let score = matcher::score_series(&self.query_facts(), c);
        let strict = ns >= 0.999
            && c.year.is_some_and(|y| self.strict_years().contains(&y))
            && score.publisher > 0.0
            && !score.format_mismatch;
        (ns, strict)
    }

    fn numeric_span(&self) -> Option<(f64, f64)> {
        let vals: Vec<f64> = self.local.iter().filter_map(|l| l.value).collect();
        let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
    }

    fn year_bounds(&self) -> (Option<i32>, Option<i32>) {
        let years: Vec<i32> = self.local.iter().filter_map(|l| l.year).collect();
        (years.iter().min().copied(), years.iter().max().copied())
    }

    /// The local series' start year for scoring.
    fn start_year(&self) -> Option<i32> {
        self.series.year.or(self.year_bounds().0)
    }

    /// Start years a strict match may carry: the series' own year and the
    /// earliest local cover year. A folder's year often names when the
    /// owner's run starts or a later relaunch (the Fantastic Four folder
    /// says 2001; its #½ and #1 are cover-dated 1998-01, the year every
    /// provider gives the volume), while the earliest cover date is
    /// evidence the provider series starts with these very issues.
    fn strict_years(&self) -> Vec<i32> {
        let mut years: Vec<i32> = self.series.year.into_iter().collect();
        if let Some(y) = self.year_bounds().0
            && !years.contains(&y)
        {
            years.push(y);
        }
        years
    }

    fn best_name_score(&self, name: &str) -> f32 {
        self.names
            .iter()
            .map(|n| matcher::name_similarity(n, name))
            .fold(0.0, f32::max)
    }
}

/// Analyse every coverage provider for a series. Providers run
/// concurrently (separate rate buckets); Metron's curated cross-reference
/// runs first so its `cv_id` / `gcd_id` seed the other two — looked up
/// by a known ComicVine / GCD id while no Metron id is known, read off
/// the linked Metron rows (main + ranges) once one is.
pub async fn analyze(state: &AppState, facts: &SeriesFacts) -> Vec<ProviderAnalysis> {
    analyze_sources(state, facts, &COVERAGE_SOURCES).await
}

/// [`analyze`] restricted to `sources` (a post-apply job analyses only the
/// providers the apply matched). Output keeps [`COVERAGE_SOURCES`] order.
pub async fn analyze_sources(
    state: &AppState,
    facts: &SeriesFacts,
    sources: &[Source],
) -> Vec<ProviderAnalysis> {
    let started = Instant::now();
    let mut bridged: HashMap<Source, Vec<String>> = HashMap::new();
    // Free cache bridge for every target.
    let known_pairs: Vec<(Source, String)> = facts
        .ext_ids
        .iter()
        .filter_map(|e| Some((Source::from_str(&e.source).ok()?, e.external_id.clone())))
        .chain(facts.applied.iter().cloned())
        .collect();
    let wanted: Vec<Source> = COVERAGE_SOURCES
        .into_iter()
        .filter(|s| sources.contains(s))
        .collect();
    let known_pairs: Vec<(Source, String)> = known_pairs
        .into_iter()
        .chain(
            facts
                .seeds
                .iter()
                .map(|s| (s.source, s.provider_series_id.clone())),
        )
        .collect();
    for target in wanted.iter().copied() {
        if known_pairs.iter().any(|(s, _)| *s == target) {
            continue;
        }
        if let Some((id, _)) =
            crate::metadata::series_link::cache_bridge(&state.db, target, &known_pairs).await
        {
            push_bridge(&mut bridged, target, &id);
        }
    }
    // Metron cross-reference (≤ 2 requests): the row carries the CV and
    // GCD ids Metron's editors curated.
    let mut metron_prespent = 0u32;
    let metron = crate::metadata::apply::build_provider(state, Source::Metron);
    if wanted.contains(&Source::Metron)
        && let Some(metron) = &metron
        && !known_pairs.iter().any(|(s, _)| *s == Source::Metron)
        && !bridged.contains_key(&Source::Metron)
    {
        for (src, id) in known_pairs
            .iter()
            .filter(|(s, _)| matches!(s, Source::ComicVine | Source::Gcd))
            .take(2)
        {
            metron_prespent += 1;
            match metron.find_series_by_cross_ref(*src, id).await {
                Ok(hits) => {
                    if let [hit] = hits.as_slice() {
                        push_bridge(&mut bridged, Source::Metron, &hit.external_id);
                        for ident in &hit.identifiers {
                            if ident.source != *src {
                                push_bridge(&mut bridged, ident.source, &ident.id);
                            }
                        }
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "coverage: Metron cross-reference failed");
                    break;
                }
            }
        }
    }
    // The other way round: a Metron id is known (linked, a range, or just
    // bridged) but a target still has none. Accepting Metron first used
    // to starve GCD here — the cross-reference above only runs while
    // Metron is unknown, so nothing read Metron's `gcd_id` any more. The
    // linked Metron rows say which GCD / ComicVine series they are, the
    // main first and then each range, which hands GCD every part of a
    // split without a search. Rows go through the metadata cache (seven
    // days), so the next analysis and the series-link resolver bridge
    // from the cache for free. A target the cache or the cross-reference
    // already bridged still gets the ranges' siblings from here.
    let unbridged: Vec<Source> = wanted
        .iter()
        .copied()
        .filter(|t| *t != Source::Metron && !known_pairs.iter().any(|(s, _)| s == t))
        .collect();
    if !unbridged.is_empty()
        && let Some(metron) = &metron
    {
        let mut metron_ids: Vec<String> = known_pairs
            .iter()
            .filter(|(s, _)| *s == Source::Metron)
            .map(|(_, id)| id.clone())
            .chain(bridged.get(&Source::Metron).cloned().unwrap_or_default())
            .collect();
        let mut ranges: Vec<&series_provider_range::Model> = facts
            .ranges
            .iter()
            .filter(|r| Source::from_str(&r.source).ok() == Some(Source::Metron))
            .collect();
        ranges.sort_by(|a, b| {
            let low = |r: &series_provider_range::Model| {
                r.range_low
                    .as_deref()
                    .and_then(numeric_value)
                    .unwrap_or(f64::MAX)
            };
            low(a)
                .partial_cmp(&low(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        metron_ids.extend(ranges.iter().map(|r| r.provider_series_id.clone()));
        let mut seen: HashSet<String> = HashSet::new();
        metron_ids.retain(|id| !id.trim().is_empty() && seen.insert(id.clone()));
        let ttl = crate::metadata::cache::CacheEntity::Series.default_ttl();
        for id in metron_ids.iter().take(MAX_METRON_ROW_BRIDGES) {
            let cached = crate::metadata::cache::get(
                &state.db,
                Source::Metron,
                crate::metadata::cache::CacheEntity::Series,
                id,
                ttl,
            )
            .await
            .ok()
            .flatten();
            let row = match cached {
                Some(row) => row,
                None => {
                    metron_prespent += 1;
                    let fetched = crate::metadata::cache::get_or_revalidate(
                        &state.db,
                        Source::Metron,
                        crate::metadata::cache::CacheEntity::Series,
                        id,
                        ttl,
                        |validators| {
                            let metron = metron.clone();
                            let id = id.clone();
                            async move {
                                metron
                                    .fetch_series_conditional(&id, validators.as_ref())
                                    .await
                            }
                        },
                    )
                    .await;
                    match fetched {
                        Ok(row) => row,
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                metron_id = %id,
                                "coverage: Metron series row unavailable for bridging"
                            );
                            break;
                        }
                    }
                }
            };
            for ident in &row.identifiers {
                if unbridged.contains(&ident.source) {
                    push_bridge(&mut bridged, ident.source, &ident.id);
                }
            }
        }
    }

    let futs = wanted.iter().map(|source| {
        let bridged = &bridged;
        let prespent = if *source == Source::Metron {
            metron_prespent
        } else {
            0
        };
        async move {
            let Some(provider) = crate::metadata::apply::build_provider(state, *source) else {
                return ProviderAnalysis::new(*source, CoverageStatus::NotConfigured);
            };
            if !provider.lists_series_issues() {
                return ProviderAnalysis::new(*source, CoverageStatus::NotListable);
            }
            let budget = Budget {
                used: prespent,
                limit: request_budget(*source),
                started,
            };
            analyze_provider(
                &state.jobs.redis,
                &state.db,
                facts,
                &*provider,
                bridged.get(source).cloned().unwrap_or_default(),
                budget,
            )
            .await
        }
    });
    futures::future::join_all(futs).await
}

/// How many linked Metron rows (the main, then the ranges in number
/// order) are read for their `cv_id` / `gcd_id` when a target provider
/// has no id of its own. Four covers every split seen so far (52/WW III
/// is four one-issue parts); each row is one Metron request, then cached.
pub const MAX_METRON_ROW_BRIDGES: usize = 4;

/// Record a bridged provider series id; blank and repeated ids are dropped.
fn push_bridge(bridged: &mut HashMap<Source, Vec<String>>, source: Source, id: &str) {
    let id = id.trim();
    if id.is_empty() {
        return;
    }
    let ids = bridged.entry(source).or_default();
    if !ids.iter().any(|x| x == id) {
        ids.push(id.to_owned());
    }
}

/// Analyse one provider. Public so tests drive it with a wiremock-backed
/// client.
pub async fn analyze_provider<C: ConnectionTrait>(
    redis: &ConnectionManager,
    db: &C,
    facts: &SeriesFacts,
    provider: &dyn MetadataProvider,
    bridged_ids: Vec<String>,
    mut budget: Budget,
) -> ProviderAnalysis {
    let source = provider.id();
    let mut out = ProviderAnalysis::new(source, CoverageStatus::Analyzed);
    let mut pending: Vec<Candidate> = Vec::new();
    let push = |pending: &mut Vec<Candidate>, c: Candidate| {
        if !pending
            .iter()
            .any(|p| p.provider_series_id == c.provider_series_id)
        {
            pending.push(c);
        }
    };
    let blank = |id: &str, origin: CandidateOrigin| Candidate {
        provider_series_id: id.to_owned(),
        name: None,
        year: None,
        publisher: None,
        url: crate::metadata::identifier::canonical_url(source, "series", id),
        origin,
        strict: origin == CandidateOrigin::UserLink,
        name_score: 0.0,
        listed_count: 0,
        listed: Vec::new(),
        partial: false,
        part: None,
    };

    // 1. Known ids.
    let mut ext: Vec<&external_id::Model> = facts
        .ext_ids
        .iter()
        .filter(|e| Source::from_str(&e.source).ok() == Some(source))
        .collect();
    ext.sort_by_key(|e| e.set_by != "user");
    for e in ext {
        let origin = if e.set_by == "user" {
            CandidateOrigin::UserLink
        } else {
            CandidateOrigin::Linked
        };
        push(&mut pending, blank(&e.external_id, origin));
    }
    for (s, id) in &facts.applied {
        if *s == source {
            push(&mut pending, blank(id, CandidateOrigin::Applied));
        }
    }
    for r in &facts.ranges {
        if Source::from_str(&r.source).ok() == Some(source) {
            let mut c = blank(&r.provider_series_id, CandidateOrigin::Range);
            c.name = r.provider_series_name.clone();
            c.year = r.declared_year;
            push(&mut pending, c);
        }
    }
    for id in &bridged_ids {
        push(&mut pending, blank(id, CandidateOrigin::Bridge));
    }
    // The series match just applied: its identity facts make it strict
    // (or not) without a name search.
    if let Some(seed) = seed_for(&facts.seeds, source) {
        let pos = match pending
            .iter()
            .position(|p| p.provider_series_id == seed.provider_series_id)
        {
            Some(i) => i,
            None => {
                pending.insert(0, blank(&seed.provider_series_id, CandidateOrigin::Applied));
                0
            }
        };
        if let Some(sc) = &seed.candidate {
            let (ns, strict) = facts.identity(sc);
            let p = &mut pending[pos];
            p.name = p.name.clone().or(Some(sc.name.clone()));
            p.year = p.year.or(sc.year);
            p.publisher = p.publisher.clone().or(sc.publisher.clone());
            p.name_score = p.name_score.max(ns);
            p.strict |= strict;
        }
    }

    // Seeded (after a series apply): list the known ids first and skip
    // the name searches — the gap issue searches below look up exactly
    // the provider series holding what the seed doesn't list (Fantastic
    // Four #600–611 → Metron 1713), at ~1–3 requests instead of a search
    // plus up to eight listings. Nothing uncovered ⇒ no search at all.
    let seeded = seed_for(&facts.seeds, source).is_some();
    let mut listed: Vec<Candidate> = Vec::new();
    let mut covered_by_known = false;
    // The known ids are listed first in every case (they are listed
    // anyway): seeded, so the gap searches can skip what the seed
    // covers; unseeded, so the names the provider gives those series
    // can seed the searches below.
    if !pending.is_empty() && out.status == CoverageStatus::Analyzed {
        let known = std::mem::take(&mut pending);
        list_candidates(
            redis,
            db,
            facts,
            provider,
            known,
            &mut listed,
            &mut budget,
            &mut out,
        )
        .await;
        covered_by_known = seeded
            && !listed.is_empty()
            && compute_cover(&facts.local, &listed, None)
                .uncovered
                .is_empty();
    }

    // 1b. Provider-name searches: a linked / bridged series the provider
    // names differently from every local name ("52 / World War III Part
    // Four: United We Stand" for the folder "52/WW III") says what to
    // search for — its base name finds the sibling parts and renamed
    // continuations the local names never would.
    let mut provider_names: Vec<String> = Vec::new();
    for c in &listed {
        if provider_names.len() >= MAX_PROVIDER_NAME_SEARCHES {
            break;
        }
        let Some(n) = c.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
            continue;
        };
        if c.name_score >= NAME_FLOOR {
            continue;
        }
        let base = split_part_name(n)
            .map(|(b, _)| b)
            .unwrap_or_else(|| n.to_owned());
        let known_name = facts
            .names
            .iter()
            .chain(provider_names.iter())
            .any(|k| matcher::name_similarity(k, &base) >= 0.999);
        if !known_name {
            provider_names.push(base);
        }
    }

    // 2. Series searches (name + aliases + provider names, no year filter).
    let (_, max_year) = facts.year_bounds();
    let query_facts = facts.query_facts();
    let gate_facts = SeriesQueryFacts {
        year: max_year.or(facts.series.year),
        ..query_facts.clone()
    };
    let mut hits: Vec<(SeriesCandidate, f32, f32, bool)> = Vec::new();
    let searches = facts
        .names
        .iter()
        .map(|n| (n, false))
        .chain(provider_names.iter().map(|n| (n, true)));
    for (name, provider_name) in searches {
        if seeded
            || out.status != CoverageStatus::Analyzed
            || pending.len() + listed.len() >= MAX_CANDIDATES
            || !budget.can_spend(search_cost(source))
        {
            break;
        }
        let q = SeriesQuery {
            name: name.clone(),
            year: None,
            publisher: facts.series.publisher.clone(),
            limit: 25,
        };
        let res = match with_quota_wait(&budget, || provider.search_series(&q)).await {
            Ok(r) => r,
            Err(h) => {
                halt(&mut out, h);
                break;
            }
        };
        budget.used += search_cost(source);
        let raw = match res {
            Ok(r) => r,
            Err(e) => {
                out.error = Some(e.to_string());
                continue;
            }
        };
        for c in pre_filter_series(raw, &gate_facts, &facts.pre_filter) {
            let (mut ns, strict) = facts.identity(&c);
            if provider_name {
                // Identity against the provider's own base name: "52 /
                // World War III Part One: A Call to Arms" is the sibling
                // of the bridged Part Four whatever the local name says.
                let hit_base = split_part_name(&c.name)
                    .map(|(b, _)| b)
                    .unwrap_or_else(|| c.name.clone());
                ns = ns.max(matcher::name_similarity(name, &hit_base));
            }
            if ns < NAME_FLOOR {
                continue;
            }
            let score = matcher::score_series(&query_facts, &c);
            if !hits.iter().any(|(h, ..)| h.external_id == c.external_id) {
                hits.push((c, ns, score.total, strict));
            }
        }
    }
    hits.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    for (c, ns, _, strict) in hits {
        // Known ids pick up the search's identity facts.
        if let Some(p) = pending
            .iter_mut()
            .chain(listed.iter_mut())
            .find(|p| p.provider_series_id == c.external_id)
        {
            p.name = p.name.clone().or(Some(c.name.clone()));
            p.year = p.year.or(c.year);
            p.publisher = p.publisher.clone().or(c.publisher.clone());
            p.name_score = p.name_score.max(ns);
            p.strict |= strict;
            continue;
        }
        if pending.len() + listed.len() >= MAX_CANDIDATES {
            continue;
        }
        pending.push(Candidate {
            provider_series_id: c.external_id.clone(),
            name: Some(c.name.clone()),
            year: c.year,
            publisher: c.publisher.clone(),
            url: c.external_url.clone().or_else(|| {
                crate::metadata::identifier::canonical_url(source, "series", &c.external_id)
            }),
            origin: CandidateOrigin::Search,
            strict,
            name_score: ns,
            listed_count: 0,
            listed: Vec::new(),
            partial: false,
            part: None,
        });
    }
    pending.truncate(MAX_CANDIDATES.saturating_sub(listed.len()));

    // 3. List every candidate.
    if out.status == CoverageStatus::Analyzed && !pending.is_empty() {
        list_candidates(
            redis,
            db,
            facts,
            provider,
            pending,
            &mut listed,
            &mut budget,
            &mut out,
        )
        .await;
    }

    // 4. Gap issue searches for local issues nothing covers.
    if out.status == CoverageStatus::Analyzed && !covered_by_known {
        let cover = compute_cover(&facts.local, &listed, None);
        let gaps = gap_representatives(&facts.local, &cover.uncovered);
        let mut found: Vec<Candidate> = Vec::new();
        for li in gaps.into_iter().take(MAX_GAP_SEARCHES) {
            if listed.len() + found.len() >= MAX_CANDIDATES || !budget.can_spend(1) {
                break;
            }
            let q = IssueQuery {
                series_external_id: None,
                series_name: Some(facts.series.name.clone()),
                series_year: None,
                issue_number: li.canonical.clone(),
                cover_year: li.year,
                limit: 25,
            };
            let res = match with_quota_wait(&budget, || provider.search_issue(&q)).await {
                Ok(r) => r,
                Err(h) => {
                    halt(&mut out, h);
                    break;
                }
            };
            // One unit per provider call; a by-name search may be several
            // HTTP requests underneath (ComicVine: the volumes, then the
            // number in each), as a GCD search already was.
            budget.used += 1;
            let Ok(cands) = res else { continue };
            for ic in cands {
                if ic
                    .issue_number
                    .as_deref()
                    .is_some_and(|n| issue_number_compare_key(n) != li.key)
                {
                    continue;
                }
                let Some(sid) = ic.series_external_id.clone().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if listed.iter().any(|c| c.provider_series_id == sid)
                    || found.iter().any(|c| c.provider_series_id == sid)
                {
                    continue;
                }
                let ns = ic
                    .series_name
                    .as_deref()
                    .map(|n| facts.best_name_score(n))
                    .unwrap_or(0.0);
                if ic.series_name.is_some() && ns < NAME_FLOOR {
                    continue;
                }
                let mut c = blank(&sid, CandidateOrigin::IssueSearch);
                c.name = ic.series_name.clone();
                c.year = ic.series_year;
                c.name_score = ns;
                found.push(c);
                break;
            }
        }
        if !found.is_empty() && out.status == CoverageStatus::Analyzed {
            list_candidates(
                redis,
                db,
                facts,
                provider,
                found,
                &mut listed,
                &mut budget,
                &mut out,
            )
            .await;
        }
    }

    out.requests = budget.used;
    if out.status == CoverageStatus::Analyzed && listed.is_empty() {
        out.status = CoverageStatus::NoCandidates;
    }
    out.candidates = listed;
    out
}

fn halt(out: &mut ProviderAnalysis, h: Halt) {
    match h {
        Halt::RateLimited(e) => {
            out.status = CoverageStatus::RateLimited;
            out.error = Some(e);
        }
        Halt::Fatal(e) => {
            out.status = CoverageStatus::Error;
            out.error = Some(e);
        }
    }
}

/// The first issue of each contiguous run of uncovered numeric local
/// issues, for gap issue searches — longest run first, so the
/// [`MAX_GAP_SEARCHES`] go to the blocks that matter (Fantastic Four's
/// #600–611 before a lone #½), then numeric order.
fn gap_representatives<'a>(
    local: &'a [LocalCovIssue],
    uncovered: &[usize],
) -> Vec<&'a LocalCovIssue> {
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (first index, length)
    let mut prev: Option<usize> = None;
    for &i in uncovered {
        if local[i].value.is_none() {
            continue;
        }
        match runs.last_mut() {
            Some((_, len)) if prev == Some(i.wrapping_sub(1)) => *len += 1,
            _ => runs.push((i, 1)),
        }
        prev = Some(i);
    }
    runs.sort_by_key(|(first, len)| (std::cmp::Reverse(*len), *first));
    runs.into_iter().map(|(first, _)| &local[first]).collect()
}

/// List `pending` candidates (within the budget) and keep the ones that
/// list at least one local number.
#[allow(clippy::too_many_arguments)]
async fn list_candidates<C: ConnectionTrait>(
    redis: &ConnectionManager,
    db: &C,
    facts: &SeriesFacts,
    provider: &dyn MetadataProvider,
    pending: Vec<Candidate>,
    listed: &mut Vec<Candidate>,
    budget: &mut Budget,
    out: &mut ProviderAnalysis,
) {
    let source = provider.id();
    let hint: Vec<String> = facts.local.iter().map(|l| l.canonical.clone()).collect();
    let local_numbers: HashSet<&str> = facts.local.iter().map(|l| l.key.as_str()).collect();
    let span = facts.numeric_span();
    for mut cand in pending {
        if budget.timed_out() {
            out.status = CoverageStatus::Partial;
            out.error = Some("analysis time budget ran out".to_owned());
            break;
        }
        // A cache hit is free, so try even with the budget spent.
        let opts = IssueListOpts {
            date_hint: hint.clone(),
            max_pages: budget.left().max(1),
        };
        let res = match with_quota_wait(budget, || {
            provider_issues_with(redis, provider, &cand.provider_series_id, &opts)
        })
        .await
        {
            Ok(r) => r,
            Err(h) => {
                halt(out, h);
                break;
            }
        };
        let list = match res {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!(
                    source = source.as_str(),
                    series = cand.provider_series_id,
                    error = %e,
                    "coverage: candidate listing failed; skipped"
                );
                continue;
            }
        };
        budget.used += list.requests;
        if budget.used > budget.limit && list.requests > 0 && !list.complete {
            out.status = CoverageStatus::Partial;
            out.error = Some(format!(
                "request budget ({}) reached; some issue lists were only partly read",
                budget.limit
            ));
        }
        cand.listed_count = list.issues.len() as u32;
        cand.partial = !list.complete;
        cand.name = cand.name.or(list.series_name.clone());
        cand.year = cand.year.or(list.year_began);
        cand.publisher = cand.publisher.or(list.publisher.clone());
        if cand.name.is_none()
            && let Some((n, y)) =
                crate::metadata::cache::series_display_meta(db, source, &cand.provider_series_id)
                    .await
        {
            cand.name = n;
            cand.year = cand.year.or(y);
        }
        if let Some(n) = cand.name.as_deref() {
            cand.name_score = cand.name_score.max(facts.best_name_score(n));
        }
        // Listings are matched on the comparison key (`"½"` → `"0.5"`).
        cand.listed = list
            .issues
            .into_iter()
            .map(|mut i| {
                i.number = issue_number_compare_key(&i.number);
                i
            })
            .filter(|i| {
                local_numbers.contains(i.number.as_str())
                    || numeric_value(&i.number)
                        .zip(span)
                        .is_some_and(|(v, (lo, hi))| v >= lo && v <= hi)
            })
            .collect();
        apply_part_remap(&mut cand);
        if cand
            .listed
            .iter()
            .any(|i| local_numbers.contains(i.number.as_str()))
        {
            listed.push(cand);
        }
        if budget.left() == 0 && out.status == CoverageStatus::Analyzed {
            out.status = CoverageStatus::Partial;
            out.error = Some(format!("request budget ({}) reached", budget.limit));
        }
    }
}

// ───────── views (API) ─────────

/// One local issue row of the grid.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CoverageLocalIssue {
    /// Canonical number — the row key.
    pub number: String,
    pub year: Option<i32>,
    pub month: Option<i32>,
    /// Annual / letter-suffixed / fractional — never a range bound.
    pub special: bool,
}

/// One grid cell: where a provider puts a local issue.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CoverageCell {
    pub number: String,
    /// `None` ⇒ no candidate series of this provider has the issue.
    pub provider_series_id: Option<String>,
    pub provider_issue_id: Option<String>,
    pub date_match: Option<DateMatch>,
}

/// A candidate as shown to the admin.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CoverageCandidateView {
    pub provider_series_id: String,
    pub name: Option<String>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    pub origin: CandidateOrigin,
    pub strict: bool,
    /// Distinct issue numbers the series lists.
    pub listed_count: u32,
    /// Local issues it lists with a non-conflicting date.
    pub local_matches: u32,
    /// Local issues the proposal assigns to it.
    pub assigned: u32,
    pub partial: bool,
}

/// What would happen to a proposed range on accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposedRangeStatus {
    /// Would be written.
    New,
    /// An existing row already maps it to the same series.
    AlreadyMapped,
    /// An existing row maps overlapping issues elsewhere (kept; never
    /// overwritten).
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProposedRange {
    pub provider_series_id: String,
    pub provider_series_name: Option<String>,
    pub declared_year: Option<i32>,
    pub low: String,
    pub high: String,
    pub issue_count: u32,
    pub status: ProposedRangeStatus,
    pub note: Option<String>,
}

/// An existing range row the proposal disagrees with.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CoverageRangeRef {
    pub id: String,
    pub provider_series_id: String,
    pub provider_series_name: Option<String>,
    /// The mapped provider series' start year, when known.
    #[serde(default)]
    pub declared_year: Option<i32>,
    pub range_low: Option<String>,
    pub range_high: Option<String>,
    pub set_by: String,
    pub reason: String,
}

/// One provider's proposal.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderCoverageView {
    pub source: String,
    pub source_label: String,
    pub status: CoverageStatus,
    pub confidence: CoverageConfidence,
    pub confidence_reasons: Vec<String>,
    /// Network requests this provider spent on the analysis.
    pub requests: u32,
    /// The request bound it held to.
    pub request_budget: u32,
    pub error: Option<String>,
    pub main_series_id: Option<String>,
    /// The provider series a series match just applied, when this
    /// analysis ran after that apply (the proposal is built around it
    /// unless the user linked another series).
    pub seeded_series_id: Option<String>,
    /// The series' current series-level id for this provider.
    pub current_series_id: Option<String>,
    pub current_series_set_by: Option<String>,
    pub candidates: Vec<CoverageCandidateView>,
    /// Aligned with the analysis' `local_issues`.
    pub cells: Vec<CoverageCell>,
    pub proposed_ranges: Vec<ProposedRange>,
    /// Local numbers no candidate series has.
    pub uncovered: Vec<String>,
    /// Specials assigned to a non-main series (can't be ranged).
    pub unranged_specials: Vec<String>,
    /// Existing automated ranges the proposal no longer supports.
    pub stale_ranges: Vec<CoverageRangeRef>,
    /// Existing user-set data that disagrees (blocks auto-accept).
    pub conflicts: Vec<String>,
    /// Accepting would change something (main id, new ranges, or stale
    /// automated ranges to remove).
    pub has_changes: bool,
    /// Eligible for automatic acceptance: high confidence, changes, no
    /// conflicts.
    pub auto_acceptable: bool,
}

/// Build a provider's view from its analysis and the current DB state.
pub fn build_view(
    analysis: &ProviderAnalysis,
    local: &[LocalCovIssue],
    ext_ids: &[external_id::Model],
    ranges: &[series_provider_range::Model],
    forced_main: Option<&str>,
    seed_main: Option<&str>,
) -> ProviderCoverageView {
    let source = analysis.source;
    let cands = &analysis.candidates;
    // A user-set series id is the main unless the admin explicitly chose
    // another candidate: the proposal is built around the user's link
    // instead of contradicting it. Without either, a post-apply job's
    // seed (the series match just applied) is the main.
    let user_link = ext_ids
        .iter()
        .find(|e| Source::from_str(&e.source).ok() == Some(source) && e.set_by == "user")
        .map(|e| e.external_id.as_str());
    let seed_main = seed_main.filter(|id| cands.iter().any(|c| c.provider_series_id == *id));
    let forced = forced_main
        .or(user_link)
        .or(seed_main)
        .and_then(|id| cands.iter().position(|c| c.provider_series_id == id));
    let cover = compute_cover(local, cands, forced);
    let (conf, reasons) = confidence(cands, &cover);

    let current = ext_ids
        .iter()
        .filter(|e| Source::from_str(&e.source).ok() == Some(source))
        .min_by_key(|e| e.set_by != "user");
    let my_ranges: Vec<&series_provider_range::Model> = ranges
        .iter()
        .filter(|r| Source::from_str(&r.source).ok() == Some(source))
        .collect();

    let cells: Vec<CoverageCell> = local
        .iter()
        .enumerate()
        .map(|(i, li)| match cover.assignment[i] {
            Some((ci, dm)) => {
                let c = &cands[ci];
                let pi = c
                    .listed
                    .iter()
                    .filter(|p| p.number == li.key)
                    .max_by_key(|p| li.date_match(p.cover_date).rank());
                CoverageCell {
                    number: li.canonical.clone(),
                    provider_series_id: Some(c.provider_series_id.clone()),
                    provider_issue_id: pi.and_then(|p| p.external_id.clone()),
                    date_match: Some(dm),
                }
            }
            None => CoverageCell {
                number: li.canonical.clone(),
                provider_series_id: None,
                provider_issue_id: None,
                date_match: None,
            },
        })
        .collect();

    let main_id = cover.main.map(|m| cands[m].provider_series_id.clone());
    let mut conflicts: Vec<String> = Vec::new();
    if let (Some(cur), Some(main)) = (current, main_id.as_deref())
        && cur.set_by == "user"
        && cur.external_id != main
    {
        conflicts.push(format!(
            "you linked {} series #{}; the proposal's main is #{main}",
            source.label(),
            cur.external_id
        ));
    }

    let proposed_ranges: Vec<ProposedRange> = cover
        .ranges
        .iter()
        .map(|r| {
            let c = &cands[r.candidate];
            let mut status = ProposedRangeStatus::New;
            let mut note = None;
            for e in &my_ranges {
                if !range_map::ranges_overlap(
                    Some(&r.low),
                    Some(&r.high),
                    e.range_low.as_deref(),
                    e.range_high.as_deref(),
                ) {
                    continue;
                }
                if e.provider_series_id == c.provider_series_id {
                    status = ProposedRangeStatus::AlreadyMapped;
                } else {
                    status = ProposedRangeStatus::Conflict;
                    note = Some(format!(
                        "overlaps {} mapping {} → #{}",
                        if e.set_by == "user" {
                            "your"
                        } else {
                            "an automated"
                        },
                        fmt_bounds(e.range_low.as_deref(), e.range_high.as_deref()),
                        e.provider_series_id
                    ));
                    if e.set_by == "user" {
                        conflicts.push(format!(
                            "your mapping {} → #{} overlaps the proposed {} → #{}",
                            fmt_bounds(e.range_low.as_deref(), e.range_high.as_deref()),
                            e.provider_series_id,
                            fmt_bounds(Some(&r.low), Some(&r.high)),
                            c.provider_series_id
                        ));
                    }
                    break;
                }
            }
            ProposedRange {
                provider_series_id: c.provider_series_id.clone(),
                provider_series_name: c.name.clone(),
                declared_year: c.year,
                low: r.low.clone(),
                high: r.high.clone(),
                issue_count: r.issues.len() as u32,
                status,
                note,
            }
        })
        .collect();

    // Existing rows the proposal doesn't support.
    let mut stale_ranges: Vec<CoverageRangeRef> = Vec::new();
    for e in &my_ranges {
        let inside: Vec<usize> = (0..local.len())
            .filter(|i| {
                range_map::issue_in_range(
                    &local[*i].canonical,
                    e.range_low.as_deref(),
                    e.range_high.as_deref(),
                )
            })
            .collect();
        let reason = if main_id.as_deref() == Some(e.provider_series_id.as_str()) {
            Some("points at the main series".to_owned())
        } else if !inside.is_empty()
            && inside.iter().all(|i| {
                cells[*i]
                    .provider_series_id
                    .as_deref()
                    .is_some_and(|sid| sid != e.provider_series_id)
            })
        {
            Some("the proposal puts these issues in another series".to_owned())
        } else {
            None
        };
        let Some(reason) = reason else { continue };
        if !auto_split::is_automatic_range(&e.set_by) {
            conflicts.push(format!(
                "your mapping {} → #{} disagrees with the proposal ({reason})",
                fmt_bounds(e.range_low.as_deref(), e.range_high.as_deref()),
                e.provider_series_id
            ));
            continue;
        }
        stale_ranges.push(CoverageRangeRef {
            id: e.id.to_string(),
            provider_series_id: e.provider_series_id.clone(),
            provider_series_name: e.provider_series_name.clone(),
            declared_year: e.declared_year,
            range_low: e.range_low.clone(),
            range_high: e.range_high.clone(),
            set_by: e.set_by.clone(),
            reason,
        });
    }

    let candidates: Vec<CoverageCandidateView> = cands
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let local_matches = local
                .iter()
                .filter(|li| {
                    c.listed.iter().any(|p| {
                        p.number == li.key && li.date_match(p.cover_date) != DateMatch::Conflict
                    })
                })
                .count() as u32;
            CoverageCandidateView {
                provider_series_id: c.provider_series_id.clone(),
                name: c.name.clone(),
                year: c.year,
                publisher: c.publisher.clone(),
                url: c.url.clone(),
                origin: c.origin,
                strict: c.strict,
                listed_count: c.listed_count,
                local_matches,
                assigned: cover
                    .assignment
                    .iter()
                    .filter(|a| matches!(a, Some((x, _)) if *x == ci))
                    .count() as u32,
                partial: c.partial,
            }
        })
        .collect();

    let main_changes = match (&main_id, current) {
        (Some(m), Some(cur)) => cur.external_id != *m && cur.set_by != "user",
        (Some(_), None) => true,
        _ => false,
    };
    // Removing stale automated ranges is a change too (the accept deletes
    // them once the main is in place).
    let has_changes = main_changes
        || !stale_ranges.is_empty()
        || proposed_ranges
            .iter()
            .any(|r| r.status == ProposedRangeStatus::New);
    let auto_acceptable = conf == CoverageConfidence::High && conflicts.is_empty() && has_changes;

    ProviderCoverageView {
        source: source.as_str().to_owned(),
        source_label: source.label().to_owned(),
        status: analysis.status,
        confidence: conf,
        confidence_reasons: reasons,
        requests: analysis.requests,
        request_budget: analysis.budget,
        error: analysis.error.clone(),
        main_series_id: main_id,
        seeded_series_id: seed_main.map(str::to_owned),
        current_series_id: current.map(|c| c.external_id.clone()),
        current_series_set_by: current.map(|c| c.set_by.clone()),
        candidates,
        cells,
        proposed_ranges,
        uncovered: cover
            .uncovered
            .iter()
            .map(|i| local[*i].canonical.clone())
            .collect(),
        unranged_specials: cover
            .unranged
            .iter()
            .map(|i| local[*i].canonical.clone())
            .collect(),
        stale_ranges,
        conflicts,
        has_changes,
        auto_acceptable,
    }
}

fn fmt_bounds(low: Option<&str>, high: Option<&str>) -> String {
    match (low, high) {
        (Some(l), Some(h)) if l == h => format!("#{l}"),
        (Some(l), Some(h)) => format!("#{l}–{h}"),
        (Some(l), None) => format!("#{l}+"),
        (None, Some(h)) => format!("up to #{h}"),
        (None, None) => "all issues".to_owned(),
    }
}

pub fn local_view(local: &[LocalCovIssue]) -> Vec<CoverageLocalIssue> {
    local
        .iter()
        .map(|l| CoverageLocalIssue {
            number: l.canonical.clone(),
            year: l.year,
            month: l.month,
            special: l.value.is_none(),
        })
        .collect()
}

// ───────── accept ─────────

/// What an accept wrote.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AcceptOutcome {
    pub source: String,
    pub main_series_id: Option<String>,
    /// The series-level id was written (or refreshed) this call.
    pub main_written: bool,
    /// Why the main id wasn't written, if it wasn't.
    pub main_note: Option<String>,
    /// Ranges written: `"low..high → id"`.
    pub ranges_created: Vec<ProposedRange>,
    /// Ranges not written (already mapped / conflicting), with the reason.
    pub ranges_skipped: Vec<ProposedRange>,
    /// Existing automated ranges the accepted proposal no longer supports
    /// that were **kept** — the accept didn't put the proposal's main in
    /// place (e.g. your own link differs), so removing them would leave
    /// those issues unmapped.
    pub stale_ranges: Vec<CoverageRangeRef>,
    /// Stale automated ranges this accept deleted (never a `user` row).
    #[serde(default)]
    pub stale_ranges_removed: Vec<CoverageRangeRef>,
    /// "Not in your library" links written or refreshed: ranges whose
    /// provider series has issues this folder lacks (Related tab).
    #[serde(default)]
    pub external_links: usize,
    /// Per-issue provider ids written from the proposal's cells (the
    /// provider issue each local issue was assigned to).
    #[serde(default)]
    pub issue_ids_written: u32,
    /// Cells whose id was not written: a user-set id, an id another issue
    /// already holds, or a duplicate local number.
    #[serde(default)]
    pub issue_ids_skipped: u32,
}

/// Accept one provider's proposal: write the main id (unless the user
/// linked another series), delete the stale **automated** ranges once that
/// main is the series' id for the provider, then write every `new` range
/// (re-checked after the deletion, so a proposal only blocked by a stale
/// row goes through). `by_user` records the main id as `user` (an admin
/// confirmed it); automatic acceptance uses `SetBy::Provider`. User-set
/// rows are never overwritten or deleted. Callers audit
/// `stale_ranges_removed` in their accept audit row.
pub async fn accept_provider(
    state: &AppState,
    series_id: Uuid,
    analysis: &ProviderAnalysis,
    forced_main: Option<&str>,
    seed_main: Option<&str>,
    by_user: bool,
) -> anyhow::Result<AcceptOutcome> {
    let db = &state.db;
    let local = load_local(db, series_id).await?;
    let ext_ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .all(db)
        .await?;
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .all(db)
        .await?;
    let view = build_view(analysis, &local, &ext_ids, &ranges, forced_main, seed_main);
    let source = analysis.source;
    let mut out = AcceptOutcome {
        source: source.as_str().to_owned(),
        main_series_id: view.main_series_id.clone(),
        ..Default::default()
    };

    let user_row = ext_ids
        .iter()
        .find(|e| Source::from_str(&e.source).ok() == Some(source) && e.set_by == "user");
    // A chosen main that contradicts the user's own link would write
    // ranges relative to a series that never becomes the default — refuse
    // the whole accept instead of half-applying it.
    if let (Some(u), Some(main)) = (user_row, view.main_series_id.as_deref())
        && u.external_id != main
    {
        out.main_note = Some(format!(
            "kept your linked series #{}; change it under External IDs first",
            u.external_id
        ));
        out.ranges_skipped = view.proposed_ranges;
        out.stale_ranges = view.stale_ranges;
        return Ok(out);
    }

    if let Some(main) = view.main_series_id.as_deref() {
        match user_row {
            Some(u) if u.external_id != main => {
                out.main_note = Some(format!("kept your linked series #{}", u.external_id));
            }
            Some(_) => {
                out.main_note = Some("already your linked series".to_owned());
            }
            None => {
                let identifier = Identifier::with_canonical_url(source, main.to_owned(), "series");
                let set_by = if by_user {
                    SetBy::User
                } else {
                    SetBy::Provider(source)
                };
                let (outcome, promoted) = writers::set_external_id_promoting(
                    db,
                    "series",
                    &series_id.to_string(),
                    &identifier,
                    set_by,
                )
                .await?;
                if promoted > 0 {
                    state.similarity.invalidate_all();
                }
                match outcome {
                    SetExternalIdOutcome::SkippedConflict { owner } => {
                        out.main_note = Some(format!(
                            "{} series #{main} is already linked to another series ({owner})",
                            source.label()
                        ));
                    }
                    SetExternalIdOutcome::KeptUserValue { .. } => {
                        out.main_note = Some("kept your linked series".to_owned());
                    }
                    _ => out.main_written = true,
                }
            }
        }
    }

    // Stale automated ranges go once the proposal's main is in place: the
    // main now files those issues (or the proposal maps them elsewhere).
    // Without the main written, deleting them would leave the issues
    // routed to nothing — they stay reported instead.
    let mut view = view;
    let main_in_place = match view.main_series_id.as_deref() {
        Some(main) => external_id::Entity::find()
            .filter(external_id::Column::EntityType.eq("series"))
            .filter(external_id::Column::EntityId.eq(series_id.to_string()))
            .filter(external_id::Column::Source.eq(source.as_str()))
            .one(db)
            .await?
            .is_some_and(|e| e.external_id == main),
        None => false,
    };
    if main_in_place && !view.stale_ranges.is_empty() {
        let stale_ids: Vec<Uuid> = view
            .stale_ranges
            .iter()
            .filter(|r| auto_split::is_automatic_range(&r.set_by))
            .filter_map(|r| Uuid::parse_str(&r.id).ok())
            .collect();
        // The set_by guard is repeated in SQL: a row a user re-pinned
        // since the analysis is never deleted.
        let deleted: Vec<Uuid> = if stale_ids.is_empty() {
            Vec::new()
        } else {
            series_provider_range::Entity::find()
                .filter(series_provider_range::Column::Id.is_in(stale_ids.clone()))
                .filter(series_provider_range::Column::SeriesId.eq(series_id))
                .filter(series_provider_range::Column::SetBy.ne("user"))
                .select_only()
                .column(series_provider_range::Column::Id)
                .into_tuple::<Uuid>()
                .all(db)
                .await?
        };
        if !deleted.is_empty() {
            series_provider_range::Entity::delete_many()
                .filter(series_provider_range::Column::Id.is_in(deleted.clone()))
                .filter(series_provider_range::Column::SetBy.ne("user"))
                .exec(db)
                .await?;
            let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut view.stale_ranges)
                .into_iter()
                .partition(|r| Uuid::parse_str(&r.id).is_ok_and(|id| deleted.contains(&id)));
            out.stale_ranges_removed = removed;
            // Proposals that only conflicted with a deleted row are new now.
            let ranges = series_provider_range::Entity::find()
                .filter(series_provider_range::Column::SeriesId.eq(series_id))
                .all(db)
                .await?;
            let ext_ids = external_id::Entity::find()
                .filter(external_id::Column::EntityType.eq("series"))
                .filter(external_id::Column::EntityId.eq(series_id.to_string()))
                .all(db)
                .await?;
            view = build_view(analysis, &local, &ext_ids, &ranges, forced_main, seed_main);
            view.stale_ranges = kept;
        }
    }

    for r in view.proposed_ranges {
        if r.status != ProposedRangeStatus::New {
            out.ranges_skipped.push(r);
            continue;
        }
        match auto_split::insert_detected_range(
            db,
            series_id,
            source,
            &r.provider_series_id,
            r.provider_series_name.clone(),
            &r.low,
            &r.high,
            r.declared_year,
            "cross_reference",
        )
        .await
        {
            Ok(_) => out.ranges_created.push(r),
            Err(e) => {
                tracing::info!(error = %e, "coverage: range insert skipped (conflict)");
                let mut r = r;
                r.status = ProposedRangeStatus::AlreadyMapped;
                r.note = Some("written concurrently".to_owned());
                out.ranges_skipped.push(r);
            }
        }
    }
    out.stale_ranges = view.stale_ranges;

    // Per-issue ids: the cell says which provider issue each local issue
    // is, so an apply or refresh can fetch it by id — the only link that
    // works when the provider files the book as one-issue parts, where a
    // range's number is never the provider's number. Written as a
    // cross-reference (never `user`): a later provider apply may refresh
    // them, and a user-set id is kept by the writer.
    let (written, skipped) = write_issue_ids(db, series_id, source, &local, &view.cells).await?;
    out.issue_ids_written = written;
    out.issue_ids_skipped = skipped;
    // Ranges whose provider series also has issues this folder lacks become
    // "not in your library" links on the Related tab (a deleted range's
    // link goes with it). Soft-fails: the accept itself already succeeded.
    match sync_coverage_links(state, series_id, analysis, &local).await {
        Ok(report) => {
            out.external_links = report.upserted;
            if report.removed > 0 || report.pairs_created > 0 {
                state.similarity.invalidate_all();
            }
        }
        Err(e) => {
            tracing::warn!(series_id = %series_id, source = source.as_str(), error = %e, "coverage: external links not synced");
        }
    }
    Ok(out)
}

/// Write the per-issue provider ids a proposal's cells carry. Each local
/// number is matched to its active issue rows (by comparison key); the
/// first row takes the id, later rows with the same number (duplicates,
/// variants) are skipped so the `(source, external_id)` uniqueness holds.
async fn write_issue_ids<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    source: Source,
    local: &[LocalCovIssue],
    cells: &[CoverageCell],
) -> Result<(u32, u32), sea_orm::DbErr> {
    let rows: Vec<(String, Option<String>)> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::SpecialType.is_null())
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::NumberRaw)
        .into_tuple()
        .all(db)
        .await?;
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    for (id, raw) in rows {
        let Some(raw) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        by_key
            .entry(LocalCovIssue::new(raw, None, None).key)
            .or_default()
            .push(id);
    }
    let mut written = 0u32;
    let mut skipped = 0u32;
    for (li, cell) in local.iter().zip(cells) {
        let Some(pid) = cell.provider_issue_id.as_deref().filter(|p| !p.is_empty()) else {
            continue;
        };
        let Some(ids) = by_key.get(&li.key) else {
            continue;
        };
        let mut first = true;
        for issue_id in ids {
            if !first {
                skipped += 1;
                continue;
            }
            first = false;
            let identifier = Identifier::with_canonical_url(source, pid.to_owned(), "issue");
            match writers::set_external_id(
                db,
                "issue",
                issue_id,
                &identifier,
                SetBy::CrossReference,
            )
            .await?
            {
                SetExternalIdOutcome::Set | SetExternalIdOutcome::Reclaimed { .. } => written += 1,
                _ => skipped += 1,
            }
        }
    }
    Ok((written, skipped))
}

/// The `stale_ranges_removed` entry of a coverage-accept audit row: every
/// deleted range with its bounds, target, former `set_by` and why it went.
pub fn removed_ranges_audit(outcome: &AcceptOutcome) -> serde_json::Value {
    serde_json::Value::Array(
        outcome
            .stale_ranges_removed
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "provider_series_id": r.provider_series_id,
                    "provider_series_name": r.provider_series_name,
                    "low": r.range_low,
                    "high": r.range_high,
                    "set_by": r.set_by,
                    "reason": r.reason,
                })
            })
            .collect(),
    )
}

/// "#612–645, #650" for compare keys (integer runs collapsed), at most
/// `max` runs, then "and N more".
fn format_number_runs(keys: &[String], max: usize) -> String {
    let mut ints: Vec<i64> = Vec::new();
    let mut other: Vec<&str> = Vec::new();
    for k in keys {
        match k.parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 && (v - v.round()).abs() < 1e-9 => {
                ints.push(v.round() as i64);
            }
            _ => other.push(k),
        }
    }
    ints.sort_unstable();
    ints.dedup();
    let mut runs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < ints.len() {
        let start = ints[i];
        let mut end = start;
        while i + 1 < ints.len() && ints[i + 1] == end + 1 {
            i += 1;
            end = ints[i];
        }
        runs.push(if start == end {
            format!("#{start}")
        } else {
            format!("#{start}–{end}")
        });
        i += 1;
    }
    runs.extend(other.iter().map(|o| format!("#{o}")));
    let total = runs.len();
    if total > max {
        runs.truncate(max);
        format!("{} and {} more", runs.join(", "), total - max)
    } else {
        runs.join(", ")
    }
}

/// The kind of a coverage link from where the provider series' extra
/// (not-owned) issues sit relative to the local ranges mapped to it.
pub fn coverage_link_kind(
    extra: &[String],
    lowest: Option<f64>,
    highest: Option<f64>,
) -> crate::relationships::RelationshipKind {
    use crate::relationships::RelationshipKind;
    let values: Vec<f64> = extra.iter().filter_map(|k| k.parse::<f64>().ok()).collect();
    if values.is_empty() || values.len() != extra.len() {
        return RelationshipKind::SeeAlso;
    }
    if let Some(hi) = highest
        && values.iter().all(|v| *v > hi)
    {
        return RelationshipKind::ContinuedBy;
    }
    if let Some(lo) = lowest
        && values.iter().all(|v| *v < lo)
    {
        return RelationshipKind::Continues;
    }
    RelationshipKind::SeeAlso
}

/// Turn this provider's ranges into "not in your library" links
/// ([`crate::relationships::external::record_coverage_links`]): a range's
/// provider series that lists numbers no local issue carries. Its issue
/// list comes from the 24 h cache, else the analysis' listing (restricted
/// to the local span, so the count is `listed_count` minus what's owned);
/// no provider call. Returns the links upserted.
async fn sync_coverage_links(
    state: &AppState,
    series_id: Uuid,
    analysis: &ProviderAnalysis,
    local: &[LocalCovIssue],
) -> anyhow::Result<crate::relationships::external::CoverageLinkReport> {
    use crate::relationships::external::{CoverageLink, record_coverage_links};
    let source = analysis.source;
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .filter(series_provider_range::Column::Source.eq(source.as_str()))
        .all(&state.db)
        .await?;
    let owned: HashSet<&str> = local.iter().map(|l| l.key.as_str()).collect();
    let mut by_pid: Vec<(String, Vec<&series_provider_range::Model>)> = Vec::new();
    for r in &ranges {
        match by_pid.iter_mut().find(|(p, _)| *p == r.provider_series_id) {
            Some((_, rs)) => rs.push(r),
            None => by_pid.push((r.provider_series_id.clone(), vec![r])),
        }
    }
    let mut links = Vec::new();
    for (pid, rs) in by_pid {
        let cached = cached_provider_issues(&state.jobs.redis, source, &pid).await;
        let candidate = analysis
            .candidates
            .iter()
            .find(|c| c.provider_series_id == pid);
        let (keys, not_owned, name, year) = if let Some(list) = &cached {
            let mut extra: Vec<String> = list
                .issues
                .iter()
                .map(|i| issue_number_compare_key(&i.number))
                .filter(|k| !owned.contains(k.as_str()))
                .collect();
            extra.sort();
            extra.dedup();
            let n = extra.len();
            (extra, n, list.series_name.clone(), list.year_began)
        } else if let Some(c) = candidate {
            let listed_keys: HashSet<String> = c
                .listed
                .iter()
                .map(|i| issue_number_compare_key(&i.number))
                .collect();
            let owned_listed = listed_keys
                .iter()
                .filter(|k| owned.contains(k.as_str()))
                .count();
            let mut extra: Vec<String> = listed_keys
                .into_iter()
                .filter(|k| !owned.contains(k.as_str()))
                .collect();
            extra.sort();
            let n = (c.listed_count as usize).saturating_sub(owned_listed);
            (extra, n, c.name.clone(), c.year)
        } else {
            continue;
        };
        if not_owned == 0 {
            continue;
        }
        let lows: Vec<f64> = rs
            .iter()
            .filter_map(|r| r.range_low.as_deref().and_then(|v| v.parse().ok()))
            .collect();
        let highs: Vec<f64> = rs
            .iter()
            .filter_map(|r| r.range_high.as_deref().and_then(|v| v.parse().ok()))
            .collect();
        let lowest = lows.iter().copied().reduce(f64::min);
        let highest = highs.iter().copied().reduce(f64::max);
        // Only a full listing can place the extra issues before / after.
        let kind = if cached.is_some() {
            coverage_link_kind(&keys, lowest, highest)
        } else {
            crate::relationships::RelationshipKind::SeeAlso
        };
        let note = if keys.len() == not_owned {
            format!("Has {}", format_number_runs(&keys, 4))
        } else {
            format!(
                "Has {not_owned} issue{} not in this series",
                if not_owned == 1 { "" } else { "s" }
            )
        };
        let first = rs[0];
        links.push(CoverageLink {
            provider_series_id: pid.clone(),
            name: name.or_else(|| first.provider_series_name.clone()),
            year: year.or(first.declared_year),
            url: first.provider_series_url.clone(),
            kind,
            note,
            not_owned,
            ranges: rs
                .iter()
                .map(|r| {
                    format!(
                        "{}–{}",
                        r.range_low.as_deref().unwrap_or("…"),
                        r.range_high.as_deref().unwrap_or("…")
                    )
                })
                .collect(),
        });
    }
    Ok(record_coverage_links(&state.db, series_id, source, &links).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, 1)
    }

    fn li(n: &str, y: i32, m: i32) -> LocalCovIssue {
        LocalCovIssue::new(n, Some(y), Some(m))
    }

    fn cand(id: &str, issues: &[(&str, Option<NaiveDate>)]) -> Candidate {
        Candidate {
            provider_series_id: id.into(),
            name: Some("Daredevil".into()),
            year: None,
            publisher: None,
            url: None,
            origin: CandidateOrigin::Search,
            strict: false,
            name_score: 1.0,
            listed_count: issues.len() as u32,
            listed: issues
                .iter()
                .map(|(n, dt)| ProviderIssue {
                    external_id: Some(format!("{id}-{n}")),
                    number: (*n).into(),
                    cover_date: *dt,
                })
                .collect(),
            partial: false,
            part: None,
        }
    }

    #[test]
    fn date_match_tolerances() {
        assert_eq!(
            date_match(Some(1998), Some(11), d(1998, 11)),
            DateMatch::Confirmed
        );
        assert_eq!(
            date_match(Some(1998), Some(11), d(1999, 5)),
            DateMatch::Confirmed
        );
        assert_eq!(
            date_match(Some(1998), Some(11), d(1999, 6)),
            DateMatch::Conflict
        );
        assert_eq!(date_match(Some(1998), None, d(1999, 12)), DateMatch::Year);
        assert_eq!(
            date_match(Some(1998), None, d(2000, 1)),
            DateMatch::Conflict
        );
        assert_eq!(date_match(None, None, d(2000, 1)), DateMatch::Unknown);
        assert_eq!(date_match(Some(1998), Some(1), None), DateMatch::Unknown);
    }

    #[test]
    fn dates_disambiguate_same_numbers_across_volumes() {
        // Local: 1998 #1–3 + legacy #500–501 (2009).
        let local = vec![
            li("1", 1998, 11),
            li("2", 1998, 12),
            li("3", 1999, 1),
            li("500", 2009, 10),
            li("501", 2009, 11),
        ];
        // 1964 volume lists #1–3 (1964) and the legacy #500–501.
        let v1964 = cand(
            "1964",
            &[
                ("1", d(1964, 4)),
                ("2", d(1964, 6)),
                ("3", d(1964, 8)),
                ("500", d(2009, 10)),
                ("501", d(2009, 11)),
            ],
        );
        let v1998 = cand(
            "1998",
            &[
                ("1", d(1998, 11)),
                ("2", d(1998, 12)),
                ("3", d(1999, 1)),
                ("4", d(1999, 2)),
            ],
        );
        let cover = compute_cover(&local, &[v1964, v1998], None);
        assert_eq!(cover.main, Some(1), "the 1998 volume covers more");
        assert!(cover.uncovered.is_empty());
        assert_eq!(cover.ranges.len(), 1);
        assert_eq!(cover.ranges[0].candidate, 0);
        assert_eq!(
            (cover.ranges[0].low.as_str(), cover.ranges[0].high.as_str()),
            ("500", "501")
        );
        for i in 0..3 {
            assert_eq!(cover.assignment[i], Some((1, DateMatch::Confirmed)));
        }
    }

    #[test]
    fn even_split_still_assigns_everything() {
        let local = vec![
            li("1", 2001, 1),
            li("2", 2001, 2),
            li("3", 2003, 1),
            li("4", 2003, 2),
        ];
        let a = cand("A", &[("1", d(2001, 1)), ("2", d(2001, 2))]);
        let b = cand("B", &[("3", d(2003, 1)), ("4", d(2003, 2))]);
        let cover = compute_cover(&local, &[a, b], None);
        assert!(cover.uncovered.is_empty());
        let main = cover.main.unwrap();
        assert_eq!(cover.ranges.len(), 1);
        assert_ne!(cover.ranges[0].candidate, main);
        // Deterministic tie-break: the lower id wins.
        assert_eq!(main, 0);
    }

    #[test]
    fn ranges_never_swallow_numbers_the_main_lists() {
        let local = vec![li("1", 2000, 1), li("10", 2010, 1), li("12", 2010, 3)];
        let main = cand("M", &[("1", d(2000, 1)), ("11", d(2010, 2))]);
        let alt = cand("X", &[("10", d(2010, 1)), ("12", d(2010, 3))]);
        // Make M the main by giving it more local issues.
        let mut main = main;
        main.listed.push(ProviderIssue {
            external_id: None,
            number: "12".into(),
            cover_date: d(1990, 1),
        });
        let cover = compute_cover(&local, &[main, alt.clone()], Some(0));
        let bounds: Vec<(String, String)> = cover
            .ranges
            .iter()
            .map(|r| (r.low.clone(), r.high.clone()))
            .collect();
        assert_eq!(
            bounds,
            vec![("10".into(), "10".into()), ("12".into(), "12".into())]
        );
    }

    #[test]
    fn uncovered_and_specials_are_reported() {
        let local = vec![
            li("1", 2000, 1),
            li("2", 2000, 2),
            LocalCovIssue::new("Annual 1", Some(2000), None),
            li("99", 2005, 1),
        ];
        let main = cand("M", &[("1", d(2000, 1)), ("2", d(2000, 2))]);
        let ann = cand("A", &[("Annual 1", d(2000, 6))]);
        let cover = compute_cover(&local, &[main, ann], None);
        assert_eq!(cover.main, Some(0));
        assert_eq!(cover.uncovered, vec![3]);
        assert_eq!(cover.unranged, vec![2]);
        assert!(cover.ranges.is_empty());
    }

    #[test]
    fn confidence_grades() {
        let local = vec![li("1", 2000, 1)];
        let mut c = cand("M", &[("1", d(2000, 1))]);
        c.strict = true;
        let cover = compute_cover(&local, std::slice::from_ref(&c), None);
        assert_eq!(confidence(&[c.clone()], &cover).0, CoverageConfidence::High);
        let mut c2 = cand("M", &[("1", None)]);
        c2.strict = true;
        let cover = compute_cover(&local, std::slice::from_ref(&c2), None);
        assert_eq!(confidence(&[c2], &cover).0, CoverageConfidence::Medium);
        let c3 = cand("M", &[("1", None)]);
        let cover = compute_cover(&local, std::slice::from_ref(&c3), None);
        assert_eq!(confidence(&[c3], &cover).0, CoverageConfidence::Low);
    }

    #[test]
    fn half_issues_match_across_spellings() {
        // The owner's "000.5" vs ComicVine / Metron's "½".
        let local = LocalCovIssue::new("000.5", Some(1998), Some(1));
        assert_eq!(local.canonical, "0.5");
        assert_eq!(local.key, "0.5");
        let list = ProviderSeriesIssues {
            issues: vec![ProviderIssue {
                external_id: Some("h".into()),
                number: "½".into(),
                cover_date: d(1998, 1),
            }],
            ..Default::default()
        };
        let hit = provider_issue_for(&list, "0.5", Some(1998), Some(1)).unwrap();
        assert_eq!(hit.external_id.as_deref(), Some("h"));
    }

    #[test]
    fn gap_searches_take_the_longest_run_first() {
        let local = vec![
            li("0.5", 1998, 1),
            li("1", 1998, 1),
            li("600", 2012, 1),
            li("601", 2012, 2),
            li("602", 2012, 3),
        ];
        let reps = gap_representatives(&local, &[0, 2, 3, 4]);
        let firsts: Vec<&str> = reps.iter().map(|l| l.canonical.as_str()).collect();
        assert_eq!(firsts, vec!["600", "0.5"]);
    }

    #[test]
    fn provider_issue_for_prefers_the_dated_entry() {
        let list = ProviderSeriesIssues {
            issues: vec![
                ProviderIssue {
                    external_id: Some("a".into()),
                    number: "1".into(),
                    cover_date: d(1964, 4),
                },
                ProviderIssue {
                    external_id: Some("b".into()),
                    number: "1".into(),
                    cover_date: d(1998, 11),
                },
            ],
            ..Default::default()
        };
        let hit = provider_issue_for(&list, "1", Some(1998), Some(11)).unwrap();
        assert_eq!(hit.external_id.as_deref(), Some("b"));
        assert!(provider_issue_for(&list, "2", None, None).is_none());
    }

    #[test]
    fn lookup_provider_issue_names_the_miss() {
        let list = ProviderSeriesIssues {
            issues: vec![ProviderIssue {
                external_id: Some("a".into()),
                number: "1".into(),
                cover_date: d(1998, 1),
            }],
            ..Default::default()
        };
        assert!(matches!(
            lookup_provider_issue(&list, "1", Some(1998), Some(2)),
            IssueListLookup::Found {
                date: DateMatch::Confirmed,
                ..
            }
        ));
        assert!(matches!(
            lookup_provider_issue(&list, "1", None, None),
            IssueListLookup::Found {
                date: DateMatch::Unknown,
                ..
            }
        ));
        assert_eq!(
            lookup_provider_issue(&list, "1", Some(1964), Some(4)),
            IssueListLookup::DateConflict
        );
        assert_eq!(
            lookup_provider_issue(&list, "2", Some(1998), Some(2)),
            IssueListLookup::NotListed
        );
    }

    #[test]
    fn hint_counts_covered_conflicts_and_missing_runs() {
        let local = vec![
            li("½", 1998, 1),
            li("1", 1998, 1),
            li("2", 1998, 2),
            li("3", 2010, 1), // a same-numbered issue of another run
            li("500", 2003, 7),
            li("501", 2003, 8),
            li("600", 2012, 1),
        ];
        let list = ProviderSeriesIssues {
            series_name: Some("Fantastic Four".into()),
            year_began: Some(1998),
            publisher: None,
            issues: [
                ("½", d(1998, 1)),
                ("1", d(1998, 1)),
                ("2", None),
                ("3", d(1998, 3)),
            ]
            .into_iter()
            .map(|(n, dt)| ProviderIssue {
                external_id: Some(format!("i-{n}")),
                number: n.into(),
                cover_date: dt,
            })
            .collect(),
            complete: true,
            dates_complete: false,
            requests: 2,
        };
        let h = hint_from_list(Source::Metron, "1711", &local, &list);
        assert_eq!(h.status, HintStatus::Computed);
        assert_eq!(h.local_total, 7);
        assert_eq!(h.covered, 3, "½, 1 (dated) and 2 (no date to compare)");
        assert_eq!(h.date_confirmed, 2);
        assert_eq!(h.date_conflicts, 1, "#3 is listed but from 1998");
        assert_eq!(h.missing_count, 4);
        // Runs are consecutive *local* issues.
        assert_eq!(h.missing_runs, vec!["#3–600".to_owned()]);
        assert_eq!(h.listed_count, 4);
        assert!(!h.partial);
        assert_eq!(h.requests, 2);
    }

    // ───────── split parts ─────────

    #[test]
    fn split_part_name_reads_part_book_chapter_markers() {
        assert_eq!(
            split_part_name("52 / World War III Part Four: United We Stand"),
            Some(("52 / World War III".to_owned(), 4))
        );
        assert_eq!(
            split_part_name("52 / World War III Part One: A Call to Arms"),
            Some(("52 / World War III".to_owned(), 1))
        );
        assert_eq!(
            split_part_name("Crisis on Infinite Earths Book 2"),
            Some(("Crisis on Infinite Earths".to_owned(), 2))
        );
        assert_eq!(
            split_part_name("The Saga Part III - The Reckoning"),
            Some(("The Saga".to_owned(), 3))
        );
        assert_eq!(
            split_part_name("Tales: Chapter twelve of twelve"),
            Some(("Tales".to_owned(), 12))
        );
        // No marker, a marker that is part of a word, or no base name.
        assert_eq!(split_part_name("Daredevil"), None);
        assert_eq!(split_part_name("Departure 2"), None);
        assert_eq!(split_part_name("Partners 3"), None);
        assert_eq!(split_part_name("Part Two"), None);
        // The first marker wins.
        assert_eq!(
            split_part_name("Saga Part One: Book Two"),
            Some(("Saga".to_owned(), 1))
        );
    }

    #[test]
    fn part_remap_rekeys_a_one_issue_part_only() {
        let mut four = cand("13156", &[("1", d(2007, 6))]);
        four.name = Some("52 / World War III Part Four: United We Stand".into());
        apply_part_remap(&mut four);
        assert_eq!(four.part, Some(4));
        assert_eq!(four.listed[0].number, "4");

        // A part series with several issues keeps its own numbering.
        let mut multi = cand("9", &[("1", d(2007, 6)), ("2", d(2007, 7))]);
        multi.name = Some("Saga Part Two".into());
        apply_part_remap(&mut multi);
        assert_eq!(multi.part, None);
        assert_eq!(multi.listed[0].number, "1");

        // No marker: untouched.
        let mut plain = cand("1", &[("1", d(2007, 6))]);
        apply_part_remap(&mut plain);
        assert_eq!(plain.part, None);
    }

    #[test]
    fn undated_issues_refuse_a_series_decades_before_the_folder_year() {
        // 52/WW III on prod: the files carry no cover date, the folder says
        // 2007. GCD's name search also returns "World War III" (1952) #1–2;
        // by number alone it out-covers two one-issue parts.
        let mut local: Vec<LocalCovIssue> = (1..=4)
            .map(|n| LocalCovIssue::new(&n.to_string(), None, None))
            .collect();
        for l in &mut local {
            l.fallback_year = Some(2007);
        }
        let mut cands: Vec<Candidate> = (1..=4)
            .map(|n| {
                let mut c = cand(&format!("part{n}"), &[("1", d(2007, 6))]);
                c.name = Some(format!("52 / World War III Part {n}"));
                apply_part_remap(&mut c);
                c
            })
            .collect();
        let mut decoy = cand("18868", &[("1", d(1952, 3)), ("2", d(1953, 5))]);
        decoy.name = Some("World War III".into());
        cands.push(decoy);

        let cover = compute_cover(&local, &cands, None);
        assert!(cover.uncovered.is_empty(), "{cover:?}");
        for (i, a) in cover.assignment.iter().enumerate() {
            let (ci, dm) = a.expect("assigned");
            assert_eq!(cands[ci].part, Some(i as u32 + 1), "local #{}", i + 1);
            assert_eq!(dm, DateMatch::Unknown, "no local date to confirm");
        }
        assert!(
            cover.ranges.iter().all(|r| r.candidate != 4),
            "decoy unused"
        );

        // Within the slack the floor is silent: a 2005 issue is not refused.
        let li = &local[0];
        assert_eq!(li.date_match(d(2005, 1)), DateMatch::Unknown);
        assert_eq!(li.date_match(d(2003, 12)), DateMatch::Conflict);
        assert_eq!(li.date_match(None), DateMatch::Unknown);
        // A dated issue ignores the floor entirely.
        let dated = LocalCovIssue::new("1", Some(1998), Some(1));
        assert_eq!(dated.date_match(d(1998, 1)), DateMatch::Confirmed);
    }

    #[test]
    fn four_one_issue_parts_cover_a_four_issue_folder() {
        // 52/WW III: local #1–4, all June 2007; Metron files four
        // one-issue series "Part One…Four", each numbered 1.
        let local = vec![
            li("1", 2007, 6),
            li("2", 2007, 6),
            li("3", 2007, 6),
            li("4", 2007, 6),
        ];
        let names = [
            "52 / World War III Part One: A Call to Arms",
            "52 / World War III Part Two: The Valiant",
            "52 / World War III Part Three: Hell Is for Heroes",
            "52 / World War III Part Four: United We Stand",
        ];
        let mut cands: Vec<Candidate> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut c = cand(&format!("1315{}", 9 - i), &[("1", d(2007, 6))]);
                c.name = Some((*n).into());
                c.name_score = 0.35;
                c
            })
            .collect();
        for c in &mut cands {
            apply_part_remap(c);
        }
        let cover = compute_cover(&local, &cands, None);
        assert!(cover.uncovered.is_empty(), "{cover:?}");
        // Local #N is assigned to Part N, never to Part Four's "#1".
        for (i, a) in cover.assignment.iter().enumerate() {
            let (ci, _) = a.expect("assigned");
            assert_eq!(cands[ci].part, Some(i as u32 + 1), "local #{}", i + 1);
        }
        // One part is the main; the other three become one-issue ranges.
        let main = cover.main.unwrap();
        assert_eq!(cover.ranges.len(), 3);
        for r in &cover.ranges {
            assert_ne!(r.candidate, main);
            assert_eq!(r.low, r.high);
            assert_eq!(r.issues.len(), 1);
        }
        assert!(cover.unranged.is_empty());
    }
}

#[cfg(test)]
mod search_names_tests {
    use super::{MAX_ALIAS_SEARCHES, MAX_FILE_NAME_SEARCHES, search_names};
    use serde_json::json;

    #[test]
    fn file_series_names_come_right_after_the_series_name() {
        // Firestorm (2004): #1–22 say "Firestorm", #23–35 say
        // "Firestorm: The Nuclear Man" — Metron / GCD file the latter as
        // its own series, so it has to be searched for by that name.
        let names = search_names(
            "Firestorm",
            [&json!([]), &json!([])],
            &["Firestorm".into(), "Firestorm: The Nuclear Man".into()],
        );
        assert_eq!(names, vec!["Firestorm", "Firestorm: The Nuclear Man"]);
    }

    #[test]
    fn near_duplicates_collapse_and_caps_hold() {
        let names = search_names(
            "The Department of Truth",
            [&json!(["Dept. of Truth", "DoT", "Truth"]), &json!(["Alt"])],
            &[
                "Department of Truth".into(), // article-only difference → same
                "The Department of Truth".into(),
                "Dept of Truth Omnibus".into(),
                "Fourth".into(),
                "Fifth".into(),
            ],
        );
        // series name + MAX_FILE_NAME_SEARCHES file names + MAX_ALIAS_SEARCHES aliases
        assert_eq!(names.len(), 1 + MAX_FILE_NAME_SEARCHES + MAX_ALIAS_SEARCHES);
        assert_eq!(names[0], "The Department of Truth");
        assert_eq!(names[1], "Dept of Truth Omnibus");
        assert_eq!(names[2], "Fourth");
        assert_eq!(names[3], "Dept. of Truth");
        assert_eq!(names[4], "DoT");
    }

    #[test]
    fn blank_and_whitespace_entries_are_skipped() {
        let names = search_names(
            "Saga",
            [&json!(["  ", null, 7]), &json!(null)],
            &["".into()],
        );
        assert_eq!(names, vec!["Saga"]);
    }
}
