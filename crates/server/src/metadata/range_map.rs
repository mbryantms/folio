//! Effective per-provider series target resolution — the single place
//! that answers "which provider series does *this* issue of *this* local
//! series belong to?" for each metadata source.
//!
//! Providers disagree on series boundaries (provider series-boundary
//! divergence). A `series_provider_range` row records an issue-number
//! range of a local series that maps to a DIFFERENT provider series than
//! the series-level `external_ids` default (see
//! `entity::series_provider_range`). This module folds the two together:
//! a range row wins for the issues it covers; otherwise the series-level
//! external id is the default.
//!
//! Search ([`orchestrator::run_issue_search`]), apply
//! ([`apply::apply_series_via_sidecar`]) and the in-UI divergence surface
//! all resolve through here so the routing decision lives in one place.

use crate::metadata::identifier::Source;
use entity::{external_id, series_provider_range};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

/// The provider series an issue should be searched / applied against for
/// one source, after folding range overrides over the series-level
/// default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveTarget {
    pub source: Source,
    /// The provider's series identifier to narrow the search to.
    pub provider_series_id: String,
    /// The mapped sub-series' start year. `Some` only for a range
    /// override; the issue-search year gate uses it instead of the
    /// parent series year. `None` ⇒ caller keeps using the parent
    /// series year (today's behaviour).
    pub declared_year: Option<i32>,
    /// Provider's display name for the mapped series (range overrides
    /// only) — drives the "alternate provider series" UI.
    pub provider_series_name: Option<String>,
    /// Canonical link to the provider series page.
    pub provider_series_url: Option<String>,
    /// `true` when this target came from a `series_provider_range` row
    /// (i.e. the issue diverges from the series default). Surfacing
    /// filters on this; search/apply treat both the same.
    pub via_range: bool,
}

/// Resolve the effective provider target per source for one issue of a
/// series, given the issue's **canonical** number
/// ([`matcher::canonical_issue_number`](crate::metadata::matcher)).
///
/// For each source that has either a covering range row or a series-level
/// external id: a covering range row wins; otherwise the series-level
/// external id is the default (`declared_year = None`). Sources with
/// neither are omitted (the broad-search fallback handles them).
pub async fn resolve_for_issue<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    canonical_issue_number: &str,
) -> Result<Vec<EffectiveTarget>, DbErr> {
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .all(db)
        .await?;
    let series_ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .all(db)
        .await?;

    Ok(fold_targets(&ranges, &series_ids, canonical_issue_number))
}

/// Pure folding step, split out so it's unit-testable without a DB and
/// reusable by the provider-coverage endpoint (which folds it across
/// every issue of a series after loading the inputs once).
pub(crate) fn fold_targets(
    ranges: &[series_provider_range::Model],
    series_ids: &[external_id::Model],
    canonical_issue_number: &str,
) -> Vec<EffectiveTarget> {
    // Every source that has any mapping (a range row or a series-level
    // external id), preserving first-seen order for stable output.
    let mut sources: Vec<Source> = Vec::new();
    let push_source = |raw: &str, sources: &mut Vec<Source>| {
        if let Ok(s) = Source::from_str(raw)
            && !sources.contains(&s)
        {
            sources.push(s);
        }
    };
    for r in ranges {
        push_source(&r.source, &mut sources);
    }
    for e in series_ids {
        push_source(&e.source, &mut sources);
    }

    sources
        .into_iter()
        .filter_map(|source| {
            // A covering range row wins.
            if let Some(r) = ranges.iter().find(|r| {
                Source::from_str(&r.source).ok() == Some(source)
                    && issue_in_range(
                        canonical_issue_number,
                        r.range_low.as_deref(),
                        r.range_high.as_deref(),
                    )
            }) {
                return Some(EffectiveTarget {
                    source,
                    provider_series_id: r.provider_series_id.clone(),
                    declared_year: r.declared_year,
                    provider_series_name: r.provider_series_name.clone(),
                    provider_series_url: r.provider_series_url.clone(),
                    via_range: true,
                });
            }
            // Otherwise the series-level external id is the default.
            series_ids
                .iter()
                .find(|e| Source::from_str(&e.source).ok() == Some(source))
                .map(|e| EffectiveTarget {
                    source,
                    provider_series_id: e.external_id.clone(),
                    declared_year: None,
                    provider_series_name: None,
                    provider_series_url: e.external_url.clone(),
                    via_range: false,
                })
        })
        .collect()
}

/// Inclusive membership test for a canonical issue number against a
/// `[low, high]` range. `None` bounds are open-ended. Numeric bounds use
/// numeric comparison ("1.5" ∈ ["1","2"]); a non-parseable bound falls
/// back to exact string equality so a non-numeric range ("Annual 1") only
/// matches itself rather than swallowing unrelated issues.
pub fn issue_in_range(number: &str, low: Option<&str>, high: Option<&str>) -> bool {
    let n = number.trim();
    let nf: Option<f64> = n.parse().ok();
    let lo_ok = match low {
        None => true,
        Some(l) => match (nf, l.trim().parse::<f64>().ok()) {
            (Some(nf), Some(lf)) => nf >= lf,
            _ => n == l.trim(),
        },
    };
    let hi_ok = match high {
        None => true,
        Some(h) => match (nf, h.trim().parse::<f64>().ok()) {
            (Some(nf), Some(hf)) => nf <= hf,
            _ => n == h.trim(),
        },
    };
    lo_ok && hi_ok
}

