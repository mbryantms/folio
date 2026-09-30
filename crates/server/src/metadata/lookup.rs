//! Search-by-provider-URL / explicit-id lookup (roadmap WP-2.8).
//!
//! The matcher's normal path is *search → score → rank*. When the
//! operator already knows the exact ComicVine volume / issue or Metron
//! series / issue they want, this module skips scoring entirely: it
//! parses a pasted provider URL (or an explicit `{source, external_id}`
//! pair) into a [`ProviderRef`], and the API layer fetches the detail
//! record through the provider's cache + rate bucket, then persists a
//! completed `metadata_run` with exactly one HIGH candidate so the
//! ordinary preview-diff + apply path works unchanged.
//!
//! Only the parse + candidate-shaping logic lives here (pure, unit
//! tested); the run persistence is
//! [`crate::metadata::orchestrator::finalize_lookup_run`] and the HTTP
//! surface is `api::metadata_search::{lookup_series, lookup_issue}`.
//!
//! **Host validation is strict.** SSRF is moot (the provider client
//! only ever speaks to its fixed base URL — the parsed id is the only
//! thing that leaves this module), but a URL for an unknown host is
//! still rejected rather than guessed at, so a pasted GCD / Marvel link
//! surfaces a clear "not a supported provider" instead of a confusing
//! ComicVine 404.

use crate::metadata::identifier::{Source, canonical_url};
use crate::metadata::provider::{GenericMetadata, IssueCandidate, SeriesCandidate};
use std::fmt;

/// Which kind of provider record a lookup targets. Mirrors the two
/// `metadata_run.scope` values a lookup can produce.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LookupEntity {
    Series,
    Issue,
}

impl LookupEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            LookupEntity::Series => "series",
            LookupEntity::Issue => "issue",
        }
    }
}

/// A fully-resolved provider record reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRef {
    pub source: Source,
    pub entity: LookupEntity,
    /// Provider-native id, **without** ComicVine's `4050-` / `4000-`
    /// type prefix (the clients add it back when building the request
    /// path, and `external_ids.external_id` stores it bare).
    pub external_id: String,
}

/// Why a lookup input couldn't be turned into a [`ProviderRef`]. Every
/// variant maps to a 422 at the API layer; `field()` names the request
/// field the message binds to so the dialog can render it inline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupError {
    /// Neither `url` nor `{source, external_id}` were supplied.
    MissingInput,
    /// `url` isn't an absolute http(s) URL.
    MalformedUrl,
    /// Host isn't one of the providers this build knows how to parse.
    UnsupportedHost(String),
    /// Host is a known provider but the path carries no recognisable
    /// series / issue id.
    NoIdInPath(Source),
    /// Metron page URLs use slugs (`/series/saga-2012/`); the API needs
    /// the numeric id. Carries the slug so the message can echo it.
    MetronSlug(String),
    /// `source` isn't one of the searchable providers.
    UnknownSource(String),
    /// `external_id` is empty or not a bare numeric id.
    BadExternalId,
    /// URL / id was for a series when an issue was requested, or vice
    /// versa. `(wanted, got)`.
    EntityMismatch(LookupEntity, LookupEntity),
}

impl LookupError {
    /// Request field the error binds to (`url` / `source` / `external_id`).
    pub fn field(&self) -> &'static str {
        match self {
            LookupError::MissingInput
            | LookupError::MalformedUrl
            | LookupError::UnsupportedHost(_)
            | LookupError::NoIdInPath(_)
            | LookupError::MetronSlug(_) => "url",
            LookupError::UnknownSource(_) => "source",
            LookupError::BadExternalId => "external_id",
            LookupError::EntityMismatch(..) => "url",
        }
    }
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::MissingInput => {
                write!(
                    f,
                    "paste a provider URL or give both source and external_id"
                )
            }
            LookupError::MalformedUrl => write!(f, "not an absolute http(s) URL"),
            LookupError::UnsupportedHost(h) => write!(
                f,
                "{h} is not a supported provider host (expected comicvine.gamespot.com or metron.cloud)"
            ),
            LookupError::NoIdInPath(s) => write!(
                f,
                "no series or issue id found in the {} URL path",
                s.as_str()
            ),
            LookupError::MetronSlug(slug) => write!(
                f,
                "Metron page URLs use a slug ({slug}); paste the numeric id from the API URL (metron.cloud/api/series/<id>/) or enter source + external_id"
            ),
            LookupError::UnknownSource(s) => {
                write!(f, "unknown source {s:?} (expected comicvine or metron)")
            }
            LookupError::BadExternalId => write!(f, "external_id must be a numeric provider id"),
            LookupError::EntityMismatch(wanted, got) => write!(
                f,
                "that is a {} link; this dialog looks up a {}",
                got.as_str(),
                wanted.as_str()
            ),
        }
    }
}

