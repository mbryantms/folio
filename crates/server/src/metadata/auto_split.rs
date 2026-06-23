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
use crate::metadata::provider::{IssueQuery, MetadataProvider};
use crate::metadata::range_map;
use chrono::Utc;
use entity::{issue, series, series_provider_range};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
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

/// What the detector found for one source — surfaced by the on-demand
/// "Detect alternate series" endpoint so the operator sees the outcome
/// (and so it's debuggable without reading server logs).
#[derive(Debug, Clone, Default)]
pub struct DetectOutcome {
    /// Issue numbers the matched provider series reported (0 ⇒ the
    /// provider couldn't enumerate — lumper / unsupported / empty).
    pub covered_count: usize,
    /// Contiguous local issue ranges the matched series didn't cover,
    /// as `(low, high)` canonical bounds.
    pub gaps: Vec<(String, String)>,
    /// Range mappings written this run.
    pub created: Vec<CreatedRange>,
}

/// Maximum alternate-series candidates we'll detail-fetch per gap while
/// resolving the alternate series id. Keeps the provider budget bounded.
const MAX_DETAIL_PROBES: usize = 4;

/// Detect issue ranges of `series_row` that the matched provider series
/// (`main_series_external_id`) does NOT cover, find the provider series
/// that does, and write `series_provider_range` rows for them.
///
/// Best-effort: returns the ranges it created. Existing ranges for the
/// same `(series, source)` are never clobbered — a gap overlapping one is
/// skipped (so a user-declared mapping wins).
pub async fn detect_and_map<C: ConnectionTrait>(
    db: &C,
    series_row: &series::Model,
    source: Source,
    main_series_external_id: &str,
    provider: &dyn MetadataProvider,
) -> anyhow::Result<DetectOutcome> {
    // 1. Enumerate the matched series' coverage. Empty ⇒ provider can't
    //    enumerate (lumper / unsupported) → nothing to split against.
    let covered: HashSet<String> = provider
        .list_series_issue_numbers(main_series_external_id)
        .await?
        .into_iter()
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

    // 2. Local active issues in reading order. Project only the two
    //    columns we need — loading full issue rows would drag the large
    //    `comic_info_raw` / `pages` JSON for every issue.
    let rows: Vec<(Option<String>, Option<i32>)> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_row.id))
        .filter(issue::Column::State.eq("active"))
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Year)
        .into_tuple()
        .all(db)
        .await?;
    let local: Vec<LocalIssue> = rows
        .into_iter()
        .filter_map(|(number_raw, year)| {
            let raw = number_raw.as_deref()?.trim().to_owned();
            if raw.is_empty() {
                return None;
            }
            Some(LocalIssue {
                canonical: canonical_issue_number(&raw),
                year,
            })
        })
        .collect();

    // 3. Contiguous runs of local issues the matched series doesn't carry.
    let gaps = contiguous_uncovered(&local, &covered);
    tracing::debug!(
        source = source.as_str(),
        local = local.len(),
        gaps = gaps.len(),
        gap_bounds = ?gaps
            .iter()
            .map(|g| format!("{}..{}", g.first().unwrap().canonical, g.last().unwrap().canonical))
            .collect::<Vec<_>>(),
        "auto-split: contiguous uncovered runs"
    );
    let gap_bounds: Vec<(String, String)> = gaps
        .iter()
        .map(|g| {
            (
                g.first().unwrap().canonical.clone(),
                g.last().unwrap().canonical.clone(),
            )
        })
        .collect();
    if gaps.is_empty() {
        return Ok(DetectOutcome {
            covered_count,
            gaps: gap_bounds,
            created: Vec::new(),
        });
    }

    // Existing ranges for this (series, source) — don't clobber.
    let existing = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_row.id))
        .filter(series_provider_range::Column::Source.eq(source.as_str()))
        .all(db)
        .await?;

    let mut created = Vec::new();
    for gap in gaps {
        let (low, high) = (gap.first().unwrap(), gap.last().unwrap());
        // Skip gaps already covered by a declared mapping.
        if existing.iter().any(|r| {
            range_map::ranges_overlap(
                Some(&low.canonical),
                Some(&high.canonical),
                r.range_low.as_deref(),
                r.range_high.as_deref(),
            )
        }) {
            continue;
        }
        let alt = resolve_alternate_series(
            provider,
            &series_row.name,
            series_row.year,
            low,
            main_series_external_id,
        )
        .await?;
        tracing::debug!(
            source = source.as_str(),
            gap = format!("{}..{}", low.canonical, high.canonical),
            resolved = alt.is_some(),
            alt_series = alt.as_ref().map(|a| a.series_id.as_str()).unwrap_or("-"),
            "auto-split: gap alternate-series resolution"
        );
        let Some(alt) = alt else {
            // Couldn't identify a distinct alternate series for this gap
            // — leave it (the broad-search issue path still works).
            continue;
        };

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
        model.insert(db).await?;
        created.push(CreatedRange {
            source,
            provider_series_id: alt.series_id,
            provider_series_name: alt.series_name,
            range_low: low.canonical.clone(),
            range_high: high.canonical.clone(),
            declared_year: alt.year_began,
        });
    }
    Ok(DetectOutcome {
        covered_count,
        gaps: gap_bounds,
        created,
    })
}

