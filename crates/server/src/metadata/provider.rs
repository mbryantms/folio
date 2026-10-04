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

use crate::metadata::cache::Validators;
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
    /// The provider's cover image can't be downloaded server-side (its
    /// CDN answers with a bot challenge). Not retryable; the apply
    /// records it as `cover_skipped_reason` and carries on.
    #[error("cover unavailable: {0}")]
    CoverUnavailable(String),
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

/// Shared retrying HTTP send every provider client routes through
/// (WP-2.9). Lives in [`crate::metadata::http`]; re-exported here so
/// the provider surface is discoverable from one module.
pub use crate::metadata::http::send_with_retry;

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
    /// WP-5.6: publication-format hint for the matcher's soft format
    /// penalty — Metron's `series_type` name when the response carries
    /// it, else a label inferred from the name / deck
    /// ([`crate::metadata::title_norm::infer_format_from_title`]).
    /// `None` = unknown (never penalised).
    #[serde(default)]
    pub format: Option<String>,
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
    /// WP-5.6: publication-format hint of the candidate's series (see
    /// [`SeriesCandidate::format`]).
    #[serde(default)]
    pub format: Option<String>,
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
    /// When this detail is an *issue*, the parent series' provider id.
    /// The issue *list* endpoints sometimes omit it, but the issue
    /// *detail* carries it — auto-split (`metadata::auto_split`) reads
    /// it to map a divergent issue range onto its alternate series.
    pub series_external_id: Option<String>,
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
    /// WP-7.8: provider series this series is linked to (Metron
    /// `associated`). Untyped and symmetric upstream — the relationship
    /// layer (`relationships::external`) gives them a kind from the two
    /// series types. Not a ComicInfo / MetronInfo field: never composed
    /// into a sidecar.
    #[serde(default)]
    pub related_series: Vec<ProviderSeriesRef>,

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
/// `<Credit><Roles><Role>` can still carry the original (when it is in
/// the schema's role enumeration) — that's orthogonal to this mapping.
/// For the junction **storage** form see [`canonical_credit_role`].
pub fn canonicalize_role(raw: &str) -> Option<&'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Lower-cased + whitespace-collapsed key so " Cover Artist ",
    // "cover artist" and the DB's own snake_case `cover_artist` (and
    // `editor-in-chief`) hit the same arm.
    let key: String = trimmed
        .to_ascii_lowercase()
        .replace(['_', '-'], " ")
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

/// The canonical **storage** form of a credit role — the lowercase
/// snake_case key the `issue_credits` / `series_credits` junctions, the
/// per-role CSV read-cache rebuild
/// ([`crate::metadata::writers::rebuild_issue_csv_cache`]), the scanner
/// (`CreditRole::as_str`), the filters and the web UI all match on:
/// `writer`, `penciller`, `inker`, `colorist`, `letterer`,
/// `cover_artist`, `editor`, `translator`.
///
/// Every provider / ComicInfo spelling [`canonicalize_role`] knows
/// (`Writer`, `CoverArtist`, `Cover Artist`, `cover`, `penciler`,
/// `artist`, …) folds onto one of those eight. A role outside that set
/// (`journalist`, `Production`, `Ink Assists`) is kept, lowercased and
/// whitespace-collapsed into snake_case (`ink_assists`), so the
/// junction still carries it. Idempotent: a canonical value maps to
/// itself. Returns `None` for an empty / whitespace-only role.
///
/// WP-8.1: the provider mappers emit the ComicInfo PascalCase names, so
/// before this every non-writeback provider apply wrote `Writer` rows
/// the lowercase CSV rebuild never matched. The migration
/// `m20270601_000001_canonical_credit_roles` carries the same table in
/// SQL for existing rows — keep the two in sync.
pub fn canonical_credit_role(raw: &str) -> Option<String> {
    if let Some(ci) = canonicalize_role(raw) {
        let key = match ci {
            "Writer" => "writer",
            "Penciller" => "penciller",
            "Inker" => "inker",
            "Colorist" => "colorist",
            "Letterer" => "letterer",
            "CoverArtist" => "cover_artist",
            "Editor" => "editor",
            "Translator" => "translator",
            other => return Some(other.to_ascii_lowercase()),
        };
        return Some(key.to_owned());
    }
    let key = raw
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    (!key.is_empty()).then_some(key)
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

/// A provider series referenced by another provider record (WP-7.8:
/// Metron series `associated`). The series may or may not be in the
/// library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSeriesRef {
    pub source: Source,
    /// The provider's series id.
    pub id: String,
    /// The provider's display label as sent ("Saga (2018)").
    pub label: String,
    /// `label` minus a trailing "(year)".
    pub name: String,
    /// The year parsed from the label, when it carries one.
    pub year: Option<i32>,
    /// Canonical provider page.
    pub url: Option<String>,
}

