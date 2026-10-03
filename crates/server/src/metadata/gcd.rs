//! Grand Comics Database client (`www.comics.org/api/`) — roadmap WP-6.1,
//! full-API pass in `feat/gcd-full-api`.
//!
//! The third [`MetadataProvider`], behind ComicVine and Metron in
//! priority. GCD's value is coverage the other two lack: Golden/Silver
//! Age runs, non-US editions, and per-story credits.
//!
//! ## API shape (verified against the published OpenAPI schema at
//! `/api/schema/?format=json` and live responses, 2026-10-01)
//!
//! - `GET /api/series/name/{name}/[year/{year}/]` — series search
//!   (`name__icontains`, exact `year_began`), paged 50. Items are full
//!   `Series` objects, including `active_issues` + `issue_descriptors`,
//!   so a search result already carries its issue index.
//! - `GET /api/series/{id}/` — series detail (same shape).
//! - `GET /api/series/{id}/overview/` — paged 50, one row per
//!   **non-variant** issue in sort order: `issue_id`, `descriptor`,
//!   `number`, the three dates, `cover_url` and the issue's
//!   `longest_story` (a full `Story` object). The narrowed issue search
//!   reads it instead of hydrating issue details.
//! - `GET /api/series/name/{name}/issue/{number}/[year/{year}/]` — issue
//!   search (`series__name__icontains`, exact `number`,
//!   `key_date__startswith=year`). Returns the slim `IssueOnly` shape
//!   (descriptor, publication_date, price, page_count, variant_of,
//!   series — no number, title, key date or cover).
//! - `GET /api/issue/{id}/` — issue detail with `story_set[]` (per-story
//!   credits/characters/genre/synopsis) and a `cover` image URL.
//! - `GET /api/publisher/{id}/` — publisher detail. Series payloads only
//!   carry the publisher's API URL, so the name costs one extra request;
//!   it is cached in Redis (see [`SUMMARY_TTL_SECS`]).
//! - Not used: `/api/series/`, `/api/publisher/` (whole-catalog crawls)
//!   and `/api/issue/on_sale_weekly/{year}/week/{week}/` (a global
//!   new-release feed with no Folio consumer). See
//!   `docs/dev/metadata-providers.md` § GCD for the field-by-field audit.
//!
//! Relations are hyperlinks (`"series": "https://…/api/series/1482/?format=json"`);
//! ids are recovered from those URLs.
//!
//! ## Request economy
//!
//! Three Redis caches keep the request count down: the series summary +
//! publisher name (7 days), the series **issue index** (`id`, `number`,
//! `descriptor` per active issue — 24 h, filled for free from search
//! results and series details), and **overview pages** (24 h). A
//! narrowed issue search estimates which overview page holds the issue
//! from the index, so matching a whole series costs ~1 request per 50
//! issues instead of 2–5 per issue.
//!
//! ## Tolerance
//!
//! GCD declares the API's fields **unstable**. Every payload is parsed as
//! an untyped [`serde_json::Value`] and read through alias lists
//! ([`str_field`] and friends): a missing, renamed (to a known alias) or
//! re-typed field degrades to `None` instead of failing the search. Only
//! a body that isn't JSON at all is an [`ProviderError::InvalidResponse`];
//! a list item with no recoverable id is skipped, not fatal.
//!
//! ## Auth + rate
//!
//! The API is readable anonymously, but GCD throttles anonymous clients
//! to **30 requests/hour** and authenticated (HTTP Basic, a free
//! comics.org account) clients to **2,000/day** (`DEFAULT_THROTTLE_RATES`
//! in the upstream `settings.py`). Folio requires the account
//! credentials so the provider runs on the user tier, and paces itself
//! with two local buckets — ~100/hour ([`rate_limit::GCD_HOUR`]) and
//! 2,000/day ([`rate_limit::GCD_DAY`]) — plus a 1 req/s velocity floor.
//! GCD sends no budget headers; the admin budget bar is derived from the
//! local day bucket. A 429 carries DRF's `Retry-After`, which
//! [`http::retry_after_secs`] honours.
//!
//! ## Covers
//!
//! Every image URL is on `files1.comics.org`, which answers non-browser
//! requests (and browser hotlinks) with a Cloudflare challenge. Folio
//! keeps the URLs as data but never tries to pass the challenge:
//! [`crate::metadata::cover_block`] remembers the host after the first
//! challenged response, cover hashing treats GCD candidates as cover-less
//! and "Apply cover" records `cover_unavailable` instead of failing.
//!
//! ## License
//!
//! GCD data is CC BY-SA 4.0; the canonical comics.org links Folio
//! stores in `external_ids` drive the attribution footer
//! (`<SourcesFooter>`).

use crate::config::Config;
use crate::metadata::budget;
use crate::metadata::http;
use crate::metadata::identifier::{Identifier, Source, canonical_url};
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::provider::{
    CreditCandidate, EntityCandidate, GenericMetadata, IssueCandidate, IssueQuery,
    MetadataProvider, ProviderError, ProviderResult, QuotaSnapshot, SeriesCandidate, SeriesQuery,
    VariantCoverCandidate,
};
use crate::metadata::rate_limit::{self, BucketDef, Reservation};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chrono::{NaiveDate, Utc};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const USER_AGENT: &str = crate::build_info::USER_AGENT_METADATA;

const DEFAULT_BASE_URL: &str = "https://www.comics.org";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Wall-clock budget for one logical API call including retries (same
/// figure as the Metron client).
const REQUEST_DEADLINE: Duration = Duration::from_secs(75);

/// GCD asks API clients to stay at or under one request per second.
const VELOCITY_FLOOR: Duration = Duration::from_millis(1100);

/// How long a series summary / publisher name stays in Redis. Both are
/// near-immutable on GCD and every issue fetch needs them, so caching
/// is what keeps an issue apply at one request instead of three.
pub const SUMMARY_TTL_SECS: u64 = 7 * 24 * 3600;

/// How long a series issue index and its overview pages stay in Redis.
/// Shorter than the summary: indexers add issues and variants daily.
pub const INDEX_TTL_SECS: u64 = 24 * 3600;

/// DRF page size of every paginated GCD route.
pub const PAGE_SIZE: usize = 50;

/// Hard cap on pages walked by one search route (series name, issue
/// name/number). Page 2 is only fetched when page 1 had no exact-name
/// hit, so the common case stays at one request per route.
pub const SEARCH_PAGE_CAP: u32 = 2;

/// Overview pages probed per narrowed issue search: the estimated page,
/// then its neighbours.
const OVERVIEW_PROBE_CAP: usize = 3;

/// Issue details hydrated when the overview can't place an issue
/// (overview route missing/renamed) — the pre-overview behaviour.
const FALLBACK_HYDRATE_CAP: usize = 2;

/// Same-number sibling details fetched per issue apply to collect its
/// variant covers (GCD models a variant as its own issue whose
/// `variant_of` points at the base).
pub const VARIANT_DETAIL_CAP: usize = 3;

/// Variant collection is a nice-to-have (GCD covers can't even be
/// downloaded): it only runs while at least this many of the hourly
/// [`rate_limit::GCD_HOUR`] requests remain, so a bulk apply drains the
/// budget into searches + issue details first. Fantastic Four (1961)
/// has newsstand/direct/price variants on 374 of 416 numbers.
pub const VARIANT_BUDGET_FLOOR: u32 = 50;

/// HTTP Basic credentials for a comics.org account.
#[derive(Clone, Debug)]
pub struct GcdCredentials {
    pub username: String,
    pub password: String,
}

impl GcdCredentials {
    /// Username + password when both are set (trimmed — a pasted value
    /// commonly drags a trailing newline), else `None` (unconfigured).
    pub fn from_config(cfg: &Config) -> Option<Self> {
        let username = cfg
            .gcd_username
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let password = cfg
            .gcd_password
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        Some(Self {
            username: username.to_owned(),
            password: password.to_owned(),
        })
    }

    fn header_value(&self) -> String {
        let creds = B64.encode(format!("{}:{}", self.username.trim(), self.password.trim()));
        format!("Basic {creds}")
    }
}

#[derive(Clone)]
pub struct GcdClient {
    inner: Arc<Inner>,
}

struct Inner {
    auth_header: String,
    base_url: String,
    http: reqwest::Client,
    redis: ConnectionManager,
    hour_bucket: BucketDef,
    day_bucket: BucketDef,
    /// Last request start — enforces [`VELOCITY_FLOOR`]. Held only to
    /// compute the sleep, never across the HTTP call.
    last_request: Mutex<Option<Instant>>,
}

/// The slice of a GCD series every issue mapping needs, cached in Redis
/// under `metadata:gcd:series:<id>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SeriesSummary {
    pub name: Option<String>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    pub publishing_format: Option<String>,
    pub binding: Option<String>,
    pub language: Option<String>,
}

/// One active issue of a series, from the parallel
/// `active_issues` / `issue_descriptors` lists. Cached per series under
/// `metadata:gcd:series_index:<id>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct IndexEntry {
    pub id: String,
    /// Canonical-ish number parsed from the descriptor (`"1"`), empty
    /// when the lists were misaligned.
    pub number: String,
    /// Raw descriptor (`"1 [Cover B]"`), kept for variant ordering.
    pub descriptor: String,
}

impl GcdClient {
    pub fn new(username: &str, password: &str, redis: ConnectionManager) -> Self {
        Self::with_base_url(username, password, DEFAULT_BASE_URL.to_owned(), redis)
    }

    pub fn with_base_url(
        username: &str,
        password: &str,
        base_url: String,
        redis: ConnectionManager,
    ) -> Self {
        Self::with_credentials(
            GcdCredentials {
                username: username.to_owned(),
                password: password.to_owned(),
            },
            base_url,
            redis,
        )
    }

    /// Build from the runtime config. `None` when the credential pair is
    /// incomplete. Doesn't consult `gcd_enabled` — that gate belongs to
    /// the caller so the admin "Test" button can exercise a
    /// disabled-but-configured provider.
    pub fn from_config(cfg: &Config, redis: ConnectionManager) -> Option<Self> {
        let creds = GcdCredentials::from_config(cfg)?;
        let base_url = cfg
            .gcd_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        Some(Self::with_credentials(creds, base_url, redis))
    }

