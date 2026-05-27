//! Provider abstraction — the trait every metadata source impl
//! (ComicVine M1, Metron M2, future GCD/MAL/AniList) plugs into.
//!
//! The trait shape is intentionally narrow:
//! - **search** entrypoints (`search_series`, `search_issue`) take a
//!   query struct and return ranked [`SeriesCandidate`] / [`IssueCandidate`]
//!   lists — the matching engine in M3 fuses these across providers.
//! - **fetch** entrypoints (`fetch_series`, `fetch_issue`) return
//!   normalized [`GenericMetadata`] — Apply jobs in M4 consume only
//!   this shape and never see CV/Metron dialect.
//! - **fetch_cover** streams image bytes (caller decides where to
//!   write).
//! - **quota** is a snapshot of the provider's current Redis token
//!   bucket state for the admin dashboard gauges.
//!
//! All HTTP rate-limit gating + response caching is wrapped *inside*
//! each impl — callers don't think about quota.
//!
//! Errors are surfaced via [`ProviderError`] with a small fixed set of
//! variants so the orchestrator can react sensibly (back off on
//! `QuotaExceeded`, fail loud on `Unauthorized`, retry on `Transport`).

use crate::metadata::identifier::Source;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors a provider impl can return. Tuned for the orchestrator's
/// decision table — adding new variants is a breaking surface so think
/// twice before extending.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// HTTP 401/403 from upstream — credentials are wrong. The
    /// orchestrator surfaces the message in the admin UI and stops
    /// retrying.
    #[error("provider rejected credentials: {0}")]
    Unauthorized(String),
    /// HTTP 429 OR our Redis token-bucket denied the reservation. The
    /// `retry_after` hint is the seconds-until-budget-refill (best-
    /// effort; some upstreams don't surface it).
    #[error("quota exceeded; retry in {retry_after_secs}s")]
    QuotaExceeded { retry_after_secs: u64 },
    /// HTTP 404 / provider-specific "not found" status code.
    #[error("not found: {0}")]
    NotFound(String),
    /// Transport-layer failure (network, timeout, TLS) — caller may
    /// retry with backoff.
    #[error("transport error: {0}")]
    Transport(String),
    /// Provider returned a body we couldn't parse into the shape we
    /// expect. Indicates an upstream schema drift — alert-worthy.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    /// Catch-all for upstream 5xx + provider-specific error codes the
    /// orchestrator doesn't have a special path for. Caller may retry.
    #[error("provider error: {0}")]
    Upstream(String),
}

impl ProviderError {
    /// True when a retry has a reasonable chance of succeeding without
    /// operator intervention.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            ProviderError::QuotaExceeded { .. }
                | ProviderError::Transport(_)
                | ProviderError::Upstream(_)
        )
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

// ───────── query inputs ─────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeriesQuery {
    /// Free-text series name. Required.
    pub name: String,
    /// Cover year (CV "start_year", Metron "year_began"). Filters the
    /// candidate list at the provider when supported, ranks when not.
    pub year: Option<i32>,
    /// Optional publisher hint, used as a tie-breaker when the
    /// provider returns multiple matches with the same name.
    pub publisher: Option<String>,
    /// Limit candidate count (≤100). 25 is a sensible default — the
    /// matching engine rarely benefits from more.
    pub limit: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IssueQuery {
    /// Series-level external id, if known — narrows the search at the
    /// provider to a single volume. When `None`, we fall back to a
    /// name+number+year query.
    pub series_external_id: Option<String>,
    pub series_name: Option<String>,
    pub series_year: Option<i32>,
    /// Issue number — "1", "1.5", "½".
    pub issue_number: String,
    /// Cover year as a soft tie-breaker.
    pub cover_year: Option<i32>,
    pub limit: u32,
}

// ───────── candidate outputs ─────────

/// Lightweight summary returned by `search_*`. Detail fetched lazily
/// via `fetch_series` / `fetch_issue` once the user (or the matcher)
/// picks a candidate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeriesCandidate {
    pub source: Source,
    pub external_id: String,
    pub external_url: Option<String>,
    pub name: String,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub issue_count: Option<i32>,
    pub cover_image_url: Option<String>,
    pub deck: Option<String>,
    /// Variant-cover image URLs (matching-accuracy-1.0 M5). When a
    /// provider surfaces multiple covers per series (CV's
    /// `associated_images`, Metron's `images[]`), the orchestrator
    /// fetches + hashes each one and the matcher takes the **minimum**
    /// Hamming distance to the local cover. Stricter threshold applies
    /// when the winning cover is an alternate (see
    /// [`crate::metadata::matcher::MIN_ALTERNATE_SCORE_THRESH`]).
    /// Empty for search responses that don't carry variant URLs.
    #[serde(default)]
    pub alternate_cover_urls: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueCandidate {
    pub source: Source,
    pub external_id: String,
    pub external_url: Option<String>,
    pub issue_number: Option<String>,
    pub name: Option<String>,
    pub cover_date: Option<NaiveDate>,
    pub series_name: Option<String>,
    pub series_year: Option<i32>,
    pub series_external_id: Option<String>,
    pub cover_image_url: Option<String>,
    /// Variant-cover image URLs (matching-accuracy-1.0 M5). Mirrors
    /// the field on [`SeriesCandidate`] — the matcher takes the min
    /// Hamming distance against the local cover so a foil/variant
    /// candidate isn't penalized for differing from the local copy
    /// when one of its alternates matches.
    #[serde(default)]
    pub alternate_cover_urls: Vec<String>,
}

