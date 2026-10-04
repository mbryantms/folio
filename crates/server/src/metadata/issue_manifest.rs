//! Exact expected-issue sets for the series collection report, from the
//! providers' own issue lists ("provider manifest").
//!
//! Without it the report interpolates: every integer between the lowest and
//! highest owned issue counts as missing, so a Fantastic Four folder holding
//! #1–70 and the legacy-renumbered #500–611 "misses" #71–499. With accepted
//! series coverage the expected set is known per provider:
//!
//! - **Segments** per provider: the series-level `external_ids` row (the
//!   main provider series) minus the numbers its `series_provider_range`
//!   rows route elsewhere, plus each range's provider series restricted to
//!   the range's `[low, high]` — the same folding as
//!   [`range_map::fold_targets`](crate::metadata::range_map).
//! - **Lists** come from the shared 24 h issue-list cache
//!   ([`coverage::cached_provider_issues`]), **cache only**: the report
//!   request never fetches. A provider with an uncached segment is "not
//!   loaded" ("run Analyze coverage").
//! - **Agreement** (owner decision pending; this is the recommended
//!   default): a number nobody owns is **missing** only when every provider
//!   with accepted coverage lists it; one only some providers list (or that
//!   a not-loaded provider can't confirm) is **possibly missing**, with each
//!   provider's view.
//!
//! When no provider list is loaded the report keeps its interpolated
//! fallback (`expected_source = "series_total"`).

use crate::metadata::coverage::{self, COVERAGE_SOURCES};
use crate::metadata::identifier::Source;
use crate::metadata::matcher::issue_number_compare_key;
use crate::metadata::provider::ProviderSeriesIssues;
use crate::metadata::range_map::issue_in_range;
use entity::{external_id, series_provider_range};
use redis::aio::ConnectionManager;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use uuid::Uuid;

/// How one provider sees a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManifestListing {
    /// The provider's accepted series lists it.
    Listed,
    /// The provider's lists are loaded and don't carry it.
    NotListed,
    /// The provider has accepted coverage but its list isn't cached.
    NotLoaded,
}

/// One provider series a provider's expected set is read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ManifestSeriesRef {
    pub provider_series_id: String,
    /// From the cached list (or the range row).
    pub name: Option<String>,
    pub year: Option<i32>,
    /// `true` for a `series_provider_range` segment.
    pub via_range: bool,
    /// The range bounds (canonical numbers) for a range segment.
    pub range_low: Option<String>,
    pub range_high: Option<String>,
    /// The issue list is in the 24 h cache.
    pub loaded: bool,
}

/// One provider with accepted coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ManifestProvider {
    pub source: String,
    /// Every segment's list is cached (only then does it vote).
    pub loaded: bool,
    pub series: Vec<ManifestSeriesRef>,
    /// Distinct numbers its segments list (0 when not loaded).
    pub listed_count: usize,
}

/// One provider's view of a possibly-missing number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ManifestProviderView {
    pub source: String,
    pub listing: ManifestListing,
}

/// A number some (not all) providers with accepted coverage list, not
/// owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct PossiblyMissingIssue {
    /// As the provider writes it ("½", "605.1").
    pub number: String,
    pub providers: Vec<ManifestProviderView>,
}

/// The provider manifest behind a collection report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ProviderManifestView {
    /// `true` when at least one provider's lists are loaded, i.e. the
    /// report's `expected_source` is `provider_manifest`.
    pub used: bool,
    pub providers: Vec<ManifestProvider>,
    /// Numbers every provider with accepted coverage lists and no local
    /// issue carries, in number order.
    pub missing: Vec<String>,
    /// Numbers only some of them list (or a not-loaded provider can't
    /// confirm), with each provider's view.
    pub possibly_missing: Vec<PossiblyMissingIssue>,
    /// e.g. "Metron provider list not loaded — run Analyze coverage".
    pub note: Option<String>,
}

impl ProviderManifestView {
    /// `missing` numbers that are main-run integers (the grid's chips).
    pub fn missing_ints(&self) -> Vec<i64> {
        self.missing.iter().filter_map(|n| as_int(n)).collect()
    }