    pub fn with_credentials(
        creds: GcdCredentials,
        base_url: String,
        redis: ConnectionManager,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                auth_header: creds.header_value(),
                base_url: base_url.trim_end_matches('/').to_owned(),
                http: http::build_client(USER_AGENT, DEFAULT_TIMEOUT),
                redis,
                hour_bucket: rate_limit::GCD_HOUR,
                day_bucket: rate_limit::GCD_DAY,
                last_request: Mutex::new(None),
            }),
        }
    }

    /// Reserve both buckets, then sleep out the velocity floor.
    async fn reserve_slot(&self) -> ProviderResult<()> {
        let mut redis = self.inner.redis.clone();
        for bucket in [&self.inner.hour_bucket, &self.inner.day_bucket] {
            match rate_limit::reserve(&mut redis, bucket).await {
                Ok(Reservation::Granted { .. }) => {}
                Ok(Reservation::Denied { retry_after_secs }) => {
                    return Err(ProviderError::QuotaExceeded { retry_after_secs });
                }
                Err(e) => return Err(ProviderError::Transport(format!("redis: {e}"))),
            }
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

    /// Build `<base>/api/<segments…>/` with each segment percent-encoded
    /// as a single path segment (series names carry spaces, `&`, `?`).
    /// A `/` can't be sent at all — GCD's Apache front end 404s an
    /// encoded slash — so callers pass names through [`search_name`].
    fn api_url(&self, segments: &[&str]) -> ProviderResult<url::Url> {
        let mut url = url::Url::parse(&self.inner.base_url)
            .map_err(|e| ProviderError::Transport(format!("bad GCD base URL: {e}")))?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| ProviderError::Transport("GCD base URL cannot be a base".into()))?;
            path.pop_if_empty().push("api");
            for s in segments {
                path.push(s);
            }
            // Trailing slash — Django's router requires it.
            path.push("");
        }
        Ok(url)
    }

    async fn get_json(&self, segments: &[&str]) -> ProviderResult<Value> {
        self.get_json_page(segments, 1).await
    }

    /// One authenticated GET returning the parsed JSON body. Bucket
    /// reservation → retrying send → status classification; the last
    /// error / clear is recorded in Redis for the admin card. `page > 1`
    /// adds DRF's `page` query parameter.
    async fn get_json_page(&self, segments: &[&str], page: u32) -> ProviderResult<Value> {
        self.reserve_slot().await?;
        let url = self.api_url(segments)?;
        let opts = http::RequestOpts {
            deadline: Some(Instant::now() + REQUEST_DEADLINE),
            ..Default::default()
        };
        let mut query: Vec<(&str, String)> = vec![("format", "json".to_owned())];
        if page > 1 {
            query.push(("page", page.to_string()));
        }
        let build = || {
            self.inner
                .http
                .get(url.clone())
                .query(&query)
                .header(AUTHORIZATION, &self.inner.auth_header)
                .header(ACCEPT, "application/json")
        };
        let resp = match http::send_with_retry(build, &opts, &|s| s.to_owned()).await {
            Ok(resp) => resp,
            Err(e) => {
                budget::record_error(&self.inner.redis, Source::Gcd, &e.to_string()).await;
                return Err(e);
            }
        };
        let status = resp.status;
        if status.is_success() {
            return match serde_json::from_slice::<Value>(&resp.body) {
                Ok(v) => {
                    budget::clear_error(&self.inner.redis, Source::Gcd).await;
                    Ok(v)
                }
                Err(e) => {
                    let err = ProviderError::InvalidResponse(format!("GCD body is not JSON: {e}"));
                    budget::record_error(&self.inner.redis, Source::Gcd, &err.to_string()).await;
                    Err(err)
                }
            };
        }
        let err = match status.as_u16() {
            401 | 403 => ProviderError::Unauthorized(resp.snippet(256)),
            404 => ProviderError::NotFound(resp.snippet(256)),
            429 => ProviderError::QuotaExceeded {
                retry_after_secs: http::retry_after_secs(
                    &resp.headers,
                    http::DEFAULT_RETRY_AFTER_SECS,
                ),
            },
            _ => ProviderError::Upstream(format!("HTTP {status}: {}", resp.snippet(256))),
        };
        // A 404 past page 1 is DRF's expected "invalid page" (an
        // overview probe ran off the end), not a provider fault worth
        // surfacing on the admin card.
        if !(page > 1 && matches!(err, ProviderError::NotFound(_))) {
            budget::record_error(&self.inner.redis, Source::Gcd, &err.to_string()).await;
        }
        Err(err)
    }

    /// Walk a paginated route from page 1, collecting `results` items,
    /// until `next` is empty, `stop(items so far)` says enough, or
    /// `max_pages` is hit. A 404 or quota error past page 1 ends the
    /// walk with what was collected; on page 1 it propagates.
    async fn walk_pages(
        &self,
        segments: &[&str],
        max_pages: u32,
        stop: &(dyn Fn(&[Value]) -> bool + Send + Sync),
    ) -> ProviderResult<Vec<Value>> {
        let mut items: Vec<Value> = Vec::new();
        for page in 1..=max_pages.max(1) {
            let body = match self.get_json_page(segments, page).await {
                Ok(b) => b,
                Err(e @ (ProviderError::NotFound(_) | ProviderError::QuotaExceeded { .. }))
                    if page > 1 =>
                {
                    tracing::debug!(page, error = %e, "gcd: pagination stopped early");
                    break;
                }
                Err(e) => return Err(e),
            };
            items.extend(result_items(&body).into_iter().cloned());
            if !has_next_page(&body) || stop(&items) {
                break;
            }
        }
        Ok(items)
    }

    async fn cache_get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let mut conn = self.inner.redis.clone();
        let raw: Option<String> = conn.get(key).await.ok().flatten();
        raw.and_then(|s| serde_json::from_str(&s).ok())
    }

    async fn cache_put<T: Serialize>(&self, key: &str, value: &T, ttl_secs: u64) {
        let Ok(raw) = serde_json::to_string(value) else {
            return;
        };
        let mut conn = self.inner.redis.clone();
        let _: Result<(), _> = conn.set_ex(key, raw, ttl_secs).await;
    }

    /// Publisher display name for a GCD publisher id, Redis-cached.
    /// Best-effort: any failure yields `None` (the field just stays
    /// empty) rather than failing the detail fetch that needed it.
    async fn publisher_name(&self, publisher_id: &str) -> Option<String> {
        if let Some(name) = self.cached_publisher_name(publisher_id).await {
            return Some(name);
        }
        let v = self.get_json(&["publisher", publisher_id]).await.ok()?;
        let name = str_field(&v, &["name", "publisher_name", "title"])?;
        self.cache_put(
            &format!("metadata:gcd:publisher:{publisher_id}"),
            &name,
            SUMMARY_TTL_SECS,
        )
        .await;
        Some(name)
    }

    /// Cache-only publisher lookup (never spends a request) — used to
    /// decorate search candidates.
    async fn cached_publisher_name(&self, publisher_id: &str) -> Option<String> {
        self.cache_get::<String>(&format!("metadata:gcd:publisher:{publisher_id}"))
            .await
    }

    /// Resolve the `publisher` field of a series payload: an inline name
    /// or `{name}` object is used directly; a hyperlink costs one
    /// (cached) publisher fetch.
    async fn resolve_publisher(&self, v: &Value) -> Option<String> {
        match pick(v, &["publisher", "publisher_url"])? {
            Value::Object(_) => str_field(pick(v, &["publisher"])?, &["name"]),
            Value::String(s) => match id_from_url(s, "publisher") {
                Some(id) => self.publisher_name(&id).await,
                None => non_empty(s),
            },
            _ => None,
        }
    }

    /// Series summary for `series_id`, Redis-cached. Costs one series
    /// fetch (+ one publisher fetch) on a miss.
    async fn series_summary(&self, series_id: &str) -> Option<SeriesSummary> {
        let key = format!("metadata:gcd:series:{series_id}");
        if let Some(s) = self.cache_get::<SeriesSummary>(&key).await {
            return Some(s);
        }
        let v = self.get_json(&["series", series_id]).await.ok()?;
        Some(self.store_series_detail(series_id, &v).await)
    }

    /// Cache everything a series payload gives us: the summary (resolving
    /// the publisher name) and the issue index.
    async fn store_series_detail(&self, series_id: &str, v: &Value) -> SeriesSummary {
        let mut summary = summary_from_series(v);
        summary.publisher = self.resolve_publisher(v).await;
        self.cache_put(
            &format!("metadata:gcd:series:{series_id}"),
            &summary,
            SUMMARY_TTL_SECS,
        )
        .await;
        self.store_index(series_id, v).await;
        summary
    }

    async fn store_index(&self, series_id: &str, v: &Value) {
        let index = series_index_entries(v);
        if !index.is_empty() {
            self.cache_put(
                &format!("metadata:gcd:series_index:{series_id}"),
                &index,
                INDEX_TTL_SECS,
            )
            .await;
        }
    }

    /// Search results are full `Series` payloads: cache their issue
    /// index for free (and the summary when the publisher name is
    /// already cached), so a narrowed issue search right after a series
    /// match skips the series-detail request.
    async fn cache_series_from_search(&self, series_id: &str, v: &Value) {
        self.store_index(series_id, v).await;
        let key = format!("metadata:gcd:series:{series_id}");
        if self.cache_get::<SeriesSummary>(&key).await.is_some() {
            return;
        }
        let publisher = match related_id(v, &["publisher", "publisher_url"], "publisher") {
            Some(pid) => match self.cached_publisher_name(&pid).await {
                Some(name) => Some(name),
                // Can't complete the summary without a request; leave
                // it for `series_summary` to fill on demand.
                None => return,
            },
            None => None,
        };
        let mut summary = summary_from_series(v);
        summary.publisher = publisher;
        self.cache_put(&key, &summary, SUMMARY_TTL_SECS).await;
    }

    /// The series' issue index — Redis, else one series-detail request
    /// (which also refreshes the summary).
    async fn series_index(&self, series_id: &str) -> ProviderResult<Vec<IndexEntry>> {
        let key = format!("metadata:gcd:series_index:{series_id}");
        if let Some(idx) = self.cache_get::<Vec<IndexEntry>>(&key).await {
            return Ok(idx);
        }
        let v = self.get_json(&["series", series_id]).await?;
        let _ = self.store_series_detail(series_id, &v).await;
        Ok(series_index_entries(&v))
    }

    /// One overview page (raw JSON), Redis-cached for [`INDEX_TTL_SECS`].
    async fn overview_page(&self, series_id: &str, page: u32) -> ProviderResult<Value> {
        let key = format!("metadata:gcd:overview:{series_id}:{page}");
        if let Some(v) = self.cache_get::<Value>(&key).await {
            return Ok(v);
        }
        let v = self
            .get_json_page(&["series", series_id, "overview"], page)
            .await?;
        self.cache_put(&key, &v, INDEX_TTL_SECS).await;
        Ok(v)
    }

    /// Fetch issue details and turn them into candidates, folding a
    /// variant's cover into its parent's `alternate_cover_urls` when the
    /// parent is in the same batch. Failures on individual details are
    /// skipped — a search shouldn't die because one variant 404s.
    async fn hydrate_issues(&self, ids: &[String]) -> ProviderResult<Vec<IssueCandidate>> {
        let mut primaries: Vec<IssueCandidate> = Vec::new();
        let mut variants: Vec<(Option<String>, IssueCandidate)> = Vec::new();
        for id in ids {
            match self.get_json(&["issue", id]).await {
                Ok(v) => {
                    let Some(cand) = issue_detail_to_candidate(&v, Some(id)) else {
                        continue;
                    };
                    match variant_parent_id(&v) {
                        Some(parent) => variants.push((Some(parent), cand)),
                        None => primaries.push(cand),
                    }
                }
                Err(e @ ProviderError::QuotaExceeded { .. }) => {
                    // Out of budget mid-batch: keep what we have, or
                    // surface the quota so the run parks instead of
                    // reporting "no match".
                    if primaries.is_empty() && variants.is_empty() {
                        return Err(e);
                    }
                    break;
                }
                Err(e) => {
                    tracing::debug!(issue_id = %id, error = %e, "gcd: issue hydrate failed; skipping");
                }
            }
        }
        for (parent, cand) in variants {
            let host = parent
                .as_deref()
                .and_then(|p| primaries.iter_mut().find(|c| c.external_id == p));
            match host {
                Some(host) => {
                    if let Some(url) = cand.cover_image_url {
                        host.alternate_cover_urls.push(url);
                    }
                }
                None => primaries.push(cand),
            }
        }
        Ok(primaries)
    }

    /// Narrowed issue search (series already matched): locate the issue
    /// in the series **overview** — one page carries cover URL, dates
    /// and main story for 50 issues — instead of hydrating issue details.
    /// The issue index says which page to read (`position / 50`); the
    /// neighbouring pages absorb drift between the index (which counts
    /// distinct numbers) and the overview (which lists non-variant
    /// issues). Falls back to hydrating details only when the overview
    /// can't place the issue.
    async fn search_issue_narrowed(
        &self,
        series_id: &str,
        number: &str,
    ) -> ProviderResult<Vec<IssueCandidate>> {
        let index = self.series_index(series_id).await?;
        let wanted = issue_number_key(number);
        let Some(pos) = distinct_number_position(&index, &wanted) else {
            return Ok(Vec::new());
        };
        let summary = self.series_summary(series_id).await.unwrap_or_default();
        let estimate = pos / PAGE_SIZE + 1;
        let mut pages: Vec<usize> = vec![estimate, estimate + 1];
        if estimate > 1 {
            pages.push(estimate - 1);
        }
        for page in pages.into_iter().take(OVERVIEW_PROBE_CAP) {
            match self.overview_page(series_id, page as u32).await {
                Ok(body) => {
                    let hits: Vec<IssueCandidate> = result_items(&body)
                        .into_iter()
                        .filter(|row| overview_row_number_key(row) == wanted)
                        .filter_map(|row| overview_row_to_candidate(row, series_id, &summary))
                        .collect();
                    if !hits.is_empty() {
                        return Ok(hits);
                    }
                }
                Err(e @ ProviderError::QuotaExceeded { .. }) => return Err(e),
                // Out-of-range page (DRF 404) — try the next probe.
                Err(ProviderError::NotFound(_)) if page > 1 => {}
                Err(e) => {
                    tracing::debug!(
                        series_id,
                        page,
                        error = %e,
                        "gcd: overview unavailable; falling back to issue details"
                    );
                    break;
                }
            }
        }
        let ids: Vec<String> = index
            .iter()
            .filter(|e| issue_number_key(&e.number) == wanted)
            .map(|e| e.id.clone())
            .take(FALLBACK_HYDRATE_CAP)
            .collect();
        self.hydrate_issues(&ids).await
    }

    /// Broad issue search (no series match yet): the
    /// `name/issue/number/year` route first, the year-less route when
    /// that has no base (non-variant) issue. No detail hydration — the
    /// `IssueOnly` rows carry the series id, descriptor and publication
    /// date the matcher needs, and GCD covers can't be hashed anyway.
    async fn search_issue_broad(
        &self,
        series_name: &str,
        number: &str,
        cover_year: Option<i32>,
        limit: usize,
    ) -> ProviderResult<Vec<IssueCandidate>> {
        let q = search_name(series_name);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let want = name_key(series_name);
        let enough = move |items: &[Value]| {
            items
                .iter()
                .filter(|it| variant_parent_id(it).is_none())
                .count()
                >= limit
        };
        let mut items: Vec<Value> = Vec::new();
        if let Some(y) = cover_year {
            let y = y.to_string();
            items = self
                .walk_pages(
                    &["series", "name", &q, "issue", number, "year", &y],
                    SEARCH_PAGE_CAP,
                    &enough,
                )
                .await?;
        }
        // The year is GCD's key date, which is often a month off the
        // local value around New Year (or the on-sale date); widen
        // rather than report nothing.
        if !items.iter().any(|it| variant_parent_id(it).is_none()) {
            items = self
                .walk_pages(
                    &["series", "name", &q, "issue", number],
                    SEARCH_PAGE_CAP,
                    &enough,
                )
                .await?;
        }
        let mut out: Vec<(bool, IssueCandidate)> = Vec::new();
        for item in &items {
            if variant_parent_id(item).is_some() {
                continue;
            }
            let Some(mut c) = issue_list_to_candidate(item) else {
                continue;
            };
            if out.iter().any(|(_, x)| x.external_id == c.external_id) {
                continue;
            }
            // Cache-only enrichment: a summary we already hold gives the
            // real publishing format (TPB vs ongoing) for WP-5.6.
            if let Some(sid) = c.series_external_id.clone()
                && let Some(summary) = self
                    .cache_get::<SeriesSummary>(&format!("metadata:gcd:series:{sid}"))
                    .await
                && let Some(hint) = gcd_format(
                    summary.publishing_format.as_deref(),
                    summary.binding.as_deref(),
                )
                .match_hint()
            {
                c.format = Some(hint.to_owned());
            }
            let exact = c.series_name.as_deref().map(name_key).as_deref() == Some(want.as_str());
            out.push((exact, c));
        }
        // Exact series-name matches first (GCD sorts by name, so
        // "All-Star Batman" precedes "Batman"); stable otherwise.
        out.sort_by_key(|(exact, _)| !*exact);
        Ok(out.into_iter().map(|(_, c)| c).take(limit).collect())
    }

    /// Variant covers of a base issue. GCD models each variant as its own
    /// issue (`variant_of` → base), listed in the series index under the
    /// same number; up to [`VARIANT_DETAIL_CAP`] siblings are fetched
    /// (bracketed descriptors first) and kept when their `variant_of`
    /// points back at `base_id`. Skipped below [`VARIANT_BUDGET_FLOOR`].
    /// Best-effort: any failure (quota, 404) just yields fewer variants.
    async fn collect_variants(
        &self,
        series_id: &str,
        base_id: &str,
        number: &str,
    ) -> Vec<VariantCoverCandidate> {
        let mut redis = self.inner.redis.clone();
        match rate_limit::snapshot(&mut redis, &self.inner.hour_bucket).await {
            Ok((remaining, _)) if remaining >= VARIANT_BUDGET_FLOOR => {}
            _ => {
                tracing::debug!(
                    base_id,
                    "gcd: hourly budget low; skipping variant collection"
                );
                return Vec::new();
            }
        }
        let Ok(index) = self.series_index(series_id).await else {
            return Vec::new();
        };
        let key = issue_number_key(number);
        let mut siblings: Vec<&IndexEntry> = index
            .iter()
            .filter(|e| e.id != base_id && !key.is_empty() && issue_number_key(&e.number) == key)
            .collect();
        siblings.sort_by_key(|e| !e.descriptor.contains('['));
        let mut out = Vec::new();
        for e in siblings.into_iter().take(VARIANT_DETAIL_CAP) {
            match self.get_json(&["issue", &e.id]).await {
                Ok(v) => {
                    if variant_parent_id(&v).as_deref() == Some(base_id) {
                        out.push(variant_candidate(&v, &e.id));
                    }
                }
                Err(ProviderError::QuotaExceeded { .. }) => break,
                Err(err) => {
                    tracing::debug!(issue_id = %e.id, error = %err, "gcd: variant detail failed; skipping");
                }
            }
        }
        out
    }

    async fn issue_detail_to_metadata(&self, v: &Value, requested_id: &str) -> GenericMetadata {
        let mut m = issue_detail_metadata(v, requested_id);
        if let Some(series_id) = m.series_external_id.clone() {
            if let Some(summary) = self.series_summary(&series_id).await {
                apply_series_summary(&mut m, v, &summary);
            }
            if variant_parent_id(v).is_none()
                && let (Some(id), Some(number)) =
                    (m.source_external_id.clone(), m.issue_number.clone())
            {
                m.variants = self.collect_variants(&series_id, &id, &number).await;
            }
        }
        m
    }
}

