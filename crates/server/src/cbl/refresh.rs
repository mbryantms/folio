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
//!   - `'catalog'` — look the path up in the configured catalog source's
//!     index; when its blob SHA equals the stored one the blob is not
//!     downloaded and the refresh only re-matches (a forced refresh always
//!     downloads).
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
//!
//! Every finished [`refresh`] (manual or scheduled) records its outcome on
//! the list row ([`record_outcome`]): the attempt time, and on failure a
//! typed [`CblRefreshErrorKind`] with a short server-composed detail, the
//! start of the failing run and the count of failures in a row. A success
//! clears them. Neither touches the entries or `last_refreshed_at`. The
//! sweep backs off a failing list ([`failure_backoff`]); a manual refresh
//! always tries.

use entity::cbl_list::CblRefreshErrorKind;
use entity::{catalog_source, cbl_list};
use sea_orm::{ConnectionTrait, EntityTrait, TransactionTrait};
use uuid::Uuid;

use super::catalog;
use super::fetch::{
    CBL_SWEEP_BUDGET, FetchBudget, SWEEP_BUDGET, fetch_cbl_url, fetch_cbl_url_conditional,
};
use super::import::{self, ImportSummary, RefreshTrigger};
use crate::util::ssrf::{Conditional, FetchBytesError, FetchOutcome, SsrfError};

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
    /// The list's source settings are incomplete (no URL, no catalog
    /// path) or of an unknown kind.
    #[error("HTTP: {0}")]
    Http(String),
    /// The vetted fetch of a URL source failed.
    #[error("HTTP: {0}")]
    Fetch(FetchBytesError),
    /// A URL source answered with a content type that is not a list.
    #[error("HTTP: URL source must return XML or a .cbl file")]
    NotCbl,
    #[error("parse: {0}")]
    Parse(String),
    /// The fetched file has more entries than an import accepts. Same
    /// message as the import refusal.
    #[error("{}", super::too_many_entries_message())]
    TooManyEntries { entries: usize },
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

    /// The failure kind and detail recorded on the list for this error.
    /// The detail is composed here from safe values only (a status number,
    /// a limit, a count): never the remote body, headers, host, resolved
    /// address or a reader's error text. Every refusal by the address guard
    /// reads the same.
    pub fn failure(&self) -> RefreshFailure {
        use CblRefreshErrorKind as K;
        match self {
            Self::Fetch(e) => fetch_failure(e),
            Self::Catalog(e) => catalog_failure(e),
            Self::Timeout => RefreshFailure::new(K::Unreachable, Some(NO_ANSWER.to_owned())),
            Self::NotCbl => {
                RefreshFailure::new(K::InvalidList, Some("not served as XML".to_owned()))
            }
            Self::Parse(_) => RefreshFailure::new(K::InvalidList, None),
            Self::TooManyEntries { entries } => RefreshFailure::new(
                K::TooManyEntries,
                Some(format!(
                    "{entries} entries, over the {} limit",
                    super::MAX_BOOKS_PER_LIST
                )),
            ),
            Self::NotFound | Self::Http(_) | Self::NotAttempted | Self::Db(_) => {
                RefreshFailure::new(K::Other, None)
            }
        }
    }
}

/// What [`record_outcome`] stores for a failed refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshFailure {
    pub kind: CblRefreshErrorKind,
    /// Composed by the server from safe values; see [`RefreshError::failure`].
    pub detail: Option<String>,
}

impl RefreshFailure {
    fn new(kind: CblRefreshErrorKind, detail: Option<String>) -> Self {
        Self { kind, detail }
    }
}

const NO_ANSWER: &str = "no answer in time";

fn status_failure(status: reqwest::StatusCode) -> RefreshFailure {
    let kind = match status.as_u16() {
        404 | 410 => CblRefreshErrorKind::Missing,
        _ => CblRefreshErrorKind::HttpStatus,
    };
    RefreshFailure::new(kind, Some(format!("HTTP {}", status.as_u16())))
}

/// `limit` bytes as a short "over the … limit" note.
fn size_limit_detail(limit: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if limit >= MIB && limit.is_multiple_of(MIB) {
        format!("over the {} MiB limit", limit / MIB)
    } else {
        format!("over the {limit} byte limit")
    }
}

