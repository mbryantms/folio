//! Series relationships (WP-7.1, spec §5.2 / Phase 7; taxonomy WP-7.5):
//! typed, directed edges between series — or from a series to a story arc.
//!
//! This module is the **only** write surface for `series_relationship`:
//!
//! - [`create_pair`] / [`create_pair_scoped`] insert a series → series edge
//!   and its inverse together (idempotent — an existing pair is returned,
//!   not an error).
//! - [`create_arc_edge`] inserts a series → arc edge (`tie_in_to` only; a
//!   single row, since an arc isn't a series and has no inverse half).
//! - [`update_edge`] changes an edge's kind and/or scope, keeping both
//!   halves in sync (a kind change is delete + create).
//! - [`delete_pair`] / [`delete_pair_by_id`] remove both halves together.
//! - [`traverse`] / [`chain`] walk the graph with a depth-capped,
//!   cycle-safe recursive CTE (one of the spec's raw-SQL escape hatches,
//!   §17 "(d) recursive series-relationship traversal").
//!
//! The HTTP surface lives in [`crate::api::series_relationships`]; the
//! suggestion engine (WP-7.2) accepts a suggestion by calling
//! [`create_pair`] with [`RelationshipSource::Suggested`].
//!
//! Edge semantics read left to right: `from sequel_of to` means *from* is
//! the sequel, so *to* comes first in reading order. See
//! `docs/dev/series-relationships.md`.

pub mod suggestions;

use chrono::Utc;
use entity::series_relationship as rel;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement, Value,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Hard cap on traversal depth (spec Phase 7: "CTE depth capped at 6").
/// Callers asking for more are clamped.
pub const MAX_TRAVERSAL_DEPTH: u32 = 6;

/// Longest accepted `from_range` / `to_range`.
pub const MAX_RANGE_LEN: usize = 100;
/// Longest accepted `note`.
pub const MAX_NOTE_LEN: usize = 500;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    // ── Story ──
    /// `from` is a narrative sequel of `to` (`to` is read first).
    SequelOf,
    /// Inverse of `sequel_of`.
    HasSequel,
    /// `from` is a narrative prequel of `to`: written later, set earlier.
    PrequelOf,
    /// Inverse of `prequel_of`.
    HasPrequel,
    /// `from` was spun off from `to`.
    SpinOffOf,
    /// Inverse of `spin_off_of`.
    HasSpinOff,
    /// `from` is a side story of `to`.
    SideStoryOf,
    /// Inverse of `side_story_of`.
    HasSideStory,
    /// `from` ties in to `to` (a series or a story arc); the qualifier
    /// carries the role (`main | tie_in | prelude | aftermath`).
    TieInTo,
    /// Inverse of `tie_in_to` (series targets only).
    HasTieIn,
    /// Self-inverse.
    CrossoverWith,
    /// Self-inverse.
    CompanionTo,
    /// Self-inverse (manual use; WP-7.6 derives it from universe data).
    SameUniverse,
    /// Self-inverse catch-all.
    SeeAlso,
    // ── Publication history ──
    /// `from` continues the publication of `to` (relaunch, retitle, …).
    Continues,
    /// Inverse of `continues`.
    ContinuedBy,
    /// `from` is an annual of `to`.
    AnnualOf,
    /// Inverse of `annual_of`.
    HasAnnual,
    /// `from` is a supplement to `to`.
    SupplementTo,
    /// Inverse of `supplement_to`.
    HasSupplement,
    // ── Editions & contents ──
    /// `from` (e.g. a TPB / omnibus series) collects `to`.
    Collects,
    /// Inverse of `collects`.
    CollectedIn,
    /// `from` reprints `to`.
    Reprints,
    /// Inverse of `reprints`.
    ReprintedIn,
    /// Self-inverse.
    AlternateEditionOf,
    /// `from` is a translation of `to`.
    TranslationOf,
    /// Inverse of `translation_of`.
    HasTranslation,
    // ── Advanced ──
    /// `from` adapts `to` (comic-to-comic).
    AdaptationOf,
    /// Inverse of `adaptation_of`.
    AdaptedAs,
    /// `from` reimagines `to`.
    ReimaginingOf,
    /// Inverse of `reimagining_of`.
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
}

impl fmt::Display for RelationshipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RelationshipKind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|k| k.as_str() == s).ok_or(())
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

/// How much of the target a collection / reprint covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipCoverage {
    Full,
    Partial,
    Unknown,
}

impl RelationshipCoverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for RelationshipCoverage {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(Self::Full),
            "partial" => Ok(Self::Partial),
            "unknown" => Ok(Self::Unknown),
            _ => Err(()),
        }
    }
}