// ───────── tolerant field access ─────────

const SERIES_NAME_KEYS: &[&str] = &["name", "series_name", "title"];
const YEAR_BEGAN_KEYS: &[&str] = &["year_began", "start_year", "year_start", "year"];
const SELF_URL_KEYS: &[&str] = &["api_url", "url", "resource_url", "self"];
const PUBLISHING_FORMAT_KEYS: &[&str] = &["publishing_format", "series_type", "format"];

/// First present (non-null) value among `keys`.
fn pick<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null()))
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// First non-empty string among `keys`; numbers are stringified so a
/// field re-typed from `"1"` to `1` still reads.
pub(crate) fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) => non_empty(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// First integer among `keys`; numeric strings (`"1961"`, `"36.000"`)
/// are accepted.
pub(crate) fn int_field(v: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => {
            let t = s.trim();
            t.parse::<i64>()
                .ok()
                .or_else(|| t.parse::<f64>().ok().map(|f| f.round() as i64))
        }
        _ => None,
    })
}

/// Pull the numeric id out of a GCD API or site URL —
/// `…/api/series/1482/?format=json` or `…/series/1482/` → `"1482"`.
pub(crate) fn id_from_url(url: &str, entity: &str) -> Option<String> {
    let needle = format!("/{entity}/");
    let start = url.find(&needle)? + needle.len();
    let digits: String = url[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

/// The record's own id: an explicit `id` / `<entity>_id` field, else
/// parsed from its self-link.
fn entity_id(v: &Value, entity: &str) -> Option<String> {
    let id_keys = ["id".to_owned(), format!("{entity}_id")];
    for k in &id_keys {
        match v.get(k.as_str()) {
            Some(Value::Number(n)) if n.as_u64().is_some() => return Some(n.to_string()),
            Some(Value::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
                return Some(s.clone());
            }
            _ => {}
        }
    }
    SELF_URL_KEYS
        .iter()
        .find_map(|k| v.get(*k)?.as_str().and_then(|u| id_from_url(u, entity)))
}

/// Id of a related record carried as a hyperlink, inline id, or
/// `{id}` / `{api_url}` object.
fn related_id(v: &Value, keys: &[&str], entity: &str) -> Option<String> {
    match pick(v, keys)? {
        Value::String(s) => id_from_url(s, entity)
            .or_else(|| s.bytes().all(|b| b.is_ascii_digit()).then(|| s.clone()))
            .filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        obj @ Value::Object(_) => entity_id(obj, entity),
        _ => None,
    }
}

fn variant_parent_id(v: &Value) -> Option<String> {
    related_id(v, &["variant_of", "variant_of_url", "parent"], "issue")
}

/// Paged envelope items (`results`), or the body itself when a future
/// version returns a bare array. Anything else is "no results".
fn result_items(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => match pick(v, &["results", "items", "data"]) {
            Some(Value::Array(a)) => a.iter().collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn has_next_page(v: &Value) -> bool {
    matches!(v.get("next"), Some(Value::String(s)) if !s.is_empty())
}

/// `"Fantastic Four (1961 series)"` → `("Fantastic Four", Some(1961))`.
/// A name without the suffix is returned unchanged.
pub(crate) fn split_series_display(raw: &str) -> (String, Option<i32>) {
    let t = raw.trim();
    if let Some(inner) = t.strip_suffix(')')
        && let Some(open) = inner.rfind('(')
    {
        let paren = inner[open + 1..].trim();
        let year_part = paren.strip_suffix("series").unwrap_or(paren).trim();
        if year_part.len() == 4
            && let Ok(y) = year_part.parse::<i32>()
        {
            return (inner[..open].trim_end().to_owned(), Some(y));
        }
    }
    (t.to_owned(), None)
}

/// The search term to send for `name`. GCD's Apache front end rejects an
/// encoded `/` (`%2F` → 404), so a slashed title (`Batman/Superman`)
/// searches on its longest slash-free fragment; `icontains` still finds
/// the real series and the matcher scores the full name.
pub(crate) fn search_name(name: &str) -> String {
    let t = name.trim();
    if !t.contains(['/', '\\']) {
        return t.to_owned();
    }
    t.split(['/', '\\'])
        .map(str::trim)
        .max_by_key(|s| s.chars().count())
        .unwrap_or_default()
        .to_owned()
}

/// Comparison key for "is this the exact series name": lowercase
/// alphanumeric words, leading article dropped.
pub(crate) fn name_key(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let words = match words.split_first() {
        Some((&"the", rest)) if !rest.is_empty() => rest.to_vec(),
        _ => words,
    };
    words.join(" ")
}

/// The issue number from a GCD descriptor: `"1 [British]"` → `"1"`,
/// `"1 - Capítulo Uno"` → `"1"`, `"v2#3"` → `"3"`, `"[nn]"` stays.
pub(crate) fn descriptor_number(descriptor: &str) -> String {
    let mut s = descriptor.trim();
    if !s.starts_with('[')
        && let Some(i) = s.find(" [")
    {
        s = &s[..i];
    }
    if let Some(i) = s.find(" - ") {
        s = &s[..i];
    }
    // Volume-prefixed display (`v2#3`).
    if let Some(rest) = s.strip_prefix('v').or_else(|| s.strip_prefix('V'))
        && let Some((vol, num)) = rest.split_once('#')
        && !vol.is_empty()
        && vol.bytes().all(|b| b.is_ascii_digit())
    {
        s = num;
    }
    s.trim().to_owned()
}

/// The bracketed variant label of a descriptor:
/// `"100 [Cover B]"` → `"Cover B"`. `None` for a plain number.
fn descriptor_bracket(descriptor: &str) -> Option<String> {
    let t = descriptor.trim();
    if t.starts_with('[') {
        return None;
    }
    first_bracket_group(t).and_then(|g| non_empty(&g))
}

fn issue_number_key(raw: &str) -> String {
    canonical_issue_number(raw).to_ascii_lowercase()
}

/// Issue index of a series payload, pairing `active_issues[i]` with
/// `issue_descriptors[i]`. When the descriptor list is absent or
/// misaligned, entries still come back with an empty number (they just
/// never match).
pub(crate) fn series_index_entries(series: &Value) -> Vec<IndexEntry> {
    let urls: Vec<&Value> = match pick(series, &["active_issues", "issues"]) {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => Vec::new(),
    };
    let descriptors: Vec<String> = match pick(series, &["issue_descriptors", "descriptors"]) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|d| d.as_str().unwrap_or_default().trim().to_owned())
            .collect(),
        _ => Vec::new(),
    };
    let aligned = descriptors.len() == urls.len();
    urls.iter()
        .enumerate()
        .filter_map(|(i, u)| {
            let id = match u {
                Value::String(s) => id_from_url(s, "issue"),
                Value::Number(n) => Some(n.to_string()),
                obj @ Value::Object(_) => entity_id(obj, "issue"),
                _ => None,
            }?;
            let descriptor = match u {
                obj @ Value::Object(_) => str_field(obj, &["descriptor", "number"]),
                _ => None,
            }
            .or_else(|| aligned.then(|| descriptors[i].clone()))
            .unwrap_or_default();
            Some(IndexEntry {
                id,
                number: descriptor_number(&descriptor),
                descriptor,
            })
        })
        .collect()
}

/// Position of `wanted` (an [`issue_number_key`]) among the series'
/// distinct issue numbers, in index order. Variants repeat their base's
/// number, so this approximates the issue's row in the overview (which
/// lists one row per non-variant issue in the same sort order).
fn distinct_number_position(index: &[IndexEntry], wanted: &str) -> Option<usize> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut pos = 0usize;
    for e in index {
        let key = issue_number_key(&e.number);
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        if key == wanted {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

fn summary_from_series(v: &Value) -> SeriesSummary {
    let raw_name = str_field(v, SERIES_NAME_KEYS);
    let (name, display_year) = match raw_name.as_deref() {
        Some(r) => {
            let (n, y) = split_series_display(r);
            (Some(n), y)
        }
        None => (None, None),
    };
    SeriesSummary {
        name,
        year_began: int_field(v, YEAR_BEGAN_KEYS)
            .map(|n| n as i32)
            .or(display_year),
        publisher: None,
        publishing_format: str_field(v, PUBLISHING_FORMAT_KEYS),
        binding: str_field(v, &["binding"]),
        language: language_code(v),
    }
}

fn language_code(v: &Value) -> Option<String> {
    str_field(v, &["language", "language_code"]).map(|l| l.to_ascii_lowercase())
}

// ───────── dates ─────────

/// GCD `key_date` (`"1961-11-00"`, `"1867-00-00"`, `"2003-01"`).
/// `strict` requires a known month (day `00` → 1st); lenient falls back
/// to Jan 1 of the year so a candidate still carries its year for the
/// matcher's gate.
fn parse_key_date(raw: &str, strict: bool) -> Option<NaiveDate> {
    let mut parts = raw.trim().splitn(3, '-');
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts
        .next()
        .and_then(|m| m.trim().parse().ok())
        .unwrap_or(0);
    let day: u32 = parts
        .next()
        .and_then(|d| d.trim().parse().ok())
        .unwrap_or(0);
    if month == 0 {
        return if strict {
            None
        } else {
            NaiveDate::from_ymd_opt(year, 1, 1)
        };
    }
    NaiveDate::from_ymd_opt(year, month, day.max(1))
        .or_else(|| NaiveDate::from_ymd_opt(year, month, 1))
}

/// A full `YYYY-MM-DD` only. GCD on-sale dates are often partial
/// (`"2003-01"`, `"2003-01-00"`); a partial store date would invent a
/// day, so it stays empty.
fn parse_full_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

const MONTHS: &[(&str, u32)] = &[
    ("january", 1),
    ("jan", 1),
    ("february", 2),
    ("feb", 2),
    ("march", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("may", 5),
    ("june", 6),
    ("jun", 6),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sept", 9),
    ("sep", 9),
    ("october", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("december", 12),
    ("dec", 12),
];

/// GCD's free-text cover `publication_date` → `(year, month, day)`:
/// `"November 1961"`, `"5 March 1977"`, `"March 5, 1977"`,
/// `"January-February 2003"` (first month), `"Spring 1962"` / `"1867"` /
/// `"março de 2024"` (year only). English month names only — other
/// languages fall back to the year.
pub(crate) fn parse_publication_date(raw: &str) -> Option<(i32, Option<u32>, Option<u32>)> {
    let lower = raw.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let year = tokens.iter().rev().find_map(|t| {
        (t.len() == 4)
            .then(|| t.parse::<i32>().ok())
            .flatten()
            .filter(|y| (1800..=2200).contains(y))
    })?;
    let month_at = tokens.iter().position(|t| {
        MONTHS
            .iter()
            .any(|(name, _)| *t == *name || (t.len() > 3 && name.starts_with(*t)))
    });
    let month = month_at.and_then(|i| {
        let t = tokens[i];
        MONTHS
            .iter()
            .find(|(name, _)| t == *name || (t.len() > 3 && name.starts_with(t)))
            .map(|(_, m)| *m)
    });
    let day = month_at.and_then(|i| {
        let as_day = |t: &str| {
            (t.len() <= 2)
                .then(|| t.parse::<u32>().ok())
                .flatten()
                .filter(|d| (1..=31).contains(d))
        };
        let before = i.checked_sub(1).and_then(|j| as_day(tokens[j]));
        let after = tokens.get(i + 1).and_then(|t| as_day(t));
        before.or(after)
    });
    Some((year, month, day))
}

/// Cover date of an issue-ish payload. The printed `publication_date`
/// is the cover date proper; GCD's `key_date` is a sortable stand-in
/// that, for modern books, is often the on-sale day. `strict` (detail
/// mapping) requires month precision; lenient (search candidates)
/// accepts a bare year as Jan 1 so the matcher's year gate still works.
fn cover_date_of(v: &Value, strict: bool) -> Option<NaiveDate> {
    let from_pub = str_field(v, &["publication_date", "cover_date"])
        .and_then(|p| parse_publication_date(&p))
        .and_then(|(y, m, d)| match m {
            Some(m) => NaiveDate::from_ymd_opt(y, m, d.unwrap_or(1))
                .or_else(|| NaiveDate::from_ymd_opt(y, m, 1)),
            None => None,
        });
    from_pub
        .or_else(|| str_field(v, &["key_date"]).and_then(|d| parse_key_date(&d, strict)))
        .or_else(|| {
            if strict {
                return None;
            }
            str_field(v, &["publication_date", "cover_date"])
                .and_then(|p| parse_publication_date(&p))
                .and_then(|(y, _, _)| NaiveDate::from_ymd_opt(y, 1, 1))
        })
}

// ───────── scalar normalisation ─────────

/// `"0.10 USD"` / `"2.99 USD; 3.99 CAD"` → `0.10` (the first listed
/// price is the primary market's); `"40,90 BRL"` → `40.90`. Non-decimal
/// prices (`"9d [0-0-9 GBP]"`, `"[none]"`) → `None`. The currency has no
/// Folio slot.
fn parse_price(raw: &str) -> Option<f64> {
    let first = raw.split(';').next()?.trim();
    let token = first.split_whitespace().next()?;
    let normalised = if token.contains(',') && !token.contains('.') {
        token.replace(',', ".")
    } else {
        token.replace(',', "")
    };
    normalised
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && *p >= 0.0)
}

/// GCD's free-text `rating` → the ComicInfo `AgeRating` vocabulary.
/// Publisher ratings (`"Rated T+"`, `"Parental Advisory"`, `"Mature
/// Readers"`, `"All Ages"`, `"Ages 12+"`, `"PSR"`) normalise; Comics-Code
/// approval text, `"[none]"` and anything unrecognised → `None` (better
/// empty than junk in the sidecar — the CCA seal isn't an age rating).
pub(crate) fn map_rating(raw: &str) -> Option<String> {
    let r = raw.to_lowercase();
    let words: Vec<&str> = r
        .split(|c: char| !(c.is_alphanumeric() || c == '+'))
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.contains(&w);
    let phrase = |p: &str| r.contains(p);
    if phrase("rating pending") {
        return Some("Rating Pending".to_owned());
    }
    if phrase("comics code") || phrase("code authority") || words.is_empty() {
        return None;
    }
    // A publisher letter code counts only as the whole value ("T+") or
    // right after "rated" ("Rated M") — a bare "a"/"e" in prose is not
    // a rating.
    let code: Option<&str> = if words.len() == 1 {
        Some(words[0])
    } else {
        words.windows(2).find(|w| w[0] == "rated").map(|w| w[1])
    };
    // An explicit minimum age wins ("Ages 12+", "16+", "18+").
    let min_age = words.iter().find_map(|w| {
        w.strip_suffix('+')
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| (3..=21).contains(n))
    });
    let label = if has("explicit")
        || has("adult")
        || has("adults")
        || code.is_some_and(|c| matches!(c, "x" | "x18+" | "ao" | "max" | "r18+"))
    {
        "Adults Only 18+"
    } else if let Some(age) = min_age {
        match age {
            18.. => "Adults Only 18+",
            17 => "Mature 17+",
            13..=16 => "Teen",
            10..=12 => "Everyone 10+",
            _ => "Everyone",
        }
    } else if has("mature") || code.is_some_and(|c| matches!(c, "m" | "ma" | "17+")) {
        "Mature 17+"
    } else if has("teen")
        || has("psr")
        || has("psr+")
        || phrase("parental advisory")
        || phrase("parental supervision")
        || code.is_some_and(|c| matches!(c, "t" | "t+" | "pg" | "psr" | "psr+" | "pa"))
    {
        "Teen"
    } else if phrase("early childhood") {
        "Early Childhood"
    } else if phrase("all ages")
        || has("everyone")
        || has("kids")
        || code.is_some_and(|c| matches!(c, "e" | "a" | "g"))
    {
        "Everyone"
    } else {
        return None;
    };
    Some(label.to_owned())
}

/// GCD `isbn` (`"978-1-58240-711-1; 1-58240-711-6"`) → the first valid
/// ISBN-10/13, digits (and a trailing `X`) only.
pub(crate) fn normalize_isbn(raw: &str) -> Option<String> {
    raw.split([';', ','])
        .map(|part| {
            part.chars()
                .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
                .collect::<String>()
                .to_ascii_uppercase()
        })
        .find(|s| {
            (s.len() == 13 && s.bytes().all(|b| b.is_ascii_digit()))
                || (s.len() == 10 && s[..9].bytes().all(|b| b.is_ascii_digit()))
        })
}

/// GCD `barcode` → identifier: UPC-A (12 digits, optionally + a 2/5-digit
/// add-on → 14/17) as `upc`; EAN-13 (13, +2/+5 → 15/18) as `gtin`, or
/// `isbn` when it is a Bookland EAN (978/979) and no ISBN is recorded.
fn barcode_identifier(raw: &str, has_isbn: bool) -> Option<Identifier> {
    let digits: String = raw
        .split(';')
        .next()?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    match digits.len() {
        12 | 14 | 17 => Some(Identifier::new(Source::Upc, digits)),
        13 | 15 | 18 => {
            if !has_isbn && (digits.starts_with("978") || digits.starts_with("979")) {
                Some(Identifier::new(Source::Isbn, digits[..13].to_owned()))
            } else {
                Some(Identifier::new(Source::Gtin, digits))
            }
        }
        _ => None,
    }
}

/// GCD's series `publishing_format` (+ `binding` fallback) normalised.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GcdFormat {
    /// Metron-vocabulary series type (`"Ongoing Series"`,
    /// `"Trade Paperback"`, …) → `series.series_type`.
    pub(crate) series_type: Option<&'static str>,
    /// ComicInfo `Format` label; `None` for an ongoing series (matches
    /// the Metron mapping — a plain ongoing book carries no `Format`).
    pub(crate) format: Option<&'static str>,
}

impl GcdFormat {
    /// The WP-5.6 matcher hint: the format label, else the series type
    /// (both understood by `title_norm::classify_format`).
    pub(crate) fn match_hint(self) -> Option<&'static str> {
        self.format.or(self.series_type)
    }
}

/// `"was ongoing series"`, `"Collected Editions; Was Ongoing Series"`,
/// `"limited series"`, `"one-shot"`, `"graphic novel"` … Collected
/// markers win over the run type (the Invincible TPB series is
/// `"Collected Editions; Was Ongoing Series"`). A `hardcover` binding
/// upgrades a collected format; a bare binding is only a fallback
/// (`softcover`/`squarebound` alone are ambiguous — prestige singles use
/// them too — and are ignored).
pub(crate) fn gcd_format(publishing_format: Option<&str>, binding: Option<&str>) -> GcdFormat {
    let pf = publishing_format.unwrap_or_default().to_lowercase();
    let bind = binding.unwrap_or_default().to_lowercase();
    let hard = bind.contains("hardcover") || bind.contains("hard cover");
    let collected = |series_type, format| GcdFormat {
        series_type: Some(series_type),
        format: Some(format),
    };
    if pf.contains("omnibus") {
        return collected("Omnibus", "Omnibus");
    }
    if pf.contains("graphic novel") {
        return collected("Graphic Novel", "Graphic Novel");
    }
    if pf.contains("hardcover") || pf.contains("hard cover") {
        return collected("Hard Cover", "Hardcover");
    }
    if pf.contains("collected edition")
        || pf.contains("trade paperback")
        || pf.contains("collection")
        || pf.contains("tpb")
    {
        return if hard {
            collected("Hard Cover", "Hardcover")
        } else {
            collected("Trade Paperback", "TPB")
        };
    }
    if pf.contains("one-shot") || pf.contains("one shot") {
        return collected("One-Shot", "One-Shot");
    }
    if pf.contains("limited series")
        || pf.contains("mini-series")
        || pf.contains("miniseries")
        || pf.contains("maxi-series")
    {
        return collected("Limited Series", "Limited Series");
    }
    if pf.contains("annual") {
        return collected("Annual Series", "Annual");
    }
    if pf.contains("ongoing") {
        return GcdFormat {
            series_type: Some("Ongoing Series"),
            format: None,
        };
    }
    if hard {
        return collected("Hard Cover", "Hardcover");
    }
    if bind.contains("trade paperback") {
        return collected("Trade Paperback", "TPB");
    }
    GcdFormat::default()
}

/// `brand_emblem` → imprint, only when it is a distinct line under the
/// publisher (`"Vertigo"` under DC). Rejected: the publisher's own
/// emblem (`"Image"` under Image), abbreviations (`"MC"`), and
/// promotional emblems carrying digits (`"Skybound Five Years
/// 2010-2015"`).
pub(crate) fn imprint_from_brand(brand: &str, publisher: Option<&str>) -> Option<String> {
    let first = brand.split(';').next()?.trim();
    if first.chars().count() <= 3 || first.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(p) = publisher {
        let (b, p) = (name_key(first), name_key(p));
        let b_core = b.trim_end_matches(" comics").trim_end_matches(" comic");
        let p_core = p.trim_end_matches(" comics").trim_end_matches(" comic");
        if b_core.is_empty() || p_core.contains(b_core) || b_core.contains(p_core) {
            return None;
        }
    }
    non_empty(first)
}

/// Words that mark a variant-name fragment as a description rather than
/// a person (`"Retailer Variant"`, `"Image Tribute Cover"`).
const VARIANT_STOP_WORDS: &[&str] = &[
    "cover",
    "covers",
    "variant",
    "edition",
    "printing",
    "print",
    "exclusive",
    "incentive",
    "retailer",
    "regular",
    "standard",
    "virgin",
    "sketch",
    "blank",
    "foil",
    "newsstand",
    "direct",
    "wraparound",
    "connecting",
    "color",
    "colour",
    "black",
    "white",
    "line",
    "art",
    "box",
    "set",
    "image",
    "marvel",
    "dc",
    "tribute",
    "anniversary",
    "convention",
    "comics",
    "comic",
    "store",
    "con",
    "photo",
    "movie",
    "game",
    "homage",
    "party",
    "british",
    "canadian",
    "price",
    "whitman",
    "second",
    "third",
    "fourth",
    "fifth",
    "sdcc",
    "nycc",
];

const NAME_PARTICLES: &[&str] = &[
    "de", "da", "del", "della", "di", "van", "von", "der", "la", "le",
];

/// Plausible personal name: 2–4 words, capitalised (particles excepted),
/// letters plus `.`/`'`/`-` only, no description words.
fn person_like(s: &str) -> bool {
    let words: Vec<&str> = s.split_whitespace().collect();
    if !(2..=4).contains(&words.len()) {
        return false;
    }
    words.iter().enumerate().all(|(i, w)| {
        let lower = w.to_lowercase();
        let lower_trim = lower.trim_matches(|c: char| !c.is_alphanumeric());
        if VARIANT_STOP_WORDS.contains(&lower_trim) {
            return false;
        }
        if !w
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, '.' | '\'' | '-' | '’'))
        {
            return false;
        }
        let first_upper = w.chars().next().is_some_and(char::is_uppercase);
        first_upper || (i > 0 && i < words.len() - 1 && NAME_PARTICLES.contains(&lower_trim))
    })
}

