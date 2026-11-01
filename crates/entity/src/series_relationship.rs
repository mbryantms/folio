//! Typed, directed series → series (or series → story arc) edge (WP-7.1,
//! WP-7.5). See migrations `m20270501_000001_series_relationship` and
//! `m20270505_000001_relationship_taxonomy` for the schema rationale.
//!
//! Series → series rows always come in inverse pairs (`A sequel_of B` +
//! `B has_sequel A`); series → arc rows (`to_arc_id`, `tie_in_to` only) are
//! single rows. Never insert or delete one directly — go through
//! `server::relationships`, which keeps both halves in sync inside one
//! transaction.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// Storage contract (AR-7): `kind` on `series_relationship`,
// `series_external_relationship` and `series_relationship_suggestion` stays
// `TEXT`, pinned by each table's `*_kind_chk` CHECK to these strings (the
// suggestion CHECK narrows to the canonical half, see
// [`RelationshipKind::is_canonical`]). `string_value` and the serde name are
// the same string (tests below); an unknown stored string fails to load with
// a `DbErr`. The domain methods live here so the inverse-pair mapping is one
// match on the enum; `server::relationships` re-exports all three types.

/// UI group of a kind (WP-7.5): the picker's section headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipGroup {
    /// Narrative relations: sequels, prequels, spin-offs, tie-ins, …
    Story,
    /// Publication history: continuations, annuals, supplements.
    Publication,
    /// Editions & contents: collections, reprints, alternate editions,
    /// translations.
    Editions,
    /// Adaptations and reimaginings (comic-to-comic).
    Advanced,
}

impl RelationshipGroup {
    pub const ALL: [Self; 4] = [
        Self::Story,
        Self::Publication,
        Self::Editions,
        Self::Advanced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Story => "Story",
            Self::Publication => "Publication history",
            Self::Editions => "Editions & contents",
            Self::Advanced => "Advanced",
        }
    }
}

/// The kind of a directed relationship edge. Wire + DB form is snake_case
/// (`sequel_of`, …); the DB CHECK in `m20270505_000001_relationship_taxonomy`
/// mirrors this list. Every kind has an inverse (self-inverse kinds are
/// their own); [`Self::ALL`] is the catalogue order (grouped, each
/// directional pair adjacent).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    // ── Story ──
    /// `from` is a narrative sequel of `to` (`to` is read first).
    #[sea_orm(string_value = "sequel_of")]
    SequelOf,
    /// Inverse of `sequel_of`.
    #[sea_orm(string_value = "has_sequel")]
    HasSequel,
    /// `from` is a narrative prequel of `to`: written later, set earlier.
    #[sea_orm(string_value = "prequel_of")]
    PrequelOf,
    /// Inverse of `prequel_of`.
    #[sea_orm(string_value = "has_prequel")]
    HasPrequel,
    /// `from` was spun off from `to`.
    #[sea_orm(string_value = "spin_off_of")]
    SpinOffOf,
    /// Inverse of `spin_off_of`.
    #[sea_orm(string_value = "has_spin_off")]
    HasSpinOff,
    /// `from` is a side story of `to`.
    #[sea_orm(string_value = "side_story_of")]
    SideStoryOf,
    /// Inverse of `side_story_of`.
    #[sea_orm(string_value = "has_side_story")]
    HasSideStory,
    /// `from` ties in to `to` (a series or a story arc); the qualifier
    /// carries the role (`main | tie_in | prelude | aftermath`).
    #[sea_orm(string_value = "tie_in_to")]
    TieInTo,
    /// Inverse of `tie_in_to` (series targets only).
    #[sea_orm(string_value = "has_tie_in")]
    HasTieIn,
    /// Self-inverse.
    #[sea_orm(string_value = "crossover_with")]
    CrossoverWith,
    /// Self-inverse.
    #[sea_orm(string_value = "companion_to")]
    CompanionTo,
    /// Self-inverse (manual use; WP-7.6 derives it from universe data).
    #[sea_orm(string_value = "same_universe")]
    SameUniverse,
    /// Self-inverse catch-all.
    #[sea_orm(string_value = "see_also")]
    SeeAlso,
    // ── Publication history ──
    /// `from` continues the publication of `to` (relaunch, retitle, …).
    #[sea_orm(string_value = "continues")]
    Continues,
    /// Inverse of `continues`.
    #[sea_orm(string_value = "continued_by")]
    ContinuedBy,
    /// `from` is an annual of `to`.
    #[sea_orm(string_value = "annual_of")]
    AnnualOf,
    /// Inverse of `annual_of`.
    #[sea_orm(string_value = "has_annual")]
    HasAnnual,
    /// `from` is a supplement to `to`.
    #[sea_orm(string_value = "supplement_to")]
    SupplementTo,
    /// Inverse of `supplement_to`.
    #[sea_orm(string_value = "has_supplement")]
    HasSupplement,
    // ── Editions & contents ──
    /// `from` (e.g. a TPB / omnibus series) collects `to`.
    #[sea_orm(string_value = "collects")]
    Collects,
    /// Inverse of `collects`.
    #[sea_orm(string_value = "collected_in")]
    CollectedIn,
    /// `from` reprints `to`.
    #[sea_orm(string_value = "reprints")]
    Reprints,
    /// Inverse of `reprints`.
    #[sea_orm(string_value = "reprinted_in")]
    ReprintedIn,
    /// Self-inverse.
    #[sea_orm(string_value = "alternate_edition_of")]
    AlternateEditionOf,
    /// `from` is a translation of `to`.
    #[sea_orm(string_value = "translation_of")]
    TranslationOf,
    /// Inverse of `translation_of`.
    #[sea_orm(string_value = "has_translation")]
    HasTranslation,
    // ── Advanced ──
    /// `from` adapts `to` (comic-to-comic).
    #[sea_orm(string_value = "adaptation_of")]
    AdaptationOf,
    /// Inverse of `adaptation_of`.
    #[sea_orm(string_value = "adapted_as")]
    AdaptedAs,
    /// `from` reimagines `to`.
    #[sea_orm(string_value = "reimagining_of")]
    ReimaginingOf,
    /// Inverse of `reimagining_of`.
    #[sea_orm(string_value = "reimagined_as")]
    ReimaginedAs,
}