impl std::error::Error for LookupError {}

/// Providers a lookup can target. GCD / Marvel / … carry ids in
/// `external_ids` but have no `MetadataProvider` client yet.
fn lookup_source(s: &str) -> Result<Source, LookupError> {
    match s.parse::<Source>() {
        Ok(src @ (Source::ComicVine | Source::Metron)) => Ok(src),
        _ => Err(LookupError::UnknownSource(s.trim().to_owned())),
    }
}

/// Strip an optional ComicVine type prefix (`4050-123` → `123`) and
/// require a bare numeric id — both providers use integer ids.
fn bare_numeric_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let bare = trimmed
        .strip_prefix("4050-")
        .or_else(|| trimmed.strip_prefix("4000-"))
        .unwrap_or(trimmed);
    (!bare.is_empty() && bare.bytes().all(|b| b.is_ascii_digit())).then(|| bare.to_owned())
}

/// Parse a ComicVine or Metron page/API URL into a [`ProviderRef`].
///
/// Accepted shapes:
/// - `https://comicvine.gamespot.com/<slug>/4050-<id>/` (volume) and
///   `…/4000-<id>/` (issue). The slug segment is optional — the
///   canonical `/volume/4050-<id>/` and `/issue/4000-<id>/` forms
///   emitted by [`canonical_url`] parse the same way.
/// - `https://metron.cloud/series/<id>/`, `…/issue/<id>/`, and the
///   API forms `…/api/series/<id>/` / `…/api/issue/<id>/`. Metron's
///   public site links use slugs, which the API can't resolve — those
///   return [`LookupError::MetronSlug`] with a hint.
pub fn parse_provider_url(raw: &str) -> Result<ProviderRef, LookupError> {
    let trimmed = raw.trim();
    // Tolerate a pasted `comicvine.gamespot.com/...` without a scheme.
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| LookupError::MalformedUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(LookupError::MalformedUrl);
    }
    let host = parsed
        .host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
        .ok_or(LookupError::MalformedUrl)?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();

    match host.as_str() {
        "comicvine.gamespot.com" | "www.comicvine.gamespot.com" => {
            for seg in &segments {
                if let Some(id) = seg.strip_prefix("4050-")
                    && !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_digit())
                {
                    return Ok(ProviderRef {
                        source: Source::ComicVine,
                        entity: LookupEntity::Series,
                        external_id: id.to_owned(),
                    });
                }
                if let Some(id) = seg.strip_prefix("4000-")
                    && !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_digit())
                {
                    return Ok(ProviderRef {
                        source: Source::ComicVine,
                        entity: LookupEntity::Issue,
                        external_id: id.to_owned(),
                    });
                }
            }
            Err(LookupError::NoIdInPath(Source::ComicVine))
        }
        "metron.cloud" | "www.metron.cloud" => {
            // `/api/series/<id>/` and `/series/<id>/` both carry the
            // id right after the entity segment.
            let rest: &[&str] = match segments.first() {
                Some(&"api") => &segments[1..],
                _ => &segments[..],
            };
            let entity = match rest.first() {
                Some(&"series") => LookupEntity::Series,
                Some(&"issue") => LookupEntity::Issue,
                _ => return Err(LookupError::NoIdInPath(Source::Metron)),
            };
            let Some(id_seg) = rest.get(1) else {
                return Err(LookupError::NoIdInPath(Source::Metron));
            };
            if id_seg.bytes().all(|b| b.is_ascii_digit()) {
                Ok(ProviderRef {
                    source: Source::Metron,
                    entity,
                    external_id: (*id_seg).to_owned(),
                })
            } else {
                Err(LookupError::MetronSlug((*id_seg).to_owned()))
            }
        }
        other => Err(LookupError::UnsupportedHost(other.to_owned())),
    }
}