/// Optional scope on an edge (WP-7.5), read from the `from` side. The
/// inverse row carries the [`Scope::mirrored`] copy (ranges swapped).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Issue range on the `from` side ("1-6", "1-6,Annual 1").
    pub from_range: Option<String>,
    /// Issue range on the `to` side.
    pub to_range: Option<String>,
    pub coverage: Option<RelationshipCoverage>,
    pub qualifier: Option<RelationshipQualifier>,
    pub note: Option<String>,
}

/// One field-level scope problem (→ 422 `details`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeIssue {
    pub field: &'static str,
    pub message: String,
}

impl Scope {
    /// Trim text fields; blank becomes `None`.
    pub fn normalized(self) -> Self {
        fn clean(v: Option<String>) -> Option<String> {
            v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
        }
        Self {
            from_range: clean(self.from_range),
            to_range: clean(self.to_range),
            coverage: self.coverage,
            qualifier: self.qualifier,
            note: clean(self.note),
        }
    }

    /// The scope as the inverse row sees it: ranges swapped, the rest
    /// shared.
    pub fn mirrored(&self) -> Self {
        Self {
            from_range: self.to_range.clone(),
            to_range: self.from_range.clone(),
            coverage: self.coverage,
            qualifier: self.qualifier,
            note: self.note.clone(),
        }
    }

    /// Read the scope columns of a row (unknown values are dropped; the DB
    /// CHECKs make that unreachable).
    pub fn of(row: &rel::Model) -> Self {
        Self {
            from_range: row.from_range.clone(),
            to_range: row.to_range.clone(),
            coverage: row.coverage.as_deref().and_then(|c| c.parse().ok()),
            qualifier: row.qualifier.as_deref().and_then(|q| q.parse().ok()),
            note: row.note.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The scope with the fields `kind` doesn't take dropped: `coverage`
    /// unless [`RelationshipKind::allows_coverage`], a `qualifier` outside
    /// [`RelationshipKind::qualifiers`]. Ranges and the note fit every kind.
    /// Used when a suggestion is accepted as a different kind (WP-7.6): the
    /// proposed scope is kept where it still means something instead of
    /// failing the accept.
    pub fn fitted(mut self, kind: RelationshipKind) -> Self {
        if !kind.allows_coverage() {
            self.coverage = None;
        }
        if self
            .qualifier
            .is_some_and(|q| !kind.qualifiers().contains(&q))
        {
            self.qualifier = None;
        }
        self
    }

    /// Validate against `kind`: ranges ≤ [`MAX_RANGE_LEN`] chars, note ≤
    /// [`MAX_NOTE_LEN`], `coverage` only on collects / reprints (and
    /// inverses), `qualifier` only from `kind`'s own set.
    pub fn validate(&self, kind: RelationshipKind) -> Vec<ScopeIssue> {
        let mut out = Vec::new();
        for (field, v) in [
            ("from_range", &self.from_range),
            ("to_range", &self.to_range),
        ] {
            if let Some(v) = v {
                if v.chars().count() > MAX_RANGE_LEN {
                    out.push(ScopeIssue {
                        field,
                        message: format!("must be at most {MAX_RANGE_LEN} characters"),
                    });
                } else if v.chars().any(char::is_control) {
                    out.push(ScopeIssue {
                        field,
                        message: "must not contain control characters".to_owned(),
                    });
                }
            }
        }
        if let Some(n) = &self.note
            && n.chars().count() > MAX_NOTE_LEN
        {
            out.push(ScopeIssue {
                field: "note",
                message: format!("must be at most {MAX_NOTE_LEN} characters"),
            });
        }
        if self.coverage.is_some() && !kind.allows_coverage() {
            out.push(ScopeIssue {
                field: "coverage",
                message: format!(
                    "coverage only applies to collects / collected in / reprints / reprinted in, not `{kind}`"
                ),
            });
        }
        if let Some(q) = self.qualifier
            && !kind.qualifiers().contains(&q)
        {
            let allowed = kind.qualifiers();
            let message = if allowed.is_empty() {
                format!("`{kind}` takes no qualifier")
            } else {
                format!(
                    "`{}` is not a qualifier of `{kind}` (allowed: {})",
                    q.as_str(),
                    allowed
                        .iter()
                        .map(|q| q.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            out.push(ScopeIssue {
                field: "qualifier",
                message,
            });
        }
        out
    }
}

/// Where an edge came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipSource {
    /// Created by an admin by hand.
    Manual,
    /// An accepted suggestion from the WP-7.2 engine.
    Suggested,
}

impl RelationshipSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Suggested => "suggested",
        }
    }
}

impl FromStr for RelationshipSource {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Self::Manual),
            "suggested" => Ok(Self::Suggested),
            _ => Err(()),
        }
    }
}