    /// `possibly_missing` numbers that are main-run integers.
    pub fn possibly_missing_ints(&self) -> Vec<i64> {
        self.possibly_missing
            .iter()
            .filter_map(|p| as_int(&p.number))
            .collect()
    }
}

/// A non-negative integral issue number ("12", "500", "12.0").
fn as_int(n: &str) -> Option<i64> {
    let key = issue_number_compare_key(n);
    let v: f64 = key.parse().ok()?;
    (v.is_finite() && v >= 0.0 && (v - v.round()).abs() < 1e-9).then(|| v.round() as i64)
}

/// Sort key for display order: numeric first, then text.
fn order_key(key: &str) -> (u8, i64, String) {
    match key.parse::<f64>() {
        Ok(v) if v.is_finite() => (0, (v * 1000.0).round() as i64, String::new()),
        _ => (1, 0, key.to_owned()),
    }
}

/// One segment to read: a provider series, optionally restricted to a
/// range or with other ranges cut out.
#[derive(Debug, Clone)]
pub struct Segment {
    pub provider_series_id: String,
    /// `Some` for a range segment: `(low, high)`.
    pub range: Option<(Option<String>, Option<String>)>,
    /// Range-row name / year (labels when the list carries none).
    pub name: Option<String>,
    pub year: Option<i32>,
}

/// The accepted coverage of a series, per provider (main first, then its
/// ranges), for the coverage sources only.
pub fn segments(
    ext_ids: &[external_id::Model],
    ranges: &[series_provider_range::Model],
) -> Vec<(Source, Vec<Segment>)> {
    let mut out = Vec::new();
    for source in COVERAGE_SOURCES {
        let mut segs = Vec::new();
        if let Some(e) = ext_ids
            .iter()
            .find(|e| Source::from_str(&e.source).ok() == Some(source))
        {
            segs.push(Segment {
                provider_series_id: e.external_id.clone(),
                range: None,
                name: None,
                year: None,
            });
        }
        for r in ranges
            .iter()
            .filter(|r| Source::from_str(&r.source).ok() == Some(source))
        {
            segs.push(Segment {
                provider_series_id: r.provider_series_id.clone(),
                range: Some((r.range_low.clone(), r.range_high.clone())),
                name: r.provider_series_name.clone(),
                year: r.declared_year,
            });
        }
        if !segs.is_empty() {
            out.push((source, segs));
        }
    }
    out
}

