//! Grand Comics Database client (`www.comics.org/api/`) — roadmap WP-6.1.
//!
//! The third [`MetadataProvider`], behind ComicVine and Metron in
//! priority. GCD's value is coverage the other two lack: Golden/Silver
//! Age runs, non-US editions, and per-story credits.
//!
//! ## API shape (verified against `apps/api/{urls,views,serializers}.py`
//! in `GrandComicsDatabase/gcd-django`, 2026-10-01)
//!
//! - `GET /api/series/name/{name}/[year/{year}/]` — series search
//!   (`name__icontains`, exact `year_began`), paged 50.
//! - `GET /api/series/{id}/` — series detail. Carries `active_issues`
//!   (issue API URLs) and the parallel `issue_descriptors` list
//!   (`"1"`, `"1 [British]"`, `"1 - Capítulo Uno"`), so one call
//!   enumerates a run — that is what `list_series_issue_numbers` and the
//!   narrowed issue search use.
//! - `GET /api/series/name/{name}/issue/{number}/[year/{year}/]` — issue
//!   search (`series__name__icontains`, exact `number`,
//!   `key_date__startswith=year`). Returns the slim `IssueOnly` shape
//!   (no cover, no key date).
//! - `GET /api/issue/{id}/` — issue detail with `story_set[]` (per-story
//!   credits/characters/genre/synopsis) and a `cover` image URL.
//! - `GET /api/publisher/{id}/` — publisher detail. Series/issue
//!   payloads only carry the publisher's API URL, so the name costs one
//!   extra request; it is cached in Redis (see [`SUMMARY_TTL_SECS`]).
//!
//! Relations are hyperlinks (`"series": "https://…/api/series/1482/?format=json"`);
//! ids are recovered from those URLs.
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

/// Issue details fetched per search to pick up covers + key dates (the
/// search endpoints carry neither). Bounds the per-search request cost.
const NARROW_HYDRATE_CAP: usize = 4;
const BROAD_HYDRATE_CAP: usize = 2;

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
pub struct SeriesSummary {
    pub name: Option<String>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    pub publishing_format: Option<String>,
    pub language: Option<String>,
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
    /// as a single path segment (series names carry spaces, `&`, `/`).
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

    /// One authenticated GET returning the parsed JSON body. Bucket
    /// reservation → retrying send → status classification; the last
    /// error / clear is recorded in Redis for the admin card.
    async fn get_json(&self, segments: &[&str]) -> ProviderResult<Value> {
        self.reserve_slot().await?;
        let url = self.api_url(segments)?;
        let opts = http::RequestOpts {
            deadline: Some(Instant::now() + REQUEST_DEADLINE),
            ..Default::default()
        };
        let query = [("format", "json")];
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
        budget::record_error(&self.inner.redis, Source::Gcd, &err.to_string()).await;
        Err(err)
    }