impl RelationshipKind {
    pub const ALL: [Self; 31] = [
        Self::SequelOf,
        Self::HasSequel,
        Self::PrequelOf,
        Self::HasPrequel,
        Self::SpinOffOf,
        Self::HasSpinOff,
        Self::SideStoryOf,
        Self::HasSideStory,
        Self::TieInTo,
        Self::HasTieIn,
        Self::CrossoverWith,
        Self::CompanionTo,
        Self::SameUniverse,
        Self::SeeAlso,
        Self::Continues,
        Self::ContinuedBy,
        Self::AnnualOf,
        Self::HasAnnual,
        Self::SupplementTo,
        Self::HasSupplement,
        Self::Collects,
        Self::CollectedIn,
        Self::Reprints,
        Self::ReprintedIn,
        Self::AlternateEditionOf,
        Self::TranslationOf,
        Self::HasTranslation,
        Self::AdaptationOf,
        Self::AdaptedAs,
        Self::ReimaginingOf,
        Self::ReimaginedAs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SequelOf => "sequel_of",
            Self::HasSequel => "has_sequel",
            Self::PrequelOf => "prequel_of",
            Self::HasPrequel => "has_prequel",
            Self::SpinOffOf => "spin_off_of",
            Self::HasSpinOff => "has_spin_off",
            Self::SideStoryOf => "side_story_of",
            Self::HasSideStory => "has_side_story",
            Self::TieInTo => "tie_in_to",
            Self::HasTieIn => "has_tie_in",
            Self::CrossoverWith => "crossover_with",
            Self::CompanionTo => "companion_to",
            Self::SameUniverse => "same_universe",
            Self::SeeAlso => "see_also",
            Self::Continues => "continues",
            Self::ContinuedBy => "continued_by",
            Self::AnnualOf => "annual_of",
            Self::HasAnnual => "has_annual",
            Self::SupplementTo => "supplement_to",
            Self::HasSupplement => "has_supplement",
            Self::Collects => "collects",
            Self::CollectedIn => "collected_in",
            Self::Reprints => "reprints",
            Self::ReprintedIn => "reprinted_in",
            Self::AlternateEditionOf => "alternate_edition_of",
            Self::TranslationOf => "translation_of",
            Self::HasTranslation => "has_translation",
            Self::AdaptationOf => "adaptation_of",
            Self::AdaptedAs => "adapted_as",
            Self::ReimaginingOf => "reimagining_of",
            Self::ReimaginedAs => "reimagined_as",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The kind of the reverse edge (`to → from`).
    pub fn inverse(self) -> Self {
        match self {
            Self::SequelOf => Self::HasSequel,
            Self::HasSequel => Self::SequelOf,
            Self::PrequelOf => Self::HasPrequel,
            Self::HasPrequel => Self::PrequelOf,
            Self::SpinOffOf => Self::HasSpinOff,
            Self::HasSpinOff => Self::SpinOffOf,
            Self::SideStoryOf => Self::HasSideStory,
            Self::HasSideStory => Self::SideStoryOf,
            Self::TieInTo => Self::HasTieIn,
            Self::HasTieIn => Self::TieInTo,
            Self::Continues => Self::ContinuedBy,
            Self::ContinuedBy => Self::Continues,
            Self::AnnualOf => Self::HasAnnual,
            Self::HasAnnual => Self::AnnualOf,
            Self::SupplementTo => Self::HasSupplement,
            Self::HasSupplement => Self::SupplementTo,
            Self::Collects => Self::CollectedIn,
            Self::CollectedIn => Self::Collects,
            Self::Reprints => Self::ReprintedIn,
            Self::ReprintedIn => Self::Reprints,
            Self::TranslationOf => Self::HasTranslation,
            Self::HasTranslation => Self::TranslationOf,
            Self::AdaptationOf => Self::AdaptedAs,
            Self::AdaptedAs => Self::AdaptationOf,
            Self::ReimaginingOf => Self::ReimaginedAs,
            Self::ReimaginedAs => Self::ReimaginingOf,
            Self::CrossoverWith
            | Self::CompanionTo
            | Self::SameUniverse
            | Self::SeeAlso
            | Self::AlternateEditionOf => self,
        }
    }

    pub fn is_self_inverse(self) -> bool {
        self.inverse() == self
    }

    /// The direction suggestion rows are stored in: self-inverse kinds and
    /// the "subject" half of every directional pair (`sequel_of`,
    /// `continues`, `collects`, …), never `has_*` / `*_by` / `*_in` / `*_as`.
    pub fn is_canonical(self) -> bool {
        matches!(
            self,
            Self::SequelOf
                | Self::PrequelOf
                | Self::SpinOffOf
                | Self::SideStoryOf
                | Self::TieInTo
                | Self::Continues
                | Self::AnnualOf
                | Self::SupplementTo
                | Self::Collects
                | Self::Reprints
                | Self::TranslationOf
                | Self::AdaptationOf
                | Self::ReimaginingOf
        ) || self.is_self_inverse()
    }

    /// Human label from the subject's point of view ("Sequel to").
    pub fn label(self) -> &'static str {
        match self {
            Self::SequelOf => "Sequel to",
            Self::HasSequel => "Has sequel",
            Self::PrequelOf => "Prequel to",
            Self::HasPrequel => "Has prequel",
            Self::SpinOffOf => "Spin-off of",
            Self::HasSpinOff => "Has spin-off",
            Self::SideStoryOf => "Side story of",
            Self::HasSideStory => "Has side story",
            Self::TieInTo => "Tie-in to",
            Self::HasTieIn => "Has tie-in",
            Self::CrossoverWith => "Crossover with",
            Self::CompanionTo => "Companion to",
            Self::SameUniverse => "Same universe as",
            Self::SeeAlso => "See also",
            Self::Continues => "Continues",
            Self::ContinuedBy => "Continued by",
            Self::AnnualOf => "Annual of",
            Self::HasAnnual => "Has annual",
            Self::SupplementTo => "Supplement to",
            Self::HasSupplement => "Has supplement",
            Self::Collects => "Collects",
            Self::CollectedIn => "Collected in",
            Self::Reprints => "Reprints",
            Self::ReprintedIn => "Reprinted in",
            Self::AlternateEditionOf => "Alternate edition of",
            Self::TranslationOf => "Translation of",
            Self::HasTranslation => "Has translation",
            Self::AdaptationOf => "Adaptation of",
            Self::AdaptedAs => "Adapted as",
            Self::ReimaginingOf => "Reimagining of",
            Self::ReimaginedAs => "Reimagined as",
        }
    }

