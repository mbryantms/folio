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
//!    metadata-cache bridge), then series searches on the local name and
//!    aliases with no year filter, run through [`PreFilter`] (publisher
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
//!    (`set_by = 'user'`) are never overwritten, and existing automated
//!    ranges are never deleted — stale ones are reported.
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
use crate::metadata::matcher::{self, SeriesQueryFacts, canonical_issue_number};
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
    let mut hits = list
        .issues
        .iter()
        .filter(|i| i.number == canonical_number)
        .collect::<Vec<_>>();
    hits.sort_by_key(|i| std::cmp::Reverse(date_match(year, month, i.cover_date).rank()));
    hits.into_iter()
        .find(|i| date_match(year, month, i.cover_date) != DateMatch::Conflict)
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

// ───────── local issues ─────────

/// One local active issue with a number.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalCovIssue {
    pub canonical: String,
    /// Numeric value for plain numbers; `None` for annuals / `14AU` / `½`.
    pub value: Option<f64>,
    pub year: Option<i32>,
    pub month: Option<i32>,
}

impl LocalCovIssue {
    pub fn new(raw: &str, year: Option<i32>, month: Option<i32>) -> Self {
        let canonical = canonical_issue_number(raw);
        let value = numeric_value(&canonical);
        Self {
            canonical,
            value,
            year,
            month: month.filter(|m| (1..=12).contains(m)),
        }
    }
}