/// The cover artist named in a GCD `variant_name`, when unambiguous:
/// `"… Color Cover - Cory Walker"` → `Cory Walker`, `"Cover B by Ryan
/// Ottley"` → `Ryan Ottley`, `"Chris Giarrusso Cover"` →
/// `Chris Giarrusso`. `"Cover A"`, `"Retailer Variant"`, `"Image Tribute
/// Cover"`, `"2nd Printing"` → `None`.
pub(crate) fn parse_variant_artist(variant_name: &str) -> Option<String> {
    let t = variant_name.trim();
    if let Some((_, right)) = t.rsplit_once(" - ") {
        let right = right.trim();
        return person_like(right).then(|| right.to_owned());
    }
    if let Some(i) = t.to_lowercase().rfind(" by ") {
        let right = t[i + 4..].trim();
        return person_like(right).then(|| right.to_owned());
    }
    let lower = t.to_lowercase();
    for suffix in [" variant cover", " virgin cover", " cover", " variant"] {
        if lower.ends_with(suffix) {
            let left = t[..t.len() - suffix.len()].trim();
            return person_like(left).then(|| left.to_owned());
        }
    }
    None
}

// ───────── free-text credit / character parsing ─────────

/// Split on `sep` at bracket depth 0 (both `()` and `[]` nest).
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth = (depth - 1).max(0);
                cur.push(c);
            }
            c if c == sep && depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// `(text with every bracketed group removed, lowercased annotations)`.