// ───────── normalized detail (read by M4 Apply jobs) ─────────

/// The shape every Apply job consumes. CV/Metron dialect dies at the
/// client boundary — anything that doesn't fit gets dropped (and
/// logged when novel).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GenericMetadata {
    // ── identity ────────────────────────────────────────────────
    pub series_name: Option<String>,
    pub series_sort_name: Option<String>,
    pub series_type: Option<String>,
    pub volume: Option<i32>,
    pub year_began: Option<i32>,
    pub year_end: Option<i32>,
    pub issue_number: Option<String>,
    pub aliases: Vec<String>,

    // ── hierarchy ───────────────────────────────────────────────
    pub publisher: Option<String>,
    pub imprint: Option<String>,

    // ── dates ───────────────────────────────────────────────────
    pub cover_date: Option<NaiveDate>,
    pub store_date: Option<NaiveDate>,
    pub foc_date: Option<NaiveDate>,

    // ── text ────────────────────────────────────────────────────
    pub title: Option<String>,
    pub deck: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub scan_information: Option<String>,

    // ── cross-cut entities (writer helpers upsert / dedup) ─────
    pub credits: Vec<CreditCandidate>,
    pub characters: Vec<EntityCandidate>,
    pub teams: Vec<EntityCandidate>,
    pub locations: Vec<EntityCandidate>,
    pub concepts: Vec<EntityCandidate>,
    pub objects: Vec<EntityCandidate>,
    pub story_arcs: Vec<EntityCandidate>,
    pub universes: Vec<EntityCandidate>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub reprints: Vec<ReprintCandidate>,
    pub variants: Vec<VariantCoverCandidate>,

    // ── cover ──────────────────────────────────────────────────
    pub cover_image_url: Option<String>,
    /// CV exposes a multi-size dict (icon / medium / screen / super /
    /// original) — we always pick the largest for `cover_image_url`
    /// and stash the alternates here for downstream sizing.
    pub cover_image_alt_urls: Vec<String>,

    // ── this entity's own identifiers ──────────────────────────
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,

    // ── misc ───────────────────────────────────────────────────
    pub age_rating: Option<String>,
    pub page_count: Option<i32>,
    pub community_rating: Option<f32>,
    pub staff_rating: Option<f32>,
    pub format: Option<String>,
    pub language_code: Option<String>,
    pub price: Option<f64>,
    pub sku: Option<String>,

    // ── provenance ─────────────────────────────────────────────
    pub source_provider: Option<Source>,
    pub source_external_id: Option<String>,
    pub source_url: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    /// CV `date_last_updated` / Metron `modified`. Lets a stale-cache
    /// check decide whether we need to re-pull.
    pub upstream_modified_at: Option<DateTime<Utc>>,
}

/// One credited person on an issue or series. `identifiers` carries
/// any cross-source IDs the provider gave us — `upsert_person` uses
/// them for identity-first dedup before falling back to normalized
/// name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditCandidate {
    pub name: String,
    pub role: String,
    pub ordinal: Option<i32>,
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,
}

/// Normalize a provider's raw role string to the canonical ComicInfo
/// role name. ComicVine's API returns roles like `"cover"`, `"penciler"`
/// (one L), `"editor in chief"` — none of which match the ComicInfo
/// spec's PascalCase `CoverArtist` / `Penciller` / `Editor` columns
/// the composer filters on. Without this normalization the composer's
/// `eq_ignore_ascii_case("CoverArtist")` silently drops every CV cover
/// credit and the rescan never lands those rows in the per-role CSV
/// cache, leaving the diff stuck at "16 → 18".
///
/// Returns the canonical name when the role maps cleanly; returns
/// `None` for roles ComicInfo can't represent (`"journalist"`,
/// `"other"`, `"production"`, …). MetronInfo's structured
/// `<Credit role="…">` element can still carry the original — that's
/// orthogonal to this mapping.
pub fn canonicalize_role(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Lower-cased + whitespace-collapsed key so " Cover Artist " and
    // "cover artist" hit the same arm.
    let key: String = trimmed
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match key.as_str() {
        // Writers
        "writer" | "writers" | "script" | "scripter" | "story" | "plotter" | "plot" => {
            Some("Writer")
        }
        // Pencillers (single + double L spellings; CV uses single L)
        "penciler" | "penciller" | "pencils" | "artist" | "art" => Some("Penciller"),
        // Inkers
        "inker" | "inkers" | "inks" => Some("Inker"),
        // Colorists (US + UK spellings)
        "colorist" | "colorists" | "colors" | "colourist" | "colours" => Some("Colorist"),
        // Letterers
        "letterer" | "letterers" | "letters" => Some("Letterer"),
        // Cover artists — CV's `"cover"` is the high-volume hit on
        // variant-cover-heavy issues like Walking Dead #1.
        "cover" | "covers" | "cover artist" | "coverartist" | "cover art" => Some("CoverArtist"),
        // Editors
        "editor" | "editors" | "editor in chief" | "executive editor" | "consulting editor"
        | "associate editor" | "assistant editor" | "senior editor" | "managing editor"
        | "group editor" => Some("Editor"),
        // Translators
        "translator" | "translators" | "translation" => Some("Translator"),
        // Roles with no ComicInfo column (journalist, production,
        // designer, photographer, …) — caller drops them from the
        // ComicInfo-shaped output; MetronInfo + the structured
        // junction still carry them.
        _ => None,
    }
}

