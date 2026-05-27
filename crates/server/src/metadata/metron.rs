//! Metron API client (`metron.cloud/api/`).
//!
//! Auth: HTTP Basic (free `metron.cloud` account; credentials are the
//! user's username + password, no API token in v1).
//! Rate: 30 req/min + 5,000 req/day (separate buckets, both gate every
//! outbound call — exhaustion of *either* denies).
//! License: CC-BY-NC-SA 4.0 (attribution required, non-commercial,
//! share-alike on derivatives — we never re-expose Metron data through
//! a public API).
//!
//! Endpoints we use:
//! - `GET /api/series/?name=...` — series search.
//! - `GET /api/series/{id}/` — series detail.
//! - `GET /api/issue/?series_id=...&number=...` — issue search.
//! - `GET /api/issue/{id}/` — issue detail.
//!
//! Response envelope (paged endpoints):
//! ```text
//! { count, next, previous, results: [...] }
//! ```
//! Detail endpoints return the entity object directly (no envelope).
//!
//! ## Cross-source IDs
//!
//! Metron stores native CV / GCD / Marvel / LoCG references on series +
//! issue rows (`cv_id`, `gcd_id`, `marvel_id`, `locg_id`). When a Metron
//! fetch returns these, the client populates corresponding `Identifier`
//! rows in `GenericMetadata.identifiers` so a single Metron call seeds
//! the `external_ids` table across every linked source for free — one
//! of the main reasons Metron is the preferred provider in the priority
//! list.

use crate::metadata::cache;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::provider::{
    CreditCandidate, EntityCandidate, GenericMetadata, IssueCandidate, IssueQuery,
    MetadataProvider, ProviderError, ProviderResult, QuotaSnapshot, ReprintCandidate,
    SeriesCandidate, SeriesQuery, VariantCoverCandidate,
};
use crate::metadata::rate_limit::{self, BucketDef, Reservation};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, NaiveDate, Utc};
use redis::aio::ConnectionManager;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = concat!("Folio/", env!("CARGO_PKG_VERSION"), " (+metadata-fetcher)");

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct MetronClient {
    inner: Arc<Inner>,
}

struct Inner {
    auth_header: String,
    base_url: String,
    http: reqwest::Client,
    redis: ConnectionManager,
    min_bucket: BucketDef,
    day_bucket: BucketDef,
}

impl MetronClient {
    pub fn new(username: &str, password: &str, redis: ConnectionManager) -> Self {
        Self::with_base_url(username, password, "https://metron.cloud".to_owned(), redis)
    }

    pub fn with_base_url(
        username: &str,
        password: &str,
        base_url: String,
        redis: ConnectionManager,
    ) -> Self {
        // Defense-in-depth trim — same paste-leak fix as the CV
        // client. Whitespace inside HTTP Basic credentials is base64-
        // encoded straight through and Metron rejects with 401.
        let username = username.trim();
        let password = password.trim();
        let creds = B64.encode(format!("{username}:{password}"));
        let auth_header = format!("Basic {creds}");
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("reqwest client init");
        Self {
            inner: Arc::new(Inner {
                auth_header,
                base_url,
                http,
                redis,
                min_bucket: rate_limit::METRON_MIN,
                day_bucket: rate_limit::METRON_DAY,
            }),
        }
    }

    pub async fn fetch_series_cached(
        &self,
        db: &DatabaseConnection,
        external_id: &str,
    ) -> ProviderResult<GenericMetadata> {
        let ttl =
            chrono::Duration::from_std(cache::CacheEntity::Series.default_ttl().to_std().unwrap())
                .unwrap_or(chrono::Duration::hours(168));
        if let Ok(Some(hit)) = cache::get(
            db,
            Source::Metron,
            cache::CacheEntity::Series,
            external_id,
            ttl,
        )
        .await
        {
            return Ok(hit);
        }
        let fresh = self.fetch_series(external_id).await?;
        let _ = cache::put(
            db,
            Source::Metron,
            cache::CacheEntity::Series,
            external_id,
            &fresh,
        )
        .await;
        Ok(fresh)
    }

