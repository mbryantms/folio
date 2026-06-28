//! Refresh entrypoint: re-fetch a CBL by `source_kind` and apply the
//! result via [`super::import::apply_parsed`].
//!
//! Three paths:
//!
//!   - `'upload'` — no remote source. Refresh is a re-match against the
//!     stored `raw_xml` (calls [`super::import::rematch_existing`]).
//!   - `'url'` — HTTP GET with conditional `If-None-Match`. 304 → skip
//!     re-parse; otherwise persist the new bytes.
//!   - `'catalog'` — fetch the path from the configured catalog source;
//!     compare blob SHAs to skip when unchanged.

use entity::{catalog_source, cbl_list};
use futures::StreamExt;
use sea_orm::{ConnectionTrait, EntityTrait, TransactionTrait};
use uuid::Uuid;

use super::catalog;
use super::import::{self, ImportSummary, RefreshTrigger};

const MAX_URL_CBL_BYTES: usize = 4 * 1024 * 1024;
const CBL_XML_TYPES: &[&str] = &[
    "application/xml",
    "text/xml",
    "application/x-cbl",
    "application/vnd.comicbooklist+xml",
];

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("list not found")]
    NotFound,
    #[error("catalog: {0}")]
    Catalog(#[from] catalog::CatalogError),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("DB: {0}")]
    Db(#[from] sea_orm::DbErr),
}

pub async fn refresh(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    trigger: RefreshTrigger,
    force: bool,
) -> Result<ImportSummary, RefreshError> {
    let list = cbl_list::Entity::find_by_id(list_id)
        .one(db)
        .await?
        .ok_or(RefreshError::NotFound)?;

    match list.source_kind.as_str() {
        "upload" => {
            // No remote — re-parse the stored XML and re-match. Useful
            // after a library scan adds new issues that previously
            // missed entries can now match against.
            let parsed = parsers::cbl::parse(list.raw_xml.as_bytes())
                .map_err(|e| RefreshError::Parse(e.to_string()))?;
            let summary =
                import::apply_parsed(db, list.id, &parsed, &list.raw_xml, None, trigger).await?;
            Ok(summary)
        }
        "url" => fetch_url_and_apply(db, &list, trigger, force).await,
        "catalog" => fetch_catalog_and_apply(db, &list, trigger, force).await,
        other => Err(RefreshError::Http(format!("unknown source_kind: {other}"))),
    }
}

/// What a refresh *would* change, computed without persisting anything.
/// Returned by [`check`]; mirrors the structural counts of a real refresh's
/// [`ImportSummary`].
#[derive(Debug, Clone, Default)]
pub struct CheckOutcome {
    pub upstream_changed: bool,
    pub added: i32,
    pub removed: i32,
    pub reordered: i32,
    pub rematched: i32,
}

impl CheckOutcome {
    /// Whether applying this refresh would alter the list (new/removed/
    /// reordered books, or entries that would newly resolve).
    pub fn has_changes(&self) -> bool {
        self.upstream_changed
            || self.added > 0
            || self.removed > 0
            || self.reordered > 0
            || self.rematched > 0
    }
}

/// Dry-run a refresh: fetch the source and compute the structural diff
/// **without persisting anything**. Runs [`import::apply_parsed`] inside a
/// transaction that is always rolled back, so the returned counts reflect
/// exactly what a real [`refresh`] would change — including the matcher
/// re-run — but no `cbl_entries`, `cbl_refresh_log`, or list-row writes
/// survive. Powers the "Check for updates" review flow.
pub async fn check(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
) -> Result<CheckOutcome, RefreshError> {
    let list = cbl_list::Entity::find_by_id(list_id)
        .one(db)
        .await?
        .ok_or(RefreshError::NotFound)?;

    // Resolve the source to (parsed, raw_xml, blob_sha). `None` means the
    // source reports no change (HTTP 304 / identical catalog blob), so a
    // real refresh would only re-match — which we still surface below.
    let fetched: Option<(String, Option<String>)> = match list.source_kind.as_str() {
        "upload" => Some((list.raw_xml.clone(), None)),
        "url" => {
            let url = list
                .source_url
                .as_deref()
                .ok_or_else(|| RefreshError::Http("url source missing source_url".into()))?;
            match fetch_url_source(url, &list, false).await? {
                UrlFetch::NotModified => None,
                UrlFetch::Fetched(fetched) => {
                    if !is_allowed_cbl_response_type(
                        fetched
                            .headers
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok()),
                        url,
                    ) {
                        return Err(RefreshError::Http(
                            "URL source must return XML or a .cbl file".into(),
                        ));
                    }
                    let xml = String::from_utf8_lossy(&fetched.bytes).into_owned();
                    Some((xml, None))
                }
            }
        }
        "catalog" => {
            let source_id = list
                .catalog_source_id
                .ok_or_else(|| RefreshError::Http("catalog source not set".into()))?;
            let path = list
                .catalog_path
                .as_deref()
                .ok_or_else(|| RefreshError::Http("catalog path not set".into()))?;
            let source = catalog_source::Entity::find_by_id(source_id)
                .one(db)
                .await?
                .ok_or(RefreshError::NotFound)?;
            let blob = catalog::fetch_blob(db, &source, path, false).await?;
            if list.github_blob_sha.as_deref() == Some(blob.blob_sha.as_str()) {
                None
            } else {
                let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
                Some((xml, Some(blob.blob_sha)))
            }
        }
        other => return Err(RefreshError::Http(format!("unknown source_kind: {other}"))),
    };

    let Some((xml, blob_sha)) = fetched else {
        // Upstream unchanged. A real refresh would still re-match in case the
        // library gained issues; reflect that as the rematch delta only.
        let parsed = parsers::cbl::parse(list.raw_xml.as_bytes())
            .map_err(|e| RefreshError::Parse(e.to_string()))?;
        return dry_run_apply(db, list.id, &parsed, &list.raw_xml, None).await;
    };

    let parsed =
        parsers::cbl::parse(xml.as_bytes()).map_err(|e| RefreshError::Parse(e.to_string()))?;
    dry_run_apply(db, list.id, &parsed, &xml, blob_sha.as_deref()).await
}