fn strip_annotations(s: &str) -> (String, String) {
    let mut depth: i32 = 0;
    let mut text = String::new();
    let mut notes = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                if depth > 0 {
                    notes.push(' ');
                }
            }
            ')' | ']' => depth = (depth - 1).max(0),
            c if depth == 0 => text.push(c),
            c => notes.push(c),
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (text, notes.to_ascii_lowercase())
}

/// Content of every top-level `(...)` group, lowercased.
fn paren_groups(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0
                    && c == ')'
                    && let Some(st) = start.take()
                {
                    out.push(s[st..i].trim().to_lowercase());
                }
                depth = depth.max(0);
            }
            _ => {}
        }
    }
    out
}

/// Content of the first top-level `[...]` group, if any.
fn first_bracket_group(s: &str) -> Option<String> {
    let mut depth: i32 = 0;
    let mut start = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' if depth == 0 => {
                start = Some(i + 1);
                depth += 1;
            }
            '[' | '(' => depth += 1,
            ']' | ')' => {
                depth -= 1;
                if depth == 0
                    && c == ']'
                    && let Some(st) = start
                {
                    return Some(s[st..i].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

const NON_NAMES: &[&str] = &[
    "none",
    "?",
    "various",
    "typeset",
    "uncredited",
    "unknown",
    "n/a",
    "anonymous",
];

/// One GCD credit string (`"Stan Lee (signed as …); Sol Brodsky ? (see
/// notes)"`) → `(name, annotations)` pairs. Dropped: placeholders
/// (`None`, `[none]`, `?`, `typeset`, `various`, `anonymous`) and
/// uncertain credits — a trailing `?` or a `(?)` group — which GCD uses
/// for unconfirmed attributions; Folio would rather omit than assert
/// them. `[as Pen Name]` / `(signed as …)` keep the real name.
pub(crate) fn parse_credit_names(raw: &str) -> Vec<(String, String)> {
    split_top_level(raw, ';')
        .into_iter()
        .filter_map(|tok| {
            let (name, notes) = strip_annotations(&tok);
            let name = name.trim().to_owned();
            if name.is_empty()
                || name.starts_with('?')
                || name.ends_with('?')
                || NON_NAMES.contains(&name.to_ascii_lowercase().as_str())
                || paren_groups(&tok).iter().any(|g| g == "?")
            {
                return None;
            }
            Some((name, notes))
        })
        .collect()
}

fn push_credit(out: &mut Vec<CreditCandidate>, name: String, role: &str) {
    if out
        .iter()
        .any(|c| c.role == role && c.name.eq_ignore_ascii_case(&name))
    {
        return;
    }
    let ordinal = Some(out.len() as i32);
    out.push(CreditCandidate {
        name,
        role: role.to_owned(),
        ordinal,
        identifiers: Vec::new(),
    });
}

fn story_type(story: &Value) -> String {
    match pick(story, &["type", "story_type"]) {
        Some(Value::String(s)) => s.trim().to_ascii_lowercase(),
        Some(obj @ Value::Object(_)) => str_field(obj, &["name"])
            .unwrap_or_default()
            .to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn stories(v: &Value) -> Vec<&Value> {
    let mut out: Vec<&Value> = match pick(v, &["story_set", "stories", "sequences"]) {
        Some(Value::Array(a)) => a.iter().filter(|s| s.is_object()).collect(),
        _ => Vec::new(),
    };
    out.sort_by_key(|s| int_field(s, &["sequence_number", "sequence"]).unwrap_or(i64::MAX));
    out
}

fn is_comic_story(s: &Value) -> bool {
    story_type(s) == "comic story"
}

/// Story page count in thousandths (`"22.000"` → 22000) for ranking.
fn story_pages(s: &Value) -> i64 {
    match pick(s, &["page_count", "pages"]) {
        Some(Value::Number(n)) => (n.as_f64().unwrap_or(0.0) * 1000.0) as i64,
        Some(Value::String(t)) => (t.trim().parse::<f64>().unwrap_or(0.0) * 1000.0) as i64,
        _ => 0,
    }
}

/// The issue's main story: the longest comic story (first on a tie) —
/// the same pick as the overview's `longest_story`.
fn main_story<'a>(comic_stories: &[&'a Value]) -> Option<&'a Value> {
    comic_stories
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|(i, s)| (story_pages(s), std::cmp::Reverse(*i)))
        .map(|(_, s)| s)
}

const STORY_ROLES: &[(&[&str], &str)] = &[
    (&["script", "writer", "writers"], "Writer"),
    (&["pencils", "penciller", "pencillers"], "Penciller"),
    (&["inks", "inker", "inkers"], "Inker"),
    (&["colors", "colours", "colorist"], "Colorist"),
    (&["letters", "letterer"], "Letterer"),
    (&["editing", "editor"], "Editor"),
];

/// Credits of one comic story into `out`.
fn push_story_credits(out: &mut Vec<CreditCandidate>, story: &Value) {
    for (keys, role) in STORY_ROLES {
        if let Some(raw) = str_field(story, keys) {
            for (name, _) in parse_credit_names(&raw) {
                push_credit(out, name, role);
            }
        }
    }
}

/// Credits across the issue: issue-level editors + every comic story's
/// script/pencils/inks/colors/letters/editing + the cover's
/// pencils/inks as `CoverArtist`. Roles land in the ComicInfo
/// vocabulary [`crate::metadata::provider::canonicalize_role`] emits.
fn collect_credits(v: &Value) -> Vec<CreditCandidate> {
    let mut out = Vec::new();
    // Issue-level `editing` mixes editors with production staff
    // ("Drew Gill (art director)", "Jim Valentino (publisher)"); keep
    // only plain or editor-annotated names.
    if let Some(raw) = str_field(v, &["editing", "editor"]) {
        for (name, notes) in parse_credit_names(&raw) {
            if notes.trim().is_empty() || notes.contains("editor") {
                push_credit(&mut out, name, "Editor");
            }
        }
    }
    for story in stories(v) {
        let ty = story_type(story);
        if ty == "cover" {
            for key in ["pencils", "inks"] {
                if let Some(raw) = str_field(story, &[key]) {
                    for (name, _) in parse_credit_names(&raw) {
                        push_credit(&mut out, name, "CoverArtist");
                    }
                }
            }
            continue;
        }
        if ty == "comic story" {
            push_story_credits(&mut out, story);
        }
    }
    out
}

fn entity(name: String, first: bool, died: bool) -> EntityCandidate {
    EntityCandidate {
        name,
        identifiers: Vec::new(),
        is_first_appearance: first,
        died_in_issue: died.then_some(true),
        disbanded_in_issue: None,
        position_in_arc: None,
    }
}

fn upsert_entity(list: &mut Vec<EntityCandidate>, name: String, first: bool, died: bool) {
    if let Some(e) = list.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&name)) {
        e.is_first_appearance |= first;
        if died {
            e.died_in_issue = Some(true);
        }
    } else {
        list.push(entity(name, first, died));
    }
}

fn is_first_appearance(notes: &str) -> bool {
    notes.contains("first appearance")
        || notes.contains("first full appearance")
        || notes.contains("introduction")
}

/// `(death)` / `(…, death)` — but not `(death in flashforward)`,
/// `(apparent death)` or `(death of …)`.
fn is_death(seg: &str) -> bool {
    paren_groups(seg)
        .iter()
        .flat_map(|g| g.split(',').map(str::trim).collect::<Vec<_>>())
        .any(|n| n == "death" || n == "dies")
}

fn usable_entity_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    !(name.is_empty()
        || name.starts_with('?')
        || l.starts_with("unnamed")
        || NON_NAMES.contains(&l.as_str()))
}

/// GCD's free-text `characters` field →
/// `(characters, teams)`. Grammar (best effort):
/// `Team [Member [Alter Ego]; Member]; Character [Alter Ego] (notes)`.
/// A bracket group holding several members (or nested brackets) marks
/// its head as a **team**; a single plain bracket is an alter ego.
/// `Label: …` prefixes (`VILLAINS:`) are dropped. `(first appearance)`
/// / `(introduction)` set the first-appearance flag, `(death)` the
/// died-in-issue flag.
pub(crate) fn parse_characters(raw: &str) -> (Vec<EntityCandidate>, Vec<EntityCandidate>) {
    let mut characters = Vec::new();
    let mut teams = Vec::new();
    for seg in split_top_level(raw, ';') {
        let seg = strip_label(&seg);
        let (head, notes) = strip_annotations(seg);
        if !usable_entity_name(&head) {
            continue;
        }
        let first = is_first_appearance(&notes);
        match first_bracket_group(seg) {
            Some(group) if group.contains(';') || group.contains('[') => {
                upsert_entity(&mut teams, head, first, false);
                for member in split_top_level(&group, ';') {
                    let (m_name, m_notes) = strip_annotations(&member);
                    if usable_entity_name(&m_name) {
                        upsert_entity(
                            &mut characters,
                            m_name,
                            is_first_appearance(&m_notes),
                            is_death(&member),
                        );
                    }
                }
            }
            _ => upsert_entity(&mut characters, head, first, is_death(seg)),
        }
    }
    (characters, teams)
}

/// Drop a short leading `Label:` (`"VILLAINS: Doctor Doom"`).
fn strip_label(seg: &str) -> &str {
    let cut = seg.find(['[', '(']).unwrap_or(seg.len());
    if let Some(colon) = seg[..cut].find(':') {
        let label = &seg[..colon];
        if label.split_whitespace().count() <= 3 && label.len() <= 25 {
            return seg[colon + 1..].trim();
        }
    }
    seg
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_list(raw: &str) -> Vec<String> {
    split_top_level(raw, ';')
        .into_iter()
        .map(|s| strip_annotations(&s).0)
        .filter(|s| !s.is_empty())
        .collect()
}

fn dedup_ci(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|s| seen.insert(s.to_ascii_lowercase()))
        .collect()
}

// ───────── mapping ─────────

fn series_to_candidate(v: &Value) -> Option<SeriesCandidate> {
    let external_id = entity_id(v, "series")?;
    let raw_name = str_field(v, SERIES_NAME_KEYS).unwrap_or_default();
    let (name, display_year) = split_series_display(&raw_name);
    let year = int_field(v, YEAR_BEGAN_KEYS)
        .map(|n| n as i32)
        .or(display_year);
    // Variants share the number of their parent; count distinct numbers.
    let issue_count = match pick(v, &["issue_descriptors", "descriptors"]) {
        Some(Value::Array(a)) => {
            let set: HashSet<String> = a
                .iter()
                .filter_map(|d| d.as_str())
                .map(descriptor_number)
                .collect();
            Some(set.len() as i32)
        }
        _ => int_field(v, &["issue_count", "count_of_issues"]).map(|n| n as i32),
    };
    // WP-5.6: GCD's own publishing format (TPB vs ongoing) beats the
    // name heuristic.
    let format = gcd_format(
        str_field(v, PUBLISHING_FORMAT_KEYS).as_deref(),
        str_field(v, &["binding"]).as_deref(),
    )
    .match_hint()
    .or_else(|| crate::metadata::title_norm::infer_format_from_title(&name, None))
    .map(str::to_owned);
    let publisher = match pick(v, &["publisher"]) {
        Some(Value::Object(o)) => o.get("name").and_then(|n| n.as_str()).and_then(non_empty),
        Some(Value::String(s)) if !s.contains("://") => non_empty(s),
        _ => None,
    };
    Some(SeriesCandidate {
        source: Source::Gcd,
        external_url: canonical_url(Source::Gcd, "series", &external_id),
        external_id,
        name,
        year,
        publisher,
        issue_count,
        cover_image_url: None,
        deck: None,
        alternate_cover_urls: Vec::new(),
        format,
    })
}

fn issue_number_of(v: &Value) -> Option<String> {
    str_field(v, &["number", "issue_number"])
        .or_else(|| str_field(v, &["descriptor"]).map(|d| descriptor_number(&d)))
        .filter(|n| !n.is_empty())
}

fn issue_list_to_candidate(v: &Value) -> Option<IssueCandidate> {
    let external_id = entity_id(v, "issue")?;
    let (series_name, series_year) = match str_field(v, &["series_name"]) {
        Some(raw) => {
            let (n, y) = split_series_display(&raw);
            (Some(n), y)
        }
        None => (None, None),
    };
    Some(IssueCandidate {
        source: Source::Gcd,
        external_url: canonical_url(Source::Gcd, "issue", &external_id),
        external_id,
        issue_number: issue_number_of(v),
        name: str_field(v, &["title"]),
        cover_date: cover_date_of(v, false),
        format: series_name
            .as_deref()
            .and_then(|n| crate::metadata::title_norm::infer_format_from_title(n, None))
            .map(str::to_owned),
        series_name,
        series_year,
        series_external_id: related_id(v, &["series", "series_url"], "series"),
        cover_image_url: None,
        alternate_cover_urls: Vec::new(),
    })
}

fn overview_row_number_key(row: &Value) -> String {
    issue_number_of(row)
        .map(|n| issue_number_key(&n))
        .unwrap_or_default()
}

