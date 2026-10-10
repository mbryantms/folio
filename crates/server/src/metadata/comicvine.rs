//! ComicVine API client (`comicvine.gamespot.com/api`).
//!
//! TOS: non-commercial only, attribution required, caching encouraged.
//! Rate: "200 requests per resource, per hour" — one Redis bucket per
//! resource (`rate_limit::comicvine_hour`), so `/issues` searches and
//! `/issue` details don't drain each other — plus a 1 req/sec velocity
//! bucket shared across every client, worker and replica
//! (`rate_limit::COMICVINE_SEC`).
//!
//! Endpoints we use:
//! - `GET /search?resources=volume,issue,publisher&query=...` — keyword search.
//! - `GET /volumes?filter=name:...` — series search (name only; year filtering
//!   is the tolerant `pre_filter_series` gate's job, not a hard provider filter).
//! - `GET /volume/4050-{id}` — series detail.
//! - `GET /issues?filter=volume:{id},issue_number:...` — narrowed issue search.
//! - `GET /issue/4000-{id}` — issue detail.
//!
//! Auth: `?api_key=...` query param (CV doesn't accept a header).
//! Response format: `?format=json`. Field-trim: `?field_list=...` (we
//! pull a known subset on detail calls to keep payloads small).
//!
//! Status-code semantics (in body, **always** with HTTP 200 unless the
//! transport itself failed):
//! - 1   → OK
//! - 100 → invalid API key
//! - 101 → object not found
//! - 105 → subscriber-only (we treat as Upstream — CV gates some content)
//! - 107 → rate limit / abuse
//! - 200 → upstream filter error (we treat as InvalidResponse)
//!
//! Velocity cap: every request takes the shared 1-second Redis bucket
//! first; when another client took it this second, the worker sleeps
//! out the window and tries again (bounded), rather than parking the
//! run. A per-client `Mutex<Instant>` couldn't do this: each job builds
//! its own client, and the search / apply / coverage workers and the API
//! handlers run side by side.

use crate::metadata::budget;
use crate::metadata::cache;
use crate::metadata::http;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::provider::{
    CreditCandidate, EntityCandidate, GenericMetadata, IssueCandidate, IssueListOpts, IssueQuery,
    MetadataProvider, ProviderError, ProviderIssue, ProviderResult, ProviderSeriesIssues,
    QuotaSnapshot, SeriesCandidate, SeriesQuery, VariantCoverCandidate,
};
use crate::metadata::rate_limit::{self, Reservation};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use redis::aio::ConnectionManager;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// User-agent reported to CV — TOS asks for a unique identifier.
const USER_AGENT: &str = crate::build_info::USER_AGENT_METADATA;

/// Floor between successful API calls. ComicVine's documented rate
/// is "≤ 1 req/sec sustained"; we conservatively wait 1s + a small
/// jitter ceiling to absorb clock skew.
/// How many 1-second windows a request waits for the shared velocity
/// bucket before giving up (a transport error, not quota: the hour
/// budget was already reserved).
const VELOCITY_WAIT_ATTEMPTS: u32 = 15;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Wall-clock budget for one logical API call *including* the shared
/// layer's retries (see [`crate::metadata::http`]).
const REQUEST_DEADLINE: Duration = Duration::from_secs(75);

const SERIES_FIELDS: &str = "id,name,start_year,publisher,deck,description,image,count_of_issues,site_detail_url,date_last_updated,aliases";
/// Field list for a volume's issue listing (provider coverage): the id,
/// number and dates, plus the volume ref for its display name.
const ISSUE_LIST_FIELDS: &str = "id,issue_number,cover_date,store_date,volume";

/// CV's maximum page size for list endpoints.
pub const ISSUE_LIST_PAGE_SIZE: usize = 100;

/// Default page cap for one volume listing (2,000 issues). Past it the
/// listing is returned with `complete = false`.
pub const ISSUE_LIST_MAX_PAGES: u32 = 20;

const ISSUE_FIELDS: &str = "id,name,issue_number,cover_date,store_date,deck,description,image,associated_images,person_credits,character_credits,team_credits,location_credits,concept_credits,object_credits,story_arc_credits,first_appearance_characters,first_appearance_teams,first_appearance_locations,first_appearance_concepts,first_appearance_objects,first_appearance_storyarcs,characters_died_in,teams_disbanded_in,volume,site_detail_url,date_last_updated,aliases";

/// Cloneable handle to the ComicVine client. The reqwest::Client +
/// Redis connection are themselves clone-safe (Arc internally), so
/// `.clone()` is cheap.
#[derive(Clone)]
pub struct ComicVineClient {
    inner: Arc<Inner>,
}

struct Inner {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    redis: ConnectionManager,
}

impl ComicVineClient {
    /// Production constructor — points at `comicvine.gamespot.com/api`.
    pub fn new(api_key: String, redis: ConnectionManager) -> Self {
        Self::with_base_url(
            api_key,
            "https://comicvine.gamespot.com/api".to_owned(),
            redis,
        )
    }

    /// Test constructor — points at an arbitrary base URL (wiremock).
    pub fn with_base_url(api_key: String, base_url: String, redis: ConnectionManager) -> Self {
        // Defense-in-depth trim: the overlay loader already strips
        // whitespace from the stored secret, but a stale value
        // written before that fix shipped (or any non-overlay caller)
        // shouldn't reach CV with a `?api_key=...%0A` URL.
        let api_key = api_key.trim().to_owned();
        let http = http::build_client(USER_AGENT, DEFAULT_TIMEOUT);
        Self {
            inner: Arc::new(Inner {
                api_key,
                base_url,
                http,
                redis,
            }),
        }
    }

    /// One-shot helper used by the orchestrator: cache lookup → live
    /// fetch if missing → cache write. Returns the normalized
    /// `GenericMetadata` either way.
    pub async fn fetch_series_cached(
        &self,
        db: &DatabaseConnection,
        external_id: &str,
    ) -> ProviderResult<GenericMetadata> {
        let ttl =
            chrono::Duration::from_std(cache::CacheEntity::Series.default_ttl().to_std().unwrap())
                .unwrap_or(chrono::Duration::hours(168));
        cache::get_or_fetch(
            db,
            Source::ComicVine,
            cache::CacheEntity::Series,
            external_id,
            ttl,
            || self.fetch_series(external_id),
        )
        .await
    }

