//! Per-request "does this need metadata pulled?" assessment for issues and
//! series. Pure scoring — every function here takes already-fetched presence
//! flags and never touches the DB, so the same logic serves the detail-DTO
//! path, the list/card badge path, the series rollup, and the saved-view
//! filter without divergence.
//!
//! The criteria mirror what a "complete" record looks like under the
//! ComicVine / Metron schemes: matched to a provider, with a cover date,
//! summary, page count, at least one credit, and a cover. Title, characters,
//! story arcs, and genres are *recommended* — surfaced as gaps but not gating
//! the tier. Title in particular is deliberately non-gating: most comic
//! issues have no distinct story title, so requiring one would mis-flag
//! perfectly complete issues.
//!
//! Field identifiers in the `missing_*` lists use [`MetadataField::key`] so
//! they line up with `field_provenance` and the frontend can map a missing
//! key straight to its edit affordance. The provider-match signal uses the
//! literal `"external_id"` (sourceless) because "matched to *any* provider"
//! isn't tied to one [`Source`](super::Source).

use super::MetadataField;
use serde::Serialize;

/// Sourceless key for the "matched to a provider" core signal. Distinct from
/// the per-source `external_id.<source>` keys emitted by
/// [`MetadataField::ExternalId`].
pub const EXTERNAL_ID_KEY: &str = "external_id";

/// Weight given to the highest-signal core fields (provider match + cover).
const HEAVY: f64 = 2.0;
/// Weight for ordinary core fields.
const LIGHT: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletenessTier {
    /// Every core field present.
    Complete,
    /// Matched to a provider and over half of core present, but gaps remain.
    Partial,
    /// Not matched to a provider, or more core fields missing than present.
    NeedsMetadata,
    /// An operator explicitly marked this issue "complete enough" despite real
    /// gaps (metadata-at-scale B4) — e.g. the provider record is thin or
    /// missing. Distinct from [`Self::Complete`]: the gaps still exist and are
    /// surfaced in `missing_core`; this only drops the issue out of the
    /// unmatched worklist. Reversible (un-accept reverts to the intrinsic tier).
    Accepted,
}

impl CompletenessTier {
    /// Overlay an operator's "mark metadata complete" acknowledgement onto the
    /// intrinsic (field-presence-derived) tier: a genuinely incomplete issue
    /// becomes [`Self::Accepted`] when accepted, so it leaves the worklist
    /// without faking field presence. An intrinsically [`Self::Complete`] issue
    /// is left unchanged — the acknowledgement is moot there.
    #[must_use]
    pub fn with_acceptance(self, accepted: bool) -> Self {
        if accepted && self != Self::Complete {
            Self::Accepted
        } else {
            self
        }
    }
}