/// One `SeriesOverviewItem` → candidate. The row carries no series
/// fields; they come from the (cached) summary.
fn overview_row_to_candidate(
    row: &Value,
    series_id: &str,
    summary: &SeriesSummary,
) -> Option<IssueCandidate> {
    let external_id = match pick(row, &["issue_id", "id"]) {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if s.bytes().all(|b| b.is_ascii_digit()) && !s.is_empty() => {
            Some(s.clone())
        }
        _ => entity_id(row, "issue"),
    }?;
    let story = pick(row, &["longest_story", "main_story", "story"]).filter(|s| s.is_object());
    let format = gcd_format(
        summary.publishing_format.as_deref(),
        summary.binding.as_deref(),
    )
    .match_hint()
    .or_else(|| {
        summary
            .name
            .as_deref()
            .and_then(|n| crate::metadata::title_norm::infer_format_from_title(n, None))
    })
    .map(str::to_owned);
    Some(IssueCandidate {
        source: Source::Gcd,
        external_url: canonical_url(Source::Gcd, "issue", &external_id),
        external_id,
        issue_number: issue_number_of(row),
        name: str_field(row, &["title"]).or_else(|| story.and_then(|s| str_field(s, &["title"]))),
        cover_date: cover_date_of(row, false),
        series_name: summary.name.clone(),
        series_year: summary.year_began,
        series_external_id: Some(series_id.to_owned()),
        cover_image_url: cover_url(row),
        alternate_cover_urls: Vec::new(),
        format,
    })
}

fn cover_url(v: &Value) -> Option<String> {
    match pick(v, &["cover", "cover_url", "image", "cover_image"])? {
        Value::String(s) => non_empty(s).filter(|s| s.starts_with("http")),
        obj @ Value::Object(_) => str_field(obj, &["url", "large", "medium"]),
        _ => None,
    }
}

fn issue_detail_to_candidate(v: &Value, requested_id: Option<&str>) -> Option<IssueCandidate> {
    let mut c = match issue_list_to_candidate(v) {
        Some(c) => c,
        None => {
            // A detail whose self-link was renamed away: fall back to the
            // id we asked for.
            let id = requested_id?.to_owned();
            let mut patched = v.clone();
            if let Value::Object(map) = &mut patched {
                map.insert("id".into(), Value::String(id));
            }
            issue_list_to_candidate(&patched)?
        }
    };
    c.name = c.name.or_else(|| {
        let comic: Vec<&Value> = stories(v)
            .into_iter()
            .filter(|s| is_comic_story(s))
            .collect();
        main_story(&comic).and_then(|s| str_field(s, &["title"]))
    });
    c.cover_image_url = cover_url(v);
    Some(c)
}

/// A variant issue detail → gallery row: label from `variant_name` (or
/// the descriptor bracket), artist parsed from it when unambiguous, the
/// variant's own GCD id + barcode as identifiers.
fn variant_candidate(v: &Value, requested_id: &str) -> VariantCoverCandidate {
    let id = entity_id(v, "issue").unwrap_or_else(|| requested_id.to_owned());
    let label = str_field(v, &["variant_name"])
        .or_else(|| str_field(v, &["descriptor"]).and_then(|d| descriptor_bracket(&d)));
    let mut identifiers = vec![Identifier::with_canonical_url(Source::Gcd, id, "issue")];
    if let Some(b) = str_field(v, &["barcode", "upc"]).and_then(|b| barcode_identifier(&b, true)) {
        identifiers.push(b);
    }
    VariantCoverCandidate {
        artist_name: label.as_deref().and_then(parse_variant_artist),
        label,
        identifiers,
        image_url: cover_url(v),
    }
}

/// Pure issue-detail mapping (no I/O). The client enriches the result
/// with the cached series summary ([`apply_series_summary`]) and the
/// variant list.
pub(crate) fn issue_detail_metadata(v: &Value, requested_id: &str) -> GenericMetadata {
    let external_id = entity_id(v, "issue").unwrap_or_else(|| requested_id.to_owned());
    let mut identifiers = vec![Identifier::with_canonical_url(
        Source::Gcd,
        external_id.clone(),
        "issue",
    )];
    let isbn = str_field(v, &["isbn"]).and_then(|i| normalize_isbn(&i));
    if let Some(isbn) = isbn.clone() {
        identifiers.push(Identifier::new(Source::Isbn, isbn));
    }
    if let Some(id) =
        str_field(v, &["barcode", "upc"]).and_then(|b| barcode_identifier(&b, isbn.is_some()))
    {
        identifiers.push(id);
    }
    let (series_name, series_year) = match str_field(v, &["series_name"]) {
        Some(raw) => {
            let (n, y) = split_series_display(&raw);
            (Some(n), y)
        }
        None => (None, None),
    };
    let all_stories = stories(v);
    let comic_stories: Vec<&Value> = all_stories
        .iter()
        .copied()
        .filter(|s| is_comic_story(s))
        .collect();
    let main = main_story(&comic_stories);

    // Summary: the main story's synopsis first, then the other comic
    // stories' (anthologies), in sequence order.
    let mut synopses: Vec<String> = Vec::new();
    if let Some(m) = main
        && let Some(s) = str_field(m, &["synopsis", "summary"])
    {
        synopses.push(s);
    }
    for s in &comic_stories {
        if main.is_some_and(|m| std::ptr::eq(m, *s)) {
            continue;
        }
        if let Some(syn) = str_field(s, &["synopsis", "summary"]) {
            synopses.push(syn);
        }
    }
    let description = (!synopses.is_empty()).then(|| synopses.join("\n\n"));

    let (mut characters, mut teams) = (Vec::new(), Vec::new());
    // Characters from the comic stories; the cover only when the issue
    // has no indexed story (avoids "first cover appearance" noise).
    let char_sources: Vec<&Value> = if comic_stories.is_empty() {
        all_stories
            .iter()
            .copied()
            .filter(|s| story_type(s) == "cover")
            .collect()
    } else {
        comic_stories.clone()
    };
    for s in char_sources {
        if let Some(raw) = str_field(s, &["characters", "appearing_characters"]) {
            let (c, t) = parse_characters(&raw);
            for e in c {
                upsert_entity(
                    &mut characters,
                    e.name,
                    e.is_first_appearance,
                    e.died_in_issue == Some(true),
                );
            }
            for e in t {
                upsert_entity(&mut teams, e.name, e.is_first_appearance, false);
            }
        }
    }

    let genres = dedup_ci(
        comic_stories
            .iter()
            .filter_map(|s| str_field(s, &["genre", "genres"]))
            .flat_map(|g| split_list(&g))
            .map(|g| title_case(&g))
            .collect(),
    );
    // Issue keywords + every comic story's keywords.
    let tags = dedup_ci(
        std::iter::once(v)
            .chain(comic_stories.iter().copied())
            .filter_map(|s| str_field(s, &["keywords", "tags"]))
            .flat_map(|k| split_list(&k))
            .collect(),
    );

    GenericMetadata {
        series_name,
        series_external_id: related_id(v, &["series", "series_url"], "series"),
        year_began: series_year,
        volume: int_field(v, &["volume"])
            .filter(|n| *n > 0)
            .map(|n| n as i32),
        issue_number: issue_number_of(v),
        title: str_field(v, &["title"]).or_else(|| main.and_then(|s| str_field(s, &["title"]))),
        cover_date: cover_date_of(v, true),
        store_date: str_field(v, &["on_sale_date", "store_date"]).and_then(|d| parse_full_date(&d)),
        description,
        notes: str_field(v, &["notes"]),
        credits: collect_credits(v),
        characters,
        teams,
        genres,
        tags,
        cover_image_url: cover_url(v),
        identifiers,
        age_rating: str_field(v, &["rating", "age_rating"]).and_then(|r| map_rating(&r)),
        page_count: int_field(v, &["page_count", "pages"])
            .filter(|n| *n > 0)
            .map(|n| n as i32),
        price: str_field(v, &["price"]).and_then(|p| parse_price(&p)),
        source_provider: Some(Source::Gcd),
        source_url: canonical_url(Source::Gcd, "issue", &external_id),
        source_external_id: Some(external_id),
        fetched_at: Some(Utc::now()),
        ..Default::default()
    }
}

/// Fold the cached series summary into an issue mapping: series
/// name/year when the issue lacked them, publisher (the indicia
/// publisher as a fallback), imprint from the brand emblem, language,
/// series type and format.
pub(crate) fn apply_series_summary(m: &mut GenericMetadata, v: &Value, summary: &SeriesSummary) {
    if m.series_name.is_none() {
        m.series_name = summary.name.clone();
    }
    if m.year_began.is_none() {
        m.year_began = summary.year_began;
    }
    m.publisher = summary
        .publisher
        .clone()
        .or_else(|| str_field(v, &["indicia_publisher"]));
    m.imprint = str_field(v, &["brand_emblem", "brand"])
        .and_then(|b| imprint_from_brand(&b, m.publisher.as_deref()));
    m.language_code = summary.language.clone();
    let f = gcd_format(
        summary.publishing_format.as_deref(),
        summary.binding.as_deref(),
    );
    m.series_type = f.series_type.map(str::to_owned);
    m.format = f.format.map(str::to_owned);
}

/// Pure series-detail mapping. `publisher` is resolved by the caller.
pub(crate) fn series_detail_metadata(
    v: &Value,
    requested_id: &str,
    publisher: Option<String>,
) -> GenericMetadata {
    let external_id = entity_id(v, "series").unwrap_or_else(|| requested_id.to_owned());
    let summary = summary_from_series(v);
    let f = gcd_format(
        summary.publishing_format.as_deref(),
        summary.binding.as_deref(),
    );
    GenericMetadata {
        series_name: summary.name,
        series_type: f.series_type.map(str::to_owned),
        format: f.format.map(str::to_owned),
        year_began: summary.year_began,
        year_end: int_field(v, &["year_ended", "year_end"]).map(|n| n as i32),
        publisher,
        notes: str_field(v, &["notes"]),
        language_code: summary.language,
        identifiers: vec![Identifier::with_canonical_url(
            Source::Gcd,
            external_id.clone(),
            "series",
        )],
        source_provider: Some(Source::Gcd),
        source_url: canonical_url(Source::Gcd, "series", &external_id),
        source_external_id: Some(external_id),
        fetched_at: Some(Utc::now()),
        ..Default::default()
    }
}

// ───────── Trait impl ─────────

#[async_trait]
impl MetadataProvider for GcdClient {
    fn id(&self) -> Source {
        Source::Gcd
    }

    async fn health_check(&self) -> ProviderResult<QuotaSnapshot> {
        // A no-match name search: exercises Basic auth (DRF rejects bad
        // credentials with 401/403 even on read-only routes) and the
        // envelope parse without pulling a payload.
        let _ = self
            .get_json(&["series", "name", "__folio_health_check__"])
            .await?;
        self.quota().await
    }

    async fn quota(&self) -> ProviderResult<QuotaSnapshot> {
        let mut redis = self.inner.redis.clone();
        let (hour_remaining, hour_ttl) = rate_limit::snapshot(&mut redis, &self.inner.hour_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        let (day_remaining, day_ttl) = rate_limit::snapshot(&mut redis, &self.inner.day_bucket)
            .await
            .map_err(|e| ProviderError::Transport(format!("redis: {e}")))?;
        let seconds_until_reset = match (hour_ttl, day_ttl) {
            (0, 0) => Some(0),
            (0, d) => Some(d),
            (h, 0) => Some(h),
            (h, d) => Some(h.min(d)),
        };
        Ok(QuotaSnapshot {
            provider: Source::Gcd,
            remaining_hour: Some(hour_remaining),
            remaining_day: Some(day_remaining),
            seconds_until_reset,
        })
    }

    async fn search_series(&self, query: &SeriesQuery) -> ProviderResult<Vec<SeriesCandidate>> {
        let name = query.name.trim();
        let q = search_name(name);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let limit = query.limit.clamp(1, 100) as usize;
        let want = name_key(name);
        let is_exact = move |item: &Value| {
            str_field(item, SERIES_NAME_KEYS)
                .map(|n| name_key(&split_series_display(&n).0) == want)
                .unwrap_or(false)
        };
        let has_exact = {
            let is_exact = is_exact.clone();
            move |items: &[Value]| items.iter().any(&is_exact)
        };
        // GCD's name search is a plain `icontains` sorted by name, so a
        // common title ("Batman") can bury the exact run. The exact-year
        // route narrows that to a handful; only when it has no exact-name
        // hit does the name-only route run (an off-by-one local year,
        // which `pre_filter_series` tolerates). Each route walks a second
        // page only while no exact-name hit has turned up.
        let mut items: Vec<Value> = Vec::new();
        if let Some(year) = query.year {
            let y = year.to_string();
            items = self
                .walk_pages(
                    &["series", "name", &q, "year", &y],
                    SEARCH_PAGE_CAP,
                    &has_exact,
                )
                .await?;
        }
        if !has_exact(&items) {
            match self
                .walk_pages(&["series", "name", &q], SEARCH_PAGE_CAP, &has_exact)
                .await
            {
                Ok(more) => items.extend(more),
                // Keep the year-route results when the widening fails.
                Err(e) if !items.is_empty() => {
                    tracing::debug!(error = %e, "gcd: name-only series search failed; keeping year results");
                }
                Err(e) => return Err(e),
            }
        }
        let mut ranked: Vec<(bool, SeriesCandidate, &Value)> = Vec::new();
        for item in &items {
            let Some(c) = series_to_candidate(item) else {
                continue;
            };
            if ranked
                .iter()
                .any(|(_, x, _)| x.external_id == c.external_id)
            {
                continue;
            }
            ranked.push((is_exact(item), c, item));
        }
        ranked.sort_by_key(|(exact, _, _)| !*exact);
        ranked.truncate(limit);
        let mut out = Vec::with_capacity(ranked.len());
        for (_, mut c, item) in ranked {
            // Free: the payload already carries the issue index.
            self.cache_series_from_search(&c.external_id, item).await;
            if c.publisher.is_none()
                && let Some(pid) = related_id(item, &["publisher"], "publisher")
            {
                c.publisher = self.cached_publisher_name(&pid).await;
            }
            out.push(c);
        }
        Ok(out)
    }

    async fn search_issue(&self, query: &IssueQuery) -> ProviderResult<Vec<IssueCandidate>> {
        let number = canonical_issue_number(&query.issue_number);
        if number.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(series_id) = query.series_external_id.as_deref() {
            return self.search_issue_narrowed(series_id, &number).await;
        }
        let Some(name) = query
            .series_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(Vec::new());
        };
        let limit = query.limit.clamp(1, 100) as usize;
        self.search_issue_broad(name, &number, query.cover_year, limit)
            .await
    }