/// Why a write refused.
#[derive(Debug)]
pub enum PairError {
    /// `from == to` (also enforced by the DB CHECK).
    SelfEdge,
    /// The opposite directional kind already links the same ordered pair
    /// (e.g. asking for `A sequel_of B` while `A has_sequel B` exists).
    Conflict {
        existing: RelationshipKind,
    },
    /// An edit would turn the edge into one that already exists.
    Duplicate {
        kind: RelationshipKind,
    },
    /// `confidence` outside 0.0–1.0.
    InvalidConfidence,
    /// The kind can't target a story arc (only `tie_in_to` can).
    ArcKind {
        kind: RelationshipKind,
    },
    /// Scope fields don't fit the kind (field-level, → 422 `details`).
    InvalidScope(Vec<ScopeIssue>),
    Db(DbErr),
}

impl From<DbErr> for PairError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge => f.write_str("a series cannot be related to itself"),
            Self::Conflict { existing } => {
                write!(f, "conflicts with existing `{existing}` relationship")
            }
            Self::Duplicate { kind } => {
                write!(f, "a `{kind}` relationship between these already exists")
            }
            Self::InvalidConfidence => f.write_str("confidence must be between 0 and 1"),
            Self::ArcKind { kind } => {
                write!(
                    f,
                    "`{kind}` cannot target a story arc (only `tie_in_to` can)"
                )
            }
            Self::InvalidScope(issues) => f.write_str(
                &issues
                    .iter()
                    .map(|i| format!("{}: {}", i.field, i.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

/// Result of [`create_pair`].
#[derive(Debug, Clone)]
pub struct PairOutcome {
    /// The `from → to` row (pre-existing or freshly inserted).
    pub forward: rel::Model,
    /// The `to → from` row.
    pub inverse: rel::Model,
    /// `true` when this call inserted the forward edge; `false` when the
    /// pair already existed (idempotent no-op apart from healing a
    /// missing inverse half).
    pub created: bool,
}

/// [`create_pair_scoped`] with no scope.
pub async fn create_pair<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
    source: RelationshipSource,
    confidence: Option<f32>,
    created_by: Option<Uuid>,
) -> Result<PairOutcome, PairError> {
    create_pair_scoped(
        conn,
        from,
        to,
        kind,
        source,
        confidence,
        created_by,
        &Scope::default(),
    )
    .await
}

fn check_confidence(confidence: Option<f32>) -> Result<(), PairError> {
    if let Some(c) = confidence
        && !(0.0..=1.0).contains(&c)
    {
        return Err(PairError::InvalidConfidence);
    }
    Ok(())
}

fn check_scope(scope: &Scope, kind: RelationshipKind) -> Result<(), PairError> {
    let issues = scope.validate(kind);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(PairError::InvalidScope(issues))
    }
}

/// Insert `from —kind→ to` and its inverse `to —kind.inverse()→ from`, with
/// `scope` on the forward row and [`Scope::mirrored`] on the inverse.
///
/// Run it inside a transaction (`conn` is usually a
/// `DatabaseTransaction`) so both halves land together. Idempotent: an
/// existing pair is returned unchanged (its scope is **not** overwritten —
/// use [`update_edge`]) with `created = false`; a missing inverse half
/// (should never happen, but the old row might predate a bug fix) is
/// re-created. Concurrent inserts are safe — both statements are
/// `ON CONFLICT DO NOTHING` on the `(from, to_series, kind)` partial unique
/// index.
///
/// Does **not** verify the series exist or that the caller may see them;
/// the FK rejects unknown ids and the HTTP layer does the ACL check.
#[allow(clippy::too_many_arguments)]
pub async fn create_pair_scoped<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
    source: RelationshipSource,
    confidence: Option<f32>,
    created_by: Option<Uuid>,
    scope: &Scope,
) -> Result<PairOutcome, PairError> {
    if from == to {
        return Err(PairError::SelfEdge);
    }
    check_confidence(confidence)?;
    check_scope(scope, kind)?;
    // A directional kind contradicts its own inverse on the same ordered
    // pair (`A sequel_of B` + `A has_sequel B` would make each the other's
    // sequel); see [`RelationshipKind::contradictions`]. Self-inverse kinds
    // can't conflict this way.
    for contradiction in kind.contradictions() {
        if find_edge(conn, from, to, contradiction).await?.is_some() {
            return Err(PairError::Conflict {
                existing: contradiction,
            });
        }
    }

    let now = Utc::now().fixed_offset();
    let insert = |a: Uuid, b: Uuid, k: RelationshipKind, s: &Scope| {
        Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO series_relationship \
               (id, from_series_id, to_series_id, kind, source, confidence, created_by, created_at, \
                from_range, to_range, coverage, qualifier, note) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (from_series_id, to_series_id, kind) \
                WHERE to_series_id IS NOT NULL DO NOTHING",
            [
                Value::from(Uuid::now_v7()),
                Value::from(a),
                Value::from(b),
                Value::from(k.as_str()),
                Value::from(source.as_str()),
                Value::from(confidence),
                Value::from(created_by),
                Value::from(now),
                Value::from(s.from_range.clone()),
                Value::from(s.to_range.clone()),
                Value::from(s.coverage.map(|c| c.as_str().to_owned())),
                Value::from(s.qualifier.map(|q| q.as_str().to_owned())),
                Value::from(s.note.clone()),
            ],
        )
    };
    let fwd = conn.execute_raw(insert(from, to, kind, scope)).await?;
    conn.execute_raw(insert(to, from, kind.inverse(), &scope.mirrored()))
        .await?;

    let forward = find_edge(conn, from, to, kind)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("series_relationship forward row".into()))?;
    let inverse = find_edge(conn, to, from, kind.inverse())
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("series_relationship inverse row".into()))?;
    Ok(PairOutcome {
        forward,
        inverse,
        created: fwd.rows_affected() > 0,
    })
}

