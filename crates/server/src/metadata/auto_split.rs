//! Auto-detection of provider series-boundary splits.
//!
//! When a local series is matched to a *splitter* provider's main series
//! (e.g. Metron's "Fantastic Four" 1998 run), some local issues may not
//! belong to that provider series at all — they're a separate, often
//! legacy-renumbered relaunch the provider files as its own series
//! ("Fantastic Four (2012)", #600–611). The series search can't surface
//! that relaunch (it's year-gated out), so we discover it here and write
//! the [`entity::series_provider_range`] mapping automatically.
//!
//! Bounded by design: one paginated enumeration of the matched series'
//! issue numbers, then — per contiguous uncovered block — one broad
//! issue search plus a small number of issue-detail fetches to resolve
//! the alternate series' id. Lumper providers (ComicVine) return no
//! issue-number list (the trait default), so this no-ops for them.

use crate::metadata::identifier::Source;
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::provider::{IssueQuery, MetadataProvider, ProviderError};
use crate::metadata::range_map;
use chrono::Utc;
use entity::{issue, series, series_provider_range};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::Serialize;
use std::collections::HashSet;
use uuid::Uuid;

/// A range mapping created by the detector.
#[derive(Debug, Clone)]
pub struct CreatedRange {
    pub source: Source,
    pub provider_series_id: String,
    pub provider_series_name: Option<String>,
    pub range_low: String,
    pub range_high: String,
    pub declared_year: Option<i32>,
}

/// What happened to one uncovered run of local issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GapStatus {
    /// A range mapping was written this run.
    Mapped,
    /// An existing mapping for this provider already covers the run.
    AlreadyMapped,
    /// No distinct provider series could be confirmed for the run (the
    /// provider doesn't carry those issues, or the candidate series
    /// didn't list them). Left alone; issue search falls back to the
    /// broad path for them.
    Unresolved,
    /// Not attempted: the per-click gap budget ran out, or an earlier
    /// gap hit the provider's rate limit.
    Skipped,
    /// The provider call for this run failed.
    Error,
}

/// The outcome for one uncovered run.
#[derive(Debug, Clone)]
pub struct GapOutcome {
    /// Inclusive canonical bounds (numeric order).
    pub low: String,
    pub high: String,
    pub issue_count: usize,
    pub status: GapStatus,
    /// The alternate provider series (mapped now or already mapped).
    pub provider_series_id: Option<String>,
    pub provider_series_name: Option<String>,
    pub error: Option<String>,
}

/// What the detector found for one source — surfaced by the on-demand
/// "Detect from providers" endpoint so the operator sees the outcome
/// (and so it's debuggable without reading server logs).
#[derive(Debug, Clone, Default)]
pub struct DetectOutcome {
    /// Distinct issue numbers the matched provider series reported (0 ⇒
    /// the provider couldn't enumerate it).
    pub covered_count: usize,
    /// Local numbered issues the matched series lists.
    pub matched_local: usize,
    /// Contiguous local issue runs the matched series didn't cover, as
    /// `(low, high)` canonical bounds.
    pub gaps: Vec<(String, String)>,
    /// Per-run detail, aligned with `gaps`.
    pub gap_outcomes: Vec<GapOutcome>,
    /// Range mappings written this run.
    pub created: Vec<CreatedRange>,
    /// Automated (non-`user`) ranges of this source whose issues the
    /// matched series now lists itself — the mapping looks stale (e.g.
    /// the series was re-matched). Reported, never deleted here.
    pub stale_range_ids: Vec<Uuid>,
    /// Uncovered local issues with a non-numeric number (annuals, `14AU`,
    /// `½`). They can't form a numeric range, so they're excluded from
    /// gap detection and only counted here.
    pub uncovered_specials: usize,
    /// Set when a provider error stopped detection part-way (e.g. the
    /// rate limit). Rows written before it stay written and are listed
    /// in `created`.
    pub error: Option<String>,
}