    async fn fetch_series(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let v = self.get_json(&["series", external_id]).await?;
        let summary = self.store_series_detail(external_id, &v).await;
        Ok(series_detail_metadata(&v, external_id, summary.publisher))
    }

    async fn fetch_issue(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let v = self.get_json(&["issue", external_id]).await?;
        Ok(self.issue_detail_to_metadata(&v, external_id).await)
    }

    fn enumerates_series_issues(&self) -> bool {
        true
    }

    async fn list_series_issue_numbers(
        &self,
        series_external_id: &str,
    ) -> ProviderResult<Vec<String>> {
        // The series index lists every active issue's descriptor — one
        // request, or none when a search / series apply just cached it.
        // (The paginated overview would cost ⌈n/50⌉.) Variants repeat
        // their parent's number; dedupe.
        let index = self.series_index(series_external_id).await?;
        let mut seen = HashSet::new();
        let mut numbers = Vec::new();
        for e in index {
            if e.number.is_empty() {
                continue;
            }
            let canon = canonical_issue_number(&e.number);
            if seen.insert(canon.clone()) {
                numbers.push(canon);
            }
        }
        Ok(numbers)
    }

    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>> {
        // files1.comics.org CDN — no auth, no rate-limit slot. https-only
        // + magic-sniffed like every provider cover (SE-6). The CDN sits
        // behind a Cloudflare challenge: the shared fetch path detects
        // it, remembers the host, and this returns `CoverUnavailable`
        // (no retries) so the apply records a clean skip.
        crate::metadata::writers::fetch_cover_bytes(url)
            .await
            .map_err(ProviderError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_from_url_handles_api_and_site_links() {
        assert_eq!(
            id_from_url(
                "https://www.comics.org/api/series/1482/?format=json",
                "series"
            ),
            Some("1482".into())
        );
        assert_eq!(
            id_from_url("https://www.comics.org/issue/16556/", "issue"),
            Some("16556".into())
        );
        assert_eq!(
            id_from_url("https://www.comics.org/api/series/", "series"),
            None
        );
        assert_eq!(id_from_url("https://x/api/issue/9/", "series"), None);
    }

    #[test]
    fn series_display_name_splits_year() {
        assert_eq!(
            split_series_display("Fantastic Four (1961 series)"),
            ("Fantastic Four".into(), Some(1961))
        );
        assert_eq!(
            split_series_display("Saga (2012)"),
            ("Saga".into(), Some(2012))
        );
        assert_eq!(split_series_display("2000 AD"), ("2000 AD".into(), None));
        assert_eq!(
            split_series_display("Daredevil (Vol. 2)"),
            ("Daredevil (Vol. 2)".into(), None)
        );
    }

    #[test]
    fn descriptor_number_strips_variant_and_title_suffixes() {
        assert_eq!(descriptor_number("1"), "1");
        assert_eq!(descriptor_number("1 [British]"), "1");
        assert_eq!(descriptor_number("402 [Direct Edition]"), "402");
        assert_eq!(descriptor_number("1 - Capítulo Uno"), "1");
        assert_eq!(descriptor_number("v2#3"), "3");
        assert_eq!(descriptor_number("[nn]"), "[nn]");
        assert_eq!(descriptor_number(""), "");
    }

    #[test]
    fn key_dates_parse_strict_and_lenient() {
        assert_eq!(
            parse_key_date("1961-11-00", true),
            NaiveDate::from_ymd_opt(1961, 11, 1)
        );
        assert_eq!(parse_key_date("1867-00-00", true), None);
        assert_eq!(
            parse_key_date("1867-00-00", false),
            NaiveDate::from_ymd_opt(1867, 1, 1)
        );
        assert_eq!(parse_key_date("garbage", false), None);
    }

    #[test]
    fn prices_and_ratings_map_conservatively() {
        assert_eq!(parse_price("0.10 USD"), Some(0.10));
        assert_eq!(parse_price("2.99 USD; 3.99 CAD"), Some(2.99));
        assert_eq!(parse_price("9d [0-0-9 GBP]"), None);
        assert_eq!(parse_price("[none]"), None);
        assert_eq!(
            map_rating("Rated M / Mature").as_deref(),
            Some("Mature 17+")
        );
        assert_eq!(map_rating("Approved by the Comics Code Authority"), None);
    }

    #[test]
    fn credit_names_drop_placeholders_and_uncertain_credits() {
        let got = parse_credit_names(
            "Stan Lee (signed as Stan Lee [early- to mid-career]); Sol Brodsky ? (see notes); None; ? (photo); typeset",
        );
        let names: Vec<_> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Stan Lee"]);
        let got = parse_credit_names("Fiona Staples (credited) (signed as FS)");
        assert_eq!(got[0].0, "Fiona Staples");
        assert!(got[0].1.contains("credited"));
    }

    #[test]
    fn characters_split_teams_members_and_alter_egos() {
        let (chars, teams) = parse_characters(
            "Fantastic Four (introduction, origin) [The Human Torch [Johnny Storm]; Invisible Girl [Susan Storm]]; Mole Man [Harvey Elder] (introduction); Giganto (antagonist); Central City Police Department [Pete; unnamed members]; VILLAINS: Doctor Doom",
        );
        let team_names: Vec<_> = teams.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            team_names,
            vec!["Fantastic Four", "Central City Police Department"]
        );
        assert!(teams[0].is_first_appearance);
        let char_names: Vec<_> = chars.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            char_names,
            vec![
                "The Human Torch",
                "Invisible Girl",
                "Mole Man",
                "Giganto",
                "Pete",
                "Doctor Doom"
            ]
        );
        let mole = chars.iter().find(|c| c.name == "Mole Man").unwrap();
        assert!(mole.is_first_appearance);
        // "first cover appearance" is not a first appearance.
        let (c, _) = parse_characters("Alana (featured) (first cover appearance of Alana)");
        assert!(!c[0].is_first_appearance);
    }

    #[test]
    fn issue_detail_maps_story_credits_and_issue_editor() {
        let v = json!({
            "api_url": "https://www.comics.org/api/issue/16556/?format=json",
            "series_name": "Fantastic Four (1961 series)",
            "number": "1",
            "volume": "1",
            "title": "",
            "key_date": "1961-11-00",
            "on_sale_date": "1961-08-08",
            "price": "0.10 USD",
            "page_count": "36.000",
            "editing": "Stan Lee (editor)",
            "rating": "Approved by the Comics Code Authority",
            "barcode": "",
            "isbn": "",
            "series": "https://www.comics.org/api/series/1482/?format=json",
            "keywords": "",
            "cover": "https://files1.comics.org//img/gcd/covers_by_id/21/w400/21867.jpg",
            "story_set": [
                {"type": "comic story", "sequence_number": 2, "title": "The Fantastic Four!",
                 "script": "Stan Lee", "pencils": "Jack Kirby", "inks": "George Klein; Sol Brodsky ? (see notes)",
                 "colors": "Stan Goldberg", "letters": "Artie Simek", "editing": "None",
                 "genre": "superhero", "characters": "Mole Man [Harvey Elder] (introduction)",
                 "synopsis": "A rocket trip."},
                {"type": "cover", "sequence_number": 0, "pencils": "Jack Kirby", "inks": "George Klein (see notes)",
                 "script": "Stan Lee", "characters": "", "genre": "superhero"}
            ]
        });
        let m = issue_detail_metadata(&v, "16556");
        assert_eq!(m.series_name.as_deref(), Some("Fantastic Four"));
        assert_eq!(m.year_began, Some(1961));
        assert_eq!(m.series_external_id.as_deref(), Some("1482"));
        assert_eq!(m.title.as_deref(), Some("The Fantastic Four!"));
        assert_eq!(m.cover_date, NaiveDate::from_ymd_opt(1961, 11, 1));
        assert_eq!(m.store_date, NaiveDate::from_ymd_opt(1961, 8, 8));
        assert_eq!(m.page_count, Some(36));
        assert_eq!(m.price, Some(0.10));
        assert_eq!(m.age_rating, None);
        assert_eq!(m.volume, Some(1));
        assert_eq!(m.genres, vec!["Superhero"]);
        let has =
            |name: &str, role: &str| m.credits.iter().any(|c| c.name == name && c.role == role);
        assert!(has("Stan Lee", "Editor"));
        assert!(has("Stan Lee", "Writer"));
        assert!(has("Jack Kirby", "Penciller"));
        assert!(has("Jack Kirby", "CoverArtist"));
        assert!(has("George Klein", "Inker"));
        assert!(has("George Klein", "CoverArtist"));
        assert!(!m.credits.iter().any(|c| c.name.contains("Brodsky")));
        assert_eq!(m.characters.len(), 1);
        assert!(m.characters[0].is_first_appearance);
        assert_eq!(m.identifiers.len(), 1);
        assert_eq!(m.identifiers[0].source, Source::Gcd);
    }

    #[test]
    fn issue_level_production_staff_are_not_editors() {
        let v = json!({
            "api_url": "https://www.comics.org/api/issue/1/",
            "editing": "Eric Stephenson (credited) (coordinator); Drew Gill (credited) (art director); Jane Doe (editor-in-chief)",
        });
        let m = issue_detail_metadata(&v, "1");
        let editors: Vec<_> = m.credits.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(editors, vec!["Jane Doe"]);
    }

    #[test]
    fn tolerant_reads_survive_renamed_and_retyped_fields() {
        // `api_url` renamed to `url`, `year_began` to `start_year` and
        // re-typed as a string, an unknown field added.
        let v = json!({
            "url": "https://www.comics.org/api/series/63051/?format=json",
            "name": "Saga",
            "start_year": "2012",
            "brand_new_field": {"x": 1},
            "issue_descriptors": ["1 [1st Printing]", "1 [2nd Printing]", "2"]
        });
        let c = series_to_candidate(&v).expect("candidate");
        assert_eq!(c.external_id, "63051");
        assert_eq!(c.year, Some(2012));
        assert_eq!(c.issue_count, Some(2));
        // No recoverable id → skipped, not an error.
        assert!(series_to_candidate(&json!({"name": "Orphan"})).is_none());
        // Envelope without `results` → empty, not an error.
        assert!(result_items(&json!({"detail": "x"})).is_empty());
        assert_eq!(result_items(&json!([{"a": 1}])).len(), 1);
    }

    #[test]
    fn series_index_pairs_urls_with_descriptors() {
        let v = json!({
            "active_issues": [
                "https://www.comics.org/api/issue/10/?format=json",
                "https://www.comics.org/api/issue/11/?format=json",
                "https://www.comics.org/api/issue/12/?format=json"
            ],
            "issue_descriptors": ["1", "1 [British]", "2"]
        });
        let e: Vec<(String, String)> = series_index_entries(&v)
            .into_iter()
            .map(|e| (e.id, e.number))
            .collect();
        assert_eq!(
            e,
            vec![
                ("10".to_owned(), "1".to_owned()),
                ("11".to_owned(), "1".to_owned()),
                ("12".to_owned(), "2".to_owned())
            ]
        );
        assert_eq!(series_index_entries(&v)[1].descriptor, "1 [British]");
        // Misaligned lists → ids with empty numbers (never match).
        let bad = json!({"active_issues": ["https://x/api/issue/10/"], "issue_descriptors": []});
        let e = series_index_entries(&bad);
        assert_eq!((e[0].id.as_str(), e[0].number.as_str()), ("10", ""));
    }

    #[test]
    fn distinct_position_skips_variants_and_estimates_the_overview_page() {
        let entry = |id: &str, d: &str| IndexEntry {
            id: id.into(),
            number: descriptor_number(d),
            descriptor: d.into(),
        };
        let idx = vec![
            entry("1", "1"),
            entry("2", "1 [Cover B]"),
            entry("3", "2"),
            entry("4", "0"),
            entry("5", "3"),
        ];
        assert_eq!(distinct_number_position(&idx, "1"), Some(0));
        assert_eq!(distinct_number_position(&idx, "2"), Some(1));
        assert_eq!(distinct_number_position(&idx, "0"), Some(2));
        assert_eq!(distinct_number_position(&idx, "3"), Some(3));
        assert_eq!(distinct_number_position(&idx, "99"), None);
    }

    #[test]
    fn publication_dates_parse_to_cover_precision() {
        assert_eq!(
            parse_publication_date("November 1961"),
            Some((1961, Some(11), None))
        );
        assert_eq!(
            parse_publication_date("5 March 1977"),
            Some((1977, Some(3), Some(5)))
        );
        assert_eq!(
            parse_publication_date("March 5, 1977"),
            Some((1977, Some(3), Some(5)))
        );
        assert_eq!(
            parse_publication_date("January-February 2003"),
            Some((2003, Some(1), None))
        );
        assert_eq!(
            parse_publication_date("Spring 1962"),
            Some((1962, None, None))
        );
        assert_eq!(
            parse_publication_date("março de 2024"),
            Some((2024, None, None))
        );
        assert_eq!(
            parse_publication_date("Sept. 1985"),
            Some((1985, Some(9), None))
        );
        assert_eq!(parse_publication_date("[nd]"), None);
    }

