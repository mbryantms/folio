//! Outbound fetch of URL-sourced reading lists, and the time budgets for
//! refreshing them.
//!
//! Every request to a list's URL — the first import, a refresh, a "check
//! for updates", conditional or not — goes through [`fetch_cbl_url`] or
//! [`fetch_cbl_url_conditional`], which are
//! [`crate::util::ssrf::fetch_public`]: https only, every hop's host
//! resolved once and refused if any address is internal, the connection
//! pinned to the vetted address with the proxy environment ignored
//! ([`ProxyMode::Direct`]), redirects followed by hand with each target
//! re-vetted, the body capped while streaming, and one overall deadline.
//!
//! Budgets come in two sizes. Request paths (refresh, check and their
//! `-all` variants, URL import) run under the 60 s JSON route timeout, so
//! they use [`REQUEST_BUDGET`] and return their own timeout error well
//! before the route timeout would cut them off. The hourly sweep has no
//! such ceiling and uses the longer [`SWEEP_BUDGET`].

use std::time::Duration;

use crate::util::ssrf::{
    self, Conditional, FetchBytesError, FetchOptions, FetchOutcome, FetchedBytes, ProxyMode,
};

/// Largest `.cbl` body accepted from a URL (same as the upload cap).
pub const MAX_URL_CBL_BYTES: usize = 4 * 1024 * 1024;

/// Bound on each step of a fetch: one request/response exchange, or the
/// wait for the next body chunk.
pub const CBL_FETCH_STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Redirect hops followed (each one re-vetted).
pub const CBL_MAX_REDIRECTS: usize = 2;

/// Time allowed for the remote part of refreshing or checking one list.
#[derive(Debug, Clone, Copy)]
pub struct FetchBudget {
    /// One deadline for a whole fetch: every hop's connect, headers and
    /// body.
    pub fetch_deadline: Duration,
    /// All remote work for one list. A URL list makes one fetch; a catalog
    /// list may make two (index and blob).
    pub list_budget: Duration,
}

/// Budget on request paths: well under the 60 s JSON route timeout, so a
/// slow host ends in the handler's own error, not a dropped request.
pub const REQUEST_BUDGET: FetchBudget = FetchBudget {
    fetch_deadline: Duration::from_secs(40),
    list_budget: Duration::from_secs(40),
};

/// Budget in the hourly sweep: two step timeouts per fetch, two fetches per
/// list.
pub const SWEEP_BUDGET: FetchBudget = FetchBudget {
    fetch_deadline: Duration::from_secs(60),
    list_budget: Duration::from_secs(120),
};

/// Bound on one `refresh-all` / `check-all` request. No list is started
/// once less than [`CBL_REQUEST_MIN_LIST_BUDGET`] of it is left; those lists
/// are reported as not attempted.
pub const CBL_REQUEST_BULK_BUDGET: Duration = Duration::from_secs(45);

/// Most time one list may take inside a bulk request, so a few stalling
/// hosts cannot use up the whole request.
pub const CBL_REQUEST_BULK_LIST_BUDGET: Duration = Duration::from_secs(15);

/// Least time worth starting one more list with in a bulk request.
pub const CBL_REQUEST_MIN_LIST_BUDGET: Duration = Duration::from_secs(5);

/// Bound on one run of the hourly refresh sweep. Once spent, the sweep
/// starts no further lists; the ones it skipped are still due and run on
/// the next tick. Half the tick interval, so runs never overlap.
pub const CBL_SWEEP_BUDGET: Duration = Duration::from_secs(30 * 60);

// Request paths must finish their remote work before the route timeout.
const _: () =
    assert!(REQUEST_BUDGET.list_budget.as_secs() + 10 < crate::app::JSON_ROUTE_TIMEOUT.as_secs());
const _: () =
    assert!(CBL_REQUEST_BULK_BUDGET.as_secs() + 10 < crate::app::JSON_ROUTE_TIMEOUT.as_secs());

impl FetchBudget {
    /// This budget, shortened to `remaining` where that is smaller.
    pub fn capped_at(self, remaining: Duration) -> FetchBudget {
        FetchBudget {
            fetch_deadline: self.fetch_deadline.min(remaining),
            list_budget: self.list_budget.min(remaining),
        }
    }
}

/// Fetch a reading list from its URL (no validators).
pub async fn fetch_cbl_url(
    url: &str,
    budget: FetchBudget,
) -> Result<FetchedBytes, FetchBytesError> {
    #[cfg(debug_assertions)]
    if let Some(out) = test_source(url, budget).await {
        return out.map(|b| *b);
    }
    ssrf::fetch_public(url, &cbl_fetch_options(budget)).await
}

/// Fetch a reading list with the stored validators; a `304 Not Modified`
/// comes back as [`FetchOutcome::NotModified`].
pub async fn fetch_cbl_url_conditional(
    url: &str,
    conditional: Conditional<'_>,
    budget: FetchBudget,
) -> Result<FetchOutcome, FetchBytesError> {
    #[cfg(debug_assertions)]
    if let Some(out) = test_source(url, budget).await {
        return out.map(FetchOutcome::Fetched);
    }
    ssrf::fetch_public_conditional(url, &cbl_fetch_options(budget), conditional).await
}

fn cbl_fetch_options(budget: FetchBudget) -> FetchOptions {
    FetchOptions {
        max_bytes: MAX_URL_CBL_BYTES,
        idle_timeout: CBL_FETCH_STEP_TIMEOUT.min(budget.fetch_deadline),
        deadline: Some(budget.fetch_deadline),
        user_agent: crate::build_info::USER_AGENT,
        max_redirects: CBL_MAX_REDIRECTS,
        require_https: true,
        // A signed-in user supplies these URLs: connect directly so the
        // address check is authoritative.
        proxy: ProxyMode::Direct,
    }
}

// ───── debug-build test seam ─────
//
// Integration tests cannot serve https from a public address, which the
// vetted fetch requires (the same constraint as the cover stub in
// `metadata::writers`). Debug builds let a test register what an exact URL
// answers: a body, or a host that never answers. Not compiled into
// release builds.

/// What a registered test URL answers.
#[cfg(debug_assertions)]
#[derive(Debug, Clone)]
pub enum TestSource {
    /// `200` with this body as `application/xml`.
    Body(Vec<u8>),
    /// Never answers; the fetch deadline ends it.
    Stall,
}

#[cfg(debug_assertions)]
static TEST_SOURCES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, TestSource>>,
> = std::sync::LazyLock::new(Default::default);

/// Register what [`fetch_cbl_url`] returns for `url`. Debug builds only;
/// for integration tests. Use a URL unique to the test.
#[cfg(debug_assertions)]
pub fn stub_source_for_tests(url: &str, source: TestSource) {
    TEST_SOURCES
        .lock()
        .expect("test sources lock")
        .insert(url.to_owned(), source);
}

#[cfg(debug_assertions)]
async fn test_source(
    url: &str,
    budget: FetchBudget,
) -> Option<Result<Box<FetchedBytes>, FetchBytesError>> {
    let source = TEST_SOURCES
        .lock()
        .expect("test sources lock")
        .get(url)
        .cloned()?;
    Some(match source {
        TestSource::Body(bytes) => {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/xml"),
            );
            Ok(Box::new(FetchedBytes {
                final_url: url::Url::parse(url).ok()?,
                headers,
                bytes,
            }))
        }
        TestSource::Stall => {
            tokio::time::timeout(budget.fetch_deadline, std::future::pending::<()>())
                .await
                .map_err(|_| FetchBytesError::Timeout)
                .and_then(|()| Err(FetchBytesError::Timeout))
        }
    })
}
