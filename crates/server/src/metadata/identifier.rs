//! `Identifier` + `Source` — single representation for an external
//! identifier across every entity type. The
//! `external_ids.{source,external_id}` columns are TEXT; this module
//! is the only place that produces the canonical string forms.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Every metadata source Folio knows about, today and tomorrow.
///
/// Adding a new source is two edits: extend this enum + add the
/// `(source, entity_type) -> URL template` branches in
/// [`canonical_url`]. Everything downstream (writer helpers, the
/// `<ExternalIdsCard>` payload, the provider trait impls in M1+)
/// reads from this enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    ComicVine,
    Metron,
    Gcd,
    Marvel,
    Locg,
    Mal,
    AniList,
    MangaUpdates,
    Isbn,
    Upc,
    Asin,
    Doi,
    /// Generic Global Trade Item Number — used by the legacy `gtin`
    /// column when the specific scheme (ISBN-13 vs UPC vs EAN-13)
    /// isn't recorded. New writes should prefer [`Source::Isbn`] or
    /// [`Source::Upc`] when the format is known.
    Gtin,
}

impl Source {
    /// Snake-case string stored in `external_ids.source`. Stable —
    /// changing this requires a migration of every row.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::ComicVine => "comicvine",
            Source::Metron => "metron",
            Source::Gcd => "gcd",
            Source::Marvel => "marvel",
            Source::Locg => "locg",
            Source::Mal => "mal",
            Source::AniList => "anilist",
            Source::MangaUpdates => "mangaupdates",
            Source::Isbn => "isbn",
            Source::Upc => "upc",
            Source::Asin => "asin",
            Source::Doi => "doi",
            Source::Gtin => "gtin",
        }
    }

    /// Human-readable label for UI rendering.
    pub fn label(self) -> &'static str {
        match self {
            Source::ComicVine => "ComicVine",
            Source::Metron => "Metron",
            Source::Gcd => "Grand Comics Database",
            Source::Marvel => "Marvel",
            Source::Locg => "League of Comic Geeks",
            Source::Mal => "MyAnimeList",
            Source::AniList => "AniList",
            Source::MangaUpdates => "MangaUpdates",
            Source::Isbn => "ISBN",
            Source::Upc => "UPC",
            Source::Asin => "ASIN",
            Source::Doi => "DOI",
            Source::Gtin => "GTIN",
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct UnknownSource(pub String);

impl fmt::Display for UnknownSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown source: {}", self.0)
    }
}

impl std::error::Error for UnknownSource {}

impl FromStr for Source {
    type Err = UnknownSource;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept canonical lowercase + a small set of common aliases
        // so folder-tag parsing (scanner M8) can be permissive.
        let normalized = s.trim().to_ascii_lowercase();
        Ok(match normalized.as_str() {
            "comicvine" | "cv" => Source::ComicVine,
            "metron" => Source::Metron,
            "gcd" => Source::Gcd,
            "marvel" => Source::Marvel,
            "locg" => Source::Locg,
            "mal" | "myanimelist" => Source::Mal,
            "anilist" => Source::AniList,
            "mangaupdates" | "mu" => Source::MangaUpdates,
            "isbn" => Source::Isbn,
            "upc" => Source::Upc,
            "asin" => Source::Asin,
            "doi" => Source::Doi,
            "gtin" => Source::Gtin,
            _ => return Err(UnknownSource(s.to_owned())),
        })
    }
}

/// One external identifier — the unit every writer / reader / API
/// payload speaks. `url` is filled by [`canonical_url`] when the
/// (source, entity_type) pair has a known template; callers can also
/// pass a provider-supplied URL through unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifier {
    pub source: Source,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Identifier {
    pub fn new(source: Source, id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            source,
            id,
            url: None,
        }
    }

    /// Build an identifier with the URL auto-filled from the
    /// canonical template for `entity_type`. Returns the identifier
    /// even when no template exists (URL stays `None` in that case).
    pub fn with_canonical_url(source: Source, id: impl Into<String>, entity_type: &str) -> Self {
        let id = id.into();
        let url = canonical_url(source, entity_type, &id);
        Self { source, id, url }
    }
}

