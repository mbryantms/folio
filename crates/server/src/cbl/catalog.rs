//! GitHub-backed CBL catalog fetcher (saved-views M4).
//!
//! Talks to the Git Trees API to enumerate `.cbl` files in a configured
//! repo, caches the parsed tree on the `catalog_sources` row keyed by
//! the response ETag, and fetches raw blobs from `raw.githubusercontent.com`.
//!
//! Auth: defaults to anonymous (60 req/hr/IP). When the env var
//! `COMIC_GITHUB_TOKEN` is set, requests are sent as
//! `Authorization: Bearer <token>` for the higher 5000 req/hr limit.
//!
//! Cache lifetime: the index is considered fresh for [`INDEX_TTL`].
//! Stale entries refetch with `If-None-Match`; a 304 keeps the existing
//! cache and just bumps `last_indexed_at`.

use chrono::{Duration, Utc};
use entity::catalog_source;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const INDEX_TTL: Duration = Duration::hours(1);
const USER_AGENT: &str = concat!("Folio/", env!("CARGO_PKG_VERSION"));
const MAX_BLOB_BYTES: usize = 4 * 1024 * 1024;

/// Cached parse of a repo's tree, filtered to `*.cbl` entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndex {
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Path within the repo (e.g. `Image/Invincible Universe.cbl`).
    pub path: String,
    /// Filename minus the `.cbl` extension.
    pub name: String,
    /// First path segment — typically the publisher folder
    /// (`Marvel`, `DC`, `Image`, …).
    pub publisher: String,
    /// Git blob SHA. Reliable change signal — when it differs from the
    /// stored `cbl_lists.github_blob_sha`, the file mutated upstream.
    pub sha: String,
    pub size: u64,
}

/// One blob fetch result. Caller persists `bytes` + `blob_sha` and
/// notes upstream change when `blob_sha` differs from what's stored.
#[derive(Debug, Clone)]
pub struct FetchedBlob {
    pub bytes: Vec<u8>,
    pub blob_sha: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("rate-limited by GitHub")]
    RateLimited,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("payload too large: {actual} > {limit}")]
    TooLarge { actual: usize, limit: usize },
    #[error("DB error: {0}")]
    Db(#[from] sea_orm::DbErr),
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client init")
    })
}

fn auth_header() -> Option<String> {
    std::env::var("COMIC_GITHUB_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|t| format!("Bearer {t}"))
}

fn apply_auth(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(h) = auth_header() {
        builder.header(reqwest::header::AUTHORIZATION, h)
    } else {
        builder
    }
}

/// Re-fetch the repo tree if the cached index is older than [`INDEX_TTL`]
/// (or `force = true`). Returns the persisted `CatalogIndex`. The
/// `catalog_sources` row is updated with the new `index_etag` /
/// `index_json` / `last_indexed_at` on a successful refresh.
pub async fn refresh_index<C: ConnectionTrait>(
    db: &C,
    source: &catalog_source::Model,
    force: bool,
) -> Result<CatalogIndex, CatalogError> {
    if !source.enabled {
        return Err(CatalogError::NotFound("catalog source disabled".into()));
    }
    if !force
        && let Some(cached) = current_index(source)
        && let Some(last) = source.last_indexed_at
    {
        let age = Utc::now() - last.with_timezone(&Utc);
        if age < INDEX_TTL {
            return Ok(cached);
        }
    }

    let url = format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
        source.github_owner, source.github_repo, source.github_branch
    );
    let mut req = http_client().get(&url);
    if let Some(etag) = source.index_etag.as_deref() {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    req = apply_auth(req);
    let resp = req
        .send()
        .await
        .map_err(|e| CatalogError::Http(e.to_string()))?;

    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        // Cache hit — surface the existing index without rewriting.
        bump_last_indexed(db, source).await?;
        return current_index(source)
            .ok_or_else(|| CatalogError::Http("304 received but no cached index".into()));
    }
    if resp.status() == reqwest::StatusCode::FORBIDDEN
        || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(CatalogError::RateLimited);
    }
    if !resp.status().is_success() {
        return Err(CatalogError::Http(format!(
            "tree fetch returned {}",
            resp.status()
        )));
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let payload: GhTreeResp = resp
        .json()
        .await
        .map_err(|e| CatalogError::Http(format!("decode tree: {e}")))?;
    let index = build_index(&payload);

    let mut am: catalog_source::ActiveModel = source.clone().into();
    am.index_etag = Set(etag);
    am.index_json = Set(Some(serde_json::to_value(&index).unwrap_or_default()));
    am.last_indexed_at = Set(Some(Utc::now().fixed_offset()));
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(index)
}