/// The SQL boolean that decides whether a single issue row counts as
/// "metadata satisfied" for the per-series completeness rollup. `alias` is
/// the table alias the caller bound the `issues` table to (e.g. `"i"`).
///
/// An operator "mark complete" acknowledgement (`metadata_review_accepted_at`)
/// short-circuits to satisfied; otherwise the intrinsic core criteria must
/// all hold — a plausible cover date, a summary, a positive page count, at
/// least one creator credit, and a ComicVine/Metron external id. `title` is
/// intentionally excluded (optional for comic issues), matching the pure
/// [`assess_issue`] scorer.
///
/// This is the single source for the predicate so every SQL rollup that
/// computes the tier — the card-badge tiers, the per-series summary, and the
/// `/series?metadata_completeness=` grid filter — agrees to the row. (The
/// saved-view `metadata_completeness` subquery in `views::compile` is the
/// last copy still inlined; it builds the same expression and is being
/// folded onto this helper.)
#[must_use]
pub fn issue_metadata_satisfied_sql(alias: &str) -> String {
    let a = alias;
    format!(
        "{a}.metadata_review_accepted_at IS NOT NULL OR ( \
         {a}.year IS NOT NULL AND {a}.year >= 1800 \
         AND COALESCE(btrim({a}.summary), '') <> '' \
         AND {a}.page_count IS NOT NULL AND {a}.page_count > 0 \
         AND (COALESCE({a}.writer, '') <> '' OR COALESCE({a}.penciller, '') <> '' \
           OR COALESCE({a}.inker, '') <> '' OR COALESCE({a}.colorist, '') <> '' \
           OR COALESCE({a}.letterer, '') <> '' OR COALESCE({a}.cover_artist, '') <> '' \
           OR COALESCE({a}.editor, '') <> '' OR COALESCE({a}.translator, '') <> '') \
         AND EXISTS (SELECT 1 FROM external_ids x \
           WHERE x.entity_type = 'issue' AND x.entity_id = {a}.id \
           AND x.source IN ('comicvine', 'metron')))"
    )
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CompletenessReport {
    pub tier: CompletenessTier,
    /// Weighted fraction of core fields present, `0.0..=1.0`.
    pub score: f64,
    /// Core field keys that are absent ([`MetadataField::key`] or
    /// [`EXTERNAL_ID_KEY`]). Empty for a [`CompletenessTier::Complete`] record.
    pub missing_core: Vec<String>,
    /// Recommended (non-gating) field keys that are absent.
    pub missing_recommended: Vec<String>,
}

/// Presence flags for an issue, gathered by the caller from the issue row,
/// its junction counts, and its `external_ids`.
#[derive(Debug, Clone, Copy, Default)]
pub struct IssueCompletenessInput {
    /// ≥1 row in `external_ids` for this issue.
    pub has_external_id: bool,
    pub has_title: bool,
    /// `issue.year` present and ≥1800 (a plausible cover date).
    pub has_cover_date: bool,
    pub has_summary: bool,
    /// `issue.page_count` present and > 0.
    pub has_page_count: bool,
    /// ≥1 row in `issue_credits`.
    pub has_credits: bool,
    /// A renderable primary cover. Effectively always true for an on-disk
    /// issue; the caller passes `false` only when cover extraction failed.
    pub has_cover: bool,
    // ── recommended (non-gating) ──
    pub has_characters: bool,
    pub has_story_arcs: bool,
    pub has_genres: bool,
}

/// Presence flags for a series.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeriesCompletenessInput {
    /// ≥1 row in `external_ids` for this series.
    pub has_external_id: bool,
    pub has_summary: bool,
    pub has_publisher: bool,
    /// `series.status` set to a real value (not the unknown/default sentinel).
    pub has_status: bool,
    pub has_total_issues: bool,
    /// `series.year` present and ≥1800.
    pub has_year_began: bool,
    /// ≥1 row in `series_genres`.
    pub has_genres: bool,
}

/// `year` present and within a plausible publication range.
pub fn plausible_year(year: Option<i32>) -> bool {
    matches!(year, Some(y) if y >= 1800)
}

/// `Some` non-empty after trimming.
pub fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

fn assess(
    matched: bool,
    core: &[(String, f64, bool)],
    recommended: &[(String, bool)],
) -> CompletenessReport {
    let total_weight: f64 = core.iter().map(|(_, w, _)| *w).sum();
    let present_weight: f64 = core
        .iter()
        .filter(|(_, _, present)| *present)
        .map(|(_, w, _)| *w)
        .sum();
    let score = if total_weight > 0.0 {
        present_weight / total_weight
    } else {
        1.0
    };

    let missing_core: Vec<String> = core
        .iter()
        .filter(|(_, _, present)| !*present)
        .map(|(key, _, _)| key.clone())
        .collect();
    let missing_recommended: Vec<String> = recommended
        .iter()
        .filter(|(_, present)| !*present)
        .map(|(key, _)| key.clone())
        .collect();

    // NeedsMetadata when unmatched, or when more core is missing than present.
    let tier = if !matched || missing_core.len() * 2 > core.len() {
        CompletenessTier::NeedsMetadata
    } else if missing_core.is_empty() {
        CompletenessTier::Complete
    } else {
        CompletenessTier::Partial
    };

    CompletenessReport {
        tier,
        score,
        missing_core,
        missing_recommended,
    }
}

/// Assess an issue's metadata completeness.
pub fn assess_issue(input: &IssueCompletenessInput) -> CompletenessReport {
    let core = [
        (EXTERNAL_ID_KEY.to_owned(), HEAVY, input.has_external_id),
        (MetadataField::CoverDate.key(), LIGHT, input.has_cover_date),
        (MetadataField::Summary.key(), LIGHT, input.has_summary),
        (MetadataField::PageCount.key(), LIGHT, input.has_page_count),
        (MetadataField::Credits.key(), LIGHT, input.has_credits),
        (MetadataField::CoverPrimary.key(), HEAVY, input.has_cover),
    ];
    // `title` is **recommended, not core**: in comics most monthly issues have
    // no distinct story title (ComicVine/Metron leave it empty), so a fully
    // matched issue legitimately lacks one. Gating completeness on it produced
    // false "needs metadata" flags, so it never drives the tier.
    let recommended = [
        (MetadataField::Title.key(), input.has_title),
        (MetadataField::Characters.key(), input.has_characters),
        (MetadataField::StoryArcs.key(), input.has_story_arcs),
        (MetadataField::Genres.key(), input.has_genres),
    ];
    assess(input.has_external_id, &core, &recommended)
}