    /// The label with the qualifier folded in where it changes the
    /// meaning: a tie-in's role ("Prelude to", "Has aftermath"). Other
    /// kinds return [`Self::label`] (a continuation qualifier is shown
    /// next to the label, not inside it).
    pub fn display_label(self, qualifier: Option<RelationshipQualifier>) -> String {
        use RelationshipQualifier as Q;
        match (self, qualifier) {
            (Self::TieInTo, Some(Q::Main)) => "Main story of".to_owned(),
            (Self::TieInTo, Some(Q::Prelude)) => "Prelude to".to_owned(),
            (Self::TieInTo, Some(Q::Aftermath)) => "Aftermath of".to_owned(),
            (Self::HasTieIn, Some(Q::Main)) => "Has main story".to_owned(),
            (Self::HasTieIn, Some(Q::Prelude)) => "Has prelude".to_owned(),
            (Self::HasTieIn, Some(Q::Aftermath)) => "Has aftermath".to_owned(),
            _ => self.label().to_owned(),
        }
    }

    pub fn group(self) -> RelationshipGroup {
        match self {
            Self::SequelOf
            | Self::HasSequel
            | Self::PrequelOf
            | Self::HasPrequel
            | Self::SpinOffOf
            | Self::HasSpinOff
            | Self::SideStoryOf
            | Self::HasSideStory
            | Self::TieInTo
            | Self::HasTieIn
            | Self::CrossoverWith
            | Self::CompanionTo
            | Self::SameUniverse
            | Self::SeeAlso => RelationshipGroup::Story,
            Self::Continues
            | Self::ContinuedBy
            | Self::AnnualOf
            | Self::HasAnnual
            | Self::SupplementTo
            | Self::HasSupplement => RelationshipGroup::Publication,
            Self::Collects
            | Self::CollectedIn
            | Self::Reprints
            | Self::ReprintedIn
            | Self::AlternateEditionOf
            | Self::TranslationOf
            | Self::HasTranslation => RelationshipGroup::Editions,
            Self::AdaptationOf | Self::AdaptedAs | Self::ReimaginingOf | Self::ReimaginedAs => {
                RelationshipGroup::Advanced
            }
        }
    }

