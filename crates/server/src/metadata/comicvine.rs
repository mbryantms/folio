//! ComicVine API client (`comicvine.gamespot.com/api`).
//!
//! TOS: non-commercial only, attribution required, caching encouraged.
//! Rate: 200 req / resource / hour + ~1 req/sec velocity cap.
//!
//! Endpoints we use:
//! - `GET /search?resources=volume,issue,publisher&query=...` — keyword search.
//! - `GET /volumes?filter=name:...,start_year:...` — narrowed series search.
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
//! Velocity cap: a `Mutex<Instant>` tracks the last successful HTTP
//! request and sleeps the worker out to the per-second floor. Combined
//! with the per-hour Redis token bucket, this keeps us inside both the
//! velocity cap and the per-resource hour budget without coordinating
//! across instances.

use crate::metadata::cache;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::provider::{
    CreditCandidate, EntityCandidate, GenericMetadata, IssueCandidate, IssueQuery,
    MetadataProvider, ProviderError, ProviderResult, QuotaSnapshot, SeriesCandidate, SeriesQuery,
};
use crate::metadata::rate_limit::{self, BucketDef, Reservation};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use redis::aio::ConnectionManager;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// User-agent reported to CV — TOS asks for a unique identifier.
const USER_AGENT: &str = concat!("Folio/", env!("CARGO_PKG_VERSION"), " (+metadata-fetcher)");

/// Floor between successful API calls. ComicVine's documented rate
/// is "≤ 1 req/sec sustained"; we conservatively wait 1s + a small
/// jitter ceiling to absorb clock skew.
const VELOCITY_FLOOR: Duration = Duration::from_millis(1100);

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const SERIES_FIELDS: &str = "id,name,start_year,publisher,deck,description,image,count_of_issues,site_detail_url,date_last_updated,aliases";
const ISSUE_FIELDS: &str = "id,name,issue_number,cover_date,store_date,deck,description,image,person_credits,character_credits,team_credits,location_credits,concept_credits,object_credits,story_arc_credits,volume,site_detail_url,date_last_updated,aliases";

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
    /// Last successful HTTP request — used to enforce the 1 req/sec
    /// velocity cap. Held briefly to compute the sleep delta; never
    /// across the actual HTTP call itself.
    last_request: Mutex<Option<Instant>>,
    bucket: BucketDef,
}

impl ComicVineClient {
    /// Production constructor — points at `comicvine.gamespot.com/api`.
    pub fn new(api_key: String, redis: ConnectionManager) -> Self {
        Self::with_base_url(api_key, "https://comicvine.gamespot.com/api".to_owned(), redis)
    }