fn fetch_failure(e: &FetchBytesError) -> RefreshFailure {
    use CblRefreshErrorKind as K;
    match e {
        // A name that does not resolve is unreachable like a refused
        // connection; it carries no resolver text.
        FetchBytesError::Ssrf(SsrfError::Unresolvable(_)) | FetchBytesError::Transport(_) => {
            RefreshFailure::new(K::Unreachable, None)
        }
        // Private address, loopback, a non-https URL, a malformed target:
        // one kind, no detail, so the record cannot be used to tell
        // internal addresses apart.
        FetchBytesError::Ssrf(_) => RefreshFailure::new(K::AddressRefused, None),
        // A redirect target the guard refused *or could not resolve*: the
        // same, so a source that redirects to a name of its choosing cannot
        // learn from the stored state whether that name resolves inside
        // the server's network. (Only the list's own URL not resolving
        // reads as unreachable.)
        FetchBytesError::RedirectRefused(_) => RefreshFailure::new(K::AddressRefused, None),
        FetchBytesError::Timeout => RefreshFailure::new(K::Unreachable, Some(NO_ANSWER.to_owned())),
        FetchBytesError::HttpStatus(status) => status_failure(*status),
        // The host name stays out: it is in the source URL already.
        FetchBytesError::Challenged { status, .. } => RefreshFailure::new(
            K::HttpStatus,
            Some(format!("HTTP {}, a bot challenge", status.as_u16())),
        ),
        FetchBytesError::TooLarge { max_bytes } => {
            RefreshFailure::new(K::TooLarge, Some(size_limit_detail(*max_bytes)))
        }
        FetchBytesError::InvalidRedirect => {
            RefreshFailure::new(K::Other, Some("invalid redirect".to_owned()))
        }
        FetchBytesError::TooManyRedirects(n) => {
            RefreshFailure::new(K::Other, Some(format!("more than {n} redirects")))
        }
    }
}

fn catalog_failure(e: &catalog::CatalogError) -> RefreshFailure {
    use CblRefreshErrorKind as K;
    use catalog::CatalogError as C;
    match e {
        C::Transport(_) => RefreshFailure::new(K::Unreachable, None),
        C::Status { status, .. } => status_failure(*status),
        C::RateLimited => RefreshFailure::new(K::HttpStatus, Some("rate limited".to_owned())),
        C::PathMissing(_) => RefreshFailure::new(K::Missing, Some("not in the catalog".to_owned())),
        C::BlobMissing(_) => RefreshFailure::new(K::Missing, Some("HTTP 404".to_owned())),
        C::TooLarge { limit } => RefreshFailure::new(K::TooLarge, Some(size_limit_detail(*limit))),
        C::Disabled => RefreshFailure::new(K::Other, Some("catalog source disabled".to_owned())),
        C::Http(_) | C::Db(_) => RefreshFailure::new(K::Other, None),
    }
}

/// Record a finished refresh on the list row. A success clears the failure
/// fields and resets the count; a failure sets the kind and detail, keeps
/// the start of the failing run and adds one to the count. Only these
/// columns are written: entries, `last_refreshed_at` and `updated_at` stay
/// as they are. A write error is logged and does not replace the refresh's
/// own result.
pub async fn record_outcome(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    failure: Option<&RefreshFailure>,
) {
    use sea_orm::{DbBackend, Statement};
    let now = chrono::Utc::now().fixed_offset();
    let stmt = match failure {
        None => Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE cbl_lists SET last_refresh_attempt_at = $2, \
             last_refresh_error_kind = NULL, last_refresh_error_detail = NULL, \
             refresh_failing_since = NULL, refresh_failure_count = 0 \
             WHERE id = $1",
            [list_id.into(), now.into()],
        ),
        Some(f) => Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE cbl_lists SET last_refresh_attempt_at = $2, \
             last_refresh_error_kind = $3, last_refresh_error_detail = $4, \
             refresh_failing_since = COALESCE(refresh_failing_since, $2), \
             refresh_failure_count = refresh_failure_count + 1 \
             WHERE id = $1",
            [
                list_id.into(),
                now.into(),
                f.kind.as_str().into(),
                f.detail.clone().into(),
            ],
        ),
    };
    if let Err(e) = db.execute_raw(stmt).await {
        tracing::warn!(list_id = %list_id, error = %e, "cbl_refresh: recording the outcome failed");
    }
}