    #[test]
    fn partial_dates_never_invent_precision() {
        // key_date partial forms.
        assert_eq!(
            parse_key_date("2003-01", true),
            NaiveDate::from_ymd_opt(2003, 1, 1)
        );
        assert_eq!(
            parse_key_date("2003-01-00", true),
            NaiveDate::from_ymd_opt(2003, 1, 1)
        );
        assert_eq!(parse_key_date("2003", true), None);
        // On-sale: only a full date becomes a store date.
        assert_eq!(parse_full_date("2003-01"), None);
        assert_eq!(parse_full_date("2003-01-00"), None);
        assert_eq!(
            parse_full_date("2003-01-22"),
            NaiveDate::from_ymd_opt(2003, 1, 22)
        );
        // The printed cover month beats a key date that is really the
        // on-sale day.
        let v = json!({"publication_date": "January 2003", "key_date": "2003-01-22"});
        assert_eq!(cover_date_of(&v, true), NaiveDate::from_ymd_opt(2003, 1, 1));
        // Unparseable publication date → key date.
        let v = json!({"publication_date": "Holiday 1985", "key_date": "1985-12-00"});
        assert_eq!(
            cover_date_of(&v, true),
            NaiveDate::from_ymd_opt(1985, 12, 1)
        );
        // Year-only: strict refuses, lenient keeps the year.
        let v = json!({"publication_date": "1867", "key_date": "1867-00-00"});
        assert_eq!(cover_date_of(&v, true), None);
        assert_eq!(
            cover_date_of(&v, false),
            NaiveDate::from_ymd_opt(1867, 1, 1)
        );
    }

    #[test]
    fn prices_parse_comma_decimals() {
        assert_eq!(parse_price("40,90 BRL"), Some(40.90));
        assert_eq!(parse_price("1,250.00 JPY"), Some(1250.0));
        assert_eq!(parse_price("[none] (see notes)"), None);
    }

    #[test]
    fn ratings_normalise_publisher_free_text() {
        let r = |s: &str| map_rating(s);
        assert_eq!(r("Rated T+").as_deref(), Some("Teen"));
        assert_eq!(r("T+").as_deref(), Some("Teen"));
        assert_eq!(r("Parental Advisory").as_deref(), Some("Teen"));
        assert_eq!(r("Marvel PSR").as_deref(), Some("Teen"));
        assert_eq!(r("PSR").as_deref(), Some("Teen"));
        assert_eq!(
            r("Suggested for Mature Readers").as_deref(),
            Some("Mature 17+")
        );
        assert_eq!(r("Rated M").as_deref(), Some("Mature 17+"));
        assert_eq!(r("Explicit Content").as_deref(), Some("Adults Only 18+"));
        assert_eq!(r("Ages 18+").as_deref(), Some("Adults Only 18+"));
        assert_eq!(r("Ages 12+").as_deref(), Some("Everyone 10+"));
        assert_eq!(r("16+").as_deref(), Some("Teen"));
        assert_eq!(r("All Ages").as_deref(), Some("Everyone"));
        assert_eq!(r("Rated E").as_deref(), Some("Everyone"));
        assert_eq!(r("Rating Pending").as_deref(), Some("Rating Pending"));
        assert_eq!(r("Approved by the Comics Code Authority"), None);
        assert_eq!(r("[none]"), None);
        assert_eq!(r("a sticker on the cover"), None);
    }

    #[test]
    fn isbn_and_barcode_normalise() {
        assert_eq!(
            normalize_isbn("978-1-58240-711-1; 1-58240-711-6").as_deref(),
            Some("9781582407111")
        );
        assert_eq!(
            normalize_isbn("0-87135-123-X").as_deref(),
            Some("087135123X")
        );
        assert_eq!(normalize_isbn("none"), None);
        let id =
            |raw: &str, has_isbn: bool| barcode_identifier(raw, has_isbn).map(|i| (i.source, i.id));
        assert_eq!(
            id("70985310507700111", false),
            Some((Source::Upc, "70985310507700111".to_owned()))
        );
        assert_eq!(
            id("9781582407111", false),
            Some((Source::Isbn, "9781582407111".to_owned()))
        );
        assert_eq!(
            id("9781582407111", true),
            Some((Source::Gtin, "9781582407111".to_owned()))
        );
        assert_eq!(
            id("4006381333931", false),
            Some((Source::Gtin, "4006381333931".to_owned()))
        );
        assert_eq!(id("12345", false), None);
    }

    #[test]
    fn publishing_format_normalises_for_series_type_and_matching() {
        let f = |pf: &str, b: &str| gcd_format(Some(pf), Some(b));
        let ongoing = f("was ongoing series", "saddle-stitched");
        assert_eq!(ongoing.series_type, Some("Ongoing Series"));
        assert_eq!(ongoing.format, None);
        assert_eq!(ongoing.match_hint(), Some("Ongoing Series"));
        let tpb = f("Collected Editions; Was Ongoing Series", "softcover");
        assert_eq!(
            (tpb.series_type, tpb.format),
            (Some("Trade Paperback"), Some("TPB"))
        );
        let hc = f("collected edition", "hardcover");
        assert_eq!(hc.format, Some("Hardcover"));
        assert_eq!(f("limited series", "").format, Some("Limited Series"));
        assert_eq!(f("one-shot", "").format, Some("One-Shot"));
        assert_eq!(f("", "Hardcover").format, Some("Hardcover"));
        assert_eq!(f("", "softcover"), GcdFormat::default());
        // Both hints classify on the matcher's side.
        use crate::metadata::title_norm::{FormatClass, classify_format};
        assert_eq!(classify_format("Ongoing Series"), Some(FormatClass::Single));
        assert_eq!(classify_format("TPB"), Some(FormatClass::Collected));
        assert_eq!(classify_format("Hardcover"), Some(FormatClass::Collected));
    }

    #[test]
    fn brand_emblem_becomes_imprint_only_when_distinct() {
        assert_eq!(imprint_from_brand("Image", Some("Image")), None);
        assert_eq!(imprint_from_brand("MC", Some("Marvel")), None);
        assert_eq!(
            imprint_from_brand("Skybound Five Years 2010-2015", Some("Image")),
            None
        );
        assert_eq!(imprint_from_brand("Marvel Comics", Some("Marvel")), None);
        assert_eq!(
            imprint_from_brand("Vertigo", Some("DC")).as_deref(),
            Some("Vertigo")
        );
        assert_eq!(
            imprint_from_brand("Skybound", Some("Image")).as_deref(),
            Some("Skybound")
        );
    }

    #[test]
    fn variant_artist_parses_only_unambiguous_names() {
        let a = |s: &str| parse_variant_artist(s);
        assert_eq!(
            a("2015 SDCC Exclusive Skybound 5th Anniversary Box Set Color Cover - Cory Walker")
                .as_deref(),
            Some("Cory Walker")
        );
        assert_eq!(a("Cover B by Ryan Ottley").as_deref(), Some("Ryan Ottley"));
        assert_eq!(
            a("Chris Giarrusso Cover").as_deref(),
            Some("Chris Giarrusso")
        );
        assert_eq!(
            a("Lorenzo De Felici Cover").as_deref(),
            Some("Lorenzo De Felici")
        );
        assert_eq!(a("Cover A"), None);
        assert_eq!(a("Retailer Variant"), None);
        assert_eq!(a("Image Tribute Cover"), None);
        assert_eq!(a("Wraparound Cover"), None);
        assert_eq!(a("2nd Printing"), None);
        assert_eq!(a("Ninth Printing - Amazon Prime video"), None);
        assert_eq!(a("Larry's Wonderful World of Comics Exclusive"), None);
    }

    #[test]
    fn credit_parser_handles_gcd_conventions() {
        let names = |raw: &str| -> Vec<String> {
            parse_credit_names(raw)
                .into_iter()
                .map(|(n, _)| n)
                .collect()
        };
        assert_eq!(
            names("Cory Walker (signed as CW [curly])"),
            vec!["Cory Walker"]
        );
        assert_eq!(names("Jack Kirby [as Jack Curtiss]"), vec!["Jack Kirby"]);
        assert_eq!(names("Jack Kirby (?); Steve Ditko"), vec!["Steve Ditko"]);
        assert_eq!(names("[none]"), Vec::<String>::new());
        assert_eq!(names("None (see notes)"), Vec::<String>::new());
        assert_eq!(names("Anonymous; ?"), Vec::<String>::new());
        assert_eq!(
            names("Robert Kirkman (credited); Cory Walker (credited) (layouts)"),
            vec!["Robert Kirkman", "Cory Walker"]
        );
    }

    #[test]
    fn character_deaths_flag_only_plain_death_notes() {
        let (c, _) = parse_characters(
            "David Hiles (second appearance, death in flashforward); Bob (death); Al (cameo, death)",
        );
        let died = |n: &str| c.iter().find(|e| e.name == n).unwrap().died_in_issue;
        assert_eq!(died("David Hiles"), None);
        assert_eq!(died("Bob"), Some(true));
        assert_eq!(died("Al"), Some(true));
    }

    #[test]
    fn main_story_is_the_longest_comic_story() {
        let v = json!({
            "api_url": "https://www.comics.org/api/issue/5/",
            "keywords": "Halloween",
            "story_set": [
                {"type": "comic story", "sequence_number": 1, "page_count": "6.000",
                 "title": "Backup", "synopsis": "Short one.", "keywords": "Ghosts; halloween"},
                {"type": "comic story", "sequence_number": 2, "page_count": "16.000",
                 "title": "Lead", "synopsis": "The big one."},
                {"type": "text story", "sequence_number": 3, "page_count": "2.000",
                 "title": "Prose", "synopsis": "Text page.", "script": "Someone Else"}
            ]
        });
        let m = issue_detail_metadata(&v, "5");
        assert_eq!(m.title.as_deref(), Some("Lead"));
        assert_eq!(m.description.as_deref(), Some("The big one.\n\nShort one."));
        assert_eq!(m.tags, vec!["Halloween", "Ghosts"]);
        assert!(!m.credits.iter().any(|c| c.name == "Someone Else"));
    }

    #[test]
    fn search_name_and_name_key() {
        assert_eq!(search_name("Batman/Superman"), "Superman");
        assert_eq!(search_name("Spider-Man/Deadpool"), "Spider-Man");
        assert_eq!(search_name("Saga"), "Saga");
        assert_eq!(name_key("The Amazing Spider-Man"), "amazing spider man");
        assert_eq!(name_key("Batman/Superman"), name_key("Batman / Superman"));
        assert_eq!(name_key("The"), "the");
    }

    #[test]
    fn overview_row_maps_cover_dates_and_main_story_title() {
        let row = json!({
            "issue_id": 276820, "descriptor": "2", "number": "2",
            "publication_date": "February 2003", "on_sale_date": "2003-02-19",
            "key_date": "2003-02-19",
            "cover_url": "https://files1.comics.org//img/gcd/covers_by_id/258/w400/258243.jpg",
            "longest_story": {"type": "comic story", "title": "Origins", "page_count": "22.000"}
        });
        let summary = SeriesSummary {
            name: Some("Invincible".into()),
            year_began: Some(2003),
            publishing_format: Some("was ongoing series".into()),
            ..Default::default()
        };
        let c = overview_row_to_candidate(&row, "17010", &summary).expect("candidate");
        assert_eq!(c.external_id, "276820");
        assert_eq!(c.issue_number.as_deref(), Some("2"));
        assert_eq!(c.name.as_deref(), Some("Origins"));
        assert_eq!(c.cover_date, NaiveDate::from_ymd_opt(2003, 2, 1));
        assert_eq!(c.series_name.as_deref(), Some("Invincible"));
        assert_eq!(c.series_year, Some(2003));
        assert_eq!(c.series_external_id.as_deref(), Some("17010"));
        assert_eq!(c.format.as_deref(), Some("Ongoing Series"));
        assert!(c.cover_image_url.unwrap().contains("258243"));
        // A renamed id field still resolves through the self-link.
        let renamed = json!({"api_url": "https://www.comics.org/api/issue/9/", "number": "9"});
        assert_eq!(
            overview_row_to_candidate(&renamed, "1", &SeriesSummary::default())
                .unwrap()
                .external_id,
            "9"
        );
    }

    #[test]
    fn series_summary_folds_publisher_fallback_imprint_and_format() {
        let v = json!({
            "brand_emblem": "Vertigo", "indicia_publisher": "DC Comics Inc.",
        });
        let mut m = GenericMetadata::default();
        apply_series_summary(
            &mut m,
            &v,
            &SeriesSummary {
                publisher: None,
                publishing_format: Some("limited series".into()),
                language: Some("en".into()),
                ..Default::default()
            },
        );
        assert_eq!(m.publisher.as_deref(), Some("DC Comics Inc."));
        assert_eq!(m.imprint.as_deref(), Some("Vertigo"));
        assert_eq!(m.series_type.as_deref(), Some("Limited Series"));
        assert_eq!(m.format.as_deref(), Some("Limited Series"));
        assert_eq!(m.language_code.as_deref(), Some("en"));
        let mut m = GenericMetadata::default();
        apply_series_summary(
            &mut m,
            &v,
            &SeriesSummary {
                publisher: Some("Vertigo".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            m.imprint, None,
            "the publisher's own emblem is not an imprint"
        );
    }
}