    /// Same shape as [`fetch_series_cached`] for issue detail.
    pub async fn fetch_issue_cached(
        &self,
        db: &DatabaseConnection,
        external_id: &str,
    ) -> ProviderResult<GenericMetadata> {
        let ttl =
            chrono::Duration::from_std(cache::CacheEntity::Issue.default_ttl().to_std().unwrap())
                .unwrap_or(chrono::Duration::hours(24));
        cache::get_or_fetch(
            db,
            Source::ComicVine,
            cache::CacheEntity::Issue,
            external_id,
            ttl,
            || self.fetch_issue(external_id),
        )
        .await
    }

    /// Take one token from the request path's hourly resource bucket,
    /// then the shared per-second velocity bucket.
    async fn reserve_slot(&self, path: &str) -> ProviderResult<()> {
        let mut redis = self.inner.redis.clone();
        let resource = rate_limit::ComicVineResource::from_path(path);
        match rate_limit::reserve(&mut redis, &rate_limit::comicvine_hour(resource)).await {
            Ok(Reservation::Granted { .. }) => {}
            Ok(Reservation::Denied { retry_after_secs }) => {
                tracing::info!(
                    resource = resource.as_str(),
                    retry_after_secs,
                    "comicvine: hourly budget for resource exhausted"
                );
                return Err(ProviderError::QuotaExceeded { retry_after_secs });
            }
            Err(e) => return Err(ProviderError::Transport(format!("redis: {e}"))),
        }
        // Velocity: one request per second across everything that talks
        // to CV. A denial is "someone else fired this second" — wait the
        // window out and try again; the hour token is already ours.
        for _ in 0..VELOCITY_WAIT_ATTEMPTS {
            match rate_limit::reserve(&mut redis, &rate_limit::COMICVINE_SEC).await {
                Ok(Reservation::Granted { .. }) => return Ok(()),
                Ok(Reservation::Denied { retry_after_secs }) => {
                    tokio::time::sleep(Duration::from_secs(retry_after_secs.clamp(1, 2))).await;
                }
                Err(e) => return Err(ProviderError::Transport(format!("redis: {e}"))),
            }
        }
        Err(ProviderError::Transport(
            "comicvine: velocity bucket busy for too long".to_owned(),
        ))
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        extra_query: &[(&str, String)],
    ) -> ProviderResult<T> {
        self.reserve_slot(path).await?;
        let result = self.request_inner(path, extra_query).await;
        match &result {
            Ok(_) => budget::clear_error(&self.inner.redis, Source::ComicVine).await,
            Err(e) => {
                budget::record_error(&self.inner.redis, Source::ComicVine, &e.to_string()).await;
            }
        }
        result
    }

    /// ComicVine said this resource is out of budget: make the local
    /// bucket agree so the other workers park instead of each burning a
    /// request to learn the same thing.
    async fn drain_resource(&self, path: &str, retry_after_secs: u64) {
        let resource = rate_limit::ComicVineResource::from_path(path);
        let mut redis = self.inner.redis.clone();
        rate_limit::exhaust(
            &mut redis,
            &rate_limit::comicvine_hour(resource),
            retry_after_secs,
        )
        .await;
    }

    async fn request_inner<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        extra_query: &[(&str, String)],
    ) -> ProviderResult<T> {
        let url = format!("{}{}", self.inner.base_url, path);
        let opts = http::RequestOpts {
            deadline: Some(Instant::now() + REQUEST_DEADLINE),
            ..Default::default()
        };
        let build = || {
            let mut req = self.inner.http.get(&url).query(&[
                ("api_key", &self.inner.api_key),
                ("format", &"json".to_owned()),
            ]);
            if !extra_query.is_empty() {
                req = req.query(extra_query);
            }
            req
        };
        // The shared layer already retried transport errors + 5xx; what
        // comes back is a 2xx/3xx/4xx to classify. 4xx (other than 429)
        // is still parsed since CV puts its real status in the envelope.
        let resp = http::send_with_retry(build, &opts, &redact_api_key).await?;
        let status = resp.status;
        if status.as_u16() == 429 {
            let retry_after_secs =
                http::retry_after_secs(&resp.headers, http::DEFAULT_RETRY_AFTER_SECS);
            self.drain_resource(path, retry_after_secs).await;
            return Err(ProviderError::QuotaExceeded { retry_after_secs });
        }
        // Parse the standard envelope first so we can map status_code
        // before the typed deserialize.
        let envelope: CvEnvelope<serde_json::Value> =
            serde_json::from_slice(&resp.body).map_err(|e| {
                ProviderError::InvalidResponse(format!(
                    "envelope parse: {e}; body={}",
                    resp.snippet(256)
                ))
            })?;
        match envelope.status_code.unwrap_or(1) {
            1 => {}
            100 => {
                return Err(ProviderError::Unauthorized(
                    envelope.error.unwrap_or_default(),
                ));
            }
            101 => return Err(ProviderError::NotFound(envelope.error.unwrap_or_default())),
            107 => {
                // CV's "rate limit exceeded" envelope. It rarely carries a
                // `Retry-After`; the hourly window is the documented
                // fallback.
                let retry_after_secs = http::retry_after_secs(&resp.headers, 3600);
                self.drain_resource(path, retry_after_secs).await;
                return Err(ProviderError::QuotaExceeded { retry_after_secs });
            }
            other => {
                return Err(ProviderError::Upstream(format!(
                    "ComicVine status_code={other}: {}",
                    envelope.error.unwrap_or_default()
                )));
            }
        }
        serde_json::from_slice::<T>(&resp.body)
            .map_err(|e| ProviderError::InvalidResponse(format!("typed parse: {e}")))
    }
}

// ───────── CV envelope shapes ─────────