    async fn cache_get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let mut conn = self.inner.redis.clone();
        let raw: Option<String> = conn.get(key).await.ok().flatten();
        raw.and_then(|s| serde_json::from_str(&s).ok())
    }

    async fn cache_put<T: Serialize>(&self, key: &str, value: &T) {
        let Ok(raw) = serde_json::to_string(value) else {
            return;
        };
        let mut conn = self.inner.redis.clone();
        let _: Result<(), _> = conn.set_ex(key, raw, SUMMARY_TTL_SECS).await;
    }

    /// Publisher display name for a GCD publisher id, Redis-cached.
    /// Best-effort: any failure yields `None` (the field just stays
    /// empty) rather than failing the detail fetch that needed it.
    async fn publisher_name(&self, publisher_id: &str) -> Option<String> {
        let key = format!("metadata:gcd:publisher:{publisher_id}");
        if let Some(name) = self.cache_get::<String>(&key).await {
            return Some(name);
        }
        let v = self.get_json(&["publisher", publisher_id]).await.ok()?;
        let name = str_field(&v, &["name", "publisher_name", "title"])?;
        self.cache_put(&key, &name).await;
        Some(name)
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
        Some(self.store_series_summary(series_id, &v).await)
    }

    async fn store_series_summary(&self, series_id: &str, v: &Value) -> SeriesSummary {
        let summary = SeriesSummary {
            name: str_field(v, SERIES_NAME_KEYS),
            year_began: int_field(v, YEAR_BEGAN_KEYS).map(|n| n as i32),
            publisher: self.resolve_publisher(v).await,
            publishing_format: str_field(v, &["publishing_format", "series_type", "format"]),
            language: str_field(v, &["language", "language_code"]),
        };
        self.cache_put(&format!("metadata:gcd:series:{series_id}"), &summary)
            .await;
        summary
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

    async fn search_issue_narrowed(
        &self,
        series_id: &str,
        number: &str,
    ) -> ProviderResult<Vec<IssueCandidate>> {
        let series = self.get_json(&["series", series_id]).await?;
        // Opportunistically refresh the summary cache from the payload
        // we already paid for.
        let _ = self.store_series_summary(series_id, &series).await;
        let wanted = issue_number_key(number);
        let ids: Vec<String> = series_issue_entries(&series)
            .into_iter()
            .filter(|(_, n)| issue_number_key(n) == wanted)
            .map(|(id, _)| id)
            .take(NARROW_HYDRATE_CAP)
            .collect();
        self.hydrate_issues(&ids).await
    }

    async fn search_issue_broad(
        &self,
        series_name: &str,
        number: &str,
        cover_year: Option<i32>,
        limit: usize,
    ) -> ProviderResult<Vec<IssueCandidate>> {
        let year = cover_year.map(|y| y.to_string());
        let mut segments = vec!["series", "name", series_name, "issue", number];
        if let Some(y) = year.as_deref() {
            segments.extend(["year", y]);
        }
        let mut body = self.get_json(&segments).await?;
        // The year is GCD's key (cover) date, which is often a month
        // off the local value around New Year; retry without it rather
        // than report nothing.
        if year.is_some() && result_items(&body).is_empty() {
            body = self
                .get_json(&["series", "name", series_name, "issue", number])
                .await?;
        }
        let items = result_items(&body);
        let mut slim: Vec<IssueCandidate> = Vec::new();
        for item in items {
            if variant_parent_id(item).is_some() {
                continue;
            }
            if let Some(c) = issue_list_to_candidate(item) {
                slim.push(c);
            }
            if slim.len() >= limit {
                break;
            }
        }
        // Covers + key dates live only on the detail — hydrate the
        // leading few so the cover-hash discriminant has something to
        // compare; the rest stay text-only candidates.
        let hydrate_ids: Vec<String> = slim
            .iter()
            .take(BROAD_HYDRATE_CAP)
            .map(|c| c.external_id.clone())
            .collect();
        // Quota on hydration keeps the text-only candidates.
        let hydrated = self.hydrate_issues(&hydrate_ids).await.unwrap_or_default();
        let mut out: Vec<IssueCandidate> = Vec::with_capacity(slim.len());
        for c in slim {
            match hydrated.iter().find(|h| h.external_id == c.external_id) {
                Some(h) => out.push(h.clone()),
                None => out.push(c),
            }
        }
        Ok(out)
    }

    async fn issue_detail_to_metadata(&self, v: &Value, requested_id: &str) -> GenericMetadata {
        let mut m = issue_detail_metadata(v, requested_id);
        if let Some(series_id) = m.series_external_id.clone()
            && let Some(summary) = self.series_summary(&series_id).await
        {
            if m.series_name.is_none() {
                m.series_name = summary.name.clone();
            }
            if m.year_began.is_none() {
                m.year_began = summary.year_began;
            }
            m.publisher = summary.publisher.clone();
            m.language_code = summary.language.clone();
            m.series_type = summary.publishing_format.clone();
            m.format = summary
                .publishing_format
                .as_deref()
                .and_then(crate::metadata::title_norm::metron_series_type_format)
                .map(str::to_owned);
        }
        m
    }
}

// ───────── tolerant field access ─────────

const SERIES_NAME_KEYS: &[&str] = &["name", "series_name", "title"];
const YEAR_BEGAN_KEYS: &[&str] = &["year_began", "start_year", "year_start", "year"];
const SELF_URL_KEYS: &[&str] = &["api_url", "url", "resource_url", "self"];

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

fn issue_number_key(raw: &str) -> String {
    canonical_issue_number(raw).to_ascii_lowercase()
}