/// Apply `parsed` inside a transaction and roll it back, returning only the
/// structural counts. The rollback discards every write `apply_parsed` makes
/// (entries, refresh-log row, list-row bumps).
async fn dry_run_apply(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    parsed: &parsers::cbl::ParsedCbl,
    raw_xml: &str,
    blob_sha: Option<&str>,
) -> Result<CheckOutcome, RefreshError> {
    let txn = db.begin().await?;
    let summary = import::apply_parsed(
        &txn,
        list_id,
        parsed,
        raw_xml,
        blob_sha,
        RefreshTrigger::Manual,
    )
    .await?;
    txn.rollback().await?;
    Ok(CheckOutcome {
        upstream_changed: summary.upstream_changed,
        added: summary.added,
        removed: summary.removed,
        reordered: summary.reordered,
        rematched: summary.rematched,
    })
}

async fn fetch_url_and_apply(
    db: &sea_orm::DatabaseConnection,
    list: &cbl_list::Model,
    trigger: RefreshTrigger,
    force: bool,
) -> Result<ImportSummary, RefreshError> {
    let url = list
        .source_url
        .as_deref()
        .ok_or_else(|| RefreshError::Http("url source missing source_url".into()))?;

    let fetched = match fetch_url_source(url, list, force).await? {
        UrlFetch::NotModified => {
            // Bytes unchanged — re-match only.
            let rematched = import::rematch_existing(db, list.id, trigger).await?;
            let mut summary = ImportSummary {
                list_id: list.id,
                upstream_changed: false,
                rematched,
                ..Default::default()
            };
            backfill_status_counts(db, list.id, &mut summary).await?;
            return Ok(summary);
        }
        UrlFetch::Fetched(fetched) => *fetched,
    };
    let etag = fetched
        .headers
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let last_modified = fetched
        .headers
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !is_allowed_cbl_response_type(
        fetched
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        url,
    ) {
        return Err(RefreshError::Http(
            "URL source must return XML or a .cbl file".into(),
        ));
    }
    let xml = String::from_utf8_lossy(&fetched.bytes).into_owned();
    let parsed =
        parsers::cbl::parse(xml.as_bytes()).map_err(|e| RefreshError::Parse(e.to_string()))?;
    let summary = import::apply_parsed(db, list.id, &parsed, &xml, None, trigger).await?;
    update_url_metadata(db, list.id, etag, last_modified).await?;
    Ok(summary)
}

enum UrlFetch {
    NotModified,
    Fetched(Box<crate::util::ssrf::FetchedBytes>),
}