#[derive(Debug, Deserialize)]
struct CvEnvelope<T> {
    status_code: Option<i32>,
    error: Option<String>,
    results: Option<T>,
    /// Total matches across every page of a list endpoint.
    #[serde(default)]
    number_of_total_results: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CvImage {
    icon_url: Option<String>,
    medium_url: Option<String>,
    screen_url: Option<String>,
    super_url: Option<String>,
    original_url: Option<String>,
    thumb_url: Option<String>,
}

/// One entry of the issue-detail `associated_images` array — CV's
/// variant-cover surface. The single `image` field carries the primary
/// cover's size renditions; variant/alternate covers live here, one
/// object per image. `image_tags` is a comma-joined gallery label
/// ("Cover", "Other Images", …); `caption` is free text we surface as
/// the variant label.
#[derive(Debug, Deserialize)]
struct CvAltImage {
    original_url: Option<String>,
    id: Option<i64>,
    caption: Option<String>,
    #[allow(dead_code)] // retained for parity with the CV field_list; not yet used for filtering
    image_tags: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CvVolume {
    id: Option<i64>,
    name: Option<String>,
    start_year: Option<String>,
    publisher: Option<CvNamedRef>,
    deck: Option<String>,
    description: Option<String>,
    image: Option<CvImage>,
    count_of_issues: Option<i32>,
    site_detail_url: Option<String>,
    date_last_updated: Option<String>,
    aliases: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // site_detail_url is in the field_list spec; keep for parity
struct CvNamedRef {
    id: Option<i64>,
    name: Option<String>,
    site_detail_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CvIssue {
    id: Option<i64>,
    name: Option<String>,
    issue_number: Option<String>,
    cover_date: Option<String>,
    store_date: Option<String>,
    deck: Option<String>,
    description: Option<String>,
    image: Option<CvImage>,
    /// Variant / alternate covers. Only present on the issue-detail
    /// endpoint, never on /issues or /search list responses.
    #[serde(default)]
    associated_images: Option<Vec<CvAltImage>>,
    person_credits: Option<Vec<CvPersonCredit>>,
    character_credits: Option<Vec<CvNamedRef>>,
    team_credits: Option<Vec<CvNamedRef>>,
    location_credits: Option<Vec<CvNamedRef>>,
    concept_credits: Option<Vec<CvNamedRef>>,
    object_credits: Option<Vec<CvNamedRef>>,
    story_arc_credits: Option<Vec<CvNamedRef>>,
    // First-appearance / death / disband relationship lists. Each is a
    // list of the entities for which *this issue* is the first
    // appearance (or the death / disband issue). We cross-reference the
    // ids against the matching `*_credits` list to stamp the per-row
    // flags `is_first_appearance` / `died_in_issue` /
    // `disbanded_in_issue` the apply path persists.
    #[serde(default)]
    first_appearance_characters: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    first_appearance_teams: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    first_appearance_locations: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    first_appearance_concepts: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    first_appearance_objects: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    first_appearance_storyarcs: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    characters_died_in: Option<Vec<CvNamedRef>>,
    #[serde(default)]
    teams_disbanded_in: Option<Vec<CvNamedRef>>,
    volume: Option<CvVolume>,
    site_detail_url: Option<String>,
    date_last_updated: Option<String>,
    aliases: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // site_detail_url is in the field_list spec; keep for parity
struct CvPersonCredit {
    id: Option<i64>,
    name: Option<String>,
    role: Option<String>,
    site_detail_url: Option<String>,
}

// CV `aliases` field is a newline-delimited list (their convention).
fn split_aliases(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .map(|s| {
            s.split('\n')
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(|t| t.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_year(raw: &Option<String>) -> Option<i32> {
    raw.as_deref()?.trim().parse().ok()
}

fn parse_date(raw: &Option<String>) -> Option<NaiveDate> {
    let s = raw.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn parse_cv_timestamp(raw: &Option<String>) -> Option<DateTime<Utc>> {
    let s = raw.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    // CV serializes "YYYY-MM-DD HH:MM:SS" in UTC implicitly.
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|ndt| ndt.and_utc())
}

fn best_image_url(image: &Option<CvImage>) -> (Option<String>, Vec<String>) {
    let Some(img) = image else {
        return (None, Vec::new());
    };
    let preferred = [
        &img.super_url,
        &img.original_url,
        &img.screen_url,
        &img.medium_url,
        &img.icon_url,
        &img.thumb_url,
    ];
    let mut chosen = None;
    let mut alts = Vec::new();
    for slot in preferred.iter() {
        if let Some(u) = slot.as_deref().filter(|s| !s.is_empty()) {
            if chosen.is_none() {
                chosen = Some(u.to_owned());
            } else {
                alts.push(u.to_owned());
            }
        }
    }
    (chosen, alts)
}

fn cv_volume_to_candidate(v: &CvVolume) -> Option<SeriesCandidate> {
    let id = v.id?;
    let external_id = id.to_string();
    let url = v.site_detail_url.clone().or_else(|| {
        crate::metadata::identifier::canonical_url(Source::ComicVine, "series", &external_id)
    });
    let (cover, _) = best_image_url(&v.image);
    Some(SeriesCandidate {
        source: Source::ComicVine,
        external_id,
        external_url: url,
        name: v.name.clone().unwrap_or_default(),
        year: parse_year(&v.start_year),
        publisher: v.publisher.as_ref().and_then(|p| p.name.clone()),
        issue_count: v.count_of_issues,
        cover_image_url: cover,
        deck: v.deck.clone(),
        // CV /volumes search response doesn't surface variant covers;
        // they live on the issue-detail endpoint via `associated_images`.
        // Populated by M5.x follow-up when the orchestrator pre-fetches
        // top-K candidate details.
        alternate_cover_urls: Vec::new(),
        // WP-5.6: CV has no volume-type field; infer from name + deck.
        format: cv_volume_format(v).map(str::to_owned),
    })
}

fn cv_issue_to_candidate(issue: &CvIssue) -> Option<IssueCandidate> {
    let id = issue.id?;
    let external_id = id.to_string();
    let url = issue.site_detail_url.clone().or_else(|| {
        crate::metadata::identifier::canonical_url(Source::ComicVine, "issue", &external_id)
    });
    let (cover, _) = best_image_url(&issue.image);
    Some(IssueCandidate {
        source: Source::ComicVine,
        external_id,
        external_url: url,
        issue_number: issue.issue_number.clone(),
        name: issue.name.clone(),
        cover_date: parse_date(&issue.cover_date),
        series_name: issue.volume.as_ref().and_then(|v| v.name.clone()),
        series_year: issue
            .volume
            .as_ref()
            .and_then(|v| parse_year(&v.start_year)),
        series_external_id: issue
            .volume
            .as_ref()
            .and_then(|v| v.id.map(|n| n.to_string())),
        cover_image_url: cover,
        // CV /issues search response doesn't surface variant covers;
        // populated by detail-fetch follow-up.
        alternate_cover_urls: Vec::new(),
        // WP-5.6: the embedded volume ref carries only its name.
        format: issue
            .volume
            .as_ref()
            .and_then(cv_volume_format)
            .map(str::to_owned),
    })
}

/// WP-5.6: ComicVine exposes no volume type, so a volume's format is
/// inferred from its name + deck (`"Saga TPB"`, `"X-Men Annual"`,
/// deck `"Collects #1-6"`). `None` when nothing says so. **Matching
/// only** (owner decision 2026-09-30): it feeds the candidate's
/// `format` hint and is never written to `Format` / `series_type` on
/// apply — only Metron's explicit `series_type` is.
fn cv_volume_format(v: &CvVolume) -> Option<&'static str> {
    crate::metadata::title_norm::infer_format_from_title(
        v.name.as_deref().unwrap_or(""),
        v.deck.as_deref(),
    )
}

fn cv_volume_to_metadata(v: CvVolume) -> GenericMetadata {
    let external_id = v.id.map(|n| n.to_string()).unwrap_or_default();
    let (cover, alts) = best_image_url(&v.image);
    let mut identifiers = vec![Identifier::with_canonical_url(
        Source::ComicVine,
        external_id.clone(),
        "series",
    )];
    if let Some(pub_ref) = v.publisher.as_ref()
        && let Some(pub_id) = pub_ref.id
    {
        identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            pub_id.to_string(),
            "publisher",
        ));
    }
    GenericMetadata {
        series_name: v.name,
        year_began: parse_year(&v.start_year),
        publisher: v.publisher.as_ref().and_then(|p| p.name.clone()),
        deck: v.deck,
        description: v.description,
        cover_image_url: cover,
        cover_image_alt_urls: alts,
        aliases: split_aliases(&v.aliases),
        identifiers,
        source_provider: Some(Source::ComicVine),
        source_external_id: if external_id.is_empty() {
            None
        } else {
            Some(external_id)
        },
        source_url: v.site_detail_url,
        fetched_at: Some(Utc::now()),
        upstream_modified_at: parse_cv_timestamp(&v.date_last_updated),
        ..Default::default()
    }
}

/// Collect the CV ids from a relationship list (e.g.
/// `first_appearance_characters`) into a set for O(1) membership tests
/// while stamping per-entity flags.
fn cv_id_set(list: &Option<Vec<CvNamedRef>>) -> HashSet<i64> {
    list.iter().flatten().filter_map(|n| n.id).collect()
}

/// Map an issue's `associated_images` array to variant-cover
/// candidates. Mirrors ComicTagger's CV talker, which treats every
/// `associated_images` entry as an alternate cover: on an issue page
/// these are overwhelmingly variant covers. The free-text `caption`
/// becomes the variant label; the CV image `id` is recorded as the
/// source identifier so a future re-pull can dedup. Entries without a
/// usable URL are dropped by the writer.
fn cv_alt_images_to_variants(images: &Option<Vec<CvAltImage>>) -> Vec<VariantCoverCandidate> {
    images
        .iter()
        .flatten()
        .filter_map(|img| {
            let url = img
                .original_url
                .as_deref()
                .filter(|s| !s.trim().is_empty())?;
            let identifiers = match img.id {
                Some(id) => vec![Identifier::new(Source::ComicVine, id.to_string())],
                None => Vec::new(),
            };
            Some(VariantCoverCandidate {
                label: img
                    .caption
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                artist_name: None,
                identifiers,
                image_url: Some(url.to_owned()),
            })
        })
        .collect()
}

/// Map a `*_credits` list into entity candidates, stamping
/// `is_first_appearance` for any whose CV id is in `first_ids`.
fn cv_entities_with_first(
    list: &Option<Vec<CvNamedRef>>,
    entity_type: &str,
    first_ids: &HashSet<i64>,
) -> Vec<EntityCandidate> {
    list.iter()
        .flatten()
        .filter_map(|n| {
            let mut e = cv_named_to_entity(n, entity_type)?;
            if let Some(id) = n.id {
                e.is_first_appearance = first_ids.contains(&id);
            }
            Some(e)
        })
        .collect()
}

fn cv_named_to_entity(n: &CvNamedRef, entity_type: &str) -> Option<EntityCandidate> {
    let name = n.name.clone().filter(|s| !s.trim().is_empty())?;
    let identifiers = match n.id {
        Some(id) => vec![Identifier::with_canonical_url(
            Source::ComicVine,
            id.to_string(),
            entity_type,
        )],
        None => Vec::new(),
    };
    Some(EntityCandidate {
        name,
        identifiers,
        is_first_appearance: false,
        died_in_issue: None,
        disbanded_in_issue: None,
        position_in_arc: None,
    })
}

fn cv_credit_to_credit(c: &CvPersonCredit) -> Option<CreditCandidate> {
    let name = c.name.clone().filter(|s| !s.trim().is_empty())?;
    // CV joins multiple roles with comma+space ("writer, cover"). The
    // writer helpers expect one row per role, so we explode at the
    // call site rather than here — emit one credit per source role
    // string and let the call site split.
    let role = c.role.clone().unwrap_or_default();
    let identifiers = match c.id {
        Some(id) => vec![Identifier::with_canonical_url(
            Source::ComicVine,
            id.to_string(),
            "person",
        )],
        None => Vec::new(),
    };
    Some(CreditCandidate {
        name,
        role,
        ordinal: None,
        identifiers,
    })
}

fn cv_issue_to_metadata(issue: CvIssue) -> GenericMetadata {
    let external_id = issue.id.map(|n| n.to_string()).unwrap_or_default();
    let (cover, alts) = best_image_url(&issue.image);
    let mut identifiers = vec![Identifier::with_canonical_url(
        Source::ComicVine,
        external_id.clone(),
        "issue",
    )];
    if let Some(vol) = issue.volume.as_ref()
        && let Some(vol_id) = vol.id
    {
        identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            vol_id.to_string(),
            "series",
        ));
    }
    let credits = issue
        .person_credits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .flat_map(|c| {
            cv_credit_to_credit(c).into_iter().flat_map(|cc| {
                if cc.role.is_empty() {
                    // No role on the CV row — keep the credit but tag
                    // it `"unknown"` so the consumer can decide. The
                    // composer will drop it (no ComicInfo column);
                    // MetronInfo's structured form would carry it.
                    vec![CreditCandidate {
                        role: "unknown".into(),
                        ..cc
                    }]
                } else {
                    cc.role
                        .split(',')
                        .map(|r| CreditCandidate {
                            name: cc.name.clone(),
                            // Canonicalize CV's role tags (e.g. `"cover"`
                            // → `"CoverArtist"`) so the composer's
                            // `eq_ignore_ascii_case("CoverArtist")`
                            // filter actually fires. Roles ComicInfo
                            // can't represent fall through to their
                            // lowercased original — MetronInfo carries
                            // them structurally.
                            role: crate::metadata::provider::canonicalize_role(r)
                                .map(str::to_owned)
                                .unwrap_or_else(|| r.trim().to_lowercase()),
                            ordinal: cc.ordinal,
                            identifiers: cc.identifiers.clone(),
                        })
                        .collect()
                }
            })
        })
        .collect();
    // First-appearance / death / disband cross-reference sets. CV
    // exposes these as separate relationship lists; we stamp the
    // matching credited entity so the apply path can persist
    // `is_first_appearance` / `died_in_issue` / `disbanded_in_issue`.
    let fa_characters = cv_id_set(&issue.first_appearance_characters);
    let fa_teams = cv_id_set(&issue.first_appearance_teams);
    let fa_locations = cv_id_set(&issue.first_appearance_locations);
    let fa_concepts = cv_id_set(&issue.first_appearance_concepts);
    let fa_objects = cv_id_set(&issue.first_appearance_objects);
    let fa_storyarcs = cv_id_set(&issue.first_appearance_storyarcs);
    let died_characters = cv_id_set(&issue.characters_died_in);
    let disbanded_teams = cv_id_set(&issue.teams_disbanded_in);

    let characters = issue
        .character_credits
        .iter()
        .flatten()
        .filter_map(|n| {
            let mut e = cv_named_to_entity(n, "character")?;
            if let Some(id) = n.id {
                e.is_first_appearance = fa_characters.contains(&id);
                e.died_in_issue = Some(died_characters.contains(&id));
            }
            Some(e)
        })
        .collect();
    let teams = issue
        .team_credits
        .iter()
        .flatten()
        .filter_map(|n| {
            let mut e = cv_named_to_entity(n, "team")?;
            if let Some(id) = n.id {
                e.is_first_appearance = fa_teams.contains(&id);
                e.disbanded_in_issue = Some(disbanded_teams.contains(&id));
            }
            Some(e)
        })
        .collect();
    let locations = cv_entities_with_first(&issue.location_credits, "location", &fa_locations);
    let concepts = cv_entities_with_first(&issue.concept_credits, "concept", &fa_concepts);
    let objects = cv_entities_with_first(&issue.object_credits, "object", &fa_objects);
    let story_arcs = cv_entities_with_first(&issue.story_arc_credits, "story_arc", &fa_storyarcs);
    let variants = cv_alt_images_to_variants(&issue.associated_images);

    GenericMetadata {
        title: issue.name,
        issue_number: issue.issue_number,
        cover_date: parse_date(&issue.cover_date),
        store_date: parse_date(&issue.store_date),
        deck: issue.deck,
        description: issue.description,
        cover_image_url: cover,
        cover_image_alt_urls: alts,
        aliases: split_aliases(&issue.aliases),
        series_name: issue.volume.as_ref().and_then(|v| v.name.clone()),
        series_external_id: issue
            .volume
            .as_ref()
            .and_then(|v| v.id.map(|n| n.to_string())),
        year_began: issue
            .volume
            .as_ref()
            .and_then(|v| parse_year(&v.start_year)),
        publisher: issue
            .volume
            .as_ref()
            .and_then(|v| v.publisher.as_ref())
            .and_then(|p| p.name.clone()),
        credits,
        characters,
        teams,
        locations,
        concepts,
        objects,
        story_arcs,
        variants,
        identifiers,
        source_provider: Some(Source::ComicVine),
        source_external_id: if external_id.is_empty() {
            None
        } else {
            Some(external_id)
        },
        source_url: issue.site_detail_url,
        fetched_at: Some(Utc::now()),
        upstream_modified_at: parse_cv_timestamp(&issue.date_last_updated),
        ..Default::default()
    }
}

// ───────── Trait impl ─────────

#[async_trait]
impl MetadataProvider for ComicVineClient {
    fn id(&self) -> Source {
        Source::ComicVine
    }

    async fn health_check(&self) -> ProviderResult<QuotaSnapshot> {
        // Cheapest call that exercises auth — a 1-result volume search
        // for a no-match string. We don't care about the results,
        // only that the envelope.status_code is 1 (or 101 = empty).
        let _: CvEnvelope<serde_json::Value> = self
            .request(
                "/volumes",
                &[
                    ("filter", "name:__folio_health_check__".to_owned()),
                    ("limit", "1".to_owned()),
                    ("field_list", "id".to_owned()),
                ],
            )
            .await?;
        self.quota().await
    }

    async fn quota(&self) -> ProviderResult<QuotaSnapshot> {
        let mut redis = self.inner.redis.clone();
        // The tightest resource binds: that's the number worth showing.
        let (remaining, ttl) = rate_limit::comicvine_hour_snapshot(&mut redis)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        Ok(QuotaSnapshot {
            provider: Source::ComicVine,
            remaining_hour: Some(remaining),
            remaining_day: None,
            seconds_until_reset: Some(ttl),
        })
    }

    async fn search_series(&self, query: &SeriesQuery) -> ProviderResult<Vec<SeriesCandidate>> {
        let limit = query.limit.clamp(1, 100).to_string();
        // Search by name only — `query.year` is NOT used as a hard
        // `start_year:` filter. The local series year is often wrong or off by
        // a year (re-collections, mislabeled folders), and an exact provider
        // filter would exclude the correct volume outright (e.g. a Spawn row
        // mislabeled 2016 would miss the real 1992 volume). Year filtering is
        // the tolerant `orchestrator::pre_filter_series` gate's job (±1), which
        // runs on the returned candidates.
        let filter = format!("name:{}", query.name.replace(',', " "));
        let envelope: CvEnvelope<Vec<CvVolume>> = self
            .request(
                "/volumes",
                &[
                    ("filter", filter),
                    ("limit", limit),
                    ("field_list", SERIES_FIELDS.to_owned()),
                ],
            )
            .await?;
        let results = envelope.results.unwrap_or_default();
        Ok(results.iter().filter_map(cv_volume_to_candidate).collect())
    }

    async fn search_issue(&self, query: &IssueQuery) -> ProviderResult<Vec<IssueCandidate>> {
        let limit = query.limit.clamp(1, 100).to_string();
        // CV stores un-padded issue numbers ("14"), so a zero-padded scan
        // value ("014") must be canonicalized or both the `/issues` filter and
        // the `/search` client-side match below would miss it.
        let qnum = canonical_issue_number(&query.issue_number);
        let mut filters = vec![format!("issue_number:{qnum}")];
        if let Some(vol) = query.series_external_id.as_deref() {
            filters.push(format!("volume:{vol}"));
        } else if let Some(name) = query.series_name.as_deref() {
            // CV's /issues endpoint doesn't filter by volume_name, so
            // fall back to the search endpoint which scores across
            // both volume and issue resources.
            let envelope: CvEnvelope<CvSearchResults> = self
                .request(
                    "/search",
                    &[
                        ("resources", "issue".to_owned()),
                        ("query", name.to_owned()),
                        ("limit", limit.clone()),
                        ("field_list", ISSUE_FIELDS.to_owned()),
                    ],
                )
                .await?;
            let mut out = envelope
                .results
                .map(|r| r.issue.unwrap_or_default())
                .unwrap_or_default();
            // Filter to matching issue_number client-side since
            // /search doesn't honour the filter param. Compare canonical
            // forms so "014" (scan) matches CV's "14".
            out.retain(|i| {
                i.issue_number
                    .as_deref()
                    .map(|n| canonical_issue_number(n) == qnum)
                    .unwrap_or(false)
            });
            return Ok(out.iter().filter_map(cv_issue_to_candidate).collect());
        }
        let envelope: CvEnvelope<Vec<CvIssue>> = self
            .request(
                "/issues",
                &[
                    ("filter", filters.join(",")),
                    ("limit", limit),
                    ("field_list", ISSUE_FIELDS.to_owned()),
                ],
            )
            .await?;
        let results = envelope.results.unwrap_or_default();
        Ok(results.iter().filter_map(cv_issue_to_candidate).collect())
    }

    async fn fetch_series(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let envelope: CvEnvelope<CvVolume> = self
            .request(
                &format!("/volume/4050-{external_id}"),
                &[("field_list", SERIES_FIELDS.to_owned())],
            )
            .await?;
        let v = envelope
            .results
            .ok_or_else(|| ProviderError::NotFound(format!("volume/{external_id}")))?;
        Ok(cv_volume_to_metadata(v))
    }

    async fn fetch_issue(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let envelope: CvEnvelope<CvIssue> = self
            .request(
                &format!("/issue/4000-{external_id}"),
                &[("field_list", ISSUE_FIELDS.to_owned())],
            )
            .await?;
        let i = envelope
            .results
            .ok_or_else(|| ProviderError::NotFound(format!("issue/{external_id}")))?;
        Ok(cv_issue_to_metadata(i))
    }

    fn lists_series_issues(&self) -> bool {
        true
    }

    /// `GET /issues/?filter=volume:<id>&field_list=id,issue_number,cover_date,store_date,volume`,
    /// 100 per page (`offset` paging, CV's maximum page size). Each page
    /// is one request against the hourly bucket and the 1 req/s floor.
    async fn list_series_issues(
        &self,
        series_external_id: &str,
        opts: &IssueListOpts,
    ) -> ProviderResult<ProviderSeriesIssues> {
        let id = series_external_id.trim();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            return Err(ProviderError::NotFound(format!("volume/{id}")));
        }
        let max_pages = if opts.max_pages == 0 {
            ISSUE_LIST_MAX_PAGES
        } else {
            opts.max_pages
        };
        let mut out = ProviderSeriesIssues {
            complete: true,
            dates_complete: true,
            ..Default::default()
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut offset = 0usize;
        loop {
            if out.requests >= max_pages {
                out.complete = false;
                tracing::warn!(
                    volume = id,
                    pages = out.requests,
                    "comicvine: volume issue listing hit its page cap; coverage may be incomplete"
                );
                break;
            }
            let envelope: CvEnvelope<Vec<CvIssue>> = self
                .request(
                    "/issues/",
                    &[
                        ("filter", format!("volume:{id}")),
                        ("field_list", ISSUE_LIST_FIELDS.to_owned()),
                        ("limit", ISSUE_LIST_PAGE_SIZE.to_string()),
                        ("offset", offset.to_string()),
                        ("sort", "id:asc".to_owned()),
                    ],
                )
                .await?;
            out.requests += 1;
            let total = envelope.number_of_total_results.unwrap_or(0).max(0) as usize;
            let page = envelope.results.unwrap_or_default();
            let n = page.len();
            for it in page {
                if out.series_name.is_none()
                    && let Some(v) = it.volume.as_ref()
                {
                    out.series_name = v.name.clone().filter(|s| !s.trim().is_empty());
                }
                let Some(raw) = it.issue_number.as_deref().filter(|s| !s.trim().is_empty()) else {
                    continue;
                };
                let number = canonical_issue_number(raw);
                if !seen.insert(number.clone()) {
                    continue;
                }
                out.issues.push(ProviderIssue {
                    external_id: it.id.map(|i| i.to_string()),
                    number,
                    cover_date: parse_date(&it.cover_date).or_else(|| parse_date(&it.store_date)),
                });
            }
            offset += n;
            if n == 0 || offset >= total {
                break;
            }
        }
        Ok(out)
    }

    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>> {
        // Cover URLs hit CV's CDN, not the API — no rate-limit slot
        // reserved.
        // https-only + magic-sniffed (SE-6, WP-6.3).
        crate::metadata::writers::fetch_cover_bytes(url)
            .await
            .map_err(ProviderError::from)
    }
}

/// Redact the `api_key` query value from a string before it reaches logs.
///
/// reqwest's error `Display` includes the full request URL, and CV auth is a
/// query param (`?api_key=…`, CV accepts no header). Without this, a routine CV
/// timeout or connection reset writes the operator's API key into `/admin/logs`
/// and any OTLP export (SEC-4). Scans for the literal `api_key=` and replaces
/// everything up to the next `&` (or end), so it's independent of the key's
/// contents and handles repeated occurrences.
fn redact_api_key(s: &str) -> String {
    const MARKER: &str = "api_key=";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(MARKER) {
        let (head, tail) = rest.split_at(pos + MARKER.len());
        out.push_str(head);
        out.push_str("REDACTED");
        let value_end = tail.find('&').unwrap_or(tail.len());
        rest = &tail[value_end..];
    }
    out.push_str(rest);
    out
}

// CV /search responses are typed per-resource-key; the API returns
// `{ results: { issue: [...], volume: [...] } }` when multiple
// resources are requested OR `{ results: [...] }` for a single
// resource. We only ask for one resource at a time, but it's still
// keyed-by-resource in the response.
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)] // `volume` reserved for future cross-resource searches
struct CvSearchResults {
    #[serde(default)]
    issue: Option<Vec<CvIssue>>,
    #[serde(default)]
    volume: Option<Vec<CvVolume>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cv_dates() {
        assert_eq!(
            parse_date(&Some("2024-01-15".into())),
            Some(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()),
        );
        assert!(parse_date(&Some("not-a-date".into())).is_none());
        assert!(parse_date(&None).is_none());
        assert!(parse_date(&Some("".into())).is_none());
    }