/// Pure manifest computation. `lists[(source, provider_series_id)]` is a
/// cached list (absent ⇒ not loaded); `owned` the compare keys
/// ([`issue_number_compare_key`]) of the local series' active issues.
pub fn compute(
    coverage: &[(Source, Vec<Segment>)],
    lists: &HashMap<(Source, String), ProviderSeriesIssues>,
    owned: &HashSet<String>,
) -> ProviderManifestView {
    // Per provider: compare key → display number, or None (not loaded).
    let mut providers = Vec::new();
    let mut sets: Vec<(Source, Option<BTreeMap<String, String>>)> = Vec::new();
    for (source, segs) in coverage {
        let ranges: Vec<&(Option<String>, Option<String>)> =
            segs.iter().filter_map(|s| s.range.as_ref()).collect();
        let mut listed: BTreeMap<String, String> = BTreeMap::new();
        let mut loaded = true;
        let mut refs = Vec::new();
        for seg in segs {
            let list = lists.get(&(*source, seg.provider_series_id.clone()));
            refs.push(ManifestSeriesRef {
                provider_series_id: seg.provider_series_id.clone(),
                name: list
                    .and_then(|l| l.series_name.clone())
                    .or_else(|| seg.name.clone()),
                year: list.and_then(|l| l.year_began).or(seg.year),
                via_range: seg.range.is_some(),
                range_low: seg.range.as_ref().and_then(|r| r.0.clone()),
                range_high: seg.range.as_ref().and_then(|r| r.1.clone()),
                loaded: list.is_some(),
            });
            let Some(list) = list else {
                loaded = false;
                continue;
            };
            for issue in &list.issues {
                let key = issue_number_compare_key(&issue.number);
                let keep = match &seg.range {
                    // A range segment: only the numbers it routes here.
                    Some((lo, hi)) => issue_in_range(&key, lo.as_deref(), hi.as_deref()),
                    // The main series: minus the numbers ranges route elsewhere.
                    None => !ranges
                        .iter()
                        .any(|(lo, hi)| issue_in_range(&key, lo.as_deref(), hi.as_deref())),
                };
                if keep {
                    listed.entry(key).or_insert_with(|| issue.number.clone());
                }
            }
        }
        providers.push(ManifestProvider {
            source: source.as_str().to_owned(),
            loaded,
            series: refs,
            listed_count: if loaded { listed.len() } else { 0 },
        });
        sets.push((*source, loaded.then_some(listed)));
    }

    let used = sets.iter().any(|(_, s)| s.is_some());
    let mut missing = Vec::new();
    let mut possibly = Vec::new();
    if used {
        // Every number some loaded provider lists that nobody owns.
        let mut union: BTreeMap<String, String> = BTreeMap::new();
        for (_, set) in &sets {
            if let Some(set) = set {
                for (k, label) in set {
                    union.entry(k.clone()).or_insert_with(|| label.clone());
                }
            }
        }
        let mut keys: Vec<(String, String)> = union
            .into_iter()
            .filter(|(k, _)| !owned.contains(k))
            .collect();
        keys.sort_by_key(|(k, _)| order_key(k));
        for (key, label) in keys {
            let views: Vec<ManifestProviderView> = sets
                .iter()
                .map(|(source, set)| ManifestProviderView {
                    source: source.as_str().to_owned(),
                    listing: match set {
                        None => ManifestListing::NotLoaded,
                        Some(s) if s.contains_key(&key) => ManifestListing::Listed,
                        Some(_) => ManifestListing::NotListed,
                    },
                })
                .collect();
            if views.iter().all(|v| v.listing == ManifestListing::Listed) {
                missing.push(label);
            } else {
                possibly.push(PossiblyMissingIssue {
                    number: label,
                    providers: views,
                });
            }
        }
    }
    let not_loaded: Vec<&str> = providers
        .iter()
        .filter(|p| !p.loaded)
        .filter_map(|p| Source::from_str(&p.source).ok())
        .map(|s| match s {
            Source::Gcd => "GCD",
            other => other.label(),
        })
        .collect();
    let note = (!not_loaded.is_empty()).then(|| {
        format!(
            "{} provider list{} not loaded — run Analyze coverage",
            not_loaded.join(", "),
            if not_loaded.len() == 1 { "" } else { "s" }
        )
    });
    ProviderManifestView {
        used,
        providers,
        missing,
        possibly_missing: possibly,
        note,
    }
}

