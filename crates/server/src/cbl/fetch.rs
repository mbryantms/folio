//! Outbound fetch of URL-sourced reading lists, and the time budgets for
//! refreshing them.
//!
//! Every request to a list's URL — the first import, a refresh, a "check
//! for updates", conditional or not — goes through [`fetch_cbl_url`], which
//! is [`crate::util::ssrf::fetch_public`]: https only, every hop's host
//! resolved once and refused if any address is internal, the connection
//! pinned to the vetted address, redirects followed by hand with each target
//! re-vetted, the body capped while streaming, and one overall deadline.

use std::time::Duration;

use crate::util::ssrf::{self, Conditional, FetchBytesError, FetchOptions, FetchOutcome};

/// Largest `.cbl` body accepted from a URL (same as the upload cap).
pub const MAX_URL_CBL_BYTES: usize = 4 * 1024 * 1024;

/// Bound on each step of a fetch: one request/response exchange, or the
/// wait for the next body chunk.
pub const CBL_FETCH_STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// One deadline for a whole fetch: every hop's connect, headers and body.
/// Two step timeouts' worth, so a host that answers but trickles its body
/// cannot hold a fetch open past it.
pub const CBL_FETCH_DEADLINE: Duration = Duration::from_secs(60);

/// Redirect hops followed (each one re-vetted).
pub const CBL_MAX_REDIRECTS: usize = 2;

/// Budget for the remote part of refreshing or checking one list. A URL
/// list makes one fetch; a catalog list may make two (index and blob), so
/// this is two fetch deadlines.
pub const CBL_LIST_FETCH_BUDGET: Duration = Duration::from_secs(120);

/// Bound on one run of the hourly refresh sweep. Once spent, the sweep
/// starts no further lists; the ones it skipped are still due and run on
/// the next tick. Half the tick interval, so runs never overlap.
pub const CBL_SWEEP_BUDGET: Duration = Duration::from_secs(30 * 60);

/// Fetch a reading list from its URL. With `conditional` validators, a
/// `304 Not Modified` comes back as [`FetchOutcome::NotModified`].
pub async fn fetch_cbl_url(
    url: &str,
    conditional: Conditional<'_>,
) -> Result<FetchOutcome, FetchBytesError> {
    ssrf::fetch_public(url, &cbl_fetch_options(), conditional).await
}

fn cbl_fetch_options() -> FetchOptions {
    FetchOptions {
        max_bytes: MAX_URL_CBL_BYTES,
        idle_timeout: CBL_FETCH_STEP_TIMEOUT,
        deadline: Some(CBL_FETCH_DEADLINE),
        user_agent: crate::build_info::USER_AGENT,
        max_redirects: CBL_MAX_REDIRECTS,
        require_https: true,
    }
}