/// Maximum alternate-series candidates we'll detail-fetch per gap while
/// resolving the alternate series id. Keeps the provider budget bounded.
const MAX_DETAIL_PROBES: usize = 4;

/// Maximum uncovered runs resolved per source per detection. Each costs a
/// broad issue search, up to [`MAX_DETAIL_PROBES`] detail fetches and one
/// enumeration of the alternate series; the rest are reported `skipped`.
pub const MAX_GAPS_RESOLVED: usize = 3;

/// Detect issue ranges of `series_row` that the matched provider series
/// (`main_series_external_id`) does NOT cover, find the provider series
/// that does, and write `series_provider_range` rows for them.
///
/// Best-effort: returns the ranges it created. Existing ranges for the
/// same `(series, source)` are never clobbered — a gap overlapping one is
/// reported `already_mapped` (so a user-declared mapping wins).
pub async fn detect_and_map<C: ConnectionTrait>(
    db: &C,
    series_row: &series::Model,
    source: Source,
    main_series_external_id: &str,
    provider: &dyn MetadataProvider,
) -> anyhow::Result<DetectOutcome> {
    // 1. Enumerate the matched series' coverage. Empty ⇒ provider can't
    //    enumerate (lumper / unsupported) → nothing to split against.
    let covered: Vec<String> = provider
        .list_series_issue_numbers(main_series_external_id)
        .await?;
    detect_with_coverage(
        db,
        series_row,
        source,
        main_series_external_id,
        &covered,
        provider,
    )
    .await
}