/// A provider series found through a **curated cross-reference** — the
/// provider's own record of another provider's id for the same series
/// (Metron stores `cv_id` / `gcd_id` on every series). Returned by
/// [`MetadataProvider::find_series_by_cross_ref`]; provider range
/// detection uses it to resolve a series' Metron / GCD id when the series
/// was only ever matched through ComicVine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossRefSeries {
    /// This provider's series id.
    pub external_id: String,
    pub name: Option<String>,
    pub year_began: Option<i32>,
    /// Every other provider id the record carries (e.g. the GCD id on a
    /// Metron series found by its ComicVine id).
    pub identifiers: Vec<crate::metadata::identifier::Identifier>,
}

/// One issue of a provider series, as listed by
/// [`MetadataProvider::list_series_issues`]. Provider-independent series
/// coverage ([`crate::metadata::coverage`]) assigns local issues by
/// number + cover date; the provider issue id lets a later lookup go
/// straight to the issue detail instead of searching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderIssue {
    /// The provider's issue id. `None` only when the listing didn't
    /// carry one (never for ComicVine / Metron; GCD index rows always
    /// do).
    pub external_id: Option<String>,
    /// Canonical issue number ([`crate::metadata::matcher::canonical_issue_number`]).
    pub number: String,
    /// Cover date (day precision is not meaningful: GCD/Metron store the
    /// first of the month).
    pub cover_date: Option<NaiveDate>,
}

/// A provider series' issue list plus the series' display identity when
/// the listing carried it for free.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSeriesIssues {
    pub series_name: Option<String>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    /// One entry per distinct canonical number (variants folded onto
    /// their base issue), in provider order.
    pub issues: Vec<ProviderIssue>,
    /// `false` when the listing stopped early (page cap / budget), so a
    /// number missing from `issues` may still exist upstream.
    pub complete: bool,
    /// `true` when every listed issue had its cover date looked up (the
    /// listing carries dates). GCD only dates the overview pages it was
    /// asked for (`IssueListOpts::date_hint`).
    pub dates_complete: bool,
    /// Network requests this call spent (0 when served from cache).
    #[serde(skip)]
    pub requests: u32,
}

/// Options for [`MetadataProvider::list_series_issues`].
#[derive(Clone, Debug, Default)]
pub struct IssueListOpts {
    /// Canonical numbers whose cover dates the caller needs. Providers
    /// whose listing carries dates ignore it; GCD reads only the overview
    /// pages holding these numbers.
    pub date_hint: Vec<String>,
    /// Upper bound on listing pages (0 ⇒ the provider default). A listing
    /// that hits it comes back with `complete = false`.
    pub max_pages: u32,
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

// ───────── conditional fetch ─────────

/// Result of a validator-carrying detail fetch. See
/// [`MetadataProvider::fetch_series_conditional`].
#[derive(Clone, Debug)]
pub enum ConditionalFetch {
    /// Upstream sent a body; `validators` are what it attached for the
    /// next conditional request (empty when it sent none).
    Fresh {
        /// Boxed: `GenericMetadata` is ~1 KiB and the enum is passed by
        /// value through the cache's single-flight path.
        payload: Box<GenericMetadata>,
        validators: Validators,
    },
    /// Upstream answered `304 Not Modified` — the cached copy is still
    /// current.
    NotModified,
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

    /// Conditional detail fetch (WP-2.9). `validators` are the
    /// `ETag` / `Last-Modified` values stored with the cached copy; a
    /// provider that supports `If-None-Match` / `If-Modified-Since`
    /// returns [`ConditionalFetch::NotModified`] on a 304 so the cache
    /// can extend the row's TTL without re-downloading the payload.
    ///
    /// The default ignores `validators` and wraps [`fetch_series`] —
    /// correct for providers without conditional support (ComicVine),
    /// which then simply never see a 304.
    ///
    /// [`fetch_series`]: MetadataProvider::fetch_series
    async fn fetch_series_conditional(
        &self,
        external_id: &str,
        _validators: Option<&Validators>,
    ) -> ProviderResult<ConditionalFetch> {
        Ok(ConditionalFetch::Fresh {
            payload: Box::new(self.fetch_series(external_id).await?),
            validators: Validators::default(),
        })
    }

    /// Issue-detail twin of [`fetch_series_conditional`].
    ///
    /// [`fetch_series_conditional`]: MetadataProvider::fetch_series_conditional
    async fn fetch_issue_conditional(
        &self,
        external_id: &str,
        _validators: Option<&Validators>,
    ) -> ProviderResult<ConditionalFetch> {
        Ok(ConditionalFetch::Fresh {
            payload: Box::new(self.fetch_issue(external_id).await?),
            validators: Validators::default(),
        })
    }