/// Canonical link back to the source's page for `(source, entity_type,
/// external_id)`. Used to satisfy the CV/Metron TOS attribution
/// requirement on every UI surface that renders provider data.
///
/// Returns `None` for combinations where there's no stable URL shape
/// (ISBN, UPC, ASIN, DOI — the id *is* the link, with no host) or
/// where the source doesn't expose a public detail page for that
/// entity type.
pub fn canonical_url(source: Source, entity_type: &str, external_id: &str) -> Option<String> {
    match (source, entity_type) {
        // ComicVine — series is a "volume" (4050 prefix), issue is 4000.
        (Source::ComicVine, "series") => Some(format!(
            "https://comicvine.gamespot.com/volume/4050-{external_id}/"
        )),
        (Source::ComicVine, "issue") => Some(format!(
            "https://comicvine.gamespot.com/issue/4000-{external_id}/"
        )),
        (Source::ComicVine, "publisher") => Some(format!(
            "https://comicvine.gamespot.com/publisher/4010-{external_id}/"
        )),
        (Source::ComicVine, "person") => Some(format!(
            "https://comicvine.gamespot.com/person/4040-{external_id}/"
        )),
        (Source::ComicVine, "character") => Some(format!(
            "https://comicvine.gamespot.com/character/4005-{external_id}/"
        )),
        (Source::ComicVine, "team") => Some(format!(
            "https://comicvine.gamespot.com/team/4060-{external_id}/"
        )),
        (Source::ComicVine, "story_arc") => Some(format!(
            "https://comicvine.gamespot.com/story-arc/4045-{external_id}/"
        )),
        (Source::ComicVine, "location") => Some(format!(
            "https://comicvine.gamespot.com/location/4020-{external_id}/"
        )),
        (Source::ComicVine, "concept") => Some(format!(
            "https://comicvine.gamespot.com/concept/4015-{external_id}/"
        )),
        (Source::ComicVine, "object") => Some(format!(
            "https://comicvine.gamespot.com/object/4055-{external_id}/"
        )),

        // Metron — entity_type maps 1:1 to path segment.
        (Source::Metron, "series") => Some(format!("https://metron.cloud/series/{external_id}/")),
        (Source::Metron, "issue") => Some(format!("https://metron.cloud/issue/{external_id}/")),
        (Source::Metron, "publisher") => {
            Some(format!("https://metron.cloud/publisher/{external_id}/"))
        }
        (Source::Metron, "imprint") => Some(format!("https://metron.cloud/imprint/{external_id}/")),
        (Source::Metron, "person") => Some(format!("https://metron.cloud/creator/{external_id}/")),
        (Source::Metron, "character") => {
            Some(format!("https://metron.cloud/character/{external_id}/"))
        }
        (Source::Metron, "team") => Some(format!("https://metron.cloud/team/{external_id}/")),
        (Source::Metron, "story_arc") => Some(format!("https://metron.cloud/arc/{external_id}/")),
        (Source::Metron, "universe") => {
            Some(format!("https://metron.cloud/universe/{external_id}/"))
        }

        // GCD — series + issue have stable detail pages.
        (Source::Gcd, "series") => Some(format!("https://www.comics.org/series/{external_id}/")),
        (Source::Gcd, "issue") => Some(format!("https://www.comics.org/issue/{external_id}/")),
        (Source::Gcd, "publisher") => {
            Some(format!("https://www.comics.org/publisher/{external_id}/"))
        }

        // Marvel — public site uses slug-shaped URLs but the numeric
        // id endpoint redirects. Only series + issue are stable.
        (Source::Marvel, "series") => Some(format!(
            "https://www.marvel.com/comics/series/{external_id}"
        )),
        (Source::Marvel, "issue") => {
            Some(format!("https://www.marvel.com/comics/issue/{external_id}"))
        }

        // League of Comic Geeks — single-prefix issue URL.
        (Source::Locg, "issue") => Some(format!(
            "https://leagueofcomicgeeks.com/comic/{external_id}/"
        )),
        (Source::Locg, "series") => Some(format!(
            "https://leagueofcomicgeeks.com/comics/series/{external_id}/"
        )),

        // Manga sources (series only, no individual chapter pages).
        (Source::Mal, "series") => Some(format!("https://myanimelist.net/manga/{external_id}/")),
        (Source::AniList, "series") => Some(format!("https://anilist.co/manga/{external_id}/")),
        (Source::MangaUpdates, "series") => Some(format!(
            "https://www.mangaupdates.com/series/{external_id}/"
        )),

        // Barcodes / catalog identifiers — searchable but no canonical
        // first-party URL. The UI renders the bare id with a copy
        // button instead of a link.
        (Source::Isbn, _)
        | (Source::Upc, _)
        | (Source::Asin, _)
        | (Source::Doi, _)
        | (Source::Gtin, _) => None,

        // Unknown combinations — caller can still create an
        // Identifier; the URL just stays None.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_round_trip() {
        for s in [
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
            Source::Gtin,
        ] {
            assert_eq!(s.as_str().parse::<Source>().unwrap(), s);
        }
    }

    #[test]
    fn source_accepts_aliases() {
        assert_eq!("cv".parse::<Source>().unwrap(), Source::ComicVine);
        assert_eq!("CV".parse::<Source>().unwrap(), Source::ComicVine);
        assert_eq!(" myanimelist ".parse::<Source>().unwrap(), Source::Mal);
        assert_eq!("mu".parse::<Source>().unwrap(), Source::MangaUpdates);
    }

    #[test]
    fn source_rejects_unknown() {
        assert!("invinciblevine".parse::<Source>().is_err());
    }

    #[test]
    fn canonical_url_covers_critical_pairs() {
        assert_eq!(
            canonical_url(Source::ComicVine, "issue", "123"),
            Some("https://comicvine.gamespot.com/issue/4000-123/".into())
        );
        assert_eq!(
            canonical_url(Source::Metron, "series", "456"),
            Some("https://metron.cloud/series/456/".into())
        );
        assert_eq!(
            canonical_url(Source::Gcd, "issue", "789"),
            Some("https://www.comics.org/issue/789/".into())
        );
    }

    #[test]
    fn canonical_url_returns_none_for_barcodes() {
        assert_eq!(canonical_url(Source::Isbn, "issue", "9780123456789"), None);
        assert_eq!(canonical_url(Source::Upc, "issue", "075678164002"), None);
    }

    #[test]
    fn identifier_with_canonical_url_fills_url_when_template_known() {
        let id = Identifier::with_canonical_url(Source::Metron, "12345", "issue");
        assert_eq!(id.source, Source::Metron);
        assert_eq!(id.id, "12345");
        assert_eq!(id.url.as_deref(), Some("https://metron.cloud/issue/12345/"));
    }

    #[test]
    fn identifier_with_canonical_url_leaves_url_none_when_template_unknown() {
        let id = Identifier::with_canonical_url(Source::Isbn, "9780123456789", "issue");
        assert!(id.url.is_none());
    }
}