/// The manifest for `series_id`, or `None` when the series has no accepted
/// coverage for ComicVine, Metron or GCD. Reads the cached issue lists only
/// (a handful of Redis GETs) — never a provider.
pub async fn for_series<C: ConnectionTrait>(
    db: &C,
    redis: &ConnectionManager,
    series_id: Uuid,
    owned: &HashSet<String>,
) -> Result<Option<ProviderManifestView>, DbErr> {
    let ext_ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .all(db)
        .await?;
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .all(db)
        .await?;
    let coverage = segments(&ext_ids, &ranges);
    if coverage.is_empty() {
        return Ok(None);
    }
    let mut lists = HashMap::new();
    for (source, segs) in &coverage {
        for seg in segs {
            let key = (*source, seg.provider_series_id.clone());
            if lists.contains_key(&key) {
                continue;
            }
            if let Some(list) =
                coverage::cached_provider_issues(redis, *source, &seg.provider_series_id).await
            {
                lists.insert(key, list);
            }
        }
    }
    Ok(Some(compute(&coverage, &lists, owned)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::provider::ProviderIssue;

    fn list(numbers: &[&str]) -> ProviderSeriesIssues {
        ProviderSeriesIssues {
            series_name: Some("X".into()),
            year_began: Some(1998),
            publisher: None,
            issues: numbers
                .iter()
                .map(|n| ProviderIssue {
                    external_id: Some(format!("i{n}")),
                    number: (*n).into(),
                    cover_date: None,
                })
                .collect(),
            complete: true,
            dates_complete: false,
            requests: 0,
        }
    }

    fn main(id: &str) -> Segment {
        Segment {
            provider_series_id: id.into(),
            range: None,
            name: None,
            year: None,
        }
    }

    fn range(id: &str, lo: &str, hi: &str) -> Segment {
        Segment {
            provider_series_id: id.into(),
            range: Some((Some(lo.into()), Some(hi.into()))),
            name: None,
            year: None,
        }
    }

    fn owned(nums: &[&str]) -> HashSet<String> {
        nums.iter().map(|n| issue_number_compare_key(n)).collect()
    }

    #[test]
    fn agreement_is_missing_disagreement_is_possibly_missing() {
        let cov = vec![
            (Source::ComicVine, vec![main("cv")]),
            (Source::Metron, vec![main("m")]),
        ];
        let mut lists = HashMap::new();
        lists.insert(
            (Source::ComicVine, "cv".into()),
            list(&["1", "2", "3", "4"]),
        );
        lists.insert((Source::Metron, "m".into()), list(&["1", "2", "3", "5"]));
        let v = compute(&cov, &lists, &owned(&["1"]));
        assert!(v.used);
        assert_eq!(v.missing, vec!["2", "3"]);
        let nums: Vec<&str> = v
            .possibly_missing
            .iter()
            .map(|p| p.number.as_str())
            .collect();
        assert_eq!(nums, vec!["4", "5"]);
        assert_eq!(
            v.possibly_missing[0].providers,
            vec![
                ManifestProviderView {
                    source: "comicvine".into(),
                    listing: ManifestListing::Listed
                },
                ManifestProviderView {
                    source: "metron".into(),
                    listing: ManifestListing::NotListed
                },
            ]
        );
        assert_eq!(v.missing_ints(), vec![2, 3]);
        assert!(v.note.is_none());
    }

    #[test]
    fn ranges_route_numbers_and_main_drops_them() {
        // Main lists 1–3 and 600 (a lumper's copy of the relaunch); the
        // range routes 600–601 to another series that lists 600–602.
        let cov = vec![(
            Source::Metron,
            vec![main("1711"), range("1713", "600", "601")],
        )];
        let mut lists = HashMap::new();
        lists.insert(
            (Source::Metron, "1711".into()),
            list(&["1", "2", "3", "600"]),
        );
        lists.insert(
            (Source::Metron, "1713".into()),
            list(&["600", "601", "602"]),
        );
        let v = compute(&cov, &lists, &owned(&["1", "2", "3", "600"]));
        // 602 is outside the range: not this folder's expected issue.
        assert_eq!(v.missing, vec!["601"]);
        assert_eq!(v.providers[0].listed_count, 5);
    }

    #[test]
    fn a_not_loaded_provider_cannot_confirm() {
        let cov = vec![
            (Source::ComicVine, vec![main("cv")]),
            (Source::Gcd, vec![main("g")]),
        ];
        let mut lists = HashMap::new();
        lists.insert((Source::ComicVine, "cv".into()), list(&["1", "2"]));
        let v = compute(&cov, &lists, &owned(&["1"]));
        assert!(v.used);
        assert!(v.missing.is_empty());
        assert_eq!(v.possibly_missing.len(), 1);
        assert_eq!(
            v.possibly_missing[0].providers[1].listing,
            ManifestListing::NotLoaded
        );
        assert_eq!(
            v.note.as_deref(),
            Some("GCD provider list not loaded — run Analyze coverage")
        );
    }

    #[test]
    fn nothing_loaded_is_unused() {
        let cov = vec![(Source::Metron, vec![main("m")])];
        let v = compute(&cov, &HashMap::new(), &owned(&["1"]));
        assert!(!v.used);
        assert!(v.missing.is_empty() && v.possibly_missing.is_empty());
        assert!(
            v.note
                .unwrap()
                .starts_with("Metron provider list not loaded")
        );
    }

    #[test]
    fn fractions_match_local_decimals() {
        let cov = vec![(Source::ComicVine, vec![main("cv")])];
        let mut lists = HashMap::new();
        lists.insert((Source::ComicVine, "cv".into()), list(&["½", "1"]));
        let v = compute(&cov, &lists, &owned(&["0.5", "1"]));
        assert!(v.missing.is_empty() && v.possibly_missing.is_empty());
    }
}