/// Active local issues with a number, deduplicated by canonical number,
/// in numeric order (numbers first, then specials in reading order).
pub async fn load_local<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<Vec<LocalCovIssue>, sea_orm::DbErr> {
    let rows: Vec<(Option<String>, Option<i32>, Option<i32>)> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Year)
        .column(issue::Column::Month)
        .into_tuple()
        .all(db)
        .await?;
    let mut out: Vec<LocalCovIssue> = Vec::new();
    for (raw, year, month) in rows {
        let Some(raw) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let li = LocalCovIssue::new(raw, year, month);
        match out.iter_mut().find(|o| o.canonical == li.canonical) {
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
    Ok(out)
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
                let Some(dates) = listing.get(li.canonical.as_str()) else {
                    continue;
                };
                let best = dates
                    .iter()
                    .map(|d| date_match(li.year, li.month, *d))
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
    pub local: Vec<LocalCovIssue>,
    pub names: Vec<String>,
    pub ext_ids: Vec<external_id::Model>,
    pub ranges: Vec<series_provider_range::Model>,
    pub applied: Vec<(Source, String)>,
    pub pre_filter: PreFilter,
}

impl SeriesFacts {
    pub async fn load(state: &AppState, series_row: &series::Model) -> anyhow::Result<Self> {
        let db = &state.db;
        let local = load_local(db, series_row.id).await?;
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
        let mut names = vec![series_row.name.clone()];
        for v in [&series_row.aliases, &series_row.alternate_names] {
            if let Some(arr) = v.as_array() {
                for a in arr.iter().filter_map(|x| x.as_str()) {
                    let a = a.trim();
                    if !a.is_empty()
                        && !names
                            .iter()
                            .any(|n| matcher::name_similarity(n, a) >= 0.999)
                    {
                        names.push(a.to_owned());
                    }
                }
            }
        }
        names.truncate(1 + MAX_ALIAS_SEARCHES);
        Ok(Self {
            series: series_row.clone(),
            local,
            names,
            ext_ids,
            ranges,
            applied,
            pre_filter,
        })
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

    /// The local series' start year for the strict check.
    fn start_year(&self) -> Option<i32> {
        self.series.year.or(self.year_bounds().0)
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
/// runs first so its `cv_id` / `gcd_id` seed the other two.
pub async fn analyze(state: &AppState, facts: &SeriesFacts) -> Vec<ProviderAnalysis> {
    let started = Instant::now();
    let mut bridged: HashMap<Source, String> = HashMap::new();
    // Free cache bridge for every target.
    let known_pairs: Vec<(Source, String)> = facts
        .ext_ids
        .iter()
        .filter_map(|e| Some((Source::from_str(&e.source).ok()?, e.external_id.clone())))
        .chain(facts.applied.iter().cloned())
        .collect();
    for target in COVERAGE_SOURCES {
        if known_pairs.iter().any(|(s, _)| *s == target) {
            continue;
        }
        if let Some((id, _)) =
            crate::metadata::series_link::cache_bridge(&state.db, target, &known_pairs).await
        {
            bridged.insert(target, id);
        }
    }
    // Metron cross-reference (≤ 2 requests): the row carries the CV and
    // GCD ids Metron's editors curated.
    let mut metron_prespent = 0u32;
    if let Some(metron) = crate::metadata::apply::build_provider(state, Source::Metron)
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
                        bridged.insert(Source::Metron, hit.external_id.clone());
                        for ident in &hit.identifiers {
                            if ident.source != *src && !ident.id.is_empty() {
                                bridged.entry(ident.source).or_insert(ident.id.clone());
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

    let futs = COVERAGE_SOURCES.iter().map(|source| {
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
                bridged.get(source).cloned(),
                budget,
            )
            .await
        }
    });
    futures::future::join_all(futs).await
}

/// Analyse one provider. Public so tests drive it with a wiremock-backed
/// client.
pub async fn analyze_provider<C: ConnectionTrait>(
    redis: &ConnectionManager,
    db: &C,
    facts: &SeriesFacts,
    provider: &dyn MetadataProvider,
    bridged_id: Option<String>,
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
    if let Some(id) = bridged_id {
        push(&mut pending, blank(&id, CandidateOrigin::Bridge));
    }

    // 2. Series searches (name + aliases, no year filter).
    let (_, max_year) = facts.year_bounds();
    let query_facts = SeriesQueryFacts {
        name: facts.series.name.clone(),
        year: facts.start_year(),
        publisher: facts.series.publisher.clone(),
        volume: None,
        format: facts.series.series_type.clone(),
    };
    let gate_facts = SeriesQueryFacts {
        year: max_year.or(facts.series.year),
        ..query_facts.clone()
    };
    let mut hits: Vec<(SeriesCandidate, f32, f32, bool)> = Vec::new();
    for name in &facts.names {
        if pending.len() >= MAX_CANDIDATES || !budget.can_spend(search_cost(source)) {
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
            let ns = facts.best_name_score(&c.name);
            if ns < NAME_FLOOR {
                continue;
            }
            let score = matcher::score_series(&query_facts, &c);
            let strict = ns >= 0.999
                && query_facts.year.is_some()
                && c.year == query_facts.year
                && score.publisher > 0.0
                && !score.format_mismatch;
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
            .find(|p| p.provider_series_id == c.external_id)
        {
            p.name = p.name.clone().or(Some(c.name.clone()));
            p.year = p.year.or(c.year);
            p.publisher = p.publisher.clone().or(c.publisher.clone());
            p.name_score = p.name_score.max(ns);
            p.strict |= strict;
            continue;
        }
        if pending.len() >= MAX_CANDIDATES {
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
        });
    }
    pending.truncate(MAX_CANDIDATES);

    // 3. List every candidate.
    let mut listed: Vec<Candidate> = Vec::new();
    if out.status == CoverageStatus::Analyzed {
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
    if out.status == CoverageStatus::Analyzed {
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
            budget.used += 1;
            let Ok(cands) = res else { continue };
            for ic in cands {
                if ic
                    .issue_number
                    .as_deref()
                    .is_some_and(|n| canonical_issue_number(n) != li.canonical)
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

/// The first issue of each contiguous run of uncovered local issues
/// (numeric ones first), for gap issue searches.
fn gap_representatives<'a>(
    local: &'a [LocalCovIssue],
    uncovered: &[usize],
) -> Vec<&'a LocalCovIssue> {
    let mut out: Vec<&LocalCovIssue> = Vec::new();
    let mut prev: Option<usize> = None;
    for &i in uncovered {
        if local[i].value.is_none() {
            continue;
        }
        if prev.is_none_or(|p| p + 1 != i) {
            out.push(&local[i]);
        }
        prev = Some(i);
    }
    out
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
    let local_numbers: HashSet<&str> = facts.local.iter().map(|l| l.canonical.as_str()).collect();
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
        cand.listed = list
            .issues
            .into_iter()
            .filter(|i| {
                local_numbers.contains(i.number.as_str())
                    || numeric_value(&i.number)
                        .zip(span)
                        .is_some_and(|(v, (lo, hi))| v >= lo && v <= hi)
            })
            .collect();
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
    /// Accepting would change something (main id or new ranges).
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
) -> ProviderCoverageView {
    let source = analysis.source;
    let cands = &analysis.candidates;
    // A user-set series id is the main unless the admin explicitly chose
    // another candidate: the proposal is built around the user's link
    // instead of contradicting it.
    let user_link = ext_ids
        .iter()
        .find(|e| Source::from_str(&e.source).ok() == Some(source) && e.set_by == "user")
        .map(|e| e.external_id.as_str());
    let forced = forced_main
        .or(user_link)
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
                    .filter(|p| p.number == li.canonical)
                    .max_by_key(|p| date_match(li.year, li.month, p.cover_date).rank());
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
        if e.set_by == "user" {
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
                        p.number == li.canonical
                            && date_match(li.year, li.month, p.cover_date) != DateMatch::Conflict
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
    let has_changes = main_changes
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
    /// (reported, never deleted).
    pub stale_ranges: Vec<CoverageRangeRef>,
}

/// Accept one provider's proposal: write the main id (unless the user
/// linked another series) and every `new` range. `by_user` records the
/// main id as `user` (an admin confirmed it); automatic acceptance uses
/// `SetBy::Provider`. Existing rows are never overwritten or deleted.
pub async fn accept_provider(
    state: &AppState,
    series_id: Uuid,
    analysis: &ProviderAnalysis,
    forced_main: Option<&str>,
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
    let view = build_view(analysis, &local, &ext_ids, &ranges, forced_main);
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
            "kept your linked series #{} (user-set ids are never overwritten); change it under External IDs first",
            u.external_id
        ));
        out.ranges_skipped = view.proposed_ranges;
        out.stale_ranges = view.stale_ranges;
        return Ok(out);
    }

    if let Some(main) = view.main_series_id.as_deref() {
        match user_row {
            Some(u) if u.external_id != main => {
                out.main_note = Some(format!(
                    "kept your linked series #{} (user-set ids are never overwritten)",
                    u.external_id
                ));
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
    Ok(out)
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
}