/// Result of [`create_arc_edge`].
#[derive(Debug, Clone)]
pub struct ArcEdgeOutcome {
    pub row: rel::Model,
    /// `false` when the edge already existed (returned unchanged).
    pub created: bool,
}

/// Insert a series → story-arc edge (`from —kind→ arc`). Only kinds with
/// [`RelationshipKind::allows_arc_target`] (`tie_in_to`); a single row, no
/// inverse. Idempotent like [`create_pair_scoped`].
#[allow(clippy::too_many_arguments)]
pub async fn create_arc_edge<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    arc_id: Uuid,
    kind: RelationshipKind,
    source: RelationshipSource,
    confidence: Option<f32>,
    created_by: Option<Uuid>,
    scope: &Scope,
) -> Result<ArcEdgeOutcome, PairError> {
    if !kind.allows_arc_target() {
        return Err(PairError::ArcKind { kind });
    }
    check_confidence(confidence)?;
    check_scope(scope, kind)?;
    let res = conn
        .execute_raw(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO series_relationship \
               (id, from_series_id, to_arc_id, kind, source, confidence, created_by, created_at, \
                from_range, to_range, coverage, qualifier, note) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (from_series_id, to_arc_id, kind) \
                WHERE to_arc_id IS NOT NULL DO NOTHING",
            [
                Value::from(Uuid::now_v7()),
                Value::from(from),
                Value::from(arc_id),
                Value::from(kind.as_str()),
                Value::from(source.as_str()),
                Value::from(confidence),
                Value::from(created_by),
                Value::from(Utc::now().fixed_offset()),
                Value::from(scope.from_range.clone()),
                Value::from(scope.to_range.clone()),
                Value::from(scope.coverage.map(|c| c.as_str().to_owned())),
                Value::from(scope.qualifier.map(|q| q.as_str().to_owned())),
                Value::from(scope.note.clone()),
            ],
        ))
        .await?;
    let row = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(from))
        .filter(rel::Column::ToArcId.eq(arc_id))
        .filter(rel::Column::Kind.eq(kind.as_str()))
        .one(conn)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("series_relationship arc row".into()))?;
    Ok(ArcEdgeOutcome {
        row,
        created: res.rows_affected() > 0,
    })
}

/// Result of [`update_edge`].
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    /// The forward row before the edit.
    pub before: rel::Model,
    /// The forward row after (a new id when the kind changed).
    pub forward: rel::Model,
    /// The inverse row after (`None` for an arc edge).
    pub inverse: Option<rel::Model>,
    pub kind_changed: bool,
}