    #[test]
    fn redact_api_key_strips_secret_from_error_strings() {
        let secret = "0123456789abcdef0123456789abcdef01234567";
        // Mid-URL with a trailing param.
        let s = format!(
            "error sending request for url (https://comicvine.example/api/issue/?api_key={secret}&format=json)"
        );
        let out = redact_api_key(&s);
        assert!(!out.contains(secret), "key leaked: {out}");
        assert!(out.contains("api_key=REDACTED"));
        assert!(out.contains("&format=json"), "non-secret params preserved");

        // Key at end of string (no trailing '&').
        let s2 = format!("https://comicvine.example/?api_key={secret}");
        let out2 = redact_api_key(&s2);
        assert!(!out2.contains(secret));
        assert!(out2.ends_with("api_key=REDACTED"));

        // No key present — unchanged.
        assert_eq!(
            redact_api_key("plain transport error"),
            "plain transport error"
        );
    }

    #[test]
    fn parses_cv_aliases_newline_delimited() {
        let v = split_aliases(&Some("Spider-Man\nWebslinger\n  Wall-Crawler  ".into()));
        assert_eq!(v, vec!["Spider-Man", "Webslinger", "Wall-Crawler"]);
    }

    #[test]
    fn picks_largest_image() {
        let img = Some(CvImage {
            icon_url: Some("icon".into()),
            medium_url: Some("medium".into()),
            screen_url: Some("screen".into()),
            super_url: Some("super".into()),
            original_url: Some("original".into()),
            thumb_url: Some("thumb".into()),
        });
        let (chosen, alts) = best_image_url(&img);
        assert_eq!(chosen.as_deref(), Some("super"));
        assert!(alts.contains(&"original".to_owned()));
        assert!(alts.contains(&"medium".to_owned()));
    }

