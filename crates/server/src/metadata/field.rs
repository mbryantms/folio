//! `MetadataField` — the typed key behind `field_provenance.field`.
//! Apply jobs iterate [`MetadataField::iter`] to build per-field
//! diffs, and writer helpers take `MetadataField` (not strings) so
//! the set of legal keys is closed at the type level.
//!
//! Adding a new field is one match-arm + one entry in the
//! [`SCALAR_FIELDS`] constant. Forgetting the constant means the
//! field never appears in `iter()` and is silently invisible to
//! Apply jobs — a unit test guards that.

use super::identifier::Source;
use std::fmt;
use std::str::FromStr;

/// Every field a provider can populate (or a user can edit) that
/// participates in the field-provenance / Apply-job machinery.
///
/// The variant ordering deliberately groups: flat scalars first,
/// then junction sets, covers, then the parameterised
/// [`MetadataField::ExternalId`] family. Apply-job code paths
/// can treat the groups uniformly via the helper predicates
/// below.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MetadataField {
    // ── Flat scalar fields (shared across series + issue where applicable) ──
    Title,
    SortName,
    SeriesType,
    YearBegan,
    YearEnd,
    Volume,
    Deck,
    Description,
    Summary,
    Notes,
    ScanInformation,
    CoverDate,
    StoreDate,
    FocDate,
    PageCount,
    AgeRating,
    Format,
    LanguageCode,
    Manga,
    Price,
    Sku,
    CommunityRating,
    StaffRating,
    Aliases,
    Status,
    Publisher,
    Imprint,

    // ── Junction sets (one provenance row covers the whole set) ──
    Credits,
    Characters,
    Teams,
    Locations,
    Concepts,
    Objects,
    StoryArcs,
    Universes,
    Genres,
    Tags,
    Reprints,

    // ── Covers ──
    CoverPrimary,
    CoverVariants,

    // ── External IDs (one provenance row per source) ──
    ExternalId(Source),
}

/// Every scalar / junction / cover variant. The parameterised
/// [`MetadataField::ExternalId`] family is appended in
/// [`MetadataField::iter`] by enumerating [`Source`].
///
/// Keeping this as a `const` array means we can't accidentally
/// drift it from the enum — a unit test enumerates the const and
/// asserts its length matches the enum's non-parameterised arm
/// count.
const SCALAR_FIELDS: &[MetadataField] = &[
    MetadataField::Title,
    MetadataField::SortName,
    MetadataField::SeriesType,
    MetadataField::YearBegan,
    MetadataField::YearEnd,
    MetadataField::Volume,
    MetadataField::Deck,
    MetadataField::Description,
    MetadataField::Summary,
    MetadataField::Notes,
    MetadataField::ScanInformation,
    MetadataField::CoverDate,
    MetadataField::StoreDate,
    MetadataField::FocDate,
    MetadataField::PageCount,
    MetadataField::AgeRating,
    MetadataField::Format,
    MetadataField::LanguageCode,
    MetadataField::Manga,
    MetadataField::Price,
    MetadataField::Sku,
    MetadataField::CommunityRating,
    MetadataField::StaffRating,
    MetadataField::Aliases,
    MetadataField::Status,
    MetadataField::Publisher,
    MetadataField::Imprint,
    MetadataField::Credits,
    MetadataField::Characters,
    MetadataField::Teams,
    MetadataField::Locations,
    MetadataField::Concepts,
    MetadataField::Objects,
    MetadataField::StoryArcs,
    MetadataField::Universes,
    MetadataField::Genres,
    MetadataField::Tags,
    MetadataField::Reprints,
    MetadataField::CoverPrimary,
    MetadataField::CoverVariants,
];

const ALL_SOURCES: &[Source] = &[
    Source::ComicVine,
    Source::Metron,
    Source::Gcd,
    Source::Marvel,
    Source::Locg,
    Source::Mal,
    Source::AniList,
    Source::MangaUpdates,
    Source::Isbn,
    Source::Upc,
    Source::Asin,
    Source::Doi,
];

