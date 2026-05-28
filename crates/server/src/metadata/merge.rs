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
//! ## Policy ("most complete")
//! - **Scalars / dates / text**: a non-empty value wins. Among
//!   providers that have a value, the one earliest in
//!   `provider_preference` wins (deterministic tiebreaker).
//! - **Junctions** (`MetadataField::is_junction`): the *richest* set
//!   wins — the provider with the most entries. Ties break by
//!   preference.
//! - **Covers**: the provider that actually has a cover (or the most
//!   variants) wins; ties break by preference.
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
    pub detail: GenericMetadata,
}

/// Tiebreaker order when multiple providers offer a value for the same
/// field. Sources not listed sort last (in their `details` order).
#[derive(Clone, Debug)]
pub struct MergePolicyConfig {
    pub provider_preference: Vec<Source>,
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
        MetadataField::Credits => detail.credits.len(),
        MetadataField::Characters => detail.characters.len(),
        MetadataField::Teams => detail.teams.len(),
        MetadataField::Locations => detail.locations.len(),
        MetadataField::Concepts => detail.concepts.len(),
        MetadataField::Objects => detail.objects.len(),
        MetadataField::StoryArcs => detail.story_arcs.len(),
        MetadataField::Universes => detail.universes.len(),
        MetadataField::Genres => detail.genres.iter().filter(|g| !g.trim().is_empty()).count(),
        MetadataField::Tags => detail.tags.iter().filter(|t| !t.trim().is_empty()).count(),
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
        MetadataField::PageCount => detail.page_count.map(|n| n.to_string()),
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
/// side. The `provider_preference` order is the primary tiebreaker;
/// lower ordinal (better-ranked candidate) breaks ties within a source.
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
        // Richest set wins; tiebreak by preference rank, then ordinal.
        details
            .iter()
            .filter(|d| field_richness(&d.detail, field, scope) > 0)
            .max_by_key(|d| {
                (
                    field_richness(&d.detail, field, scope),
                    std::cmp::Reverse(policy.rank(d.source)),
                    std::cmp::Reverse(d.ordinal),
                )
            })
            .map(|d| d.ordinal)
    } else {
        // Non-empty wins; among those, lowest (preference rank, ordinal).
        details
            .iter()
            .filter(|d| field_value_as_string(&d.detail, field, scope).is_some())
            .min_by_key(|d| (policy.rank(d.source), d.ordinal))
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
        ProviderDetail {
            source,
            external_id: ordinal.to_string(),
            ordinal,
            detail,
        }
    }

    fn policy(order: &[Source]) -> MergePolicyConfig {
        MergePolicyConfig {
            provider_preference: order.to_vec(),
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
        let details = vec![pd(Source::Metron, 20, metron), pd(Source::ComicVine, 10, cv)];
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
        let details = vec![pd(Source::ComicVine, 10, cv), pd(Source::Metron, 20, metron)];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(MetadataField::Title, &details, &pref, MergeScope::Issue),
            Some(20)
        );
    }

    #[test]
    fn junction_richer_set_wins() {
        let mut cv = detail();
        cv.characters = vec![entity("Hazel")];
        let mut metron = detail();
        metron.characters = vec![entity("Hazel"), entity("Marko"), entity("Alana")];
        let details = vec![pd(Source::ComicVine, 10, cv), pd(Source::Metron, 20, metron)];
        // Even with CV preferred, Metron's richer set wins.
        let pref = policy(&[Source::ComicVine, Source::Metron]);
        assert_eq!(
            choose_field_candidate(MetadataField::Characters, &details, &pref, MergeScope::Issue),
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
        let details = vec![pd(Source::ComicVine, 10, cv), pd(Source::Metron, 20, metron)];
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
            choose_field_candidate(MetadataField::Characters, &details, &pref, MergeScope::Issue),
            Some(21)
        );
    }

    #[test]
    fn cover_present_beats_absent() {
        let mut cv = detail();
        cv.cover_image_url = Some("https://cdn/cv.jpg".into());
        let metron = detail();
        let details = vec![pd(Source::Metron, 20, metron), pd(Source::ComicVine, 10, cv)];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        assert_eq!(
            choose_field_candidate(MetadataField::CoverPrimary, &details, &pref, MergeScope::Issue),
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
        let details = vec![pd(Source::ComicVine, 10, cv), pd(Source::Metron, 20, metron)];
        let pref = policy(&[Source::Metron, Source::ComicVine]);
        let merged = build_default_merge(&details, &pref, MergeScope::Issue);
        let by_field =
            |f: MetadataField| merged.iter().find(|c| c.field == f).unwrap().ordinal;
        // description only on CV(10); characters only on Metron(20);
        // title on both → Metron(20, preferred).
        assert_eq!(by_field(MetadataField::Description), Some(10));
        assert_eq!(by_field(MetadataField::Characters), Some(20));
        assert_eq!(by_field(MetadataField::Title), Some(20));
        // page_count absent everywhere → None.
        assert_eq!(by_field(MetadataField::PageCount), None);
    }
}
