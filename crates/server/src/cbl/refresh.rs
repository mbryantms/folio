//! Refresh entrypoint: re-fetch a CBL by `source_kind` and apply the
//! result via [`super::import::apply_parsed`].
//!
//! Three paths:
//!
//!   - `'upload'` — no remote source. Refresh is a re-match against the
//!     stored `raw_xml` (calls [`super::import::rematch_existing`]).
//!   - `'url'` — HTTP GET with conditional `If-None-Match` /
//!     `If-Modified-Since`, through the same vetted fetch as the import
//!     ([`super::fetch::fetch_cbl_url`]). 304 → skip re-parse; otherwise
//!     persist the new bytes.
//!   - `'catalog'` — fetch the path from the configured catalog source;
//!     compare blob SHAs to skip when unchanged.
//!
//! New content (a changed URL body or catalog blob) with more than
//! [`super::MAX_BOOKS_PER_LIST`] entries is refused
//! ([`RefreshError::TooManyEntries`]) before anything is written. Stored
//! content that has not changed is not re-checked, so a list already over
//! the cap keeps re-matching until its source changes.
//!
//! The remote part of one refresh runs first, under the caller's
//! [`FetchBudget`]; the entry replacement, the list-row update and the
//! refresh-log row then commit in one transaction, so an error or a
//! dropped request leaves the stored list exactly as it was.

use entity::{catalog_source, cbl_list};
use sea_orm::{ConnectionTrait, EntityTrait, TransactionTrait};
use uuid::Uuid;

use super::catalog;
use super::fetch::{
    CBL_SWEEP_BUDGET, FetchBudget, SWEEP_BUDGET, fetch_cbl_url, fetch_cbl_url_conditional,
};
use super::import::{self, ImportSummary, RefreshTrigger};
use crate::util::ssrf::{Conditional, FetchOutcome};

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
    /// The fetched file has more entries than an import accepts. Same
    /// message as the import refusal.
    #[error("{}", super::too_many_entries_message())]
    TooManyEntries,
    #[error("timed out fetching the list")]
    Timeout,
    /// A bulk request ran out of time before reaching this list.
    #[error("not attempted: the request ran out of time; run it again")]
    NotAttempted,
    #[error("DB: {0}")]
    Db(#[from] sea_orm::DbErr),
}

impl RefreshError {
    /// The text a response may carry: a database error's own text stays
    /// in the log.
    pub fn public_message(&self) -> String {
        match self {
            Self::Db(_) => crate::api::INTERNAL_ERROR_TEXT.to_owned(),
            Self::Catalog(c) => format!("catalog: {}", c.public_message()),
            other => other.to_string(),
        }
    }
}

/// Refresh one list. `budget` bounds the remote work: request paths pass
/// [`super::fetch::REQUEST_BUDGET`], the sweep [`SWEEP_BUDGET`].
pub async fn refresh(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    trigger: RefreshTrigger,
    force: bool,
    budget: FetchBudget,
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
            let parsed = parse_stored(&list.raw_xml)?;
            apply_atomically(db, list.id, &parsed, &list.raw_xml, None, trigger, None).await
        }
        "url" => fetch_url_and_apply(db, &list, trigger, force, budget).await,
        "catalog" => fetch_catalog_and_apply(db, &list, trigger, force, budget).await,
        other => Err(RefreshError::Http(format!("unknown source_kind: {other}"))),
    }
}