/// `(issue_id, issue_number)` for every active issue of a series
/// detail, pairing `active_issues[i]` with `issue_descriptors[i]`. When
/// the descriptor list is absent or misaligned, entries still come
/// back with an empty number (they just never match).
fn series_issue_entries(series: &Value) -> Vec<(String, String)> {
    let urls: Vec<&Value> = match pick(series, &["active_issues", "issues"]) {
        Some(Value::Array(a)) => a.iter().collect(),
        _ => Vec::new(),
    };
    let descriptors: Vec<String> = match pick(series, &["issue_descriptors", "descriptors"]) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|d| d.as_str().map(descriptor_number).unwrap_or_default())
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
            let number = match u {
                obj @ Value::Object(_) => {
                    str_field(obj, &["number", "descriptor"]).map(|d| descriptor_number(&d))
                }
                _ => None,
            }
            .or_else(|| aligned.then(|| descriptors[i].clone()))
            .unwrap_or_default();
            Some((id, number))
        })
        .collect()
}

/// GCD `key_date` (`"1961-11-00"`, `"1867-00-00"`). `strict` requires a
/// known month (day `00` → 1st); lenient falls back to Jan 1 of the
/// year so a candidate still carries its year for the matcher's gate.
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

/// A full `YYYY-MM-DD` only (GCD on-sale dates are often `YYYY-MM`).
fn parse_full_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

/// `"0.10 USD"` / `"2.99 USD; 3.99 CAD"` → `0.10`. Non-decimal prices
/// (`"9d [0-0-9 GBP]"`, `"[none]"`) → `None`.
fn parse_price(raw: &str) -> Option<f64> {
    let first = raw.split(';').next()?.trim();
    first.split_whitespace().next()?.parse::<f64>().ok()
}

/// GCD's free-text `rating` mapped onto the ComicInfo `AgeRating`
/// vocabulary. Comics-Code approval text and anything unrecognised →
/// `None` (better empty than junk in the sidecar).
fn map_rating(raw: &str) -> Option<String> {
    let r = raw.to_ascii_lowercase();
    let label = if r.contains("adults only") || r.contains("18+") {
        "Adults Only 18+"
    } else if r.contains("mature") || r.contains("17+") {
        "Mature 17+"
    } else if r.contains("teen") {
        "Teen"
    } else if r.contains("all ages") || r.contains("everyone") {
        "Everyone"
    } else {
        return None;
    };
    Some(label.to_owned())
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
];

/// One GCD credit string (`"Stan Lee (signed as …); Sol Brodsky ? (see
/// notes)"`) → `(name, annotations)` pairs. Placeholders (`None`, `?`,
/// `typeset`, `various`) and uncertain credits (a trailing `?`) are
/// dropped — GCD marks unconfirmed attributions that way and Folio
/// would rather omit than assert them.
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

/// Credits across the issue: issue-level editors + every comic story's
/// script/pencils/inks/colors/letters/editing + the cover's
/// pencils/inks as `CoverArtist`. Roles land in the ComicInfo
/// vocabulary [`crate::metadata::provider::canonicalize_role`] emits.
fn collect_credits(v: &Value) -> Vec<CreditCandidate> {
    let mut out = Vec::new();
    // Issue-level `editing` mixes editors with production staff
    // ("Drew Gill (art director)"); keep only plain or editor-annotated
    // names.
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
        if ty != "comic story" {
            continue;
        }
        for (keys, role) in [
            (&["script", "writer", "writers"][..], "Writer"),
            (&["pencils", "penciller", "pencillers"][..], "Penciller"),
            (&["inks", "inker", "inkers"][..], "Inker"),
            (&["colors", "colours", "colorist"][..], "Colorist"),
            (&["letters", "letterer"][..], "Letterer"),
            (&["editing", "editor"][..], "Editor"),
        ] {
            if let Some(raw) = str_field(story, keys) {
                for (name, _) in parse_credit_names(&raw) {
                    push_credit(&mut out, name, role);
                }
            }
        }
    }
    out
}

fn entity(name: String, first: bool) -> EntityCandidate {
    EntityCandidate {
        name,
        identifiers: Vec::new(),
        is_first_appearance: first,
        died_in_issue: None,
        disbanded_in_issue: None,
        position_in_arc: None,
    }
}

fn upsert_entity(list: &mut Vec<EntityCandidate>, name: String, first: bool) {
    if let Some(e) = list.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&name)) {
        e.is_first_appearance |= first;
    } else {
        list.push(entity(name, first));
    }
}

fn is_first_appearance(notes: &str) -> bool {
    notes.contains("first appearance") || notes.contains("introduction")
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
/// `Label: …` prefixes (`VILLAINS:`) are dropped.
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
                upsert_entity(&mut teams, head, first);
                for member in split_top_level(&group, ';') {
                    let (m_name, m_notes) = strip_annotations(&member);
                    if usable_entity_name(&m_name) {
                        upsert_entity(&mut characters, m_name, is_first_appearance(&m_notes));
                    }
                }
            }
            _ => upsert_entity(&mut characters, head, first),
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
    let publishing_format = str_field(v, &["publishing_format", "series_type", "format"]);
    let format = publishing_format.clone().or_else(|| {
        crate::metadata::title_norm::infer_format_from_title(&name, None).map(str::to_owned)
    });
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