/// Resolve the lookup request body — a pasted `url` **or** an explicit
/// `{source, external_id}` pair — into a [`ProviderRef`] for `wanted`.
/// The URL wins when both are supplied. An explicit pair is trusted to
/// be the right entity kind (there is no URL to disagree with).
pub fn resolve(
    wanted: LookupEntity,
    url: Option<&str>,
    source: Option<&str>,
    external_id: Option<&str>,
) -> Result<ProviderRef, LookupError> {
    if let Some(u) = url.map(str::trim).filter(|u| !u.is_empty()) {
        let r = parse_provider_url(u)?;
        if r.entity != wanted {
            return Err(LookupError::EntityMismatch(wanted, r.entity));
        }
        return Ok(r);
    }
    match (source, external_id) {
        (Some(s), Some(id)) if !s.trim().is_empty() => {
            let source = lookup_source(s)?;
            let external_id = bare_numeric_id(id).ok_or(LookupError::BadExternalId)?;
            Ok(ProviderRef {
                source,
                entity: wanted,
                external_id,
            })
        }
        _ => Err(LookupError::MissingInput),
    }
}

/// Shape a fetched series detail record as the [`SeriesCandidate`] the
/// candidate row / dialog card expects. `name` falls back to the id so
/// the card never renders blank.
pub fn series_candidate_from_detail(
    source: Source,
    external_id: &str,
    detail: &GenericMetadata,
) -> SeriesCandidate {
    SeriesCandidate {
        source,
        external_id: external_id.to_owned(),
        external_url: detail
            .source_url
            .clone()
            .or_else(|| canonical_url(source, "series", external_id)),
        name: detail
            .series_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| format!("{} {external_id}", source.as_str())),
        year: detail.year_began,
        publisher: detail.publisher.clone(),
        issue_count: None,
        cover_image_url: detail.cover_image_url.clone(),
        deck: detail.deck.clone(),
        alternate_cover_urls: detail.cover_image_alt_urls.clone(),
        // WP-5.6: format hint for the matcher.
        format: detail.series_type.clone().or_else(|| detail.format.clone()),
    }
}