struct LocalIssue {
    canonical: String,
    year: Option<i32>,
}

struct AltSeries {
    series_id: String,
    series_name: Option<String>,
    year_began: Option<i32>,
}

/// Group local issues into maximal contiguous runs (by reading order)
/// whose canonical numbers are absent from `covered`.
fn contiguous_uncovered<'a>(
    local: &'a [LocalIssue],
    covered: &HashSet<String>,
) -> Vec<Vec<&'a LocalIssue>> {
    let mut runs: Vec<Vec<&LocalIssue>> = Vec::new();
    let mut current: Vec<&LocalIssue> = Vec::new();
    for li in local {
        if covered.contains(&li.canonical) {
            if !current.is_empty() {
                runs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(li);
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Broad-search a representative gap issue and resolve which provider
/// series actually carries it (different from the matched main series).
async fn resolve_alternate_series(
    provider: &dyn MetadataProvider,
    series_name: &str,
    series_year: Option<i32>,
    representative: &LocalIssue,
    main_series_external_id: &str,
) -> anyhow::Result<Option<AltSeries>> {
    let query = IssueQuery {
        series_external_id: None,
        series_name: Some(series_name.to_owned()),
        series_year,
        issue_number: representative.canonical.clone(),
        cover_year: representative.year,
        limit: 25,
    };
    let candidates = provider.search_issue(&query).await?;
    // Prefer candidates whose number matches the representative.
    let mut ordered: Vec<_> = candidates.iter().collect();
    ordered.sort_by_key(|c| {
        let matches = c
            .issue_number
            .as_deref()
            .map(|n| canonical_issue_number(n) == representative.canonical)
            .unwrap_or(false);
        // false sorts after true → number-matches first.
        !matches
    });

    let mut probes = 0;
    for cand in ordered {
        // Cheap path: the search candidate already carries the series id.
        if let Some(sid) = cand.series_external_id.as_deref()
            && !sid.is_empty()
            && sid != main_series_external_id
        {
            return Ok(Some(AltSeries {
                series_id: sid.to_owned(),
                series_name: cand.series_name.clone(),
                year_began: cand.series_year,
            }));
        }
        // The issue *list* sometimes omits the series id (Metron) — fall
        // back to the detail endpoint, which carries it.
        if cand.series_external_id.is_none() && probes < MAX_DETAIL_PROBES {
            probes += 1;
            if let Ok(detail) = provider.fetch_issue(&cand.external_id).await
                && let Some(sid) = detail.series_external_id.as_deref()
                && !sid.is_empty()
                && sid != main_series_external_id
            {
                return Ok(Some(AltSeries {
                    series_id: sid.to_owned(),
                    series_name: detail.series_name.or_else(|| cand.series_name.clone()),
                    year_began: detail.year_began.or(cand.series_year),
                }));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn li(n: &str) -> LocalIssue {
        LocalIssue {
            canonical: n.to_owned(),
            year: None,
        }
    }

    #[test]
    fn contiguous_uncovered_finds_the_tail_block() {
        let local = vec![li("1"), li("2"), li("3"), li("600"), li("601"), li("611")];
        let covered: HashSet<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let runs = contiguous_uncovered(&local, &covered);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first().unwrap().canonical, "600");
        assert_eq!(runs[0].last().unwrap().canonical, "611");
    }

    #[test]
    fn contiguous_uncovered_splits_separate_blocks() {
        let local = vec![li("1"), li("50"), li("100"), li("200")];
        // covered = 1, 100 → two separate uncovered runs: [50], [200]
        let covered: HashSet<String> = ["1", "100"].iter().map(|s| s.to_string()).collect();
        let runs = contiguous_uncovered(&local, &covered);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0][0].canonical, "50");
        assert_eq!(runs[1][0].canonical, "200");
    }

    #[test]
    fn no_gaps_when_all_covered() {
        let local = vec![li("1"), li("2")];
        let covered: HashSet<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        assert!(contiguous_uncovered(&local, &covered).is_empty());
    }
}