fn issue_list_to_candidate(v: &Value) -> Option<IssueCandidate> {
    let external_id = entity_id(v, "issue")?;
    let (series_name, series_year) = match str_field(v, &["series_name"]) {
        Some(raw) => {
            let (n, y) = split_series_display(&raw);
            (Some(n), y)
        }
        None => (None, None),
    };
    let issue_number = str_field(v, &["number", "issue_number"])
        .or_else(|| str_field(v, &["descriptor"]).map(|d| descriptor_number(&d)));
    let cover_date = str_field(v, &["key_date"])
        .and_then(|d| parse_key_date(&d, false))
        .or_else(|| {
            // `publication_date` is free text ("November 1961"); the
            // trailing four-digit token is the year.
            str_field(v, &["publication_date", "cover_date"]).and_then(|p| {
                p.split_whitespace()
                    .rev()
                    .find_map(|t| (t.len() == 4).then(|| t.parse::<i32>().ok()).flatten())
                    .and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1))
            })
        });
    Some(IssueCandidate {
        source: Source::Gcd,
        external_url: canonical_url(Source::Gcd, "issue", &external_id),
        external_id,
        issue_number,
        name: str_field(v, &["title"]),
        cover_date,
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
    c.name = c.name.or_else(|| first_story_title(v));
    c.cover_image_url = cover_url(v);
    Some(c)
}

fn first_story_title(v: &Value) -> Option<String> {
    stories(v)
        .into_iter()
        .filter(|s| story_type(s) == "comic story")
        .find_map(|s| str_field(s, &["title"]))
}

/// Pure issue-detail mapping (no I/O). The client enriches the result
/// with the cached series summary (publisher, language, format).
pub(crate) fn issue_detail_metadata(v: &Value, requested_id: &str) -> GenericMetadata {
    let external_id = entity_id(v, "issue").unwrap_or_else(|| requested_id.to_owned());
    let mut identifiers = vec![Identifier::with_canonical_url(
        Source::Gcd,
        external_id.clone(),
        "issue",
    )];
    if let Some(isbn) = str_field(v, &["isbn"]) {
        identifiers.push(Identifier::new(Source::Isbn, isbn));
    }
    if let Some(barcode) = str_field(v, &["barcode", "upc"]) {
        let digits: String = barcode.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 12 {
            identifiers.push(Identifier::new(Source::Upc, digits));
        }
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
        .filter(|s| story_type(s) == "comic story")
        .collect();

    let synopses: Vec<String> = comic_stories
        .iter()
        .filter_map(|s| str_field(s, &["synopsis", "summary"]))
        .collect();
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
                upsert_entity(&mut characters, e.name, e.is_first_appearance);
            }
            for e in t {
                upsert_entity(&mut teams, e.name, e.is_first_appearance);
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
    let tags = dedup_ci(
        str_field(v, &["keywords", "tags"])
            .map(|k| split_list(&k))
            .unwrap_or_default(),
    );

    GenericMetadata {
        series_name,
        series_external_id: related_id(v, &["series", "series_url"], "series"),
        year_began: series_year,
        volume: int_field(v, &["volume"]).map(|n| n as i32),
        issue_number: str_field(v, &["number", "issue_number"])
            .or_else(|| str_field(v, &["descriptor"]).map(|d| descriptor_number(&d))),
        title: str_field(v, &["title"]).or_else(|| first_story_title(v)),
        cover_date: str_field(v, &["key_date"]).and_then(|d| parse_key_date(&d, true)),
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

/// Pure series-detail mapping. `publisher` is resolved by the caller.
pub(crate) fn series_detail_metadata(
    v: &Value,
    requested_id: &str,
    publisher: Option<String>,
) -> GenericMetadata {
    let external_id = entity_id(v, "series").unwrap_or_else(|| requested_id.to_owned());
    let raw_name = str_field(v, SERIES_NAME_KEYS);
    let (name, display_year) = match raw_name.as_deref() {
        Some(r) => {
            let (n, y) = split_series_display(r);
            (Some(n), y)
        }
        None => (None, None),
    };
    let publishing_format = str_field(v, &["publishing_format", "series_type", "format"]);
    let binding = str_field(v, &["binding"]);
    let format = publishing_format
        .as_deref()
        .and_then(crate::metadata::title_norm::metron_series_type_format)
        .or_else(|| {
            binding
                .as_deref()
                .and_then(crate::metadata::title_norm::metron_series_type_format)
        })
        .map(str::to_owned);
    GenericMetadata {
        series_name: name,
        series_type: publishing_format,
        format,
        year_began: int_field(v, YEAR_BEGAN_KEYS)
            .map(|n| n as i32)
            .or(display_year),
        year_end: int_field(v, &["year_ended", "year_end"]).map(|n| n as i32),
        publisher,
        notes: str_field(v, &["notes"]),
        language_code: str_field(v, &["language", "language_code"]),
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
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let limit = query.limit.clamp(1, 100) as usize;
        let mut out: Vec<SeriesCandidate> = Vec::new();
        let push_all = |body: &Value, out: &mut Vec<SeriesCandidate>| {
            for item in result_items(body) {
                if out.len() >= limit {
                    break;
                }
                if let Some(c) = series_to_candidate(item)
                    && !out.iter().any(|x| x.external_id == c.external_id)
                {
                    out.push(c);
                }
            }
        };
        // GCD's name search is a plain `icontains` sorted by name, so a
        // common title ("Batman") buries the exact run past page 1. The
        // exact-year route narrows that to a handful; the name-only page
        // then fills the remainder so an off-by-one local year (which
        // `pre_filter_series` tolerates) still has candidates.
        if let Some(year) = query.year {
            let y = year.to_string();
            let body = self.get_json(&["series", "name", name, "year", &y]).await?;
            push_all(&body, &mut out);
        }
        if out.len() < limit {
            let body = self.get_json(&["series", "name", name]).await?;
            push_all(&body, &mut out);
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
        let summary = self.store_series_summary(external_id, &v).await;
        Ok(series_detail_metadata(&v, external_id, summary.publisher))
    }

    async fn fetch_issue(&self, external_id: &str) -> ProviderResult<GenericMetadata> {
        let v = self.get_json(&["issue", external_id]).await?;
        Ok(self.issue_detail_to_metadata(&v, external_id).await)
    }

    async fn list_series_issue_numbers(
        &self,
        series_external_id: &str,
    ) -> ProviderResult<Vec<String>> {
        // One call: the series detail lists every active issue's
        // descriptor. Variants repeat their parent's number; dedupe.
        let v = self.get_json(&["series", series_external_id]).await?;
        let mut seen = HashSet::new();
        let mut numbers = Vec::new();
        for (_, n) in series_issue_entries(&v) {
            if n.is_empty() {
                continue;
            }
            let canon = canonical_issue_number(&n);
            if seen.insert(canon.clone()) {
                numbers.push(canon);
            }
        }
        if numbers.is_empty() && has_next_page(&v) {
            tracing::warn!(
                series_id = series_external_id,
                "gcd: series detail was paginated unexpectedly; issue enumeration skipped"
            );
        }
        Ok(numbers)
    }

    async fn fetch_cover(&self, url: &str) -> ProviderResult<Vec<u8>> {
        // files1.comics.org CDN — no auth, no rate-limit slot.
        let fetched = crate::util::ssrf::fetch_public_bytes(
            url,
            crate::util::ssrf::MAX_IMAGE_BYTES,
            Duration::from_secs(20),
            crate::build_info::USER_AGENT_COVER,
            2,
            false,
        )
        .await
        .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(fetched.bytes)
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
    fn series_entries_pair_urls_with_descriptors() {
        let v = json!({
            "active_issues": [
                "https://www.comics.org/api/issue/10/?format=json",
                "https://www.comics.org/api/issue/11/?format=json",
                "https://www.comics.org/api/issue/12/?format=json"
            ],
            "issue_descriptors": ["1", "1 [British]", "2"]
        });
        let e = series_issue_entries(&v);
        assert_eq!(
            e,
            vec![
                ("10".to_owned(), "1".to_owned()),
                ("11".to_owned(), "1".to_owned()),
                ("12".to_owned(), "2".to_owned())
            ]
        );
        // Misaligned lists → ids with empty numbers (never match).
        let bad = json!({"active_issues": ["https://x/api/issue/10/"], "issue_descriptors": []});
        assert_eq!(
            series_issue_entries(&bad),
            vec![("10".to_owned(), String::new())]
        );
    }
}