/// Refresh one list and record the outcome on it ([`record_outcome`]). `budget` bounds the remote work: request paths pass
/// [`super::fetch::REQUEST_BUDGET`], the sweep [`SWEEP_BUDGET`].
pub async fn refresh(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    trigger: RefreshTrigger,
    force: bool,
    budget: FetchBudget,
) -> Result<ImportSummary, RefreshError> {
    let started = tokio::time::Instant::now();
    let result = refresh_source(db, list_id, trigger, force, budget).await;
    match &result {
        Ok(_) => record_outcome(db, list_id, None).await,
        Err(e) if !e.is_source_failure(budget, started.elapsed()) => {}
        Err(e) => record_outcome(db, list_id, Some(&e.failure())).await,
    }
    result
}

/// Whether `budget` is shorter than the sweep's ([`SWEEP_BUDGET`]): the
/// request paths (40 s) and `refresh-all` (at most 15 s per list, less as
/// its deadline runs down).
pub fn is_capped_budget(budget: FetchBudget) -> bool {
    budget.fetch_deadline < SWEEP_BUDGET.fetch_deadline
        || budget.list_budget < SWEEP_BUDGET.list_budget
}

impl RefreshError {
    /// Whether this error says something about the list's *source*, and so
    /// is recorded on the list ([`record_outcome`]). Not recorded, leaving
    /// the refresh state as it was:
    ///
    /// - a timeout that ran into a budget capped below the sweep's
    ///   ([`is_capped_budget`]) after `elapsed` reached that cap: a slow
    ///   host the sweep would wait for is not failing (a timeout before the
    ///   cap, e.g. a 30 s step timeout under the 40 s request budget, is
    ///   the fetcher's own and is recorded);
    /// - a database error, which is the server's, not the source's.
    pub fn is_source_failure(&self, budget: FetchBudget, elapsed: std::time::Duration) -> bool {
        match self {
            Self::Db(_) | Self::Catalog(catalog::CatalogError::Db(_)) => false,
            Self::Timeout => {
                let cap = budget.fetch_deadline.min(budget.list_budget);
                !(is_capped_budget(budget) && elapsed >= cap)
            }
            _ => true,
        }
    }
}

async fn refresh_source(
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
                        return Err(RefreshError::NotCbl);
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
            // The index's SHA decides first: an unchanged blob is not
            // downloaded (nor size-checked; see `catalog_blob_if_changed`).
            match within_budget(
                budget,
                catalog_blob_if_changed(db, &source, path, &list, false),
            )
            .await?
            {
                None => None,
                Some(blob) => {
                    let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
                    Some((xml, Some(blob.blob_sha)))
                }
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
        return Err(RefreshError::NotCbl);
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
        FetchBytesError::Timeout => RefreshError::Timeout,
        other => RefreshError::Fetch(other),
    })
}

/// Run the remote part of one refresh / check under the budget's
/// `list_budget`. Only network work and the catalog's own index bookkeeping
/// go in here (`catalog::index_entry` may update the catalog source row, a
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
        return Err(RefreshError::TooManyEntries {
            entries: parsed.books.len(),
        });
    }
    Ok(parsed)
}

/// Longest the sweep waits between attempts at a failing list.
pub const REFRESH_BACKOFF_CAP: chrono::Duration = chrono::Duration::hours(24);

/// How long the sweep waits after the last attempt at a list that has
/// failed `failures` times in a row: one hour after the first failure
/// (the sweep's own cadence, so nothing changes until a second failure),
/// then doubling per further failure up to [`REFRESH_BACKOFF_CAP`]. Zero
/// for a list that is not failing.
pub fn failure_backoff(failures: i32) -> chrono::Duration {
    if failures <= 0 {
        return chrono::Duration::zero();
    }
    let hours = 1i64 << (failures - 1).min(5);
    chrono::Duration::hours(hours).min(REFRESH_BACKOFF_CAP)
}