impl MetadataField {
    /// Canonical string stored in `field_provenance.field`. Stable —
    /// changing this requires migrating every row.
    pub fn key(&self) -> String {
        match self {
            MetadataField::Title => "title".into(),
            MetadataField::SortName => "sort_name".into(),
            MetadataField::SeriesType => "series_type".into(),
            MetadataField::YearBegan => "year_began".into(),
            MetadataField::YearEnd => "year_end".into(),
            MetadataField::Volume => "volume".into(),
            MetadataField::Deck => "deck".into(),
            MetadataField::Description => "description".into(),
            MetadataField::Summary => "summary".into(),
            MetadataField::Notes => "notes".into(),
            MetadataField::ScanInformation => "scan_information".into(),
            MetadataField::CoverDate => "cover_date".into(),
            MetadataField::StoreDate => "store_date".into(),
            MetadataField::FocDate => "foc_date".into(),
            MetadataField::PageCount => "page_count".into(),
            MetadataField::AgeRating => "age_rating".into(),
            MetadataField::Format => "format".into(),
            MetadataField::LanguageCode => "language_code".into(),
            MetadataField::Manga => "manga".into(),
            MetadataField::Price => "price".into(),
            MetadataField::Sku => "sku".into(),
            MetadataField::CommunityRating => "community_rating".into(),
            MetadataField::StaffRating => "staff_rating".into(),
            MetadataField::Aliases => "aliases".into(),
            MetadataField::Status => "status".into(),
            MetadataField::Publisher => "publisher".into(),
            MetadataField::Imprint => "imprint".into(),
            MetadataField::Credits => "credits".into(),
            MetadataField::Characters => "characters".into(),
            MetadataField::Teams => "teams".into(),
            MetadataField::Locations => "locations".into(),
            MetadataField::Concepts => "concepts".into(),
            MetadataField::Objects => "objects".into(),
            MetadataField::StoryArcs => "story_arcs".into(),
            MetadataField::Universes => "universes".into(),
            MetadataField::Genres => "genres".into(),
            MetadataField::Tags => "tags".into(),
            MetadataField::Reprints => "reprints".into(),
            MetadataField::CoverPrimary => "cover.primary".into(),
            MetadataField::CoverVariants => "cover.variants".into(),
            MetadataField::ExternalId(s) => format!("external_id.{}", s.as_str()),
        }
    }

    /// Iterate every legal [`MetadataField`] — scalar variants
    /// followed by one [`MetadataField::ExternalId`] per [`Source`].
    pub fn iter() -> impl Iterator<Item = MetadataField> {
        SCALAR_FIELDS
            .iter()
            .copied()
            .chain(ALL_SOURCES.iter().copied().map(MetadataField::ExternalId))
    }

    /// True for fields whose write also touches a junction table
    /// (caller must also call the matching `set_issue_*` /
    /// `set_series_*` helper). Apply jobs short-circuit the "flat
    /// column update" path for these.
    pub fn is_junction(&self) -> bool {
        matches!(
            self,
            MetadataField::Credits
                | MetadataField::Characters
                | MetadataField::Teams
                | MetadataField::Locations
                | MetadataField::Concepts
                | MetadataField::Objects
                | MetadataField::StoryArcs
                | MetadataField::Universes
                | MetadataField::Genres
                | MetadataField::Tags
                | MetadataField::Reprints
        )
    }

    /// True for cover-shaped fields (`apply_cover` handles them).
    pub fn is_cover(&self) -> bool {
        matches!(
            self,
            MetadataField::CoverPrimary | MetadataField::CoverVariants
        )
    }
}

impl fmt::Display for MetadataField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

#[derive(Debug, Clone)]
pub struct UnknownField(pub String);

impl fmt::Display for UnknownField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown metadata field: {}", self.0)
    }
}

impl std::error::Error for UnknownField {}

impl FromStr for MetadataField {
    type Err = UnknownField;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix("external_id.") {
            return rest
                .parse::<Source>()
                .map(MetadataField::ExternalId)
                .map_err(|_| UnknownField(s.to_owned()));
        }
        for f in SCALAR_FIELDS {
            if f.key() == s {
                return Ok(*f);
            }
        }
        Err(UnknownField(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn key_round_trip_for_every_variant() {
        for f in MetadataField::iter() {
            let key = f.key();
            let parsed: MetadataField = key.parse().unwrap();
            assert_eq!(f, parsed, "round trip failed for {key}");
        }
    }

    #[test]
    fn iter_produces_unique_keys() {
        let all: Vec<MetadataField> = MetadataField::iter().collect();
        let unique: HashSet<String> = all.iter().map(MetadataField::key).collect();
        assert_eq!(all.len(), unique.len(), "duplicate keys in iter()");
        // Sanity: 39 scalar + 12 external_id sources = 51 keys today.
        assert_eq!(all.len(), SCALAR_FIELDS.len() + ALL_SOURCES.len());
    }

    #[test]
    fn external_id_key_uses_source_string() {
        assert_eq!(
            MetadataField::ExternalId(Source::ComicVine).key(),
            "external_id.comicvine"
        );
        assert_eq!(
            MetadataField::ExternalId(Source::Metron).key(),
            "external_id.metron"
        );
    }

    #[test]
    fn unknown_field_rejected() {
        assert!("not_a_field".parse::<MetadataField>().is_err());
        assert!("external_id.banana".parse::<MetadataField>().is_err());
    }

    #[test]
    fn is_junction_covers_expected_set() {
        assert!(MetadataField::Credits.is_junction());
        assert!(MetadataField::Characters.is_junction());
        assert!(MetadataField::Reprints.is_junction());
        assert!(!MetadataField::Title.is_junction());
        assert!(!MetadataField::CoverPrimary.is_junction());
        assert!(!MetadataField::ExternalId(Source::Metron).is_junction());
    }

    #[test]
    fn is_cover_covers_expected_set() {
        assert!(MetadataField::CoverPrimary.is_cover());
        assert!(MetadataField::CoverVariants.is_cover());
        assert!(!MetadataField::Credits.is_cover());
        assert!(!MetadataField::Title.is_cover());
    }
}