/// Replace the list's entries from `parsed` and record the run, in one
/// transaction: entries, list row, URL validators and the refresh-log row
/// commit together or not at all. No network work happens inside it.
async fn apply_atomically(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    parsed: &parsers::cbl::ParsedCbl,
    raw_xml: &str,
    blob_sha: Option<&str>,
    trigger: RefreshTrigger,
    url_validators: Option<(Option<String>, Option<String>)>,
) -> Result<ImportSummary, RefreshError> {
    let txn = db.begin().await?;
    let summary = import::apply_parsed(&txn, list_id, parsed, raw_xml, blob_sha, trigger).await?;
    if let Some((etag, last_modified)) = url_validators {
        update_url_metadata(&txn, list_id, etag, last_modified).await?;
    }
    txn.commit().await?;
    Ok(summary)
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
///
/// `caller_view` is `Some` for a caller who does not see the whole library
/// ([`crate::library::access::sees_whole_library`]): `rematched` is then
/// the entries that newly read as matched to that caller, snapshotted
/// inside the dry-run transaction before and after the apply, instead of
/// the global matcher count. `None` keeps the global count.
pub async fn check(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    caller_view: Option<&crate::library::access::VisibleLibraries>,
    budget: FetchBudget,
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
            match within_budget(budget, fetch_url_source(url, &list, false, budget)).await? {
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
            let blob = within_budget(budget, async {
                Ok::<_, RefreshError>(catalog::fetch_blob(db, &source, path, false).await?)
            })
            .await?;
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
        let parsed = parse_stored(&list.raw_xml)?;
        return dry_run_apply(db, list.id, &parsed, &list.raw_xml, None, caller_view).await;
    };

    // An upload's stored XML is not new content; a fetched body is, unless
    // its digest matches what is stored.
    let parsed = if list.source_kind == "upload" {
        parse_stored(&xml)?
    } else {
        parse_fetched(&xml, &list.raw_sha256)?
    };
    dry_run_apply(db, list.id, &parsed, &xml, blob_sha.as_deref(), caller_view).await
}

/// Apply `parsed` inside a transaction and roll it back, returning only the
/// structural counts. The rollback discards every write `apply_parsed` makes
/// (entries, refresh-log row, list-row bumps). With `caller_view`,
/// `rematched` is the caller-view delta (see [`check`]).
async fn dry_run_apply(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    parsed: &parsers::cbl::ParsedCbl,
    raw_xml: &str,
    blob_sha: Option<&str>,
    caller_view: Option<&crate::library::access::VisibleLibraries>,
) -> Result<CheckOutcome, RefreshError> {
    use crate::api::cbl_lists::caller_matched_keys;
    let txn = db.begin().await?;
    let before = match caller_view {
        Some(acl) => Some(caller_matched_keys(&txn, list_id, acl).await?),
        None => None,
    };
    let mut summary = import::apply_parsed(
        &txn,
        list_id,
        parsed,
        raw_xml,
        blob_sha,
        RefreshTrigger::Manual,
    )
    .await?;
    if let (Some(acl), Some(before)) = (caller_view, before.as_ref()) {
        let after = caller_matched_keys(&txn, list_id, acl).await?;
        summary.rematched = after.difference(before).count() as i32;
    }
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
    budget: FetchBudget,
) -> Result<ImportSummary, RefreshError> {
    let url = list
        .source_url
        .as_deref()
        .ok_or_else(|| RefreshError::Http("url source missing source_url".into()))?;

    let fetched = match within_budget(budget, fetch_url_source(url, list, force, budget)).await? {
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
    let parsed = parse_fetched(&xml, &list.raw_sha256)?;
    apply_atomically(
        db,
        list.id,
        &parsed,
        &xml,
        None,
        trigger,
        Some((etag, last_modified)),
    )
    .await
}

enum UrlFetch {
    NotModified,
    Fetched(Box<crate::util::ssrf::FetchedBytes>),
}

/// Fetch a URL list's source. Unless `force`, the stored ETag /
/// Last-Modified go along as validators. Conditional or not, the request is
/// the import's ([`fetch_cbl_url`]): vetted, pinned, direct, re-vetted on
/// every redirect, capped and under one deadline.
async fn fetch_url_source(
    url: &str,
    list: &cbl_list::Model,
    force: bool,
    budget: FetchBudget,
) -> Result<UrlFetch, RefreshError> {
    let result = if force {
        fetch_cbl_url(url, budget)
            .await
            .map(|f| UrlFetch::Fetched(Box::new(f)))
    } else {
        let conditional = Conditional {
            etag: list.source_etag.as_deref(),
            last_modified: list.source_last_modified.as_deref(),
        };
        fetch_cbl_url_conditional(url, conditional, budget)
            .await
            .map(|o| match o {
                FetchOutcome::NotModified => UrlFetch::NotModified,
                FetchOutcome::Fetched(f) => UrlFetch::Fetched(f),
            })
    };
    result.map_err(|e| match e {
        crate::util::ssrf::FetchBytesError::Timeout => RefreshError::Timeout,
        other => RefreshError::Http(other.to_string()),
    })
}

/// Run the remote part of one refresh / check under the budget's
/// `list_budget`. Only network work and the catalog's own index bookkeeping
/// go in here (`catalog::fetch_blob` may update the catalog source row, a
/// single-row write that is harmless if cut off); list entries are never
/// written inside it.
async fn within_budget<T>(
    budget: FetchBudget,
    fut: impl std::future::Future<Output = Result<T, RefreshError>>,
) -> Result<T, RefreshError> {
    tokio::time::timeout(budget.list_budget, fut)
        .await
        .map_err(|_| RefreshError::Timeout)?
}

fn parse(xml: &str) -> Result<parsers::cbl::ParsedCbl, RefreshError> {
    parsers::cbl::parse(xml.as_bytes()).map_err(|e| RefreshError::Parse(e.to_string()))
}

/// Parse stored content. Not checked against the entry cap: it was
/// accepted when it was stored, and the cap is for new content.
fn parse_stored(xml: &str) -> Result<parsers::cbl::ParsedCbl, RefreshError> {
    parse(xml)
}

/// Parse fetched content, refusing it when it is new (its digest differs
/// from the stored `raw_sha256`) and has more entries than an import
/// accepts, before anything is written.
fn parse_fetched(xml: &str, stored_sha256: &[u8]) -> Result<parsers::cbl::ParsedCbl, RefreshError> {
    let parsed = parse(xml)?;
    if parsed.books.len() > super::MAX_BOOKS_PER_LIST
        && import::sha256_of(xml.as_bytes()) != stored_sha256
    {
        return Err(RefreshError::TooManyEntries);
    }
    Ok(parsed)
}

/// One run of the scheduled refresh sweep over the lists that are due.
/// The order is shuffled so lists whose hosts keep failing cannot starve
/// the rest, and once [`CBL_SWEEP_BUDGET`] is spent no further list is
/// started (the skipped ones stay due for the next run).
pub async fn run_sweep(db: &sea_orm::DatabaseConnection, mut due: Vec<Uuid>) {
    use rand::seq::SliceRandom;
    due.shuffle(&mut rand::rng());
    let started = tokio::time::Instant::now();
    let total = due.len();
    for (done, list_id) in due.into_iter().enumerate() {
        if started.elapsed() >= CBL_SWEEP_BUDGET {
            tracing::warn!(
                skipped = total - done,
                "cbl_refresh sweep: time budget spent; remaining lists run next sweep",
            );
            break;
        }
        match refresh(db, list_id, RefreshTrigger::Scheduled, false, SWEEP_BUDGET).await {
            Ok(s) => {
                if s.upstream_changed {
                    tracing::info!(
                        list_id = %list_id,
                        added = s.added,
                        removed = s.removed,
                        rematched = s.rematched,
                        "cbl_refresh applied diff",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(list_id = %list_id, error = %e, "cbl_refresh failed");
            }
        }
    }
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
    budget: FetchBudget,
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

    let blob = within_budget(budget, async {
        Ok::<_, RefreshError>(catalog::fetch_blob(db, &source, path, force).await?)
    })
    .await?;
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
    let parsed = parse_fetched(&xml, &list.raw_sha256)?;
    apply_atomically(
        db,
        list.id,
        &parsed,
        &xml,
        Some(&blob.blob_sha),
        trigger,
        None,
    )
    .await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_error_text_stays_out_of_the_public_message() {
        let sql = "relation \"cbl_list\" does not exist";
        let db = || sea_orm::DbErr::Custom(sql.to_owned());
        for msg in [
            RefreshError::Db(db()).public_message(),
            RefreshError::Catalog(catalog::CatalogError::Db(db())).public_message(),
            catalog::CatalogError::Db(db()).public_message(),
        ] {
            assert!(!msg.contains("cbl_list"), "{msg}");
            assert!(msg.ends_with(crate::api::INTERNAL_ERROR_TEXT), "{msg}");
        }
        assert_eq!(
            RefreshError::Http("503".into()).public_message(),
            "HTTP: 503"
        );
    }
}