    /// `coverage` is meaningful (collections and reprints, both halves).
    pub fn allows_coverage(self) -> bool {
        matches!(
            self,
            Self::Collects | Self::CollectedIn | Self::Reprints | Self::ReprintedIn
        )
    }

    /// The qualifiers this kind accepts (empty = none).
    pub fn qualifiers(self) -> &'static [RelationshipQualifier] {
        use RelationshipQualifier as Q;
        match self {
            Self::Continues | Self::ContinuedBy => {
                &[Q::Relaunch, Q::Retitle, Q::Merge, Q::Split, Q::Numbering]
            }
            Self::TieInTo | Self::HasTieIn => &[Q::Main, Q::TieIn, Q::Prelude, Q::Aftermath],
            _ => &[],
        }
    }

    /// Kinds that contradict `self` on the same ordered pair: the inverse
    /// of a directional kind (`A sequel_of B` vs `A has_sequel B`). The two
    /// reading-order families count as one (`sequel_of` / `continues` both
    /// put *to* first), so `A continues B` also contradicts `A has_sequel
    /// B`. Self-inverse kinds contradict nothing.
    pub fn contradictions(self) -> Vec<Self> {
        match self {
            Self::SequelOf | Self::Continues => vec![Self::HasSequel, Self::ContinuedBy],
            Self::HasSequel | Self::ContinuedBy => vec![Self::SequelOf, Self::Continues],
            k if k.is_self_inverse() => Vec::new(),
            k => vec![k.inverse()],
        }
    }

    /// May target a story arc instead of a series (`tie_in_to` only).
    pub fn allows_arc_target(self) -> bool {
        self == Self::TieInTo
    }

    /// The suggestion engine may pair series in **different** libraries
    /// for this kind (WP-8.2): the edition kinds — `collects`, `reprints`,
    /// `alternate_edition_of`, `translation_of` and their inverses — since
    /// trades and translations often live in a library of their own. Story
    /// and publication kinds stay within one library.
    pub fn may_cross_libraries(self) -> bool {
        self.group() == RelationshipGroup::Editions
    }
}

impl fmt::Display for RelationshipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RelationshipKind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

/// Continuation qualifier (`continues` / `continued_by`) or tie-in role
/// (`tie_in_to` / `has_tie_in`). The DB CHECK binds each set to its kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipQualifier {
    Relaunch,
    Retitle,
    Merge,
    Split,
    Numbering,
    Main,
    TieIn,
    Prelude,
    Aftermath,
}

impl RelationshipQualifier {
    pub const ALL: [Self; 9] = [
        Self::Relaunch,
        Self::Retitle,
        Self::Merge,
        Self::Split,
        Self::Numbering,
        Self::Main,
        Self::TieIn,
        Self::Prelude,
        Self::Aftermath,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relaunch => "relaunch",
            Self::Retitle => "retitle",
            Self::Merge => "merge",
            Self::Split => "split",
            Self::Numbering => "numbering",
            Self::Main => "main",
            Self::TieIn => "tie_in",
            Self::Prelude => "prelude",
            Self::Aftermath => "aftermath",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Relaunch => "Relaunch",
            Self::Retitle => "Retitle",
            Self::Merge => "Merge",
            Self::Split => "Split",
            Self::Numbering => "Numbering change",
            Self::Main => "Main story",
            Self::TieIn => "Tie-in",
            Self::Prelude => "Prelude",
            Self::Aftermath => "Aftermath",
        }
    }
}

