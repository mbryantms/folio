//! Cross-provider merge policy (multi-provider collaborative metadata).
//!
//! Given the chosen candidate detail from each included provider, pick
//! — per field — which provider supplies the "most complete" value, so
//! the composite apply can assemble a best-of-all-providers result in a
//! single operation.
//!
//! This module is PURE: it operates on already-fetched
//! [`GenericMetadata`] details and a policy config, with no DB or
//! network. The composite preview/apply (`composite.rs`) drives it.
//!
//! ## Policy ("most complete, then most trusted")
//! - **Scalars / dates / text**: a non-empty value wins (a page count
//!   of 0 counts as empty). Among providers that have a value, the
//!   candidate the matcher is *surer* of wins — a `high` match beats a
//!   `medium` one, whatever the preference order says, because a
//!   weaker match is more likely to be a different issue. Within the
//!   same confidence the one earliest in `provider_preference` wins
//!   (deterministic tiebreaker).
//! - **Junctions** (`MetadataField::is_junction`): the *richest* set
//!   wins — the provider with the most *distinct* entries (names are
//!   compared case-insensitively; a credit is a name + canonical role
//!   pair, and a credit whose role the provider did not supply is not
//!   counted). Credits with the same pair count break by distinct
//!   people, then every junction by confidence, then preference.
//! - **Covers**: the provider that actually has a cover (or the most
//!   variants) wins; ties break by confidence, then preference.
//! - **External IDs**: NOT chosen here — external IDs are additive
//!   (unioned across all included providers at apply time), so the
//!   policy never picks a single source for them.

use crate::metadata::field::MetadataField;
use crate::metadata::identifier::Source;
use crate::metadata::provider::GenericMetadata;

/// Which entity a merge targets. Only [`MetadataField::Title`] maps to
/// a different `GenericMetadata` accessor per scope (series → series
/// name, issue → issue title); every other field reads the same
/// accessor regardless.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MergeScope {
    Series,
    Issue,
}

/// One included provider's chosen candidate + its fetched detail.
#[derive(Clone, Debug)]
pub struct ProviderDetail {
    pub source: Source,
    pub external_id: String,
    pub ordinal: i32,
    /// How sure the matcher was that this candidate is the entity:
    /// [`confidence_rank`] of the candidate's bucket. Breaks every tie
    /// before provider preference does.
    pub confidence: u8,
    pub detail: GenericMetadata,
}