/// [`detect_and_map`] with the matched series' issue numbers already in
/// hand (the provider-link resolver enumerates a candidate to confirm it,
/// so detection reuses that list instead of paying for it twice).
pub async fn detect_with_coverage<C: ConnectionTrait>(
    db: &C,
    series_row: &series::Model,
    source: Source,
    main_series_external_id: &str,
    covered_numbers: &[String],
    provider: &dyn MetadataProvider,
) -> anyhow::Result<DetectOutcome> {
    let covered: HashSet<String> = covered_numbers
        .iter()
        .map(|n| canonical_issue_number(n))
        .collect();
    let covered_count = covered.len();
    tracing::debug!(
        source = source.as_str(),
        main_series = main_series_external_id,
        covered = covered_count,
        "auto-split: enumerated matched-series coverage"
    );
    if covered.is_empty() {
        // No issue list (lumper / unsupported / empty response) — can't
        // reason about gaps, so there's nothing to split.
        return Ok(DetectOutcome::default());
    }

    // 2. Local active issues. Project only the two columns we need —
    //    loading full issue rows would drag the large `comic_info_raw` /
    //    `pages` JSON for every issue.
    let local = load_local_issues(db, series_row.id).await?;
    let matched_local = local
        .iter()
        .filter(|li| covered.contains(&li.canonical))
        .count();
    let uncovered_specials = local
        .iter()
        .filter(|li| li.value.is_none() && !covered.contains(&li.canonical))
        .count();

    // Existing ranges for this (series, source) — never clobbered; a
    // non-user one the matched series now covers is reported stale.
    let existing = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_row.id))
        .filter(series_provider_range::Column::Source.eq(source.as_str()))
        .all(db)
        .await?;
    let stale_range_ids = stale_ranges(&existing, &local, &covered, main_series_external_id);

    // 3. Contiguous runs of local issues the matched series doesn't carry.
    let gaps = numeric_gaps(&local, &covered);
    let gap_bounds: Vec<(String, String)> = gaps
        .iter()
        .map(|g| {
            (
                g.first().unwrap().canonical.clone(),
                g.last().unwrap().canonical.clone(),
            )
        })
        .collect();
    tracing::debug!(
        source = source.as_str(),
        local = local.len(),
        gaps = ?gap_bounds,
        "auto-split: contiguous uncovered runs"
    );

    let mut outcome = DetectOutcome {
        covered_count,
        matched_local,
        gaps: gap_bounds,
        gap_outcomes: Vec::new(),
        created: Vec::new(),
        stale_range_ids,
        uncovered_specials,
        error: None,
    };

    let mut attempted = 0usize;
    let mut halted = false;
    for gap in gaps {
        let (low, high) = (gap.first().unwrap(), gap.last().unwrap());
        let mut go = GapOutcome {
            low: low.canonical.clone(),
            high: high.canonical.clone(),
            issue_count: gap.len(),
            status: GapStatus::Unresolved,
            provider_series_id: None,
            provider_series_name: None,
            error: None,
        };
        // Already covered by a declared mapping (user or automated).
        if let Some(r) = existing.iter().find(|r| {
            range_map::ranges_overlap(
                Some(&low.canonical),
                Some(&high.canonical),
                r.range_low.as_deref(),
                r.range_high.as_deref(),
            )
        }) {
            go.status = GapStatus::AlreadyMapped;
            go.provider_series_id = Some(r.provider_series_id.clone());
            go.provider_series_name = r.provider_series_name.clone();
            outcome.gap_outcomes.push(go);
            continue;
        }
        if halted || attempted >= MAX_GAPS_RESOLVED {
            go.status = GapStatus::Skipped;
            outcome.gap_outcomes.push(go);
            continue;
        }
        attempted += 1;

        let alt = match resolve_alternate_series(
            provider,
            &series_row.name,
            series_row.year,
            &gap,
            main_series_external_id,
        )
        .await
        {
            Ok(alt) => alt,
            Err(e) => {
                tracing::info!(
                    source = source.as_str(),
                    gap = format!("{}..{}", low.canonical, high.canonical),
                    error = %e,
                    "auto-split: gap resolution failed"
                );
                // A rate-limit / auth failure will fail every later gap
                // too — stop spending and say so.
                if matches!(
                    e,
                    ProviderError::QuotaExceeded { .. } | ProviderError::Unauthorized(_)
                ) {
                    halted = true;
                    outcome.error = Some(e.to_string());
                }
                go.status = GapStatus::Error;
                go.error = Some(e.to_string());
                outcome.gap_outcomes.push(go);
                continue;
            }
        };
        tracing::debug!(
            source = source.as_str(),
            gap = format!("{}..{}", low.canonical, high.canonical),
            resolved = alt.is_some(),
            alt_series = alt.as_ref().map(|a| a.series_id.as_str()).unwrap_or("-"),
            "auto-split: gap alternate-series resolution"
        );
        let Some(alt) = alt else {
            // Couldn't confirm a distinct alternate series for this gap
            // — leave it (the broad-search issue path still works).
            outcome.gap_outcomes.push(go);
            continue;
        };
        go.provider_series_id = Some(alt.series_id.clone());
        go.provider_series_name = alt.series_name.clone();

        let now = Utc::now().fixed_offset();
        let model = series_provider_range::ActiveModel {
            id: Set(Uuid::new_v4()),
            series_id: Set(series_row.id),
            source: Set(source.as_str().to_owned()),
            provider_series_id: Set(alt.series_id.clone()),
            provider_series_url: Set(crate::metadata::identifier::canonical_url(
                source,
                "series",
                &alt.series_id,
            )),
            provider_series_name: Set(alt.series_name.clone()),
            range_low: Set(Some(low.canonical.clone())),
            range_high: Set(Some(high.canonical.clone())),
            declared_year: Set(alt.year_began),
            // Auto-detected — not 'user', so a later refresh / a user edit
            // can override it.
            set_by: Set("cross_reference".to_owned()),
            first_set_at: Set(now),
            last_synced_at: Set(now),
        };
        // Tolerate a unique-index conflict (a concurrent apply-hook /
        // detect already wrote this exact mapping) — report it as
        // already mapped without aborting detection of the other gaps.
        if let Err(e) = model.insert(db).await {
            tracing::info!(
                series_id = %series_row.id,
                source = source.as_str(),
                gap = format!("{}..{}", low.canonical, high.canonical),
                error = %e,
                "auto-split: range insert skipped (already exists / conflict)"
            );
            go.status = GapStatus::AlreadyMapped;
            outcome.gap_outcomes.push(go);
            continue;
        }
        go.status = GapStatus::Mapped;
        outcome.gap_outcomes.push(go);
        outcome.created.push(CreatedRange {
            source,
            provider_series_id: alt.series_id,
            provider_series_name: alt.series_name,
            range_low: low.canonical.clone(),
            range_high: high.canonical.clone(),
            declared_year: alt.year_began,
        });
    }
    Ok(outcome)
}