    #[test]
    fn falls_back_when_super_missing() {
        let img = Some(CvImage {
            icon_url: Some("icon".into()),
            medium_url: None,
            screen_url: None,
            super_url: None,
            original_url: Some("original".into()),
            thumb_url: None,
        });
        let (chosen, alts) = best_image_url(&img);
        assert_eq!(chosen.as_deref(), Some("original"));
        assert_eq!(alts, vec!["icon".to_owned()]);
    }

    #[test]
    fn maps_cv_volume_to_candidate() {
        let v = CvVolume {
            id: Some(12345),
            name: Some("Saga".into()),
            start_year: Some("2012".into()),
            publisher: Some(CvNamedRef {
                id: Some(99),
                name: Some("Image Comics".into()),
                site_detail_url: None,
            }),
            deck: Some("Sci-fi epic".into()),
            description: None,
            image: None,
            count_of_issues: Some(60),
            site_detail_url: Some("https://comicvine.gamespot.com/volume/4050-12345/".into()),
            date_last_updated: Some("2024-01-15 12:34:56".into()),
            aliases: None,
        };
        let c = cv_volume_to_candidate(&v).unwrap();
        assert_eq!(c.source, Source::ComicVine);
        assert_eq!(c.external_id, "12345");
        assert_eq!(c.name, "Saga");
        assert_eq!(c.year, Some(2012));
        assert_eq!(c.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(c.issue_count, Some(60));
        // WP-5.6: an ongoing name + non-"Collects" deck → no format.
        assert_eq!(c.format, None);
    }

    #[test]
    fn infers_cv_volume_format_from_name_and_deck() {
        let vol = |name: &str, deck: Option<&str>| CvVolume {
            id: Some(1),
            name: Some(name.into()),
            start_year: Some("2012".into()),
            publisher: None,
            deck: deck.map(str::to_owned),
            description: None,
            image: None,
            count_of_issues: None,
            site_detail_url: None,
            date_last_updated: None,
            aliases: None,
        };
        let c = cv_volume_to_candidate(&vol("Saga", Some("Collects #1-6."))).unwrap();
        assert_eq!(c.format.as_deref(), Some("TPB"));
        let c = cv_volume_to_candidate(&vol("X-Men Annual", None)).unwrap();
        assert_eq!(c.format.as_deref(), Some("Annual"));
        assert_eq!(
            cv_volume_to_candidate(&vol("Saga", None)).unwrap().format,
            None
        );
        // Matching only (owner decision): never written on apply.
        for (name, deck) in [("Saga", Some("Collects #1-6.")), ("X-Men Annual", None)] {
            let m = cv_volume_to_metadata(vol(name, deck));
            assert_eq!(m.format, None, "{name}");
            assert_eq!(m.series_type, None, "{name}");
        }
    }

    #[test]
    fn issue_metadata_explodes_multi_role_credits() {
        let issue = CvIssue {
            id: Some(1),
            name: None,
            issue_number: Some("1".into()),
            cover_date: None,
            store_date: None,
            deck: None,
            description: None,
            image: None,
            associated_images: None,
            person_credits: Some(vec![CvPersonCredit {
                id: Some(7),
                name: Some("Brian K. Vaughan".into()),
                role: Some("writer, cover".into()),
                site_detail_url: None,
            }]),
            character_credits: None,
            team_credits: None,
            location_credits: None,
            concept_credits: None,
            object_credits: None,
            story_arc_credits: None,
            first_appearance_characters: None,
            first_appearance_teams: None,
            first_appearance_locations: None,
            first_appearance_concepts: None,
            first_appearance_objects: None,
            first_appearance_storyarcs: None,
            characters_died_in: None,
            teams_disbanded_in: None,
            volume: None,
            site_detail_url: None,
            date_last_updated: None,
            aliases: None,
        };
        let m = cv_issue_to_metadata(issue);
        assert_eq!(m.credits.len(), 2);
        // Roles are canonicalized to the ComicInfo standard names —
        // CV's `"cover"` becomes `"CoverArtist"`. The composer's
        // per-role filter relies on this; assert it here so we catch
        // regressions in the mapping at the provider boundary.
        assert!(m.credits.iter().any(|c| c.role == "Writer"));
        assert!(m.credits.iter().any(|c| c.role == "CoverArtist"));
        assert!(m.credits.iter().all(|c| c.name == "Brian K. Vaughan"));
        // Both credits carry the CV person id.
        assert!(m.credits.iter().all(|c| c.identifiers.len() == 1));
    }

    /// Minimal CvIssue with everything empty — keeps the
    /// variant/first-appearance tests focused on the fields under test.
    fn empty_issue(id: i64) -> CvIssue {
        CvIssue {
            id: Some(id),
            name: None,
            issue_number: Some("1".into()),
            cover_date: None,
            store_date: None,
            deck: None,
            description: None,
            image: None,
            associated_images: None,
            person_credits: None,
            character_credits: None,
            team_credits: None,
            location_credits: None,
            concept_credits: None,
            object_credits: None,
            story_arc_credits: None,
            first_appearance_characters: None,
            first_appearance_teams: None,
            first_appearance_locations: None,
            first_appearance_concepts: None,
            first_appearance_objects: None,
            first_appearance_storyarcs: None,
            characters_died_in: None,
            teams_disbanded_in: None,
            volume: None,
            site_detail_url: None,
            date_last_updated: None,
            aliases: None,
        }
    }

    fn named(id: i64, name: &str) -> CvNamedRef {
        CvNamedRef {
            id: Some(id),
            name: Some(name.into()),
            site_detail_url: None,
        }
    }

    #[test]
    fn maps_associated_images_to_variants() {
        let mut issue = empty_issue(1);
        issue.associated_images = Some(vec![
            CvAltImage {
                original_url: Some("https://cdn/variant-a.jpg".into()),
                id: Some(501),
                caption: Some("  Cover B by Artist  ".into()),
                image_tags: Some("Cover".into()),
            },
            CvAltImage {
                // No usable URL → dropped.
                original_url: Some("   ".into()),
                id: Some(502),
                caption: None,
                image_tags: None,
            },
            CvAltImage {
                original_url: Some("https://cdn/variant-c.jpg".into()),
                id: None,
                caption: None,
                image_tags: None,
            },
        ]);
        let m = cv_issue_to_metadata(issue);
        assert_eq!(m.variants.len(), 2, "blank-URL variant should be dropped");
        let first = &m.variants[0];
        assert_eq!(
            first.image_url.as_deref(),
            Some("https://cdn/variant-a.jpg")
        );
        assert_eq!(first.label.as_deref(), Some("Cover B by Artist"));
        assert_eq!(first.identifiers.len(), 1);
        assert_eq!(first.identifiers[0].source, Source::ComicVine);
        assert_eq!(first.identifiers[0].id, "501");
        // URL-only variant with no CV id keeps the URL but carries no id.
        let second = &m.variants[1];
        assert_eq!(
            second.image_url.as_deref(),
            Some("https://cdn/variant-c.jpg")
        );
        assert!(second.identifiers.is_empty());
        assert!(second.label.is_none());
    }

    #[test]
    fn stamps_first_appearance_and_death_flags() {
        let mut issue = empty_issue(1);
        issue.character_credits = Some(vec![named(100, "Hazel"), named(101, "The Stalk")]);
        issue.team_credits = Some(vec![named(200, "The Heralds"), named(201, "Wreath")]);
        issue.location_credits = Some(vec![named(300, "Cleave")]);
        // Hazel is a first appearance; The Stalk dies here.
        issue.first_appearance_characters = Some(vec![named(100, "Hazel")]);
        issue.characters_died_in = Some(vec![named(101, "The Stalk")]);
        // The Heralds first appear; Wreath disbands here.
        issue.first_appearance_teams = Some(vec![named(200, "The Heralds")]);
        issue.teams_disbanded_in = Some(vec![named(201, "Wreath")]);
        issue.first_appearance_locations = Some(vec![named(300, "Cleave")]);

        let m = cv_issue_to_metadata(issue);

        let hazel = m.characters.iter().find(|c| c.name == "Hazel").unwrap();
        assert!(hazel.is_first_appearance);
        assert_eq!(hazel.died_in_issue, Some(false));
        let stalk = m.characters.iter().find(|c| c.name == "The Stalk").unwrap();
        assert!(!stalk.is_first_appearance);
        assert_eq!(stalk.died_in_issue, Some(true));

        let heralds = m.teams.iter().find(|t| t.name == "The Heralds").unwrap();
        assert!(heralds.is_first_appearance);
        assert_eq!(heralds.disbanded_in_issue, Some(false));
        let wreath = m.teams.iter().find(|t| t.name == "Wreath").unwrap();
        assert!(!wreath.is_first_appearance);
        assert_eq!(wreath.disbanded_in_issue, Some(true));

        let cleave = m.locations.iter().find(|l| l.name == "Cleave").unwrap();
        assert!(cleave.is_first_appearance);
    }
}