/// The cadence of a `refresh_schedule` value in hours; `None` for
/// `manual`. Unknown values fall back to weekly so a typo doesn't disable
/// refresh entirely.
fn cadence_hours(schedule: &str) -> Option<i64> {
    match schedule.trim() {
        "@hourly" => Some(1),
        "@daily" => Some(24),
        "@weekly" | "0 0 * * 0" => Some(24 * 7),
        "@monthly" => Some(24 * 30),
        "manual" => None,
        _ => Some(24 * 7),
    }
}

/// The refresh state of one scheduled list, as [`is_due`] reads it.
#[derive(Debug, Clone, Copy)]
pub struct DueState<'a> {
    pub schedule: &'a str,
    pub last_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub failure_count: i32,
    pub last_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Whether the sweep at `now` should refresh this list: its cadence has
/// passed since the last success (or the import), and, while it is
/// failing, its [`failure_backoff`] has passed since the last attempt.
/// The backoff is measured with [`CBL_SWEEP_BUDGET`] of slack, because an
/// attempt lands up to that long after the tick that started it.
pub fn is_due(state: DueState<'_>, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(cadence) = cadence_hours(state.schedule) else {
        return false;
    };
    let last = state.last_refreshed_at.unwrap_or(state.imported_at);
    if now - last < chrono::Duration::hours(cadence) {
        return false;
    }
    match state.last_attempt_at {
        Some(at) if state.failure_count > 0 => {
            let slack = chrono::Duration::from_std(CBL_SWEEP_BUDGET)
                .unwrap_or_else(|_| chrono::Duration::zero());
            now - at + slack >= failure_backoff(state.failure_count)
        }
        _ => true,
    }
}