/// Change the kind and/or scope of `forward` (the half read from the
/// caller's point of view; `scope` is read from `forward.from`'s side).
/// Both halves stay in sync: the inverse row gets [`Scope::mirrored`]. A
/// kind change is a delete + create of the pair (new ids; source,
/// confidence and creator kept), so the contradiction rule applies to the
/// new kind; turning the edge into a pair that already exists is
/// [`PairError::Duplicate`]. Arc edges keep `tie_in_to` and are updated in
/// place. Run inside a transaction so a refused create rolls the delete
/// back.
pub async fn update_edge<C: ConnectionTrait>(
    conn: &C,
    forward: rel::Model,
    kind: RelationshipKind,
    scope: Scope,
) -> Result<UpdateOutcome, PairError> {
    let old_kind: RelationshipKind = forward
        .kind
        .parse()
        .map_err(|()| DbErr::Custom(format!("bad relationship kind {}", forward.kind)))?;
    check_scope(&scope, kind)?;

    // Arc edge: single row, kind must stay arc-capable.
    if forward.to_arc_id.is_some() {
        if !kind.allows_arc_target() {
            return Err(PairError::ArcKind { kind });
        }
        let row = set_scope(conn, forward.id, kind, &scope).await?;
        return Ok(UpdateOutcome {
            before: forward,
            forward: row,
            inverse: None,
            kind_changed: kind != old_kind,
        });
    }
    let to = forward
        .to_series_id
        .ok_or_else(|| DbErr::Custom("relationship row has no target".into()))?;

    if kind == old_kind {
        let fwd = set_scope(conn, forward.id, kind, &scope).await?;
        let inverse = match find_edge(conn, to, forward.from_series_id, kind.inverse()).await? {
            Some(inv) => set_scope(conn, inv.id, kind.inverse(), &scope.mirrored()).await?,
            None => {
                // Heal a missing inverse half (create_pair semantics).
                let source = forward.source.parse().unwrap_or(RelationshipSource::Manual);
                create_pair_scoped(
                    conn,
                    forward.from_series_id,
                    to,
                    kind,
                    source,
                    forward.confidence,
                    forward.created_by,
                    &scope,
                )
                .await?
                .inverse
            }
        };
        return Ok(UpdateOutcome {
            before: forward,
            forward: fwd,
            inverse: Some(inverse),
            kind_changed: false,
        });
    }

    delete_pair(conn, forward.from_series_id, to, old_kind).await?;
    let source = forward.source.parse().unwrap_or(RelationshipSource::Manual);
    let out = create_pair_scoped(
        conn,
        forward.from_series_id,
        to,
        kind,
        source,
        forward.confidence,
        forward.created_by,
        &scope,
    )
    .await?;
    if !out.created {
        return Err(PairError::Duplicate { kind });
    }
    Ok(UpdateOutcome {
        before: forward,
        forward: out.forward,
        inverse: Some(out.inverse),
        kind_changed: true,
    })
}

async fn set_scope<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
    kind: RelationshipKind,
    scope: &Scope,
) -> Result<rel::Model, DbErr> {
    use sea_orm::{ActiveModelTrait, Set};
    let am = rel::ActiveModel {
        id: Set(id),
        kind: Set(kind.as_str().to_owned()),
        from_range: Set(scope.from_range.clone()),
        to_range: Set(scope.to_range.clone()),
        coverage: Set(scope.coverage.map(|c| c.as_str().to_owned())),
        qualifier: Set(scope.qualifier.map(|q| q.as_str().to_owned())),
        note: Set(scope.note.clone()),
        ..Default::default()
    };
    am.update(conn).await
}

/// Delete `from —kind→ to` and its inverse. Returns the deleted forward
/// row, or `None` when the edge didn't exist (a stray inverse half is
/// still removed). Run inside a transaction.
pub async fn delete_pair<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> Result<Option<rel::Model>, DbErr> {
    let forward = find_edge(conn, from, to, kind).await?;
    let pair_filter = sea_orm::Condition::any()
        .add(
            sea_orm::Condition::all()
                .add(rel::Column::FromSeriesId.eq(from))
                .add(rel::Column::ToSeriesId.eq(to))
                .add(rel::Column::Kind.eq(kind.as_str())),
        )
        .add(
            sea_orm::Condition::all()
                .add(rel::Column::FromSeriesId.eq(to))
                .add(rel::Column::ToSeriesId.eq(from))
                .add(rel::Column::Kind.eq(kind.inverse().as_str())),
        );
    rel::Entity::delete_many()
        .filter(pair_filter)
        .exec(conn)
        .await?;
    Ok(forward)
}

/// [`delete_pair`] keyed by either half's row id (or an arc edge's id).
/// `None` when no such row.
pub async fn delete_pair_by_id<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<rel::Model>, DbErr> {
    let Some(row) = rel::Entity::find_by_id(id).one(conn).await? else {
        return Ok(None);
    };
    let (Some(to), Ok(kind)) = (row.to_series_id, row.kind.parse::<RelationshipKind>()) else {
        // An arc edge (single row), or an unknown kind (unreachable under
        // the DB CHECK): drop the lone row.
        rel::Entity::delete_by_id(id).exec(conn).await?;
        return Ok(Some(row));
    };
    delete_pair(conn, row.from_series_id, to, kind).await
}

async fn find_edge<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> Result<Option<rel::Model>, DbErr> {
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(from))
        .filter(rel::Column::ToSeriesId.eq(to))
        .filter(rel::Column::Kind.eq(kind.as_str()))
        .one(conn)
        .await
}