/// Issue-scope sibling of [`series_candidate_from_detail`].
pub fn issue_candidate_from_detail(
    source: Source,
    external_id: &str,
    detail: &GenericMetadata,
) -> IssueCandidate {
    IssueCandidate {
        source,
        external_id: external_id.to_owned(),
        external_url: detail
            .source_url
            .clone()
            .or_else(|| canonical_url(source, "issue", external_id)),
        issue_number: detail.issue_number.clone(),
        name: detail.title.clone(),
        cover_date: detail.cover_date,
        series_name: detail.series_name.clone(),
        series_year: detail.year_began,
        series_external_id: detail.series_external_id.clone(),
        cover_image_url: detail.cover_image_url.clone(),
        alternate_cover_urls: detail.cover_image_alt_urls.clone(),
        // WP-5.6: format hint for the matcher.
        format: detail.series_type.clone().or_else(|| detail.format.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(entity: LookupEntity, id: &str) -> ProviderRef {
        ProviderRef {
            source: Source::ComicVine,
            entity,
            external_id: id.into(),
        }
    }

    fn metron(entity: LookupEntity, id: &str) -> ProviderRef {
        ProviderRef {
            source: Source::Metron,
            entity,
            external_id: id.into(),
        }
    }

    #[test]
    fn parses_comicvine_volume_and_issue_urls() {
        assert_eq!(
            parse_provider_url("https://comicvine.gamespot.com/saga/4050-56100/").unwrap(),
            cv(LookupEntity::Series, "56100")
        );
        assert_eq!(
            parse_provider_url("https://comicvine.gamespot.com/volume/4050-56100/").unwrap(),
            cv(LookupEntity::Series, "56100")
        );
        assert_eq!(
            parse_provider_url("https://comicvine.gamespot.com/saga-1/4000-341012/").unwrap(),
            cv(LookupEntity::Issue, "341012")
        );
        // www. prefix + query string + no trailing slash all tolerated.
        assert_eq!(
            parse_provider_url("https://www.comicvine.gamespot.com/x/4000-7?ref=nav").unwrap(),
            cv(LookupEntity::Issue, "7")
        );
        // Scheme-less paste.
        assert_eq!(
            parse_provider_url("comicvine.gamespot.com/volume/4050-1/").unwrap(),
            cv(LookupEntity::Series, "1")
        );
    }

    #[test]
    fn parses_metron_numeric_page_and_api_urls() {
        assert_eq!(
            parse_provider_url("https://metron.cloud/series/1234/").unwrap(),
            metron(LookupEntity::Series, "1234")
        );
        assert_eq!(
            parse_provider_url("https://metron.cloud/api/series/1234/").unwrap(),
            metron(LookupEntity::Series, "1234")
        );
        assert_eq!(
            parse_provider_url("https://metron.cloud/api/issue/99/").unwrap(),
            metron(LookupEntity::Issue, "99")
        );
        assert_eq!(
            parse_provider_url("https://metron.cloud/issue/99").unwrap(),
            metron(LookupEntity::Issue, "99")
        );
    }

    #[test]
    fn metron_slug_urls_get_a_hint_not_a_guess() {
        assert_eq!(
            parse_provider_url("https://metron.cloud/series/saga-2012/"),
            Err(LookupError::MetronSlug("saga-2012".into()))
        );
    }

    #[test]
    fn rejects_unknown_hosts_and_junk() {
        assert_eq!(
            parse_provider_url("https://www.comics.org/series/12345/"),
            Err(LookupError::UnsupportedHost("www.comics.org".into()))
        );
        assert_eq!(
            parse_provider_url("https://comicvine.gamespot.com/saga/"),
            Err(LookupError::NoIdInPath(Source::ComicVine))
        );
        assert_eq!(
            parse_provider_url("https://metron.cloud/publisher/5/"),
            Err(LookupError::NoIdInPath(Source::Metron))
        );
        assert_eq!(
            parse_provider_url("ftp://comicvine.gamespot.com/volume/4050-1/"),
            Err(LookupError::MalformedUrl)
        );
        assert_eq!(
            parse_provider_url("not a url"),
            Err(LookupError::MalformedUrl)
        );
        // A CV-looking id on the wrong host is still rejected — the host
        // is authoritative, the id shape is not.
        assert_eq!(
            parse_provider_url("https://evil.example/volume/4050-1/"),
            Err(LookupError::UnsupportedHost("evil.example".into()))
        );
    }

    #[test]
    fn resolve_prefers_url_and_checks_entity_kind() {
        assert_eq!(
            resolve(
                LookupEntity::Series,
                Some("https://comicvine.gamespot.com/volume/4050-5/"),
                Some("metron"),
                Some("9"),
            )
            .unwrap(),
            cv(LookupEntity::Series, "5")
        );
        assert_eq!(
            resolve(
                LookupEntity::Series,
                Some("https://comicvine.gamespot.com/issue/4000-5/"),
                None,
                None,
            ),
            Err(LookupError::EntityMismatch(
                LookupEntity::Series,
                LookupEntity::Issue
            ))
        );
    }

    #[test]
    fn resolve_explicit_pair_strips_cv_prefix_and_validates() {
        assert_eq!(
            resolve(LookupEntity::Issue, None, Some("cv"), Some("4000-77")).unwrap(),
            cv(LookupEntity::Issue, "77")
        );
        assert_eq!(
            resolve(
                LookupEntity::Issue,
                Some("  "),
                Some("metron"),
                Some(" 12 ")
            )
            .unwrap(),
            metron(LookupEntity::Issue, "12")
        );
        assert_eq!(
            resolve(LookupEntity::Series, None, Some("gcd"), Some("1")),
            Err(LookupError::UnknownSource("gcd".into()))
        );
        assert_eq!(
            resolve(
                LookupEntity::Series,
                None,
                Some("metron"),
                Some("saga-2012")
            ),
            Err(LookupError::BadExternalId)
        );
        assert_eq!(
            resolve(LookupEntity::Series, None, None, None),
            Err(LookupError::MissingInput)
        );
        assert_eq!(
            resolve(LookupEntity::Series, None, Some("metron"), None),
            Err(LookupError::MissingInput)
        );
    }

    #[test]
    fn candidate_builders_fall_back_to_canonical_url_and_id_name() {
        let detail = GenericMetadata {
            year_began: Some(2012),
            publisher: Some("Image".into()),
            ..Default::default()
        };
        let c = series_candidate_from_detail(Source::ComicVine, "56100", &detail);
        assert_eq!(c.name, "comicvine 56100");
        assert_eq!(
            c.external_url.as_deref(),
            Some("https://comicvine.gamespot.com/volume/4050-56100/")
        );
        assert_eq!(c.year, Some(2012));

        let detail = GenericMetadata {
            issue_number: Some("1".into()),
            title: Some("Chapter One".into()),
            series_name: Some("Saga".into()),
            series_external_id: Some("56100".into()),
            source_url: Some("https://metron.cloud/issue/saga-2012-1/".into()),
            ..Default::default()
        };
        let i = issue_candidate_from_detail(Source::Metron, "9", &detail);
        assert_eq!(i.issue_number.as_deref(), Some("1"));
        assert_eq!(i.name.as_deref(), Some("Chapter One"));
        assert_eq!(i.series_external_id.as_deref(), Some("56100"));
        assert_eq!(
            i.external_url.as_deref(),
            Some("https://metron.cloud/issue/saga-2012-1/")
        );
    }
}