    pub async fn fetch_issue_cached(
        &self,
        db: &DatabaseConnection,
        external_id: &str,
    ) -> ProviderResult<GenericMetadata> {
        let ttl =
            chrono::Duration::from_std(cache::CacheEntity::Issue.default_ttl().to_std().unwrap())
                .unwrap_or(chrono::Duration::hours(24));
        if let Ok(Some(hit)) = cache::get(
            db,
            Source::Metron,
            cache::CacheEntity::Issue,
            external_id,
            ttl,
        )
        .await
        {
            return Ok(hit);
        }
        let fresh = self.fetch_issue(external_id).await?;
        let _ = cache::put(
            db,
            Source::Metron,
            cache::CacheEntity::Issue,
            external_id,
            &fresh,
        )
        .await;
        Ok(fresh)
    }

    /// Reserve both rate-limit buckets atomically. Either denial floors
    /// the caller's retry-after to the longest applicable wait so an
    /// exhausted daily budget doesn't suggest a 60s retry.
    async fn reserve_slot(&self) -> ProviderResult<()> {
        let mut redis = self.inner.redis.clone();
        let min = rate_limit::reserve(&mut redis, &self.inner.min_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        if let Reservation::Denied { retry_after_secs } = min {
            return Err(ProviderError::QuotaExceeded { retry_after_secs });
        }
        let day = rate_limit::reserve(&mut redis, &self.inner.day_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        if let Reservation::Denied { retry_after_secs } = day {
            return Err(ProviderError::QuotaExceeded { retry_after_secs });
        }
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
            .header(reqwest::header::AUTHORIZATION, &self.inner.auth_header)
            .header(reqwest::header::ACCEPT, "application/json");
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
            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Unauthorized(truncate(&body, 256).to_owned()),
                404 => ProviderError::NotFound(truncate(&body, 256).to_owned()),
                429 => ProviderError::QuotaExceeded {
                    retry_after_secs: 60,
                },
                500..=599 => ProviderError::Upstream(format!("HTTP {status}")),
                _ => ProviderError::Upstream(format!("HTTP {status}: {}", truncate(&body, 256))),
            });
        }
        serde_json::from_str::<T>(&body)
            .map_err(|e| ProviderError::InvalidResponse(format!("typed parse: {e}")))
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

// ───────── Metron envelope shapes ─────────

#[derive(Debug, Deserialize)]
struct Paged<T> {
    #[allow(dead_code)]
    count: Option<u64>,
    #[allow(dead_code)]
    next: Option<String>,
    #[allow(dead_code)]
    previous: Option<String>,
    results: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct MNamedRef {
    id: Option<i64>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MSeriesType {
    #[allow(dead_code)]
    id: Option<i64>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MGenre {
    #[allow(dead_code)]
    id: Option<i64>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields preserved for future ranking / sort signals
struct MSeriesList {
    id: Option<i64>,
    series: Option<String>,
    year_began: Option<i32>,
    issue_count: Option<i32>,
    modified: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // issue_count not yet propagated to GenericMetadata; future use
struct MSeriesDetail {
    id: Option<i64>,
    name: Option<String>,
    sort_name: Option<String>,
    volume: Option<i32>,
    series_type: Option<MSeriesType>,
    publisher: Option<MNamedRef>,
    imprint: Option<MNamedRef>,
    year_began: Option<i32>,
    year_end: Option<i32>,
    desc: Option<String>,
    issue_count: Option<i32>,
    #[serde(default)]
    genres: Vec<MGenre>,
    #[serde(default)]
    associated: Vec<MNamedRef>,
    cv_id: Option<i64>,
    gcd_id: Option<i64>,
    #[serde(default)]
    resource_url: Option<String>,
    modified: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MIssueListItem {
    id: Option<i64>,
    series: Option<MIssueSeriesRef>,
    number: Option<String>,
    issue: Option<String>, // sometimes used by Metron list endpoints
    name: Option<Vec<String>>,
    cover_date: Option<String>,
    image: Option<String>,
    #[allow(dead_code)]
    modified: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MIssueSeriesRef {
    id: Option<i64>,
    name: Option<String>,
    sort_name: Option<String>,
    volume: Option<i32>,
    year_began: Option<i32>,
    series_type: Option<MSeriesType>,
    #[serde(default)]
    #[allow(dead_code)]
    genres: Vec<MGenre>,
}

#[derive(Debug, Deserialize)]
struct MCredit {
    #[allow(dead_code)]
    id: Option<i64>,
    creator: Option<String>,
    creator_id: Option<i64>,
    #[serde(default)]
    role: Vec<MNamedRef>,
}

#[derive(Debug, Deserialize)]
struct MVariant {
    name: Option<String>,
    sku: Option<String>,
    upc: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MReprint {
    id: Option<i64>,
    issue: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MIssueDetail {
    id: Option<i64>,
    publisher: Option<MNamedRef>,
    imprint: Option<MNamedRef>,
    series: Option<MIssueSeriesRef>,
    number: Option<String>,
    title: Option<String>,
    #[serde(default)]
    name: Vec<String>,
    cover_date: Option<String>,
    store_date: Option<String>,
    foc_date: Option<String>,
    price: Option<String>,
    rating: Option<MNamedRef>,
    sku: Option<String>,
    isbn: Option<String>,
    upc: Option<String>,
    page: Option<i32>,
    desc: Option<String>,
    image: Option<String>,
    #[allow(dead_code)]
    cover_hash: Option<String>,
    #[serde(default)]
    arcs: Vec<MNamedRef>,
    #[serde(default)]
    credits: Vec<MCredit>,
    #[serde(default)]
    characters: Vec<MNamedRef>,
    #[serde(default)]
    teams: Vec<MNamedRef>,
    #[serde(default)]
    universes: Vec<MNamedRef>,
    #[serde(default)]
    reprints: Vec<MReprint>,
    #[serde(default)]
    variants: Vec<MVariant>,
    cv_id: Option<i64>,
    gcd_id: Option<i64>,
    resource_url: Option<String>,
    modified: Option<String>,
}

// ───────── mapping helpers ─────────

fn parse_date(raw: &Option<String>) -> Option<NaiveDate> {
    let s = raw.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn parse_metron_timestamp(raw: &Option<String>) -> Option<DateTime<Utc>> {
    let s = raw.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    // Metron timestamps are ISO-8601 / RFC-3339 UTC.
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn parse_price(raw: &Option<String>) -> Option<f64> {
    raw.as_deref()?.trim().parse::<f64>().ok()
}

fn named_to_entity(r: &MNamedRef, entity_type: &str) -> Option<EntityCandidate> {
    let name = r.name.clone().filter(|s| !s.trim().is_empty())?;
    let identifiers = match r.id {
        Some(id) => vec![Identifier::with_canonical_url(
            Source::Metron,
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

fn credit_to_credit(c: &MCredit) -> Vec<CreditCandidate> {
    let Some(name) = c.creator.clone().filter(|s| !s.trim().is_empty()) else {
        return Vec::new();
    };
    let identifiers = match c.creator_id {
        Some(id) => vec![Identifier::with_canonical_url(
            Source::Metron,
            id.to_string(),
            "person",
        )],
        None => Vec::new(),
    };
    if c.role.is_empty() {
        return vec![CreditCandidate {
            name,
            role: "unknown".into(),
            ordinal: None,
            identifiers,
        }];
    }
    c.role
        .iter()
        .filter_map(|r| {
            let raw = r.name.as_deref()?.trim();
            if raw.is_empty() {
                return None;
            }
            // Canonicalize Metron's role tags (`"Artist"`, `"Cover"`,
            // …) onto the ComicInfo standard names so the composer's
            // per-role filter fires. See
            // [`crate::metadata::provider::canonicalize_role`] for the
            // full mapping rationale.
            let role = crate::metadata::provider::canonicalize_role(raw)
                .map(str::to_owned)
                .unwrap_or_else(|| raw.to_lowercase());
            Some(CreditCandidate {
                name: name.clone(),
                role,
                ordinal: None,
                identifiers: identifiers.clone(),
            })
        })
        .collect()
}

fn series_list_to_candidate(s: &MSeriesList) -> Option<SeriesCandidate> {
    let id = s.id?;
    let external_id = id.to_string();
    Some(SeriesCandidate {
        source: Source::Metron,
        external_id: external_id.clone(),
        external_url: crate::metadata::identifier::canonical_url(
            Source::Metron,
            "series",
            &external_id,
        ),
        name: s.series.clone().unwrap_or_default(),
        year: s.year_began,
        publisher: None,
        issue_count: s.issue_count,
        cover_image_url: None,
        deck: None,
    })
}

fn issue_list_to_candidate(i: &MIssueListItem) -> Option<IssueCandidate> {
    let id = i.id?;
    let external_id = id.to_string();
    let series_ref = i.series.as_ref();
    Some(IssueCandidate {
        source: Source::Metron,
        external_id: external_id.clone(),
        external_url: crate::metadata::identifier::canonical_url(
            Source::Metron,
            "issue",
            &external_id,
        ),
        issue_number: i.number.clone().or_else(|| i.issue.clone()),
        name: i.name.as_ref().and_then(|v| v.first().cloned()),
        cover_date: parse_date(&i.cover_date),
        series_name: series_ref.and_then(|s| s.name.clone()),
        series_year: series_ref.and_then(|s| s.year_began),
        series_external_id: series_ref.and_then(|s| s.id.map(|n| n.to_string())),
        cover_image_url: i.image.clone(),
    })
}

fn series_detail_to_metadata(s: MSeriesDetail) -> GenericMetadata {
    let external_id = s.id.map(|n| n.to_string()).unwrap_or_default();
    let mut identifiers = if external_id.is_empty() {
        Vec::new()
    } else {
        vec![Identifier::with_canonical_url(
            Source::Metron,
            external_id.clone(),
            "series",
        )]
    };
    if let Some(cv) = s.cv_id {
        identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            cv.to_string(),
            "series",
        ));
    }
    if let Some(gcd) = s.gcd_id {
        identifiers.push(Identifier::with_canonical_url(
            Source::Gcd,
            gcd.to_string(),
            "series",
        ));
    }
    GenericMetadata {
        series_name: s.name,
        series_sort_name: s.sort_name,
        series_type: s.series_type.and_then(|t| t.name),
        volume: s.volume,
        year_began: s.year_began,
        year_end: s.year_end,
        description: s.desc,
        publisher: s.publisher.as_ref().and_then(|p| p.name.clone()),
        imprint: s.imprint.as_ref().and_then(|p| p.name.clone()),
        aliases: s.associated.iter().filter_map(|a| a.name.clone()).collect(),
        genres: s.genres.into_iter().filter_map(|g| g.name).collect(),
        identifiers,
        source_provider: Some(Source::Metron),
        source_external_id: if external_id.is_empty() {
            None
        } else {
            Some(external_id)
        },
        source_url: s.resource_url,
        fetched_at: Some(Utc::now()),
        upstream_modified_at: parse_metron_timestamp(&s.modified),
        ..Default::default()
    }
}

fn issue_detail_to_metadata(i: MIssueDetail) -> GenericMetadata {
    let external_id = i.id.map(|n| n.to_string()).unwrap_or_default();
    let mut identifiers = if external_id.is_empty() {
        Vec::new()
    } else {
        vec![Identifier::with_canonical_url(
            Source::Metron,
            external_id.clone(),
            "issue",
        )]
    };
    if let Some(cv) = i.cv_id {
        identifiers.push(Identifier::with_canonical_url(
            Source::ComicVine,
            cv.to_string(),
            "issue",
        ));
    }
    if let Some(gcd) = i.gcd_id {
        identifiers.push(Identifier::with_canonical_url(
            Source::Gcd,
            gcd.to_string(),
            "issue",
        ));
    }
    if let Some(isbn) = i.isbn.as_deref().filter(|s| !s.trim().is_empty()) {
        identifiers.push(Identifier::new(Source::Isbn, isbn.trim().to_owned()));
    }
    if let Some(upc) = i.upc.as_deref().filter(|s| !s.trim().is_empty()) {
        identifiers.push(Identifier::new(Source::Upc, upc.trim().to_owned()));
    }

    let series_ref = i.series.as_ref();
    let credits = i.credits.iter().flat_map(credit_to_credit).collect();
    let characters = i
        .characters
        .iter()
        .filter_map(|n| named_to_entity(n, "character"))
        .collect();
    let teams = i
        .teams
        .iter()
        .filter_map(|n| named_to_entity(n, "team"))
        .collect();
    let universes = i
        .universes
        .iter()
        .filter_map(|n| named_to_entity(n, "universe"))
        .collect();
    let story_arcs = i
        .arcs
        .iter()
        .filter_map(|n| named_to_entity(n, "story_arc"))
        .collect();
    let reprints = i
        .reprints
        .iter()
        .filter_map(|r| {
            let label = r.issue.clone()?;
            let identifiers =
                r.id.map(|id| {
                    vec![Identifier::with_canonical_url(
                        Source::Metron,
                        id.to_string(),
                        "issue",
                    )]
                })
                .unwrap_or_default();
            Some(ReprintCandidate { label, identifiers })
        })
        .collect();
    let variants = i
        .variants
        .iter()
        .map(|v| VariantCoverCandidate {
            label: v.name.clone(),
            artist_name: None,
            identifiers: {
                let mut ids = Vec::new();
                if let Some(sku) = v.sku.as_deref().filter(|s| !s.trim().is_empty()) {
                    // Metron's `sku` is a publisher catalog number, not
                    // an Identifier source we model — skip for now and
                    // surface in the variant_label via the UI.
                    let _ = sku;
                }
                if let Some(upc) = v.upc.as_deref().filter(|s| !s.trim().is_empty()) {
                    ids.push(Identifier::new(Source::Upc, upc.trim().to_owned()));
                }
                ids
            },
            image_url: v.image.clone(),
        })
        .collect();

    GenericMetadata {
        title: i.title.or_else(|| i.name.first().cloned()),
        issue_number: i.number,
        cover_date: parse_date(&i.cover_date),
        store_date: parse_date(&i.store_date),
        foc_date: parse_date(&i.foc_date),
        description: i.desc,
        cover_image_url: i.image,
        page_count: i.page,
        age_rating: i.rating.and_then(|r| r.name),
        price: parse_price(&i.price),
        sku: i.sku,
        publisher: i.publisher.as_ref().and_then(|p| p.name.clone()),
        imprint: i.imprint.as_ref().and_then(|p| p.name.clone()),
        series_name: series_ref.and_then(|s| s.name.clone()),
        series_sort_name: series_ref.and_then(|s| s.sort_name.clone()),
        volume: series_ref.and_then(|s| s.volume),
        year_began: series_ref.and_then(|s| s.year_began),
        series_type: series_ref
            .and_then(|s| s.series_type.as_ref())
            .and_then(|t| t.name.clone()),
        credits,
        characters,
        teams,
        universes,
        story_arcs,
        reprints,
        variants,
        identifiers,
        source_provider: Some(Source::Metron),
        source_external_id: if external_id.is_empty() {
            None
        } else {
            Some(external_id)
        },
        source_url: i.resource_url,
        fetched_at: Some(Utc::now()),
        upstream_modified_at: parse_metron_timestamp(&i.modified),
        ..Default::default()
    }
}

// ───────── Trait impl ─────────

#[async_trait]
impl MetadataProvider for MetronClient {
    fn id(&self) -> Source {
        Source::Metron
    }

    async fn health_check(&self) -> ProviderResult<QuotaSnapshot> {
        // Metron's lightest authenticated endpoint is a 1-result series
        // list with a no-match name. Exercises auth + parsing without
        // pulling much over the wire.
        let _: Paged<MSeriesList> = self
            .request(
                "/api/series/",
                &[
                    ("name", "__folio_health_check__".to_owned()),
                    ("page_size", "1".to_owned()),
                ],
            )
            .await?;
        self.quota().await
    }

    async fn quota(&self) -> ProviderResult<QuotaSnapshot> {
        let mut redis = self.inner.redis.clone();
        let (min_remaining, min_ttl) = rate_limit::snapshot(&mut redis, &self.inner.min_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        let (day_remaining, day_ttl) = rate_limit::snapshot(&mut redis, &self.inner.day_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        // Report whichever budget is the tighter constraint as the
        // headline "seconds until reset".
        let seconds_until_reset = if min_ttl == 0 && day_ttl == 0 {
            Some(0)
        } else if min_ttl == 0 {
            Some(day_ttl)
        } else if day_ttl == 0 {
            Some(min_ttl)
        } else {
            Some(min_ttl.min(day_ttl))
        };
        Ok(QuotaSnapshot {
            provider: Source::Metron,
            remaining_hour: Some(min_remaining),
            remaining_day: Some(day_remaining),
            seconds_until_reset,
        })
    }

    async fn search_series(&self, query: &SeriesQuery) -> ProviderResult<Vec<SeriesCandidate>> {
        let mut params = vec![("name", query.name.clone())];
        if let Some(year) = query.year {
            params.push(("year_began", year.to_string()));
        }
        params.push(("page_size", query.limit.clamp(1, 100).to_string()));
        let envelope: Paged<MSeriesList> = self.request("/api/series/", &params).await?;
        Ok(envelope
            .results
            .iter()
            .filter_map(series_list_to_candidate)
            .collect())
    }

    async fn search_issue(&self, query: &IssueQuery) -> ProviderResult<Vec<IssueCandidate>> {
        let mut params = vec![("number", query.issue_number.clone())];
        if let Some(ref vol) = query.series_external_id {
            params.push(("series_id", vol.clone()));
        } else if let Some(ref name) = query.series_name {
            params.push(("series_name", name.clone()));
        }
        if let Some(year) = query.cover_year {
            // Metron filters issues by cover_year (4-digit).
            params.push(("cover_year", year.to_string()));
        }
        params.push(("page_size", query.limit.clamp(1, 100).to_string()));
        let envelope: Paged<MIssueListItem> = self.request("/api/issue/", &params).await?;
        Ok(envelope
            .results
            .iter()
            .filter_map(issue_list_to_candidate)
            .collect())
    }

    async fn fetch_series(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let detail: MSeriesDetail = self
            .request(&format!("/api/series/{external_id}/"), &[])
            .await?;
        Ok(series_detail_to_metadata(detail))
    }

    async fn fetch_issue(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let detail: MIssueDetail = self
            .request(&format!("/api/issue/{external_id}/"), &[])
            .await?;
        Ok(issue_detail_to_metadata(detail))
    }

    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>> {
        // Cover URLs hit the static.metron.cloud CDN — no auth, no
        // rate-limit slot.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn series_detail_promotes_cv_and_gcd_ids() {
        let raw = json!({
            "id": 123,
            "name": "Saga",
            "sort_name": "Saga",
            "volume": 1,
            "series_type": {"id": 1, "name": "Ongoing Series"},
            "publisher": {"id": 5, "name": "Image Comics"},
            "imprint": null,
            "year_began": 2012,
            "year_end": null,
            "desc": "Sci-fi epic.",
            "issue_count": 60,
            "genres": [{"id": 1, "name": "Science-Fiction"}],
            "associated": [{"id": 9, "name": "Saga: Compendium"}],
            "cv_id": 12345,
            "gcd_id": 98765,
            "resource_url": "https://metron.cloud/series/saga-2012/",
            "modified": "2024-01-15T12:34:56.123456Z"
        });
        let detail: MSeriesDetail = serde_json::from_value(raw).unwrap();
        let m = series_detail_to_metadata(detail);
        assert_eq!(m.series_name.as_deref(), Some("Saga"));
        assert_eq!(m.year_began, Some(2012));
        assert_eq!(m.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(m.aliases, vec!["Saga: Compendium"]);
        // 3 identifiers: Metron self + CV + GCD.
        assert_eq!(m.identifiers.len(), 3);
        let by_source = |s: Source| m.identifiers.iter().find(|i| i.source == s);
        assert_eq!(
            by_source(Source::Metron).map(|i| i.id.as_str()),
            Some("123")
        );
        assert_eq!(
            by_source(Source::ComicVine).map(|i| i.id.as_str()),
            Some("12345")
        );
        assert_eq!(by_source(Source::Gcd).map(|i| i.id.as_str()), Some("98765"));
    }

    #[test]
    fn issue_detail_explodes_multi_role_credits_and_carries_barcodes() {
        let raw = json!({
            "id": 456,
            "publisher": {"id": 5, "name": "Image Comics"},
            "imprint": null,
            "series": {
                "id": 123,
                "name": "Saga",
                "sort_name": "Saga",
                "volume": 1,
                "year_began": 2012,
                "series_type": {"id": 1, "name": "Ongoing Series"},
                "genres": []
            },
            "number": "1",
            "title": "Chapter One",
            "name": ["Chapter One"],
            "cover_date": "2012-03-14",
            "store_date": "2012-03-14",
            "foc_date": null,
            "price": "2.99",
            "rating": {"id": 1, "name": "Teen Plus"},
            "sku": "JAN120494",
            "isbn": "",
            "upc": "75960608437600111",
            "page": 36,
            "desc": "Premiere issue.",
            "image": "https://static.metron.cloud/saga-1.jpg",
            "cover_hash": "abc",
            "arcs": [{"id": 10, "name": "Beginning"}],
            "credits": [
                {"id": 1, "creator": "Brian K. Vaughan", "creator_id": 7, "role": [{"id": 1, "name": "Writer"}, {"id": 9, "name": "Cover"}]},
                {"id": 2, "creator": "Fiona Staples", "creator_id": 8, "role": [{"id": 2, "name": "Artist"}]}
            ],
            "characters": [{"id": 100, "name": "Hazel"}],
            "teams": [],
            "universes": [],
            "reprints": [],
            "variants": [],
            "cv_id": 67890,
            "gcd_id": 11111,
            "resource_url": "https://metron.cloud/issue/saga-1-2012/",
            "modified": "2024-02-20T08:00:00Z"
        });
        let detail: MIssueDetail = serde_json::from_value(raw).unwrap();
        let m = issue_detail_to_metadata(detail);
        assert_eq!(m.issue_number.as_deref(), Some("1"));
        assert_eq!(m.title.as_deref(), Some("Chapter One"));
        assert_eq!(m.price, Some(2.99));
        assert_eq!(m.page_count, Some(36));
        assert_eq!(m.age_rating.as_deref(), Some("Teen Plus"));
        assert_eq!(m.series_name.as_deref(), Some("Saga"));
        assert_eq!(m.volume, Some(1));
        // 3 credits: BKV's two roles ("Writer" + "Cover") exploded
        // plus Fiona's single "Artist" role. Roles are canonicalized
        // onto the ComicInfo standard names (`Cover`/`Artist` →
        // `CoverArtist`/`Penciller`) at the provider boundary.
        assert_eq!(m.credits.len(), 3);
        assert!(m.credits.iter().any(|c| c.role == "Writer"));
        assert!(m.credits.iter().any(|c| c.role == "CoverArtist"));
        assert!(m.credits.iter().any(|c| c.role == "Penciller"));
        // Identifiers: Metron self + CV + GCD + UPC (ISBN empty → skipped).
        let ids: Vec<_> = m.identifiers.iter().map(|i| i.source).collect();
        assert!(ids.contains(&Source::Metron));
        assert!(ids.contains(&Source::ComicVine));
        assert!(ids.contains(&Source::Gcd));
        assert!(ids.contains(&Source::Upc));
        assert!(!ids.contains(&Source::Isbn));
        assert_eq!(m.characters.len(), 1);
        assert_eq!(m.story_arcs.len(), 1);
    }

    #[test]
    fn parses_metron_rfc3339_timestamps() {
        let ts = parse_metron_timestamp(&Some("2024-01-15T12:34:56Z".into()));
        assert!(ts.is_some());
        assert!(parse_metron_timestamp(&Some("nonsense".into())).is_none());
    }

    #[test]
    fn parses_metron_price_strings() {
        assert_eq!(parse_price(&Some("2.99".into())), Some(2.99));
        assert_eq!(parse_price(&Some("4".into())), Some(4.0));
        assert!(parse_price(&Some("".into())).is_none());
        assert!(parse_price(&None).is_none());
    }
}