/// Assess a series' metadata completeness.
pub fn assess_series(input: &SeriesCompletenessInput) -> CompletenessReport {
    let core = [
        (EXTERNAL_ID_KEY.to_owned(), HEAVY, input.has_external_id),
        (MetadataField::Summary.key(), LIGHT, input.has_summary),
        (MetadataField::Publisher.key(), LIGHT, input.has_publisher),
        (MetadataField::Status.key(), LIGHT, input.has_status),
        (MetadataField::YearBegan.key(), LIGHT, input.has_year_began),
        ("total_issues".to_owned(), LIGHT, input.has_total_issues),
    ];
    let recommended = [(MetadataField::Genres.key(), input.has_genres)];
    assess(input.has_external_id, &core, &recommended)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_populated_issue_is_complete() {
        let r = assess_issue(&IssueCompletenessInput {
            has_external_id: true,
            has_title: true,
            has_cover_date: true,
            has_summary: true,
            has_page_count: true,
            has_credits: true,
            has_cover: true,
            has_characters: true,
            has_story_arcs: true,
            has_genres: true,
        });
        assert_eq!(r.tier, CompletenessTier::Complete);
        assert_eq!(r.score, 1.0);
        assert!(r.missing_core.is_empty());
        assert!(r.missing_recommended.is_empty());
    }

    #[test]
    fn unmatched_issue_needs_metadata_even_if_fields_present() {
        // No provider match → NeedsMetadata regardless of other fields, and
        // `external_id` shows up first in missing_core.
        let r = assess_issue(&IssueCompletenessInput {
            has_external_id: false,
            has_title: true,
            has_cover_date: true,
            has_summary: true,
            has_page_count: true,
            has_credits: true,
            has_cover: true,
            ..Default::default()
        });
        assert_eq!(r.tier, CompletenessTier::NeedsMetadata);
        assert_eq!(r.missing_core, vec![EXTERNAL_ID_KEY.to_owned()]);
    }

    #[test]
    fn missing_title_alone_does_not_block_completeness() {
        // Comics frequently have no issue title — it must not gate the tier.
        let r = assess_issue(&IssueCompletenessInput {
            has_external_id: true,
            has_title: false,
            has_cover_date: true,
            has_summary: true,
            has_page_count: true,
            has_credits: true,
            has_cover: true,
            ..Default::default()
        });
        assert_eq!(r.tier, CompletenessTier::Complete);
        assert!(!r.missing_core.contains(&MetadataField::Title.key()));
        assert!(r.missing_recommended.contains(&MetadataField::Title.key()));
    }

    #[test]
    fn matched_issue_with_small_gap_is_partial() {
        // Matched + only summary missing → Partial (not Complete, not Needs).
        let r = assess_issue(&IssueCompletenessInput {
            has_external_id: true,
            has_title: true,
            has_cover_date: true,
            has_summary: false,
            has_page_count: true,
            has_credits: true,
            has_cover: true,
            ..Default::default()
        });
        assert_eq!(r.tier, CompletenessTier::Partial);
        assert_eq!(r.missing_core, vec![MetadataField::Summary.key()]);
        assert!(r.score > 0.5 && r.score < 1.0);
    }

    #[test]
    fn matched_issue_with_majority_missing_needs_metadata() {
        // Matched but most core absent → NeedsMetadata.
        let r = assess_issue(&IssueCompletenessInput {
            has_external_id: true,
            has_cover: true,
            ..Default::default()
        });
        assert_eq!(r.tier, CompletenessTier::NeedsMetadata);
    }

    #[test]
    fn series_complete_and_needs_metadata() {
        let complete = assess_series(&SeriesCompletenessInput {
            has_external_id: true,
            has_summary: true,
            has_publisher: true,
            has_status: true,
            has_total_issues: true,
            has_year_began: true,
            has_genres: true,
        });
        assert_eq!(complete.tier, CompletenessTier::Complete);

        let bare = assess_series(&SeriesCompletenessInput::default());
        assert_eq!(bare.tier, CompletenessTier::NeedsMetadata);
        assert!(bare.missing_core.contains(&EXTERNAL_ID_KEY.to_owned()));
    }

    #[test]
    fn helpers_validate_year_and_strings() {
        assert!(plausible_year(Some(1994)));
        assert!(!plausible_year(Some(1200)));
        assert!(!plausible_year(None));
        assert!(non_empty(Some("x")));
        assert!(!non_empty(Some("   ")));
        assert!(!non_empty(None));
    }
}