impl FromStr for RelationshipQualifier {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|q| q.as_str() == s).ok_or(())
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_relationship")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Subject of the edge: "`from` is a sequel of `to`".
    pub from_series_id: Uuid,
    /// The other series. NULL exactly when `to_arc_id` is set (DB CHECK).
    #[sea_orm(nullable)]
    pub to_series_id: Option<Uuid>,
    /// Story-arc target (WP-7.5; `tie_in_to` only, no inverse row).
    #[sea_orm(nullable)]
    pub to_arc_id: Option<Uuid>,
    /// One of the 31 kinds (DB CHECK).
    pub kind: RelationshipKind,
    /// `manual | suggested`.
    pub source: String,
    /// Suggestion confidence 0.0–1.0; NULL for manual edges.
    #[sea_orm(nullable)]
    pub confidence: Option<f32>,
    /// Admin who created the edge (NULL once the account is deleted).
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    /// Issue range on the `from` side ("1-6", "1-6,Annual 1"); ≤ 100 chars.
    #[sea_orm(nullable)]
    pub from_range: Option<String>,
    /// Issue range on the `to` side; ≤ 100 chars.
    #[sea_orm(nullable)]
    pub to_range: Option<String>,
    /// `full | partial | unknown` — collects / reprints (and inverses) only.
    #[sea_orm(nullable)]
    pub coverage: Option<String>,
    /// Continuation qualifier (`relaunch | retitle | merge | split |
    /// numbering`) or tie-in role (`main | tie_in | prelude | aftermath`).
    #[sea_orm(nullable)]
    pub qualifier: Option<String>,
    /// Free text, ≤ 500 chars.
    #[sea_orm(nullable)]
    pub note: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn kind_round_trips_exact_db_strings() {
        let pairs = [
            (RelationshipKind::SequelOf, "sequel_of"),
            (RelationshipKind::HasSequel, "has_sequel"),
            (RelationshipKind::PrequelOf, "prequel_of"),
            (RelationshipKind::HasPrequel, "has_prequel"),
            (RelationshipKind::SpinOffOf, "spin_off_of"),
            (RelationshipKind::HasSpinOff, "has_spin_off"),
            (RelationshipKind::SideStoryOf, "side_story_of"),
            (RelationshipKind::HasSideStory, "has_side_story"),
            (RelationshipKind::TieInTo, "tie_in_to"),
            (RelationshipKind::HasTieIn, "has_tie_in"),
            (RelationshipKind::CrossoverWith, "crossover_with"),
            (RelationshipKind::CompanionTo, "companion_to"),
            (RelationshipKind::SameUniverse, "same_universe"),
            (RelationshipKind::SeeAlso, "see_also"),
            (RelationshipKind::Continues, "continues"),
            (RelationshipKind::ContinuedBy, "continued_by"),
            (RelationshipKind::AnnualOf, "annual_of"),
            (RelationshipKind::HasAnnual, "has_annual"),
            (RelationshipKind::SupplementTo, "supplement_to"),
            (RelationshipKind::HasSupplement, "has_supplement"),
            (RelationshipKind::Collects, "collects"),
            (RelationshipKind::CollectedIn, "collected_in"),
            (RelationshipKind::Reprints, "reprints"),
            (RelationshipKind::ReprintedIn, "reprinted_in"),
            (RelationshipKind::AlternateEditionOf, "alternate_edition_of"),
            (RelationshipKind::TranslationOf, "translation_of"),
            (RelationshipKind::HasTranslation, "has_translation"),
            (RelationshipKind::AdaptationOf, "adaptation_of"),
            (RelationshipKind::AdaptedAs, "adapted_as"),
            (RelationshipKind::ReimaginingOf, "reimagining_of"),
            (RelationshipKind::ReimaginedAs, "reimagined_as"),
        ];
        assert_exact_strings(
            &pairs,
            RelationshipKind::as_str,
            RelationshipKind::parse,
            &["sequel", "Sequel_Of", "prequel", "related", ""],
        );
        // `ALL` is the catalogue order and must list every variant once.
        assert_eq!(
            RelationshipKind::ALL.to_vec(),
            pairs.iter().map(|&(k, _)| k).collect::<Vec<_>>()
        );
    }
}