/// Do two canonical `[low, high]` ranges overlap? Used by the
/// provider-ranges API to reject overlapping mappings per `(series,
/// source)` with a 409. `None` bounds are open-ended. Non-numeric bounds
/// are treated conservatively as overlapping (can't prove disjoint), so
/// the API rejects rather than silently accepting an ambiguous pair.
pub fn ranges_overlap(
    a_low: Option<&str>,
    a_high: Option<&str>,
    b_low: Option<&str>,
    b_high: Option<&str>,
) -> bool {
    // a starts after b ends  OR  b starts after a ends  ⇒ disjoint.
    let a_after_b = gt(a_low, b_high); // a_low > b_high
    let b_after_a = gt(b_low, a_high); // b_low > a_high
    !(a_after_b || b_after_a)
}

/// `lo > hi` for optional canonical bounds: `None` low never exceeds
/// anything; `None` high is +∞ so nothing exceeds it. Non-numeric → can't
/// prove the gap, return `false` (treated as overlapping upstream).
fn gt(lo: Option<&str>, hi: Option<&str>) -> bool {
    match (lo, hi) {
        (None, _) | (_, None) => false,
        (Some(l), Some(h)) => match (l.trim().parse::<f64>(), h.trim().parse::<f64>()) {
            (Ok(lf), Ok(hf)) => lf > hf,
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn range(
        source: &str,
        id: &str,
        low: Option<&str>,
        high: Option<&str>,
        year: Option<i32>,
    ) -> series_provider_range::Model {
        series_provider_range::Model {
            id: Uuid::nil(),
            series_id: Uuid::nil(),
            source: source.into(),
            provider_series_id: id.into(),
            provider_series_url: Some("https://metron.cloud/series/fantastic-four-2012/".into()),
            provider_series_name: Some("Fantastic Four (2012)".into()),
            range_low: low.map(str::to_owned),
            range_high: high.map(str::to_owned),
            declared_year: year,
            set_by: "user".into(),
            first_set_at: Utc::now().into(),
            last_synced_at: Utc::now().into(),
        }
    }

    fn ext(source: &str, id: &str) -> external_id::Model {
        external_id::Model {
            entity_type: "series".into(),
            entity_id: Uuid::nil().to_string(),
            source: source.into(),
            external_id: id.into(),
            external_url: Some("https://comicvine.example/volume".into()),
            set_by: "metron".into(),
            first_set_at: Utc::now().into(),
            last_synced_at: Utc::now().into(),
        }
    }

    #[test]
    fn in_range_numeric_and_open_ended() {
        assert!(issue_in_range("600", Some("600"), Some("611")));
        assert!(issue_in_range("611", Some("600"), Some("611")));
        assert!(!issue_in_range("599", Some("600"), Some("611")));
        assert!(!issue_in_range("612", Some("600"), Some("611")));
        assert!(issue_in_range("1.5", Some("1"), Some("2")));
        // open-ended bounds
        assert!(issue_in_range("9000", Some("600"), None));
        assert!(issue_in_range("1", None, Some("611")));
        assert!(issue_in_range("anything", None, None));
        // non-numeric only matches itself
        assert!(issue_in_range(
            "Annual 1",
            Some("Annual 1"),
            Some("Annual 1")
        ));
        assert!(!issue_in_range("600", Some("Annual 1"), Some("Annual 1")));
    }

    #[test]
    fn range_wins_over_series_default() {
        let ranges = vec![range(
            "metron",
            "2012-series",
            Some("600"),
            Some("611"),
            Some(2012),
        )];
        let series = vec![ext("metron", "main-run"), ext("comicvine", "cv-volume")];

        // #600 → metron routes to the splitter, cv keeps the volume.
        let got = fold_targets(&ranges, &series, "600");
        let metron = got.iter().find(|t| t.source == Source::Metron).unwrap();
        assert_eq!(metron.provider_series_id, "2012-series");
        assert_eq!(metron.declared_year, Some(2012));
        assert!(metron.via_range);
        let cv = got.iter().find(|t| t.source == Source::ComicVine).unwrap();
        assert_eq!(cv.provider_series_id, "cv-volume");
        assert_eq!(cv.declared_year, None);
        assert!(!cv.via_range);
    }

    #[test]
    fn outside_range_falls_back_to_series_default() {
        let ranges = vec![range(
            "metron",
            "2012-series",
            Some("600"),
            Some("611"),
            Some(2012),
        )];
        let series = vec![ext("metron", "main-run")];
        let got = fold_targets(&ranges, &series, "5");
        let metron = got.iter().find(|t| t.source == Source::Metron).unwrap();
        assert_eq!(metron.provider_series_id, "main-run");
        assert!(!metron.via_range);
    }

    #[test]
    fn range_without_series_default_still_resolves_in_range() {
        // Source has a range but no series-level external id.
        let ranges = vec![range("gcd", "62349", Some("600"), Some("611"), Some(2012))];
        let series: Vec<external_id::Model> = vec![];
        let in_range = fold_targets(&ranges, &series, "605");
        assert_eq!(in_range.len(), 1);
        assert!(in_range[0].via_range);
        // Outside the range and no default ⇒ no target for that source.
        let outside = fold_targets(&ranges, &series, "5");
        assert!(outside.is_empty());
    }

    #[test]
    fn overlap_detection() {
        assert!(ranges_overlap(
            Some("600"),
            Some("611"),
            Some("605"),
            Some("620")
        ));
        assert!(!ranges_overlap(
            Some("1"),
            Some("599"),
            Some("600"),
            Some("611")
        ));
        assert!(!ranges_overlap(Some("612"), None, Some("600"), Some("611")));
        // open-ended low overlaps everything below its high
        assert!(ranges_overlap(None, Some("700"), Some("600"), Some("611")));
    }
}