/// The forward row of the pair `row` belongs to, seen from `series_id`:
/// `row` itself when it starts at `series_id`, else its inverse partner
/// (`None` when `row` doesn't touch `series_id`, or the partner is
/// missing). Arc edges only match from their own series.
pub async fn row_from_perspective<C: ConnectionTrait>(
    conn: &C,
    row: rel::Model,
    series_id: Uuid,
) -> Result<Option<rel::Model>, DbErr> {
    if row.from_series_id == series_id {
        return Ok(Some(row));
    }
    if row.to_series_id != Some(series_id) {
        return Ok(None);
    }
    let Ok(kind) = row.kind.parse::<RelationshipKind>() else {
        return Ok(None);
    };
    find_edge(conn, series_id, row.from_series_id, kind.inverse()).await
}

/// Series → arc edges out of `series_id`, oldest first.
pub async fn arc_edges<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
) -> Result<Vec<rel::Model>, DbErr> {
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(series_id))
        .filter(rel::Column::ToArcId.is_not_null())
        .order_by_asc(rel::Column::CreatedAt)
        .order_by_asc(rel::Column::Id)
        .all(conn)
        .await
}

/// Every series → arc edge pointing at `arc_id` ("series that tie in to
/// this arc"), oldest first. Unfiltered — the HTTP layer
/// (`GET /arcs/{slug}/tie-ins`) applies the ACL and paginates in SQL.
pub async fn arc_tie_ins<C: ConnectionTrait>(
    conn: &C,
    arc_id: Uuid,
) -> Result<Vec<rel::Model>, DbErr> {
    rel::Entity::find()
        .filter(rel::Column::ToArcId.eq(arc_id))
        .order_by_asc(rel::Column::CreatedAt)
        .order_by_asc(rel::Column::Id)
        .all(conn)
        .await
}

/// Direct (depth-1) series → series edges out of `series_id`, oldest first.
/// Because every such edge is stored with its inverse, this is the complete
/// series neighbourhood. Arc edges are [`arc_edges`].
pub async fn direct<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
) -> Result<Vec<rel::Model>, DbErr> {
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(series_id))
        .filter(rel::Column::ToSeriesId.is_not_null())
        .order_by_asc(rel::Column::CreatedAt)
        .order_by_asc(rel::Column::Id)
        .all(conn)
        .await
}

/// One node reached by [`traverse`].
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct TraversalNode {
    pub series_id: Uuid,
    /// Shortest hop count from the start (≥ 1).
    pub depth: i32,
    /// The node this one was reached from on a shortest path (the start
    /// series for depth-1 nodes). Lets callers prune a subtree when an
    /// intermediate node is hidden from the viewer.
    pub parent_id: Uuid,
}

/// Every series reachable from `start` by following edges whose kind is in
/// `kinds`, up to `max_depth` hops (clamped to [`MAX_TRAVERSAL_DEPTH`]).
/// Cycle-safe: a path never revisits a node (the CTE carries the visited
/// path), and each node is reported once at its shortest depth. The start
/// series itself is never returned. Ordered by depth, then id.
pub async fn traverse<C: ConnectionTrait>(
    conn: &C,
    start: Uuid,
    kinds: &[RelationshipKind],
    max_depth: u32,
) -> Result<Vec<TraversalNode>, DbErr> {
    let depth = max_depth.min(MAX_TRAVERSAL_DEPTH);
    if depth == 0 || kinds.is_empty() {
        return Ok(Vec::new());
    }
    let kinds: Vec<String> = kinds.iter().map(|k| k.as_str().to_owned()).collect();
    let sql = r#"
        WITH RECURSIVE walk(series_id, parent_id, depth, path) AS (
            SELECT r.to_series_id, r.from_series_id, 1, ARRAY[r.from_series_id, r.to_series_id]
              FROM series_relationship r
             WHERE r.from_series_id = $1 AND r.kind = ANY($2)
               AND r.to_series_id IS NOT NULL
            UNION ALL
            SELECT r.to_series_id, w.series_id, w.depth + 1, w.path || r.to_series_id
              FROM walk w
              JOIN series_relationship r ON r.from_series_id = w.series_id
             WHERE w.depth < $3
               AND r.kind = ANY($2)
               AND r.to_series_id IS NOT NULL
               AND NOT (r.to_series_id = ANY(w.path))
        )
        SELECT DISTINCT ON (series_id) series_id, depth, parent_id
          FROM walk
         ORDER BY series_id, depth, parent_id
    "#;
    let stmt = Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        [
            Value::from(start),
            Value::from(kinds),
            Value::from(i32::try_from(depth).unwrap_or(6)),
        ],
    );
    let mut nodes = TraversalNode::find_by_statement(stmt).all(conn).await?;
    nodes.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.series_id.cmp(&b.series_id)));
    Ok(nodes)
}