/// One local issue: canonical number, its numeric value (`None` for
/// annuals / letter suffixes / vulgar fractions), cover year.
#[derive(Clone, Debug)]
pub(crate) struct LocalIssue {
    pub(crate) canonical: String,
    pub(crate) value: Option<f64>,
    pub(crate) year: Option<i32>,
}

impl LocalIssue {
    pub(crate) fn new(raw: &str, year: Option<i32>) -> Self {
        let canonical = canonical_issue_number(raw);
        let value = numeric_value(&canonical);
        Self {
            canonical,
            value,
            year,
        }
    }
}

/// A canonical number's numeric value, when it is a plain number
/// (`"600"`, `"600.1"`, `"-1"`). `NaN` / `inf` don't count.
fn numeric_value(canonical: &str) -> Option<f64> {
    canonical.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Active local issues with a number, in reading order.
pub(crate) async fn load_local_issues<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<Vec<LocalIssue>, sea_orm::DbErr> {
    let rows: Vec<(Option<String>, Option<i32>)> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Year)
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(number_raw, year)| {
            let raw = number_raw.as_deref()?.trim();
            (!raw.is_empty()).then(|| LocalIssue::new(raw, year))
        })
        .collect())
}

struct AltSeries {
    series_id: String,
    series_name: Option<String>,
    year_began: Option<i32>,
}

