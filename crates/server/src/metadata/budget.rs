//! Per-provider request budget as reported *by the upstream* (WP-2.9).
//!
//! The Redis token buckets in [`crate::metadata::rate_limit`] are our
//! local pre-request throttle — they count what *we* sent. The budget
//! here is what the provider says is left, which is the number that
//! matters when the same account is also used by another tool (a
//! tagger, a second Folio instance), or when the operator's tier gives
//! more than the documented default.
//!
//! - **Metron** returns `X-RateLimit-{Burst,Sustained}-{Limit,Remaining,
//!   Reset}` on every response (`Reset` is a Unix timestamp; see
//!   `api/RATELIMIT.md` in the Metron repo). [`parse_metron_headers`]
//!   turns them into one [`RequestBudget`] per window and the client
//!   stores the pair under `metadata:budget:metron` after each call.
//! - **ComicVine** has no budget headers; its 200/h per-resource cap is
//!   enforced upstream with no feedback, so the admin surface derives
//!   the budget from the local bucket state via [`RequestBudget::from_bucket`].
//! - **GCD** (WP-6.1) likewise sends no budget headers; the bar is the
//!   local 2,000/day bucket mirroring the upstream user throttle.
//!
//! The last provider error is kept alongside (`metadata:last_error:<p>`)
//! so the admin card can show *why* a provider went quiet without the
//! operator digging through the job log. Both keys are best-effort:
//! a Redis hiccup never fails the request that produced the data.

use crate::metadata::identifier::Source;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

/// Refill window a budget figure applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BudgetWindow {
    Minute,
    Hour,
    Day,
}

/// One window's budget: how many requests the provider allows, how many
/// are left, and when the counter resets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestBudget {
    pub limit: u32,
    pub remaining: u32,
    pub reset_at: DateTime<Utc>,
    pub window: BudgetWindow,
}

impl RequestBudget {
    /// Derive a budget from a local token-bucket snapshot
    /// (`capacity`, `remaining`, `ttl_secs`). `ttl_secs == 0` means the
    /// window hasn't started — the bucket is full and "resets" now.
    pub fn from_bucket(capacity: u32, remaining: u32, ttl_secs: u64, window: BudgetWindow) -> Self {
        Self {
            limit: capacity,
            remaining: remaining.min(capacity),
            reset_at: Utc::now() + chrono::Duration::seconds(ttl_secs as i64),
            window,
        }
    }

    /// Fraction of the window's budget still available, `0.0..=1.0`.
    pub fn fraction_remaining(&self) -> f64 {
        if self.limit == 0 {
            return 0.0;
        }
        f64::from(self.remaining) / f64::from(self.limit)
    }
}

/// Everything the upstream told us on the last response, plus when.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BudgetState {
    pub windows: Vec<RequestBudget>,
    pub observed_at: Option<DateTime<Utc>>,
}

impl BudgetState {
    /// The window that governs the admin bar: the longest one (day over
    /// minute) — a burst window refills in seconds and isn't a "budget"
    /// an operator plans around.
    pub fn headline(&self) -> Option<&RequestBudget> {
        self.windows.iter().max_by_key(|b| match b.window {
            BudgetWindow::Minute => 0,
            BudgetWindow::Hour => 1,
            BudgetWindow::Day => 2,
        })
    }
}

/// The most recent provider error, for the admin card.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderLastError {
    pub message: String,
    pub at: DateTime<Utc>,
}

// ───────── Metron header parsing ─────────