/// Rank of a matcher bucket for tie-breaking — higher is surer. Unknown
/// buckets rank below `low`, so a candidate the matcher never scored
/// (a direct lookup) does not outrank one it did.
pub fn confidence_rank(bucket: &str) -> u8 {
    match bucket {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn norm_key(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Distinct entries of a named junction, compared case-insensitively.
fn distinct_names<'a>(names: impl Iterator<Item = &'a str>) -> usize {
    names
        .map(norm_key)
        .filter(|s| !s.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Distinct `(name, role)` credit pairs. A credit whose role the provider
/// did not supply (`"unknown"`) carries no information the composer can
/// write, so it does not make a provider "richer".
fn credit_pairs(detail: &GenericMetadata) -> usize {
    detail
        .credits
        .iter()
        .filter(|c| !c.role.eq_ignore_ascii_case("unknown"))
        .map(|c| (norm_key(&c.name), norm_key(&c.role)))
        .filter(|(n, _)| !n.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Distinct credited people (any role).
fn credit_people(detail: &GenericMetadata) -> usize {
    distinct_names(detail.credits.iter().map(|c| c.name.as_str()))
}

/// Tiebreaker order when multiple providers offer a value for the same
/// field. Sources not listed sort last (in their `details` order).
#[derive(Clone, Debug)]
pub struct MergePolicyConfig {
    pub provider_preference: Vec<Source>,
    /// Cover-only source override. When `Some(src)`, the primary-cover field
    /// prefers `src`'s candidate over the normal `provider_preference` order
    /// (every other field keeps the normal preference). `None` = use
    /// `provider_preference` for covers too. Drives the bulk "prefer
    /// ComicVine covers" requirement without perturbing scalar/junction picks.
    pub preferred_cover_provider: Option<Source>,
}

impl MergePolicyConfig {
    /// Rank of a source for the preference tiebreaker — lower wins.
    /// Unlisted sources rank after every listed one.
    fn rank(&self, source: Source) -> usize {
        self.provider_preference
            .iter()
            .position(|s| *s == source)
            .unwrap_or(usize::MAX)
    }
}

/// The merge decision for one field: which provider won it (if any) and
/// the display value that won.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChoice {
    pub field: MetadataField,
    /// The chosen candidate's `ordinal` (unique within a run), or `None`
    /// when no included candidate has a value for this field.
    pub ordinal: Option<i32>,
    pub value: Option<String>,
}

/// Fields the apply path writes for a given scope, in display order.
/// Mirrors the field set `diff.rs` surfaces so the composite comparison
/// and the single-candidate diff stay aligned. `ExternalId(_)` is
/// deliberately excluded — external IDs are additive, handled outside
/// the per-field merge.
pub fn scope_fields(scope: MergeScope) -> &'static [MetadataField] {
    match scope {
        MergeScope::Series => &[
            MetadataField::Title,
            MetadataField::SortName,
            MetadataField::SeriesType,
            MetadataField::YearBegan,
            MetadataField::YearEnd,
            MetadataField::Volume,
            MetadataField::Publisher,
            MetadataField::Imprint,
            MetadataField::Deck,
            MetadataField::Description,
        ],
        MergeScope::Issue => &[
            MetadataField::Title,
            MetadataField::Deck,
            MetadataField::Description,
            MetadataField::AgeRating,
            MetadataField::PageCount,
            MetadataField::Sku,
            MetadataField::CoverDate,
            MetadataField::Credits,
            MetadataField::Characters,
            MetadataField::Teams,
            MetadataField::Locations,
            MetadataField::StoryArcs,
            MetadataField::Tags,
            MetadataField::Genres,
            MetadataField::CoverPrimary,
            MetadataField::CoverVariants,
        ],
    }
}

/// Count of non-empty variant covers (matches the composer's "skip
/// entries with no image URL" filter).
fn variant_count(detail: &GenericMetadata) -> usize {
    detail
        .variants
        .iter()
        .filter(|v| v.image_url.as_deref().is_some_and(|s| !s.trim().is_empty()))
        .count()
}

/// "Richness" of a field on a detail — drives junction selection and
/// presence checks. Scalars/covers: 1 when a value is present, else 0.
/// Junctions: the entry count.
pub fn field_richness(detail: &GenericMetadata, field: MetadataField, scope: MergeScope) -> usize {
    match field {
        MetadataField::Credits => credit_pairs(detail),
        MetadataField::Characters => {
            distinct_names(detail.characters.iter().map(|e| e.name.as_str()))
        }
        MetadataField::Teams => distinct_names(detail.teams.iter().map(|e| e.name.as_str())),
        MetadataField::Locations => {
            distinct_names(detail.locations.iter().map(|e| e.name.as_str()))
        }
        MetadataField::Concepts => distinct_names(detail.concepts.iter().map(|e| e.name.as_str())),
        MetadataField::Objects => distinct_names(detail.objects.iter().map(|e| e.name.as_str())),
        MetadataField::StoryArcs => {
            distinct_names(detail.story_arcs.iter().map(|e| e.name.as_str()))
        }
        MetadataField::Universes => {
            distinct_names(detail.universes.iter().map(|e| e.name.as_str()))
        }
        MetadataField::Genres => distinct_names(detail.genres.iter().map(String::as_str)),
        MetadataField::Tags => distinct_names(detail.tags.iter().map(String::as_str)),
        MetadataField::Reprints => detail.reprints.len(),
        MetadataField::CoverVariants => variant_count(detail),
        // Scalars + cover-primary + external-id: presence is 0/1.
        _ => usize::from(field_value_as_string(detail, field, scope).is_some()),
    }
}

/// Display string for a field on a detail, or `None` when empty. Used
/// both for the comparison view (one value per provider) and as the
/// non-empty presence check for scalars. Junctions render as a count
/// string (`"3 items"`); cover-primary renders the URL.
pub fn field_value_as_string(
    detail: &GenericMetadata,
    field: MetadataField,
    scope: MergeScope,
) -> Option<String> {
    fn norm(s: &Option<String>) -> Option<String> {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
    match field {
        MetadataField::Title => match scope {
            MergeScope::Series => norm(&detail.series_name),
            MergeScope::Issue => norm(&detail.title),
        },
        MetadataField::SortName => norm(&detail.series_sort_name),
        MetadataField::SeriesType => norm(&detail.series_type),
        MetadataField::Publisher => norm(&detail.publisher),
        MetadataField::Imprint => norm(&detail.imprint),
        MetadataField::Deck => norm(&detail.deck),
        MetadataField::Description => norm(&detail.description),
        MetadataField::Notes => norm(&detail.notes),
        MetadataField::ScanInformation => norm(&detail.scan_information),
        MetadataField::AgeRating => norm(&detail.age_rating),
        MetadataField::Format => norm(&detail.format),
        MetadataField::LanguageCode => norm(&detail.language_code),
        MetadataField::Sku => norm(&detail.sku),
        MetadataField::YearBegan => detail.year_began.map(|n| n.to_string()),
        MetadataField::YearEnd => detail.year_end.map(|n| n.to_string()),
        MetadataField::Volume => detail.volume.map(|n| n.to_string()),
        // A page count of 0 is a provider's "unknown", not a value.
        MetadataField::PageCount => detail.page_count.filter(|n| *n > 0).map(|n| n.to_string()),
        MetadataField::CommunityRating => detail.community_rating.map(|n| n.to_string()),
        MetadataField::StaffRating => detail.staff_rating.map(|n| n.to_string()),
        MetadataField::Price => detail.price.map(|n| n.to_string()),
        MetadataField::CoverDate => detail.cover_date.map(|d| d.to_string()),
        MetadataField::StoreDate => detail.store_date.map(|d| d.to_string()),
        MetadataField::FocDate => detail.foc_date.map(|d| d.to_string()),
        MetadataField::Aliases => {
            let joined = detail
                .aliases
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            (!joined.is_empty()).then_some(joined)
        }
        MetadataField::CoverPrimary => norm(&detail.cover_image_url),
        // Credits say how many people the pairs cover, so "13 credits"
        // from two providers can still be told apart in the compare view.
        MetadataField::Credits => {
            let pairs = credit_pairs(detail);
            let people = credit_people(detail);
            (pairs > 0).then(|| {
                if people == pairs {
                    format!("{pairs} credits")
                } else {
                    format!("{pairs} credits ({people} people)")
                }
            })
        }
        // Junction-shaped fields render as a count string.
        f if f.is_junction() || matches!(f, MetadataField::CoverVariants) => {
            let n = field_richness(detail, field, scope);
            (n > 0).then(|| format_count(n))
        }
        // Series Status / Manga / external ids aren't supplied as a
        // scalar string by GenericMetadata's merge surface.
        _ => None,
    }
}

fn format_count(n: usize) -> String {
    match n {
        0 => "none".to_owned(),
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    }
}

/// Pick the candidate (by `ordinal`) that supplies the "most complete"
/// value for `field`, or `None` when no included candidate has a value.
/// External IDs always return `None` (additive, never single-sourced).
///
/// Candidates are identified by `ordinal` (unique within a run), so
/// multiple candidates from the SAME provider can be compared side by
/// side. Ties break by the matcher's confidence first (a `high` match
/// over a `medium` one), then the `provider_preference` order, then
/// lower ordinal (better-ranked candidate) within a source.
pub fn choose_field_candidate(
    field: MetadataField,
    details: &[ProviderDetail],
    policy: &MergePolicyConfig,
    scope: MergeScope,
) -> Option<i32> {
    if matches!(field, MetadataField::ExternalId(_)) {
        return None;
    }
    if field.is_junction() || matches!(field, MetadataField::CoverVariants) {
        // Richest set wins; credits with the same pair count prefer more
        // people; then confidence, preference rank, ordinal.
        details
            .iter()
            .filter(|d| field_richness(&d.detail, field, scope) > 0)
            .max_by_key(|d| {
                (
                    field_richness(&d.detail, field, scope),
                    if field == MetadataField::Credits {
                        credit_people(&d.detail)
                    } else {
                        0
                    },
                    d.confidence,
                    std::cmp::Reverse(policy.rank(d.source)),
                    std::cmp::Reverse(d.ordinal),
                )
            })
            .map(|d| d.ordinal)
    } else {
        // Non-empty wins; among those, lowest (preference rank, ordinal).
        // Covers honour the cover-only preference override first: if the
        // preferred source supplied a cover, take its best candidate; only
        // when it has none do we fall through to the normal preference order.
        if field == MetadataField::CoverPrimary
            && let Some(preferred) = policy.preferred_cover_provider
        {
            let from_preferred = details
                .iter()
                .filter(|d| d.source == preferred)
                .filter(|d| field_value_as_string(&d.detail, field, scope).is_some())
                .min_by_key(|d| d.ordinal)
                .map(|d| d.ordinal);
            if from_preferred.is_some() {
                return from_preferred;
            }
        }
        details
            .iter()
            .filter(|d| field_value_as_string(&d.detail, field, scope).is_some())
            .min_by_key(|d| {
                (
                    std::cmp::Reverse(d.confidence),
                    policy.rank(d.source),
                    d.ordinal,
                )
            })
            .map(|d| d.ordinal)
    }
}

/// Build the default per-field merge across all included candidates for
/// `scope`. Skips external IDs (additive). The result drives the
/// preview's default selection and the composite apply's default map.
pub fn build_default_merge(
    details: &[ProviderDetail],
    policy: &MergePolicyConfig,
    scope: MergeScope,
) -> Vec<FieldChoice> {
    scope_fields(scope)
        .iter()
        .copied()
        .map(|field| {
            let ordinal = choose_field_candidate(field, details, policy, scope);
            let value = ordinal.and_then(|ord| {
                details
                    .iter()
                    .find(|d| d.ordinal == ord)
                    .and_then(|d| field_value_as_string(&d.detail, field, scope))
            });
            FieldChoice {
                field,
                ordinal,
                value,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::provider::{CreditCandidate, EntityCandidate};

    fn detail() -> GenericMetadata {
        GenericMetadata::default()
    }

    fn pd(source: Source, ordinal: i32, detail: GenericMetadata) -> ProviderDetail {
        pd_at(source, ordinal, "high", detail)
    }

    fn pd_at(
        source: Source,
        ordinal: i32,
        bucket: &str,
        detail: GenericMetadata,
    ) -> ProviderDetail {
        ProviderDetail {
            source,
            external_id: ordinal.to_string(),
            ordinal,
            confidence: confidence_rank(bucket),
            detail,
        }
    }

    fn credit(name: &str, role: &str) -> CreditCandidate {
        CreditCandidate {
            name: name.into(),
            role: role.into(),
            ordinal: None,
            identifiers: vec![],
        }
    }

    #[test]
    fn surer_match_beats_preference_on_a_scalar_tie() {
        let mut cv = detail();
        cv.description = Some("CV text".into());
        let mut metron = detail();
        metron.description = Some("Metron text".into());
        let details = vec![
            pd_at(Source::Metron, 20, "medium", metron),
            pd_at(Source::ComicVine, 10, "high", cv),
        ];
        // Metron is preferred, but its candidate is only a medium match:
        // the high ComicVine match is likelier to be this issue.
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(
                MetadataField::Description,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(10)
        );
    }

    #[test]
    fn surer_match_beats_preference_on_a_junction_tie() {
        let mut cv = detail();
        cv.characters = vec![entity("A"), entity("B")];
        let mut metron = detail();
        metron.characters = vec![entity("A"), entity("B")];
        let details = vec![
            pd_at(Source::Metron, 20, "medium", metron),
            pd_at(Source::ComicVine, 10, "high", cv),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(
                MetadataField::Characters,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(10)
        );
    }

    #[test]
    fn credits_count_distinct_pairs_and_ignore_unknown_roles() {
        // ComicVine: 4 rows but one duplicate and one role-less row.
        let mut cv = detail();
        cv.credits = vec![
            credit("Ann", "Writer"),
            credit("ann", "writer"),
            credit("Bob", "unknown"),
            credit("Cy", "Penciller"),
        ];
        // Metron: 3 distinct pairs.
        let mut metron = detail();
        metron.credits = vec![
            credit("Ann", "Writer"),
            credit("Cy", "Penciller"),
            credit("Cy", "Inker"),
        ];
        assert_eq!(
            field_richness(&cv, MetadataField::Credits, MergeScope::Issue),
            2
        );
        assert_eq!(
            field_richness(&metron, MetadataField::Credits, MergeScope::Issue),
            3
        );
        assert_eq!(
            field_value_as_string(&metron, MetadataField::Credits, MergeScope::Issue).as_deref(),
            Some("3 credits (2 people)")
        );
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::ComicVine, Source::Metron]);
        assert_eq!(
            choose_field_candidate(MetadataField::Credits, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn credits_same_pairs_prefer_more_people() {
        let mut cv = detail();
        cv.credits = vec![credit("Ann", "Writer"), credit("Ann", "Penciller")];
        let mut metron = detail();
        metron.credits = vec![credit("Ann", "Writer"), credit("Bob", "Penciller")];
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::ComicVine, Source::Metron]);
        assert_eq!(
            choose_field_candidate(MetadataField::Credits, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn zero_page_count_is_not_a_value() {
        let mut cv = detail();
        cv.page_count = Some(0);
        let mut metron = detail();
        metron.page_count = Some(32);
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::ComicVine, Source::Metron]);
        assert_eq!(
            choose_field_candidate(MetadataField::PageCount, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    fn policy(order: &[Source]) -> MergePolicyConfig {
        MergePolicyConfig {
            provider_preference: order.to_vec(),
            preferred_cover_provider: None,
        }
    }

    fn entity(name: &str) -> EntityCandidate {
        EntityCandidate {
            name: name.into(),
            identifiers: vec![],
            is_first_appearance: false,
            died_in_issue: None,
            disbanded_in_issue: None,
            position_in_arc: None,
        }
    }

    // Candidates carry distinct ordinals (CV=10, Metron=20) so the
    // policy returns a specific candidate, not just a source.
    #[test]
    fn scalar_non_null_wins_over_null() {
        let mut cv = detail();
        cv.title = Some("Saga".into());
        let metron = detail(); // no title
        let details = vec![
            pd(Source::Metron, 20, metron),
            pd(Source::ComicVine, 10, cv),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        // Metron preferred, but only ComicVine has a value → CV candidate (10).
        assert_eq!(
            choose_field_candidate(MetadataField::Title, &details, &pref, MergeScope::Issue),
            Some(10)
        );
    }

    #[test]
    fn scalar_preference_breaks_tie_when_both_present() {
        let mut cv = detail();
        cv.title = Some("Saga (CV)".into());
        let mut metron = detail();
        metron.title = Some("Saga (Metron)".into());
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(MetadataField::Title, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn cover_preference_override_picks_comicvine_under_metron_first() {
        let mut cv = detail();
        cv.cover_image_url = Some("https://cv/cover.jpg".into());
        let mut metron = detail();
        metron.cover_image_url = Some("https://metron/cover.jpg".into());
        let details = vec![
            pd(Source::Metron, 20, metron),
            pd(Source::ComicVine, 10, cv),
        ];
        // Metron is the global preference, but the cover-only override prefers
        // ComicVine → the CV candidate (10) wins the primary-cover field.
        let pref = MergePolicyConfig {
            provider_preference: vec![Source::Metron, Source::ComicVine],
            preferred_cover_provider: Some(Source::ComicVine),
        };
        assert_eq!(
            choose_field_candidate(
                MetadataField::CoverPrimary,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(10)
        );
        // Other scalar fields are untouched by the cover override — Metron
        // still wins the preference tiebreak when both have a value.
        let mut cv2 = detail();
        cv2.title = Some("CV".into());
        let mut metron2 = detail();
        metron2.title = Some("Metron".into());
        let details2 = vec![
            pd(Source::ComicVine, 10, cv2),
            pd(Source::Metron, 20, metron2),
        ];
        assert_eq!(
            choose_field_candidate(MetadataField::Title, &details2, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn cover_preference_override_falls_back_when_preferred_has_no_cover() {
        // Only Metron supplied a cover; the ComicVine override falls through
        // to the normal preference order rather than dropping the cover.
        let mut metron = detail();
        metron.cover_image_url = Some("https://metron/cover.jpg".into());
        let cv = detail(); // no cover
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = MergePolicyConfig {
            provider_preference: vec![Source::Metron, Source::ComicVine],
            preferred_cover_provider: Some(Source::ComicVine),
        };
        assert_eq!(
            choose_field_candidate(
                MetadataField::CoverPrimary,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(20)
        );
    }

    #[test]
    fn junction_richer_set_wins() {
        let mut cv = detail();
        cv.characters = vec![entity("Hazel")];
        let mut metron = detail();
        metron.characters = vec![entity("Hazel"), entity("Marko"), entity("Alana")];
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        // Even with CV preferred, Metron's richer set wins.
        let pref = policy(&[Source::ComicVine, Source::Metron]);
        assert_eq!(
            choose_field_candidate(
                MetadataField::Characters,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(20)
        );
    }

    #[test]
    fn junction_equal_size_breaks_by_preference() {
        let mut cv = detail();
        cv.credits = vec![CreditCandidate {
            name: "A".into(),
            role: "Writer".into(),
            ordinal: None,
            identifiers: vec![],
        }];
        let mut metron = detail();
        metron.credits = vec![CreditCandidate {
            name: "B".into(),
            role: "Writer".into(),
            ordinal: None,
            identifiers: vec![],
        }];
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(MetadataField::Credits, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn two_candidates_same_provider_richest_wins() {
        // Two Metron candidates + one CV: the richest characters set wins
        // regardless of which candidate it came from.
        let mut metron_a = detail();
        metron_a.characters = vec![entity("Hazel")];
        let mut metron_b = detail();
        metron_b.characters = vec![entity("Hazel"), entity("Marko"), entity("Alana")];
        let mut cv = detail();
        cv.characters = vec![entity("Hazel"), entity("Marko")];
        let details = vec![
            pd(Source::Metron, 20, metron_a),
            pd(Source::Metron, 21, metron_b),
            pd(Source::ComicVine, 10, cv),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        // metron_b (ordinal 21) has the most → it wins, not the lower-
        // ordinal metron_a.
        assert_eq!(
            choose_field_candidate(
                MetadataField::Characters,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(21)
        );
    }

    #[test]
    fn cover_present_beats_absent() {
        let mut cv = detail();
        cv.cover_image_url = Some("https://cdn/cv.jpg".into());
        let metron = detail();
        let details = vec![
            pd(Source::Metron, 20, metron),
            pd(Source::ComicVine, 10, cv),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(
                MetadataField::CoverPrimary,
                &details,
                &pref,
                MergeScope::Issue
            ),
            Some(10)
        );
    }

    #[test]
    fn empty_details_yield_none() {
        let details: Vec<ProviderDetail> = vec![];
        let pref = policy(&[Source::Metron]);
        assert_eq!(
            choose_field_candidate(MetadataField::Title, &details, &pref, MergeScope::Issue),
            None
        );
    }

    #[test]
    fn external_id_is_never_chosen() {
        let mut cv = detail();
        cv.title = Some("x".into());
        let details = vec![pd(Source::ComicVine, 10, cv)];
        let pref = policy(&[Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(
                MetadataField::ExternalId(Source::ComicVine),
                &details,
                &pref,
                MergeScope::Issue
            ),
            None
        );
    }

    #[test]
    fn build_default_merge_picks_best_of_both() {
        let mut cv = detail();
        cv.title = Some("Saga".into());
        cv.description = Some("A space opera.".into());
        let mut metron = detail();
        metron.title = Some("Saga".into());
        metron.characters = vec![entity("Hazel"), entity("Marko")];
        let details = vec![
            pd(Source::ComicVine, 10, cv),
            pd(Source::Metron, 20, metron),
        ];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        let merged = build_default_merge(&details, &pref, MergeScope::Issue);
        let by_field = |f: MetadataField| merged.iter().find(|c| c.field == f).unwrap().ordinal;
        // description only on CV(10); characters only on Metron(20);
        // title on both → Metron(20, preferred).
        assert_eq!(by_field(MetadataField::Description), Some(10));
        assert_eq!(by_field(MetadataField::Characters), Some(20));
        assert_eq!(by_field(MetadataField::Title), Some(20));
        // page_count absent everywhere → None.
        assert_eq!(by_field(MetadataField::PageCount), None);
    }
}