    /// Test constructor — points at an arbitrary base URL (wiremock).
    pub fn with_base_url(api_key: String, base_url: String, redis: ConnectionManager) -> Self {
        // Defense-in-depth trim: the overlay loader already strips
        // whitespace from the stored secret, but a stale value
        // written before that fix shipped (or any non-overlay caller)
        // shouldn't reach CV with a `?api_key=...%0A` URL.
        let api_key = api_key.trim().to_owned();
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("reqwest client init");
        Self {
            inner: Arc::new(Inner {
                api_key,
                base_url,
                http,
                redis,
                last_request: Mutex::new(None),
                bucket: rate_limit::COMICVINE_HOUR,
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
        let ttl = chrono::Duration::from_std(cache::CacheEntity::Series.default_ttl().to_std().unwrap()).unwrap_or(chrono::Duration::hours(168));
        if let Ok(Some(hit)) = cache::get(db, Source::ComicVine, cache::CacheEntity::Series, external_id, ttl).await {
            return Ok(hit);
        }
        let fresh = self.fetch_series(external_id).await?;
        let _ = cache::put(db, Source::ComicVine, cache::CacheEntity::Series, external_id, &fresh).await;
        Ok(fresh)
    }

    /// Same shape as [`fetch_series_cached`] for issue detail.
    pub async fn fetch_issue_cached(
        &self,
        db: &DatabaseConnection,
        external_id: &str,
    ) -> ProviderResult<GenericMetadata> {
        let ttl = chrono::Duration::from_std(cache::CacheEntity::Issue.default_ttl().to_std().unwrap()).unwrap_or(chrono::Duration::hours(24));
        if let Ok(Some(hit)) = cache::get(db, Source::ComicVine, cache::CacheEntity::Issue, external_id, ttl).await {
            return Ok(hit);
        }
        let fresh = self.fetch_issue(external_id).await?;
        let _ = cache::put(db, Source::ComicVine, cache::CacheEntity::Issue, external_id, &fresh).await;
        Ok(fresh)
    }

    async fn reserve_slot(&self) -> ProviderResult<()> {
        let mut redis = self.inner.redis.clone();
        match rate_limit::reserve(&mut redis, &self.inner.bucket).await {
            Ok(Reservation::Granted { .. }) => {}
            Ok(Reservation::Denied { retry_after_secs }) => {
                return Err(ProviderError::QuotaExceeded { retry_after_secs });
            }
            Err(e) => return Err(ProviderError::Transport(format!("redis: {e}"))),
        }
        let mut last = self.inner.last_request.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < VELOCITY_FLOOR {
                let wait = VELOCITY_FLOOR - elapsed;
                drop(last);
                tokio::time::sleep(wait).await;
                last = self.inner.last_request.lock().await;
            }
        }
        *last = Some(Instant::now());
        Ok(())
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        extra_query: &[(&str, String)],
    ) -> ProviderResult<T> {
        self.reserve_slot().await?;
        let url = format!("{}{}", self.inner.base_url, path);
        let mut req = self
            .inner
            .http
            .get(&url)
            .query(&[("api_key", &self.inner.api_key), ("format", &"json".to_owned())]);
        if !extra_query.is_empty() {
            req = req.query(extra_query);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        if !status.is_success() {
            // CV occasionally returns non-200 for 5xx; surface those
            // distinctly. 4xx (other than 429) we still try to parse
            // since the body usually carries a status_code envelope.
            if status.as_u16() == 429 {
                return Err(ProviderError::QuotaExceeded { retry_after_secs: 60 });
            }
            if status.is_server_error() {
                return Err(ProviderError::Upstream(format!("HTTP {status}: {body}")));
            }
        }
        // Parse the standard envelope first so we can map status_code
        // before the typed deserialize.
        let envelope: CvEnvelope<serde_json::Value> = serde_json::from_str(&body).map_err(|e| {
            ProviderError::InvalidResponse(format!("envelope parse: {e}; body={}", truncate(&body, 256)))
        })?;
        match envelope.status_code.unwrap_or(1) {
            1 => {}
            100 => return Err(ProviderError::Unauthorized(envelope.error.unwrap_or_default())),
            101 => return Err(ProviderError::NotFound(envelope.error.unwrap_or_default())),
            107 => return Err(ProviderError::QuotaExceeded { retry_after_secs: 3600 }),
            other => {
                return Err(ProviderError::Upstream(format!(
                    "ComicVine status_code={other}: {}",
                    envelope.error.unwrap_or_default()
                )));
            }
        }
        serde_json::from_str::<T>(&body)
            .map_err(|e| ProviderError::InvalidResponse(format!("typed parse: {e}")))
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

// ───────── CV envelope shapes ─────────

#[derive(Debug, Deserialize)]
struct CvEnvelope<T> {
    status_code: Option<i32>,
    error: Option<String>,
    results: Option<T>,
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
    person_credits: Option<Vec<CvPersonCredit>>,
    character_credits: Option<Vec<CvNamedRef>>,
    team_credits: Option<Vec<CvNamedRef>>,
    location_credits: Option<Vec<CvNamedRef>>,
    concept_credits: Option<Vec<CvNamedRef>>,
    object_credits: Option<Vec<CvNamedRef>>,
    story_arc_credits: Option<Vec<CvNamedRef>>,
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
        series_year: issue.volume.as_ref().and_then(|v| parse_year(&v.start_year)),
        series_external_id: issue.volume.as_ref().and_then(|v| v.id.map(|n| n.to_string())),
        cover_image_url: cover,
    })
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

fn cv_named_to_entity(
    n: &CvNamedRef,
    entity_type: &str,
) -> Option<EntityCandidate> {
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
                    vec![CreditCandidate { role: "unknown".into(), ..cc }]
                } else {
                    cc.role
                        .split(',')
                        .map(|r| CreditCandidate {
                            name: cc.name.clone(),
                            role: r.trim().to_lowercase(),
                            ordinal: cc.ordinal,
                            identifiers: cc.identifiers.clone(),
                        })
                        .collect()
                }
            })
        })
        .collect();
    let entities = |list: Option<&Vec<CvNamedRef>>, ty: &str| -> Vec<EntityCandidate> {
        list.into_iter()
            .flatten()
            .filter_map(|n| cv_named_to_entity(n, ty))
            .collect()
    };
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
        year_began: issue.volume.as_ref().and_then(|v| parse_year(&v.start_year)),
        publisher: issue
            .volume
            .as_ref()
            .and_then(|v| v.publisher.as_ref())
            .and_then(|p| p.name.clone()),
        credits,
        characters: entities(issue.character_credits.as_ref(), "character"),
        teams: entities(issue.team_credits.as_ref(), "team"),
        locations: entities(issue.location_credits.as_ref(), "location"),
        concepts: entities(issue.concept_credits.as_ref(), "concept"),
        objects: entities(issue.object_credits.as_ref(), "object"),
        story_arcs: entities(issue.story_arc_credits.as_ref(), "story_arc"),
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
        let (remaining, ttl) = rate_limit::snapshot(&mut redis, &self.inner.bucket)
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
        let mut filters = vec![format!("name:{}", query.name.replace(',', " "))];
        if let Some(year) = query.year {
            filters.push(format!("start_year:{year}"));
        }
        let envelope: CvEnvelope<Vec<CvVolume>> = self
            .request(
                "/volumes",
                &[
                    ("filter", filters.join(",")),
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
        let mut filters = vec![format!("issue_number:{}", query.issue_number)];
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
            // /search doesn't honour the filter param.
            out.retain(|i| {
                i.issue_number
                    .as_deref()
                    .map(|n| n == query.issue_number)
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

    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>> {
        // Cover URLs hit CV's CDN, not the API — no rate-limit slot
        // reserved.
        let resp = self
            .inner
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::Upstream(format!("cover HTTP {status}")));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(bytes.to_vec())
    }
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
            volume: None,
            site_detail_url: None,
            date_last_updated: None,
            aliases: None,
        };
        let m = cv_issue_to_metadata(issue);
        assert_eq!(m.credits.len(), 2);
        assert!(m.credits.iter().any(|c| c.role == "writer"));
        assert!(m.credits.iter().any(|c| c.role == "cover"));
        assert!(m.credits.iter().all(|c| c.name == "Brian K. Vaughan"));
        // Both credits carry the CV person id.
        assert!(m.credits.iter().all(|c| c.identifiers.len() == 1));
    }
}