/// One named non-credit entity (character / team / location /
/// concept / object / arc / universe). The provider may carry
/// per-relationship hints (first-appearance, died-in-issue).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityCandidate {
    pub name: String,
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,
    #[serde(default)]
    pub is_first_appearance: bool,
    /// Character-specific. None when the entity isn't a character.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub died_in_issue: Option<bool>,
    /// Team-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disbanded_in_issue: Option<bool>,
    /// Story-arc-specific reading position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_in_arc: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReprintCandidate {
    pub label: String,
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VariantCoverCandidate {
    pub label: Option<String>,
    pub artist_name: Option<String>,
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,
    pub image_url: Option<String>,
}

// ───────── quota gauge ─────────

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub provider: Source,
    pub remaining_hour: Option<u32>,
    pub remaining_day: Option<u32>,
    pub seconds_until_reset: Option<u64>,
}

// ───────── trait ─────────

#[async_trait]
pub trait MetadataProvider: Send + Sync + 'static {
    /// Stable identity used in audit / cache / settings keys.
    fn id(&self) -> Source;

    /// Inexpensive round-trip used by the admin "Test" button — just
    /// confirms credentials work + returns a usable quota snapshot.
    async fn health_check(&self) -> ProviderResult<QuotaSnapshot>;

    /// Snapshot the current token-bucket state (no I/O).
    async fn quota(&self) -> ProviderResult<QuotaSnapshot>;

    async fn search_series(&self, query: &SeriesQuery) -> ProviderResult<Vec<SeriesCandidate>>;

    async fn search_issue(&self, query: &IssueQuery) -> ProviderResult<Vec<IssueCandidate>>;

    async fn fetch_series(&self, external_id: &str) -> ProviderResult<GenericMetadata>;

    async fn fetch_issue(&self, external_id: &str) -> ProviderResult<GenericMetadata>;

    /// Streams cover bytes. Caller decides the on-disk path. Provider
    /// impls should re-use the per-provider HTTP client (kept alive
    /// for connection pooling) but bypass the rate-limit bucket — CDN
    /// hits don't count against the API budget.
    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_transience_classification() {
        assert!(
            ProviderError::QuotaExceeded {
                retry_after_secs: 30
            }
            .is_transient()
        );
        assert!(ProviderError::Transport("dns".into()).is_transient());
        assert!(ProviderError::Upstream("503".into()).is_transient());
        assert!(!ProviderError::Unauthorized("bad key".into()).is_transient());
        assert!(!ProviderError::NotFound("4000-123".into()).is_transient());
        assert!(!ProviderError::InvalidResponse("schema".into()).is_transient());
    }

    #[test]
    fn generic_metadata_default_is_empty() {
        let m = GenericMetadata::default();
        assert!(m.series_name.is_none());
        assert!(m.credits.is_empty());
        assert!(m.identifiers.is_empty());
    }

    #[test]
    fn canonicalize_role_maps_provider_idioms_to_comic_info_names() {
        // CV's high-volume case: `"cover"` → `"CoverArtist"`. Without
        // this, the dozen-cover-artists problem from Walking Dead #1
        // returns.
        assert_eq!(canonicalize_role("cover"), Some("CoverArtist"));
        assert_eq!(canonicalize_role("Cover"), Some("CoverArtist"));
        assert_eq!(canonicalize_role("cover artist"), Some("CoverArtist"));
        // CV's one-L `penciler` collides with ComicInfo's two-L
        // `Penciller`.
        assert_eq!(canonicalize_role("penciler"), Some("Penciller"));
        assert_eq!(canonicalize_role("penciller"), Some("Penciller"));
        // Synonyms that pull in extra mainstream tagger output.
        assert_eq!(canonicalize_role("artist"), Some("Penciller"));
        assert_eq!(canonicalize_role("scripter"), Some("Writer"));
        assert_eq!(canonicalize_role("colourist"), Some("Colorist"));
        assert_eq!(canonicalize_role("editor in chief"), Some("Editor"));
        // Roles ComicInfo can't represent → None so the composer drops
        // them. The structured junction + MetronInfo still carry them.
        assert_eq!(canonicalize_role("journalist"), None);
        assert_eq!(canonicalize_role("other"), None);
        assert_eq!(canonicalize_role("production"), None);
        // Empty / whitespace input → None.
        assert_eq!(canonicalize_role(""), None);
        assert_eq!(canonicalize_role("   "), None);
    }
}