async fn fetch_url_source(
    url: &str,
    list: &cbl_list::Model,
    force: bool,
) -> Result<UrlFetch, RefreshError> {
    if force || (list.source_etag.is_none() && list.source_last_modified.is_none()) {
        return crate::util::ssrf::fetch_public_bytes(
            url,
            MAX_URL_CBL_BYTES,
            std::time::Duration::from_secs(30),
            crate::build_info::USER_AGENT,
            2,
            true,
        )
        .await
        .map(|fetched| UrlFetch::Fetched(Box::new(fetched)))
        .map_err(|e| RefreshError::Http(e.to_string()));
    }

    // Conditional refresh needs request headers; keep it local but use the
    // same public-URL validation, redirect refusal, and byte cap.
    let parsed_url = crate::util::ssrf::validate_outbound_url(url)
        .map_err(|e| RefreshError::Http(e.to_string()))?;
    if let Some(host) = parsed_url.host_str() {
        let port = parsed_url.port_or_known_default().unwrap_or(443);
        crate::util::ssrf::check_host_resolves_public(host, port)
            .await
            .map_err(|e| RefreshError::Http(e.to_string()))?;
    }
    let client = reqwest::Client::builder()
        .user_agent(crate::build_info::USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| RefreshError::Http(e.to_string()))?;
    let mut req = client.get(parsed_url.clone());
    if let Some(etag) = list.source_etag.as_deref() {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(lm) = list.source_last_modified.as_deref() {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| RefreshError::Http(e.to_string()))?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(UrlFetch::NotModified);
    }
    if resp.status().is_redirection() {
        return crate::util::ssrf::fetch_public_bytes(
            url,
            MAX_URL_CBL_BYTES,
            std::time::Duration::from_secs(30),
            crate::build_info::USER_AGENT,
            2,
            true,
        )
        .await
        .map(|fetched| UrlFetch::Fetched(Box::new(fetched)))
        .map_err(|e| RefreshError::Http(e.to_string()));
    }
    if !resp.status().is_success() {
        return Err(RefreshError::Http(format!("status {}", resp.status())));
    }
    if resp
        .content_length()
        .is_some_and(|len| len > MAX_URL_CBL_BYTES as u64)
    {
        return Err(RefreshError::Http("exceeds 4 MiB".into()));
    }
    let headers = resp.headers().clone();
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| RefreshError::Http(e.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_URL_CBL_BYTES {
            return Err(RefreshError::Http("exceeds 4 MiB".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(UrlFetch::Fetched(Box::new(
        crate::util::ssrf::FetchedBytes {
            final_url: parsed_url,
            headers,
            bytes,
        },
    )))
}

fn is_allowed_cbl_response_type(content_type: Option<&str>, url: &str) -> bool {
    let has_cbl_path = url::Url::parse(url)
        .ok()
        .map(|u| u.path().to_ascii_lowercase().ends_with(".cbl"))
        .unwrap_or(false);
    let Some(content_type) = content_type else {
        return true;
    };
    let normalized = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if CBL_XML_TYPES.contains(&normalized.as_str()) {
        return true;
    }
    matches!(
        normalized.as_str(),
        "application/octet-stream" | "text/plain"
    ) && has_cbl_path
}

async fn fetch_catalog_and_apply(
    db: &sea_orm::DatabaseConnection,
    list: &cbl_list::Model,
    trigger: RefreshTrigger,
    force: bool,
) -> Result<ImportSummary, RefreshError> {
    let source_id = list
        .catalog_source_id
        .ok_or_else(|| RefreshError::Http("catalog source not set".into()))?;
    let path = list
        .catalog_path
        .as_deref()
        .ok_or_else(|| RefreshError::Http("catalog path not set".into()))?;
    let source = catalog_source::Entity::find_by_id(source_id)
        .one(db)
        .await?
        .ok_or(RefreshError::NotFound)?;

    let blob = catalog::fetch_blob(db, &source, path, force).await?;
    if !force
        && let Some(prev_sha) = list.github_blob_sha.as_deref()
        && prev_sha == blob.blob_sha
    {
        // Same SHA → upstream unchanged. Re-match only.
        let rematched = import::rematch_existing(db, list.id, trigger).await?;
        let mut summary = ImportSummary {
            list_id: list.id,
            upstream_changed: false,
            rematched,
            ..Default::default()
        };
        backfill_status_counts(db, list.id, &mut summary).await?;
        return Ok(summary);
    }
    let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
    let parsed =
        parsers::cbl::parse(xml.as_bytes()).map_err(|e| RefreshError::Parse(e.to_string()))?;
    let summary =
        import::apply_parsed(db, list.id, &parsed, &xml, Some(&blob.blob_sha), trigger).await?;
    Ok(summary)
}

async fn update_url_metadata<C: ConnectionTrait>(
    db: &C,
    list_id: Uuid,
    etag: Option<String>,
    last_modified: Option<String>,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    let Some(list) = cbl_list::Entity::find_by_id(list_id).one(db).await? else {
        return Ok(());
    };
    let mut am: cbl_list::ActiveModel = list.into();
    if etag.is_some() {
        am.source_etag = Set(etag);
    }
    if last_modified.is_some() {
        am.source_last_modified = Set(last_modified);
    }
    am.update(db).await?;
    Ok(())
}

async fn backfill_status_counts<C: ConnectionTrait>(
    db: &C,
    list_id: Uuid,
    summary: &mut ImportSummary,
) -> Result<(), sea_orm::DbErr> {
    use entity::cbl_entry;
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    let base = cbl_entry::Entity::find().filter(cbl_entry::Column::CblListId.eq(list_id));
    summary.matched = base
        .clone()
        .filter(cbl_entry::Column::MatchStatus.eq("matched"))
        .count(db)
        .await? as i32;
    summary.ambiguous = base
        .clone()
        .filter(cbl_entry::Column::MatchStatus.eq("ambiguous"))
        .count(db)
        .await? as i32;
    summary.missing = base
        .clone()
        .filter(cbl_entry::Column::MatchStatus.eq("missing"))
        .count(db)
        .await? as i32;
    summary.manual = base
        .filter(cbl_entry::Column::MatchStatus.eq("manual"))
        .count(db)
        .await? as i32;
    Ok(())
}