    /// List the **canonical** issue numbers a provider series contains.
    /// Used by [`crate::metadata::auto_split`] to find local issues a
    /// matched series doesn't cover (a split / legacy-renumbered run).
    ///
    /// Default `Ok(vec![])` means "enumeration unsupported" — auto-split
    /// then skips this provider, which is the right behaviour for a
    /// lumper like ComicVine that needs no split. Splitter providers
    /// (Metron, GCD) override it and [`Self::enumerates_series_issues`].
    async fn list_series_issue_numbers(
        &self,
        _series_external_id: &str,
    ) -> ProviderResult<Vec<String>> {
        Ok(Vec::new())
    }

    /// `true` when [`Self::list_series_issue_numbers`] is implemented —
    /// i.e. the provider can enumerate a series' issue numbers, which is
    /// what provider range detection needs. Lumpers (ComicVine) keep the
    /// default `false`, so detection reports them as "can't enumerate"
    /// without spending a request.
    fn enumerates_series_issues(&self) -> bool {
        false
    }

    /// List a provider series' issues with their ids and cover dates
    /// (provider-independent coverage, [`crate::metadata::coverage`]).
    ///
    /// The default derives the list from [`Self::list_series_issue_numbers`]
    /// (numbers only, no ids or dates). ComicVine, Metron and GCD override
    /// it. Callers go through [`crate::metadata::coverage::provider_issues`],
    /// which caches the result for 24 h.
    async fn list_series_issues(
        &self,
        series_external_id: &str,
        _opts: &IssueListOpts,
    ) -> ProviderResult<ProviderSeriesIssues> {
        let numbers = self.list_series_issue_numbers(series_external_id).await?;
        let mut seen = std::collections::HashSet::new();
        Ok(ProviderSeriesIssues {
            issues: numbers
                .into_iter()
                .filter(|n| seen.insert(n.clone()))
                .map(|number| ProviderIssue {
                    external_id: None,
                    number,
                    cover_date: None,
                })
                .collect(),
            complete: true,
            ..Default::default()
        })
    }

    /// `true` when [`Self::list_series_issues`] returns real data. Unlike
    /// [`Self::enumerates_series_issues`] (the split detector's gate,
    /// which stays `false` for ComicVine), this includes ComicVine, whose
    /// volume issue list is paginated `/issues/?filter=volume:`.
    fn lists_series_issues(&self) -> bool {
        self.enumerates_series_issues()
    }

    /// Find this provider's series by **another** provider's series id,
    /// using the provider's own curated cross-reference (one request).
    /// `source` is the provider the `external_id` belongs to.
    ///
    /// Default `Ok(vec![])` means "no cross-reference index". Metron
    /// overrides it (`/api/series/?cv_id=` / `?gcd_id=`). More than one
    /// result means the cross-reference is ambiguous; callers must not
    /// treat that as a confident match.
    async fn find_series_by_cross_ref(
        &self,
        _source: Source,
        _external_id: &str,
    ) -> ProviderResult<Vec<CrossRefSeries>> {
        Ok(Vec::new())
    }

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
    fn canonical_credit_role_folds_every_spelling_onto_the_storage_key() {
        for (raw, want) in [
            ("Writer", "writer"),
            ("writer", "writer"),
            (" WRITER ", "writer"),
            ("Script", "writer"),
            ("Penciller", "penciller"),
            ("penciler", "penciller"),
            ("Artist", "penciller"),
            ("Inker", "inker"),
            ("Colorist", "colorist"),
            ("colourist", "colorist"),
            ("Letterer", "letterer"),
            ("CoverArtist", "cover_artist"),
            ("Cover Artist", "cover_artist"),
            ("cover_artist", "cover_artist"),
            ("Cover", "cover_artist"),
            ("Editor", "editor"),
            ("Editor In Chief", "editor"),
            ("editor-in-chief", "editor"),
            ("Translator", "translator"),
            // Outside the eight: kept, snake_cased.
            ("journalist", "journalist"),
            ("Production", "production"),
            ("Ink Assists", "ink_assists"),
            ("unknown", "unknown"),
        ] {
            assert_eq!(canonical_credit_role(raw).as_deref(), Some(want), "{raw}");
            // Idempotent.
            assert_eq!(canonical_credit_role(want).as_deref(), Some(want), "{want}");
        }
        assert_eq!(canonical_credit_role(""), None);
        assert_eq!(canonical_credit_role("  "), None);
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