/// One step of a reading-order chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainNode {
    pub series_id: Uuid,
    /// Signed reading-order offset from the start series: negative =
    /// read before (prequels), `0` = the start series, positive = read
    /// after (sequels). Several nodes can share a position when the chain
    /// branches.
    pub position: i32,
    pub parent_id: Option<Uuid>,
}

/// Kinds the reading-order chain follows toward what is read **before**
/// the start (`start sequel_of X` / `start continues X` ⇒ X first).
pub const CHAIN_BEFORE: [RelationshipKind; 2] =
    [RelationshipKind::SequelOf, RelationshipKind::Continues];
/// Kinds the chain follows toward what is read **after** the start (the
/// inverses of [`CHAIN_BEFORE`]).
pub const CHAIN_AFTER: [RelationshipKind; 2] =
    [RelationshipKind::HasSequel, RelationshipKind::ContinuedBy];

/// The reading-order chain through `start` (WP-7.5): walks narrative
/// sequels (`sequel_of` / `has_sequel`) and publication continuity
/// (`continues` / `continued_by`) — mixed freely along a path — backwards
/// for everything read before and forwards for everything read after, each
/// up to [`MAX_TRAVERSAL_DEPTH`] hops. A narrative `prequel_of` is **not**
/// part of the chain (it is shown in its own group). Returns an empty list
/// when `start` has no chain edges; otherwise the start series is included
/// at position 0.
pub async fn chain<C: ConnectionTrait>(conn: &C, start: Uuid) -> Result<Vec<ChainNode>, DbErr> {
    let before = traverse(conn, start, &CHAIN_BEFORE, MAX_TRAVERSAL_DEPTH).await?;
    let after = traverse(conn, start, &CHAIN_AFTER, MAX_TRAVERSAL_DEPTH).await?;
    if before.is_empty() && after.is_empty() {
        return Ok(Vec::new());
    }
    // A cycle (A sequel_of B sequel_of A) can put the same node on both
    // sides; keep it on the "before" side only so each node shows once.
    let before_ids: std::collections::HashSet<Uuid> = before.iter().map(|n| n.series_id).collect();
    let mut out: Vec<ChainNode> = before
        .iter()
        .map(|n| ChainNode {
            series_id: n.series_id,
            position: -n.depth,
            parent_id: Some(n.parent_id),
        })
        .collect();
    // Furthest-back first (stable, so ties keep id order).
    out.sort_by_key(|n| n.position);
    out.push(ChainNode {
        series_id: start,
        position: 0,
        parent_id: None,
    });
    out.extend(
        after
            .iter()
            .filter(|n| !before_ids.contains(&n.series_id))
            .map(|n| ChainNode {
                series_id: n.series_id,
                position: n.depth,
                parent_id: Some(n.parent_id),
            }),
    );
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use RelationshipKind as K;

    #[test]
    fn inverse_is_an_involution() {
        for k in K::ALL {
            assert_eq!(k.inverse().inverse(), k, "{k}");
            assert_eq!(k.inverse().group(), k.group(), "{k}: pair shares a group");
        }
    }

    /// The full inverse table (WP-7.5). `prequel_of` is a narrative
    /// prequel with its own inverse, not the inverse of `sequel_of`.
    #[test]
    fn inverse_table() {
        let table = [
            (K::SequelOf, K::HasSequel),
            (K::PrequelOf, K::HasPrequel),
            (K::SpinOffOf, K::HasSpinOff),
            (K::SideStoryOf, K::HasSideStory),
            (K::TieInTo, K::HasTieIn),
            (K::Continues, K::ContinuedBy),
            (K::AnnualOf, K::HasAnnual),
            (K::SupplementTo, K::HasSupplement),
            (K::Collects, K::CollectedIn),
            (K::Reprints, K::ReprintedIn),
            (K::TranslationOf, K::HasTranslation),
            (K::AdaptationOf, K::AdaptedAs),
            (K::ReimaginingOf, K::ReimaginedAs),
        ];
        let selfies = [
            K::CrossoverWith,
            K::CompanionTo,
            K::SameUniverse,
            K::SeeAlso,
            K::AlternateEditionOf,
        ];
        let mut covered = std::collections::HashSet::new();
        for (a, b) in table {
            assert_eq!(a.inverse(), b, "{a}");
            assert_eq!(b.inverse(), a, "{b}");
            assert!(a.is_canonical() && !b.is_canonical(), "{a}/{b}");
            covered.insert(a);
            covered.insert(b);
        }
        for k in selfies {
            assert_eq!(k.inverse(), k, "{k}");
            assert!(k.is_canonical());
            covered.insert(k);
        }
        assert_eq!(covered.len(), K::ALL.len(), "every kind is in the table");
        assert_ne!(K::SequelOf.inverse(), K::PrequelOf);
    }

    #[test]
    fn groups_and_labels() {
        use RelationshipGroup as G;
        assert_eq!(K::SequelOf.group(), G::Story);
        assert_eq!(K::Continues.group(), G::Publication);
        assert_eq!(K::Reprints.group(), G::Editions);
        assert_eq!(K::ReimaginedAs.group(), G::Advanced);
        // Catalogue order is grouped (one contiguous run per group).
        let order: Vec<G> = K::ALL.iter().map(|k| k.group()).collect();
        let mut runs = order.clone();
        runs.dedup();
        assert_eq!(runs, G::ALL.to_vec());
        for k in K::ALL {
            assert!(!k.label().is_empty());
        }
        assert_eq!(
            K::TieInTo.display_label(Some(RelationshipQualifier::Prelude)),
            "Prelude to"
        );
        assert_eq!(
            K::HasTieIn.display_label(Some(RelationshipQualifier::Aftermath)),
            "Has aftermath"
        );
        assert_eq!(K::TieInTo.display_label(None), "Tie-in to");
        assert_eq!(
            K::Continues.display_label(Some(RelationshipQualifier::Relaunch)),
            "Continues"
        );
    }

    #[test]
    fn scope_rules() {
        use RelationshipCoverage as Cov;
        use RelationshipQualifier as Q;
        let ok = Scope {
            from_range: Some("1-6,Annual 1".into()),
            to_range: Some("1-6".into()),
            coverage: Some(Cov::Partial),
            qualifier: None,
            note: Some("TPB vol. 1".into()),
        };
        assert!(ok.validate(K::Collects).is_empty());
        assert!(ok.validate(K::ReprintedIn).is_empty());
        let fields =
            |s: &Scope, k| -> Vec<&str> { s.validate(k).iter().map(|i| i.field).collect() };
        assert_eq!(fields(&ok, K::SequelOf), vec!["coverage"]);
        let q = Scope {
            qualifier: Some(Q::Relaunch),
            ..Default::default()
        };
        assert!(q.validate(K::Continues).is_empty());
        assert!(q.validate(K::ContinuedBy).is_empty());
        assert_eq!(fields(&q, K::TieInTo), vec!["qualifier"]);
        assert_eq!(fields(&q, K::SeeAlso), vec!["qualifier"]);
        let role = Scope {
            qualifier: Some(Q::Prelude),
            ..Default::default()
        };
        assert!(role.validate(K::TieInTo).is_empty());
        assert_eq!(fields(&role, K::Continues), vec!["qualifier"]);
        let long = Scope {
            from_range: Some("1".repeat(101)),
            note: Some("x".repeat(501)),
            ..Default::default()
        };
        assert_eq!(fields(&long, K::SeeAlso), vec!["from_range", "note"]);
        // Mirroring swaps ranges, keeps the rest.
        let m = ok.mirrored();
        assert_eq!(m.from_range.as_deref(), Some("1-6"));
        assert_eq!(m.to_range.as_deref(), Some("1-6,Annual 1"));
        assert_eq!(m.coverage, ok.coverage);
        assert_eq!(m.mirrored(), ok);
        // Blank text normalizes away.
        let blank = Scope {
            note: Some("   ".into()),
            from_range: Some(" 1-3 ".into()),
            ..Default::default()
        }
        .normalized();
        assert_eq!(blank.note, None);
        assert_eq!(blank.from_range.as_deref(), Some("1-3"));
    }

    #[test]
    fn contradictions_cover_every_directional_kind() {
        for k in K::ALL {
            let c = k.contradictions();
            if k.is_self_inverse() {
                assert!(c.is_empty(), "{k}");
            } else {
                assert!(c.contains(&k.inverse()), "{k}");
                assert!(!c.contains(&k), "{k}");
            }
        }
        assert!(K::Continues.contradictions().contains(&K::HasSequel));
        assert!(K::HasSequel.contradictions().contains(&K::Continues));
    }

    #[test]
    fn arc_targets() {
        let arc: Vec<_> = K::ALL
            .into_iter()
            .filter(|k| k.allows_arc_target())
            .collect();
        assert_eq!(arc, vec![K::TieInTo]);
    }

    #[test]
    fn str_round_trip_matches_serde() {
        for k in K::ALL {
            assert_eq!(k.as_str().parse::<K>(), Ok(k));
            assert_eq!(
                serde_json::to_value(k).unwrap(),
                serde_json::Value::from(k.as_str())
            );
        }
        for q in RelationshipQualifier::ALL {
            assert_eq!(q.as_str().parse::<RelationshipQualifier>(), Ok(q));
            assert_eq!(
                serde_json::to_value(q).unwrap(),
                serde_json::Value::from(q.as_str())
            );
        }
        assert!("sequel".parse::<K>().is_err());
    }
}