/// The scheduled lists the sweep at `now` should refresh ([`is_due`]).
pub async fn due_lists(
    db: &sea_orm::DatabaseConnection,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Uuid>, sea_orm::DbErr> {
    use sea_orm::prelude::DateTimeWithTimeZone;
    use sea_orm::{ColumnTrait, QueryFilter, QuerySelect};
    type Row = (
        Uuid,
        Option<String>,
        Option<DateTimeWithTimeZone>,
        DateTimeWithTimeZone,
        i32,
        Option<DateTimeWithTimeZone>,
    );
    let rows: Vec<Row> = cbl_list::Entity::find()
        .select_only()
        .columns([
            cbl_list::Column::Id,
            cbl_list::Column::RefreshSchedule,
            cbl_list::Column::LastRefreshedAt,
            cbl_list::Column::ImportedAt,
            cbl_list::Column::RefreshFailureCount,
            cbl_list::Column::LastRefreshAttemptAt,
        ])
        .filter(cbl_list::Column::RefreshSchedule.is_not_null())
        .into_tuple()
        .all(db)
        .await?;
    let utc = |d: DateTimeWithTimeZone| d.with_timezone(&chrono::Utc);
    Ok(rows
        .into_iter()
        .filter_map(|(id, schedule, refreshed, imported, failures, attempt)| {
            let state = DueState {
                schedule: schedule.as_deref()?,
                last_refreshed_at: refreshed.map(utc),
                imported_at: utc(imported),
                failure_count: failures,
                last_attempt_at: attempt.map(utc),
            };
            is_due(state, now).then_some(id)
        })
        .collect())
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

    let blob = within_budget(
        budget,
        catalog_blob_if_changed(db, &source, path, list, force),
    )
    .await?;
    let Some(blob) = blob else {
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
    };
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

/// A catalog list's blob when it changed upstream, `None` when it did not.
///
/// The catalog index (refreshed per its TTL, or always when `force`) is
/// read first. Unless `force`, an index SHA equal to the list's stored
/// `github_blob_sha` means the stored content is current: `None`, with no
/// download. A path the index no longer lists is
/// [`catalog::CatalogError::PathMissing`] either way.
///
/// The size check sits after the SHA comparison, inside
/// [`catalog::download_blob`]: stored content that has not changed is not
/// re-checked (the same rule as the entry cap, see the module docs), so
/// only a blob that is about to be downloaded can be refused as too large.
/// A forced refresh always downloads.
async fn catalog_blob_if_changed(
    db: &sea_orm::DatabaseConnection,
    source: &catalog_source::Model,
    path: &str,
    list: &cbl_list::Model,
    force: bool,
) -> Result<Option<catalog::FetchedBlob>, RefreshError> {
    let entry = catalog::index_entry(db, source, path, force).await?;
    if !force && list.github_blob_sha.as_deref() == Some(entry.sha.as_str()) {
        return Ok(None);
    }
    Ok(Some(catalog::download_blob(source, &entry).await?))
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
    fn backoff_doubles_from_one_hour_up_to_a_day() {
        let h = chrono::Duration::hours;
        let got: Vec<_> = (0..=8).map(failure_backoff).collect();
        assert_eq!(
            got,
            [
                chrono::Duration::zero(),
                h(1),
                h(2),
                h(4),
                h(8),
                h(16),
                h(24),
                h(24),
                h(24)
            ]
        );
        assert_eq!(failure_backoff(i32::MAX), h(24));
    }

    #[test]
    fn due_follows_the_cadence_and_backs_off_while_failing() {
        let now = chrono::Utc::now();
        let h = chrono::Duration::hours;
        let m = chrono::Duration::minutes;
        let base = DueState {
            schedule: "@hourly",
            last_refreshed_at: Some(now - h(3)),
            imported_at: now - h(100),
            failure_count: 0,
            last_attempt_at: None,
        };
        assert!(is_due(base, now));
        assert!(!is_due(
            DueState {
                schedule: "manual",
                ..base
            },
            now
        ));
        assert!(!is_due(
            DueState {
                schedule: "@daily",
                ..base
            },
            now
        ));
        // A healthy list is due by cadence alone, whenever it last tried.
        assert!(is_due(
            DueState {
                last_attempt_at: Some(now - m(1)),
                ..base
            },
            now
        ));
        // One failure: the next hourly tick still tries (an attempt lands a
        // few minutes after its tick).
        let failing = |n, ago| DueState {
            failure_count: n,
            last_attempt_at: Some(now - ago),
            ..base
        };
        assert!(is_due(failing(1, m(58)), now));
        // Three failures: wait four hours.
        assert!(!is_due(failing(3, h(2)), now));
        assert!(is_due(failing(3, h(4) - m(5)), now));
        // Many failures: at most a day.
        assert!(!is_due(failing(20, h(12)), now));
        assert!(is_due(failing(20, h(24)), now));
    }

    #[test]
    fn only_source_failures_are_recorded() {
        use super::super::fetch::{CBL_REQUEST_BULK_LIST_BUDGET, REQUEST_BUDGET};
        let s = std::time::Duration::from_secs;
        let bulk = REQUEST_BUDGET.capped_at(CBL_REQUEST_BULK_LIST_BUDGET);
        assert!(!is_capped_budget(SWEEP_BUDGET));
        assert!(is_capped_budget(REQUEST_BUDGET));
        assert!(is_capped_budget(bulk));
        let t = RefreshError::Timeout;
        // Ran into a capped budget: not the source's failure.
        assert!(!t.is_source_failure(REQUEST_BUDGET, s(40)));
        assert!(!t.is_source_failure(bulk, s(15)));
        assert!(!t.is_source_failure(REQUEST_BUDGET.capped_at(s(6)), s(7)));
        // Timed out before the cap (the fetcher's own step timeout): is.
        assert!(t.is_source_failure(REQUEST_BUDGET, s(30)));
        // Under the sweep's budget, any timeout is.
        assert!(t.is_source_failure(SWEEP_BUDGET, s(60)));
        assert!(t.is_source_failure(SWEEP_BUDGET, s(120)));
        // A database error never is; other errors always are.
        let db = || sea_orm::DbErr::Custom("x".into());
        assert!(!RefreshError::Db(db()).is_source_failure(SWEEP_BUDGET, s(1)));
        assert!(
            !RefreshError::Catalog(catalog::CatalogError::Db(db()))
                .is_source_failure(SWEEP_BUDGET, s(1))
        );
        assert!(RefreshError::NotCbl.is_source_failure(REQUEST_BUDGET, s(40)));
    }

    /// Every failure path maps to one kind, and no detail carries remote
    /// text, a host or an address.
    #[test]
    fn failures_map_to_kinds_with_safe_details() {
        use CblRefreshErrorKind as K;
        use catalog::CatalogError as C;
        let s = reqwest::StatusCode::from_u16;
        let ip = |a: &str| -> std::net::IpAddr { a.parse().unwrap() };
        let cases: Vec<(RefreshError, K, Option<&str>)> = vec![
            (
                RefreshError::Fetch(FetchBytesError::Transport(
                    "error sending request for url (https://10.1.2.3/x): secret".into(),
                )),
                K::Unreachable,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::Unresolvable(
                    "failed to lookup address information: internal.corp".into(),
                ))),
                K::Unreachable,
                None,
            ),
            (RefreshError::Timeout, K::Unreachable, Some(NO_ANSWER)),
            (
                RefreshError::Fetch(FetchBytesError::Timeout),
                K::Unreachable,
                Some(NO_ANSWER),
            ),
            (
                RefreshError::Fetch(FetchBytesError::HttpStatus(s(503).unwrap())),
                K::HttpStatus,
                Some("HTTP 503"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::HttpStatus(s(404).unwrap())),
                K::Missing,
                Some("HTTP 404"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::HttpStatus(s(410).unwrap())),
                K::Missing,
                Some("HTTP 410"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::Challenged {
                    host: "lists.example.test".into(),
                    status: s(403).unwrap(),
                }),
                K::HttpStatus,
                Some("HTTP 403, a bot challenge"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::PrivateAddress(ip(
                    "10.0.0.7",
                )))),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::PrivateAddress(ip(
                    "fd12::1",
                )))),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::RedirectRefused(SsrfError::PrivateAddress(
                    ip("192.168.1.1"),
                ))),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::RedirectRefused(SsrfError::Unresolvable(
                    "intranet.corp: no such host".into(),
                ))),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::SchemeNotHttps)),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::MissingHost)),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::Ssrf(SsrfError::InvalidUrl(
                    "relative URL without a base".into(),
                ))),
                K::AddressRefused,
                None,
            ),
            (
                RefreshError::Fetch(FetchBytesError::TooLarge {
                    max_bytes: super::super::fetch::MAX_URL_CBL_BYTES,
                }),
                K::TooLarge,
                Some("over the 4 MiB limit"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::InvalidRedirect),
                K::Other,
                Some("invalid redirect"),
            ),
            (
                RefreshError::Fetch(FetchBytesError::TooManyRedirects(2)),
                K::Other,
                Some("more than 2 redirects"),
            ),
            (
                RefreshError::NotCbl,
                K::InvalidList,
                Some("not served as XML"),
            ),
            (
                RefreshError::Parse("unexpected <script> at line 1".into()),
                K::InvalidList,
                None,
            ),
            (
                RefreshError::TooManyEntries { entries: 5001 },
                K::TooManyEntries,
                Some("5001 entries, over the 5000 limit"),
            ),
            (
                RefreshError::Http("url source missing source_url".into()),
                K::Other,
                None,
            ),
            (RefreshError::NotFound, K::Other, None),
            (
                RefreshError::Db(sea_orm::DbErr::Custom("relation x".into())),
                K::Other,
                None,
            ),
            (
                RefreshError::Catalog(C::Transport("dns error: secret".into())),
                K::Unreachable,
                None,
            ),
            (
                RefreshError::Catalog(C::Status {
                    what: "tree",
                    status: s(500).unwrap(),
                }),
                K::HttpStatus,
                Some("HTTP 500"),
            ),
            (
                RefreshError::Catalog(C::RateLimited),
                K::HttpStatus,
                Some("rate limited"),
            ),
            (
                RefreshError::Catalog(C::PathMissing("Marvel/Old.cbl".into())),
                K::Missing,
                Some("not in the catalog"),
            ),
            (
                RefreshError::Catalog(C::BlobMissing("Marvel/Old.cbl".into())),
                K::Missing,
                Some("HTTP 404"),
            ),
            (
                RefreshError::Catalog(C::TooLarge { limit: 1000 }),
                K::TooLarge,
                Some("over the 1000 byte limit"),
            ),
            (
                RefreshError::Catalog(C::Disabled),
                K::Other,
                Some("catalog source disabled"),
            ),
            (
                RefreshError::Catalog(C::Http("decode tree: expected value".into())),
                K::Other,
                None,
            ),
        ];
        for (err, kind, detail) in cases {
            let f = err.failure();
            assert_eq!(f.kind, kind, "{err:?}");
            assert_eq!(f.detail.as_deref(), detail, "{err:?}");
        }
    }

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