/// Parse Metron's `X-RateLimit-*` headers. Returns one entry per window
/// that carried all three of `Limit` / `Remaining` / `Reset`; a partial
/// or absent set yields an empty vec rather than a half-filled budget.
pub fn parse_metron_headers(headers: &HeaderMap) -> Vec<RequestBudget> {
    let mut out = Vec::with_capacity(2);
    for (scope, window) in [
        ("Burst", BudgetWindow::Minute),
        ("Sustained", BudgetWindow::Day),
    ] {
        let get = |suffix: &str| -> Option<i64> {
            headers
                .get(format!("X-RateLimit-{scope}-{suffix}").to_ascii_lowercase())
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<i64>().ok())
        };
        let (Some(limit), Some(remaining), Some(reset)) =
            (get("Limit"), get("Remaining"), get("Reset"))
        else {
            continue;
        };
        let Some(reset_at) = DateTime::<Utc>::from_timestamp(reset, 0) else {
            continue;
        };
        out.push(RequestBudget {
            limit: limit.clamp(0, i64::from(u32::MAX)) as u32,
            remaining: remaining.clamp(0, i64::from(u32::MAX)) as u32,
            reset_at,
            window,
        });
    }
    out
}

/// The budget the admin bar + search dialog show for `source`:
/// - Metron: the last upstream-reported *sustained* (daily) window,
///   falling back to the local day bucket before the first response
///   has been seen.
/// - ComicVine: derived from the local hourly bucket (the upstream
///   enforces 200/h per resource with no feedback headers).
pub async fn for_provider(redis: &ConnectionManager, source: Source) -> Option<RequestBudget> {
    use crate::metadata::rate_limit::{self, COMICVINE_HOUR_CAPACITY, GCD_DAY, METRON_DAY};
    match source {
        Source::Metron => {
            if let Some(headline) = load(redis, source)
                .await
                .and_then(|s| s.headline().cloned())
            {
                return Some(headline);
            }
            let mut conn = redis.clone();
            let (remaining, ttl) = rate_limit::snapshot(&mut conn, &METRON_DAY).await.ok()?;
            Some(RequestBudget::from_bucket(
                METRON_DAY.capacity,
                remaining,
                ttl,
                BudgetWindow::Day,
            ))
        }
        Source::ComicVine => {
            let mut conn = redis.clone();
            // Per-resource buckets; the tightest one is the budget.
            let (remaining, ttl) = rate_limit::comicvine_hour_snapshot(&mut conn).await.ok()?;
            Some(RequestBudget::from_bucket(
                COMICVINE_HOUR_CAPACITY,
                remaining,
                ttl,
                BudgetWindow::Hour,
            ))
        }
        // GCD (WP-6.1) sends no budget headers; its binding limit is the
        // 2,000/day user throttle, mirrored by the local day bucket.
        Source::Gcd => {
            let mut conn = redis.clone();
            let (remaining, ttl) = rate_limit::snapshot(&mut conn, &GCD_DAY).await.ok()?;
            Some(RequestBudget::from_bucket(
                GCD_DAY.capacity,
                remaining,
                ttl,
                BudgetWindow::Day,
            ))
        }
        _ => None,
    }
}

// ───────── Redis persistence ─────────

/// Budget rows outlive the longest window by a margin so a quiet
/// provider still shows yesterday's figure (marked stale by
/// `observed_at`) instead of "no data".
const BUDGET_TTL_SECS: u64 = 26 * 3600;
const LAST_ERROR_TTL_SECS: u64 = 7 * 24 * 3600;

fn budget_key(source: Source) -> String {
    format!("metadata:budget:{}", source.as_str())
}

fn last_error_key(source: Source) -> String {
    format!("metadata:last_error:{}", source.as_str())
}

/// Persist the budget the upstream just reported. No-op on an empty
/// window list (a response without the headers shouldn't wipe the last
/// good figure).
pub async fn store(redis: &ConnectionManager, source: Source, windows: Vec<RequestBudget>) {
    if windows.is_empty() {
        return;
    }
    let state = BudgetState {
        windows,
        observed_at: Some(Utc::now()),
    };
    let Ok(json) = serde_json::to_string(&state) else {
        return;
    };
    let mut conn = redis.clone();
    let res: Result<(), redis::RedisError> =
        conn.set_ex(budget_key(source), json, BUDGET_TTL_SECS).await;
    if let Err(e) = res {
        tracing::debug!(provider = source.as_str(), error = %e, "budget store failed");
    }
}