/// Fetch one `.cbl` raw blob by path. Refreshes the index implicitly on
/// the first call so the path → SHA mapping is current; pass `force =
/// true` to skip the cache.
pub async fn fetch_blob<C: ConnectionTrait>(
    db: &C,
    source: &catalog_source::Model,
    path: &str,
    force_index_refresh: bool,
) -> Result<FetchedBlob, CatalogError> {
    let index = refresh_index(db, source, force_index_refresh).await?;
    let entry = index
        .entries
        .iter()
        .find(|e| e.path == path)
        .ok_or_else(|| CatalogError::NotFound(format!("{path} not in catalog")))?;
    if (entry.size as usize) > MAX_BLOB_BYTES {
        return Err(CatalogError::TooLarge {
            actual: entry.size as usize,
            limit: MAX_BLOB_BYTES,
        });
    }

    // Per-segment encoding so spaces / parens in filenames don't break
    // the URL. Path separators are passed through.
    let encoded_path = path
        .split('/')
        .map(|seg| {
            url::form_urlencoded::byte_serialize(seg.as_bytes())
                .collect::<String>()
                // form_urlencoded encodes ' ' as '+', but raw.githubusercontent.com
                // expects %20 in the path component.
                .replace('+', "%20")
        })
        .collect::<Vec<_>>()
        .join("/");
    let url = format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        source.github_owner, source.github_repo, source.github_branch, encoded_path,
    );
    let resp = apply_auth(http_client().get(&url))
        .send()
        .await
        .map_err(|e| CatalogError::Http(e.to_string()))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(CatalogError::NotFound(format!("blob {path}")));
    }
    if !resp.status().is_success() {
        return Err(CatalogError::Http(format!(
            "blob fetch returned {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CatalogError::Http(e.to_string()))?;
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(CatalogError::TooLarge {
            actual: bytes.len(),
            limit: MAX_BLOB_BYTES,
        });
    }
    Ok(FetchedBlob {
        bytes: bytes.to_vec(),
        blob_sha: entry.sha.clone(),
    })
}

fn current_index(source: &catalog_source::Model) -> Option<CatalogIndex> {
    source
        .index_json
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

async fn bump_last_indexed<C: ConnectionTrait>(
    db: &C,
    source: &catalog_source::Model,
) -> Result<(), sea_orm::DbErr> {
    let mut am: catalog_source::ActiveModel = source.clone().into();
    am.last_indexed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(db).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GhTreeResp {
    tree: Vec<GhTreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct GhTreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    #[serde(default)]
    size: Option<u64>,
}

fn build_index(resp: &GhTreeResp) -> CatalogIndex {
    if resp.truncated {
        // GitHub truncates at ~100k entries / 7 MB. The CBL catalogs
        // we know of are well under that, but log so a future
        // truncation surfaces in observability.
        tracing::warn!("catalog tree response was truncated; some entries may be missing");
    }
    let mut entries: Vec<CatalogEntry> = resp
        .tree
        .iter()
        .filter(|e| e.kind == "blob" && e.path.to_lowercase().ends_with(".cbl"))
        .map(|e| {
            let publisher = e.path.split('/').next().unwrap_or("Other").to_string();
            let name = std::path::Path::new(&e.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&e.path)
                .to_string();
            CatalogEntry {
                path: e.path.clone(),
                name,
                publisher,
                sha: e.sha.clone(),
                size: e.size.unwrap_or(0),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        a.publisher
            .to_lowercase()
            .cmp(&b.publisher.to_lowercase())
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    CatalogIndex { entries }
}

/// Test-only helper: parse a tree response without HTTP (for unit
/// coverage of the publisher-grouping / filtering logic).
#[cfg(test)]
pub fn build_index_for_test(json: &str) -> CatalogIndex {
    let resp: GhTreeResp = serde_json::from_str(json).expect("test fixture");
    build_index(&resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_index_filters_to_cbl_blobs() {
        let json = r#"{
          "tree": [
            { "path": "README.md", "type": "blob", "sha": "a", "size": 100 },
            { "path": "Marvel", "type": "tree", "sha": "b" },
            { "path": "Marvel/Avengers.cbl", "type": "blob", "sha": "c", "size": 1024 },
            { "path": "DC/Batman.cbl", "type": "blob", "sha": "d", "size": 2048 }
          ],
          "truncated": false
        }"#;
        let idx = build_index_for_test(json);
        assert_eq!(idx.entries.len(), 2);
        // Sorted by (publisher, name).
        assert_eq!(idx.entries[0].publisher, "DC");
        assert_eq!(idx.entries[0].name, "Batman");
        assert_eq!(idx.entries[0].path, "DC/Batman.cbl");
        assert_eq!(idx.entries[0].sha, "d");
        assert_eq!(idx.entries[1].publisher, "Marvel");
        assert_eq!(idx.entries[1].name, "Avengers");
    }

    #[test]
    fn build_index_handles_no_cbls() {
        let json = r#"{ "tree": [], "truncated": false }"#;
        let idx = build_index_for_test(json);
        assert!(idx.entries.is_empty());
    }
}