/// Group the **numerically numbered** local issues into maximal runs the
/// matched series doesn't cover, in numeric order.
///
/// - Only plain numbers take part: a range row's bounds are compared
///   numerically ([`range_map::issue_in_range`]), so a run ending in
///   `"Annual 1"` would match nothing past its first bound. Annuals and
///   letter-suffixed numbers are counted separately instead.
/// - Runs are cut at every covered local issue *and* at every number the
///   matched series lists between two uncovered local issues, so a
///   written `[low, high]` range never swallows an issue the main series
///   carries (even one not in the library yet).
/// - Duplicate numbers (two files of `#600`) count once.
fn numeric_gaps<'a>(
    local: &'a [LocalIssue],
    covered: &HashSet<String>,
) -> Vec<Vec<&'a LocalIssue>> {
    let mut numeric: Vec<&LocalIssue> = local.iter().filter(|li| li.value.is_some()).collect();
    numeric.sort_by(|a, b| {
        a.value
            .partial_cmp(&b.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    numeric.dedup_by(|a, b| a.canonical == b.canonical);

    let mut covered_vals: Vec<f64> = covered.iter().filter_map(|c| numeric_value(c)).collect();
    covered_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Is any covered number strictly between `lo` and `hi`?
    let covered_between = |lo: f64, hi: f64| {
        let i = covered_vals.partition_point(|v| *v <= lo);
        covered_vals.get(i).is_some_and(|v| *v < hi)
    };

    let mut runs: Vec<Vec<&LocalIssue>> = Vec::new();
    let mut current: Vec<&LocalIssue> = Vec::new();
    for li in numeric {
        if covered.contains(&li.canonical) {
            if !current.is_empty() {
                runs.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some(prev) = current.last()
            && covered_between(prev.value.unwrap(), li.value.unwrap())
        {
            runs.push(std::mem::take(&mut current));
        }
        current.push(li);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Automated ranges whose local issues the matched series now lists
/// itself (or that point at the matched series) — candidates for removal.
fn stale_ranges(
    existing: &[series_provider_range::Model],
    local: &[LocalIssue],
    covered: &HashSet<String>,
    main_series_external_id: &str,
) -> Vec<Uuid> {
    existing
        .iter()
        .filter(|r| r.set_by != "user")
        .filter(|r| {
            if r.provider_series_id == main_series_external_id {
                return true;
            }
            let inside: Vec<&LocalIssue> = local
                .iter()
                .filter(|li| {
                    range_map::issue_in_range(
                        &li.canonical,
                        r.range_low.as_deref(),
                        r.range_high.as_deref(),
                    )
                })
                .collect();
            !inside.is_empty() && inside.iter().all(|li| covered.contains(&li.canonical))
        })
        .map(|r| r.id)
        .collect()
}

/// Broad-search a representative gap issue and resolve which provider
/// series actually carries the gap (different from the matched main
/// series). A candidate series is only accepted once its own issue list
/// carries at least half of the gap — a same-named series that merely
/// has *a* `#600` isn't enough.
async fn resolve_alternate_series(
    provider: &dyn MetadataProvider,
    series_name: &str,
    series_year: Option<i32>,
    gap: &[&LocalIssue],
    main_series_external_id: &str,
) -> Result<Option<AltSeries>, ProviderError> {
    let representative = gap[0];
    let query = IssueQuery {
        series_external_id: None,
        series_name: Some(series_name.to_owned()),
        series_year,
        issue_number: representative.canonical.clone(),
        cover_year: representative.year,
        limit: 25,
    };
    let candidates = provider.search_issue(&query).await?;
    // Only candidates for the representative's number (or with no number
    // at all) can identify the gap's series.
    let ordered: Vec<_> = candidates
        .iter()
        .filter(|c| {
            c.issue_number
                .as_deref()
                .is_none_or(|n| canonical_issue_number(n) == representative.canonical)
        })
        .collect();

    let mut probes = 0;
    let mut tried: HashSet<String> = HashSet::new();
    for cand in ordered {
        // Cheap path: the search candidate already carries the series id.
        let alt = match cand.series_external_id.as_deref().filter(|s| !s.is_empty()) {
            Some(sid) => AltSeries {
                series_id: sid.to_owned(),
                series_name: cand.series_name.clone(),
                year_began: cand.series_year,
            },
            // The issue *list* sometimes omits the series id (Metron) —
            // fall back to the detail endpoint, which carries it.
            None if probes < MAX_DETAIL_PROBES => {
                probes += 1;
                let Ok(detail) = provider.fetch_issue(&cand.external_id).await else {
                    continue;
                };
                let Some(sid) = detail.series_external_id.filter(|s| !s.is_empty()) else {
                    continue;
                };
                AltSeries {
                    series_id: sid,
                    series_name: detail.series_name.or_else(|| cand.series_name.clone()),
                    year_began: detail.year_began.or(cand.series_year),
                }
            }
            None => continue,
        };
        if alt.series_id == main_series_external_id || !tried.insert(alt.series_id.clone()) {
            continue;
        }
        // Confirm: the alternate series must list most of the gap.
        let listed: HashSet<String> = provider
            .list_series_issue_numbers(&alt.series_id)
            .await?
            .iter()
            .map(|n| canonical_issue_number(n))
            .collect();
        let hits = gap
            .iter()
            .filter(|li| listed.contains(&li.canonical))
            .count();
        if hits * 2 >= gap.len() {
            return Ok(Some(alt));
        }
        tracing::debug!(
            alt_series = alt.series_id,
            hits,
            gap = gap.len(),
            "auto-split: candidate series doesn't carry the gap; trying the next"
        );
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn li(n: &str) -> LocalIssue {
        LocalIssue::new(n, None)
    }

    fn set(ns: &[&str]) -> HashSet<String> {
        ns.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn contiguous_uncovered_finds_the_tail_block() {
        let local = vec![li("1"), li("2"), li("3"), li("600"), li("601"), li("611")];
        let covered: HashSet<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let runs = numeric_gaps(&local, &covered);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first().unwrap().canonical, "600");
        assert_eq!(runs[0].last().unwrap().canonical, "611");
    }

    #[test]
    fn contiguous_uncovered_splits_separate_blocks() {
        let local = vec![li("1"), li("50"), li("100"), li("200")];
        // covered = 1, 100 → two separate uncovered runs: [50], [200]
        let covered: HashSet<String> = ["1", "100"].iter().map(|s| s.to_string()).collect();
        let runs = numeric_gaps(&local, &covered);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0][0].canonical, "50");
        assert_eq!(runs[1][0].canonical, "200");
    }

    #[test]
    fn no_gaps_when_all_covered() {
        let local = vec![li("1"), li("2")];
        let covered: HashSet<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        assert!(numeric_gaps(&local, &covered).is_empty());
    }

    #[test]
    fn annuals_and_letter_suffixes_never_join_a_numeric_run() {
        // Pre-fix, a run ending in "Annual 1" wrote range 600..Annual 1,
        // which `issue_in_range` can never match past its low bound.
        let local = vec![li("1"), li("600"), li("601"), li("Annual 1"), li("14AU")];
        let runs = numeric_gaps(&local, &set(&["1"]));
        assert_eq!(runs.len(), 1);
        let bounds: Vec<&str> = runs[0].iter().map(|l| l.canonical.as_str()).collect();
        assert_eq!(bounds, ["600", "601"]);
    }

    #[test]
    fn runs_follow_numeric_order_not_reading_order() {
        // Reading order (sort_number) put #611 before #600 — the run's
        // bounds must still be low ≤ high.
        let local = vec![li("1"), li("611"), li("600"), li("605")];
        let runs = numeric_gaps(&local, &set(&["1"]));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first().unwrap().canonical, "600");
        assert_eq!(runs[0].last().unwrap().canonical, "611");
    }

    #[test]
    fn a_covered_number_the_library_lacks_still_cuts_the_run() {
        // The main series lists #602, which isn't in the library: a
        // 600..604 range would wrongly route a later-acquired #602.
        let local = vec![li("1"), li("600"), li("601"), li("603"), li("604")];
        let runs = numeric_gaps(&local, &set(&["1", "602"]));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].last().unwrap().canonical, "601");
        assert_eq!(runs[1].first().unwrap().canonical, "603");
    }

    #[test]
    fn decimals_and_duplicates_are_handled() {
        let local = vec![li("600"), li("600"), li("600.1"), li("601"), li("1")];
        let runs = numeric_gaps(&local, &set(&["1"]));
        assert_eq!(runs.len(), 1);
        let nums: Vec<&str> = runs[0].iter().map(|l| l.canonical.as_str()).collect();
        assert_eq!(nums, ["600", "600.1", "601"]);
    }

    fn range(
        id: u128,
        set_by: &str,
        sid: &str,
        lo: &str,
        hi: &str,
    ) -> series_provider_range::Model {
        series_provider_range::Model {
            id: Uuid::from_u128(id),
            series_id: Uuid::nil(),
            source: "metron".into(),
            provider_series_id: sid.into(),
            provider_series_url: None,
            provider_series_name: None,
            range_low: Some(lo.into()),
            range_high: Some(hi.into()),
            declared_year: None,
            set_by: set_by.into(),
            first_set_at: Utc::now().into(),
            last_synced_at: Utc::now().into(),
        }
    }

    #[test]
    fn stale_ranges_are_automated_rows_the_main_series_now_covers() {
        let local = vec![li("1"), li("600"), li("601"), li("700")];
        let covered = set(&["1", "600", "601"]);
        let existing = vec![
            // Main series now lists 600–601 → stale.
            range(1, "cross_reference", "ALT", "600", "601"),
            // A user row is never reported.
            range(2, "user", "ALT", "600", "601"),
            // 700 still uncovered → live.
            range(3, "cross_reference", "ALT2", "700", "700"),
            // Points at the main series itself → stale.
            range(4, "cross_reference", "MAIN", "800", "900"),
        ];
        let stale = stale_ranges(&existing, &local, &covered, "MAIN");
        assert_eq!(stale, vec![Uuid::from_u128(1), Uuid::from_u128(4)]);
    }
}