/// Load the last stored budget, if any.
pub async fn load(redis: &ConnectionManager, source: Source) -> Option<BudgetState> {
    let mut conn = redis.clone();
    let raw: Option<String> = conn.get(budget_key(source)).await.ok().flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// Record the most recent failure for the admin card. Called by the
/// clients on any error path; the message is already secret-redacted
/// by the time it reaches a `ProviderError`.
pub async fn record_error(redis: &ConnectionManager, source: Source, message: &str) {
    let entry = ProviderLastError {
        message: message.chars().take(512).collect(),
        at: Utc::now(),
    };
    let Ok(json) = serde_json::to_string(&entry) else {
        return;
    };
    let mut conn = redis.clone();
    let res: Result<(), redis::RedisError> = conn
        .set_ex(last_error_key(source), json, LAST_ERROR_TTL_SECS)
        .await;
    if let Err(e) = res {
        tracing::debug!(provider = source.as_str(), error = %e, "last_error store failed");
    }
}

/// Clear the last-error marker after a successful call so the card
/// only shows a failure while it's current.
pub async fn clear_error(redis: &ConnectionManager, source: Source) {
    let mut conn = redis.clone();
    let _: Result<(), redis::RedisError> = conn.del(last_error_key(source)).await;
}

pub async fn load_last_error(
    redis: &ConnectionManager,
    source: Source,
) -> Option<ProviderLastError> {
    let mut conn = redis.clone();
    let raw: Option<String> = conn.get(last_error_key(source)).await.ok().flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn parses_both_metron_windows() {
        let headers = h(&[
            ("x-ratelimit-burst-limit", "20"),
            ("x-ratelimit-burst-remaining", "17"),
            ("x-ratelimit-burst-reset", "1700000060"),
            ("x-ratelimit-sustained-limit", "5000"),
            ("x-ratelimit-sustained-remaining", "4982"),
            ("x-ratelimit-sustained-reset", "1700003600"),
        ]);
        let got = parse_metron_headers(&headers);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].window, BudgetWindow::Minute);
        assert_eq!(got[0].limit, 20);
        assert_eq!(got[0].remaining, 17);
        assert_eq!(got[0].reset_at.timestamp(), 1_700_000_060);
        assert_eq!(got[1].window, BudgetWindow::Day);
        assert_eq!(got[1].limit, 5000);
        assert_eq!(got[1].remaining, 4982);
        let state = BudgetState {
            windows: got,
            observed_at: None,
        };
        assert_eq!(state.headline().unwrap().window, BudgetWindow::Day);
    }

    #[test]
    fn partial_window_is_dropped() {
        // Sustained is missing `Reset` → only burst survives.
        let headers = h(&[
            ("x-ratelimit-burst-limit", "20"),
            ("x-ratelimit-burst-remaining", "0"),
            ("x-ratelimit-burst-reset", "1700000060"),
            ("x-ratelimit-sustained-limit", "5000"),
            ("x-ratelimit-sustained-remaining", "10"),
        ]);
        let got = parse_metron_headers(&headers);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].window, BudgetWindow::Minute);
        assert!(parse_metron_headers(&HeaderMap::new()).is_empty());
    }

    #[test]
    fn bucket_derived_budget_clamps_remaining() {
        let b = RequestBudget::from_bucket(200, 250, 0, BudgetWindow::Hour);
        assert_eq!(b.remaining, 200);
        assert_eq!(b.limit, 200);
        assert!((b.fraction_remaining() - 1.0).abs() < f64::EPSILON);
        let z = RequestBudget::from_bucket(0, 0, 0, BudgetWindow::Hour);
        assert_eq!(z.fraction_remaining(), 0.0);
    }
}
