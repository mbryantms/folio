//! Shared outbound-HTTP layer for the provider clients (WP-2.9).
//!
//! Every provider client (ComicVine, Metron, future GCD) sends its API
//! calls through [`send_with_retry`] so the resilience rules live in
//! one place:
//!
//! - **Client construction** — [`build_client`] sets a connect timeout
//!   and a two-hop redirect cap on top of the caller's total timeout.
//!   Base URLs are provider-owned constants, so the redirect cap is a
//!   tripwire, not an SSRF guard (cover fetches go through
//!   [`crate::util::ssrf`] instead).
//! - **Bounded retry** — transport errors and 5xx responses are retried
//!   up to [`RetryPolicy::max_retries`] times with jittered exponential
//!   backoff (200 ms base, 5 s cap). 4xx responses are *never* retried:
//!   a 401/404/429 is an answer, not a hiccup, and the caller classifies
//!   it. A retry is also skipped when its backoff would land past the
//!   caller's deadline — a search job with a budget must fail fast
//!   rather than sleep through it.
//! - **Body cap** — API JSON is streamed into memory with
//!   [`MAX_BODY_BYTES`] enforced, so a misbehaving upstream can't balloon
//!   the worker's heap. Covers keep their own (larger) cap in the SSRF
//!   fetcher.
//! - **`Retry-After`** — [`retry_after_secs`] parses both the
//!   delta-seconds and the HTTP-date forms so a 429 carries the
//!   upstream's real wait instead of a hardcoded 60.
//!
//! The Redis token bucket + velocity floor stay *in front* of this
//! layer as the pre-request throttle; the retry loop never re-reserves
//! a bucket token because the upstream counts the retried request
//! against the same window either way.

use crate::metadata::provider::{ProviderError, ProviderResult};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{StatusCode, redirect::Policy};
use std::time::{Duration, Instant};

/// TCP/TLS connect budget. Distinct from the total request timeout so
/// a black-holed upstream fails in 10 s instead of eating the whole
/// 30 s request budget before the first byte.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Max redirect hops the API clients follow. Provider base URLs are
/// constants; anything beyond a single `http → https` or trailing-slash
/// bounce is suspicious.
pub const MAX_REDIRECTS: usize = 2;

/// Hard cap on an API response body. The largest legitimate payload we
/// pull (a CV issue detail with hundreds of credits) is well under 1 MiB.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Fallback `retry_after_secs` when a 429 carries no usable
/// `Retry-After` header.
pub const DEFAULT_RETRY_AFTER_SECS: u64 = 60;

/// Build the per-provider `reqwest::Client` with the hardening knobs
/// applied uniformly.
pub fn build_client(user_agent: &'static str, timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(Policy::limited(MAX_REDIRECTS))
        .build()
        .expect("reqwest client init")
}

/// Retry shape for transport errors + 5xx. `max_retries` counts
/// *additional* attempts after the first, so the default of 3 means
/// four requests worst-case.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base: Duration,
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base: Duration::from_millis(200),
            cap: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// No retries at all — for callers (tests, one-shot probes) that
    /// want the raw upstream answer.
    pub const NONE: RetryPolicy = RetryPolicy {
        max_retries: 0,
        base: Duration::from_millis(0),
        cap: Duration::from_millis(0),
    };

    /// Backoff before retry number `attempt` (0-based). Exponential on
    /// `base`, capped at `cap`, then scaled by `0.5 + jitter/2` so
    /// concurrent workers don't stampede the upstream in lockstep.
    /// `jitter` is a uniform sample in `[0, 1)`; it's a parameter (not
    /// drawn inside) so the schedule is unit-testable.
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let exp = self
            .base
            .checked_mul(1u32 << attempt.min(16))
            .unwrap_or(self.cap)
            .min(self.cap);
        exp.mul_f64(0.5 + jitter.clamp(0.0, 1.0) * 0.5)
    }
}

/// Per-call knobs for [`send_with_retry`].
#[derive(Clone, Copy, Debug)]
pub struct RequestOpts {
    pub policy: RetryPolicy,
    /// Absolute wall-clock cutoff. A retry whose backoff would end after
    /// this instant is not attempted; the last error is returned instead.
    pub deadline: Option<Instant>,
    pub max_body_bytes: usize,
}

impl Default for RequestOpts {
    fn default() -> Self {
        Self {
            policy: RetryPolicy::default(),
            deadline: None,
            max_body_bytes: MAX_BODY_BYTES,
        }
    }
}

/// A fully-buffered upstream response. Status + headers are kept so the
/// caller can classify (`429` → quota, `304` → not-modified) and read
/// rate-limit / validator headers without a second round-trip.
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    /// Lossy UTF-8 view of the body for error messages + JSON parsing.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Body prefix for error messages — never echoes a whole payload.
    pub fn snippet(&self, max: usize) -> String {
        let text = self.text();
        let mut end = text.len().min(max);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    }
}

/// Why an attempt failed in a way the loop may retry.
enum Retryable {
    Transport(String),
    Status(StatusCode, String),
}

impl Retryable {
    fn into_error(self) -> ProviderError {
        match self {
            Retryable::Transport(msg) => ProviderError::Transport(msg),
            Retryable::Status(status, snippet) => {
                if snippet.is_empty() {
                    ProviderError::Upstream(format!("HTTP {status}"))
                } else {
                    ProviderError::Upstream(format!("HTTP {status}: {snippet}"))
                }
            }
        }
    }
}

/// Send `build()` with bounded retry on transport errors + 5xx.
///
/// `build` is called once per attempt because `reqwest::RequestBuilder`
/// is consumed by `send()`. `redact` scrubs secrets (CV's `api_key`
/// query param) out of transport error strings before they reach logs
/// or the admin UI.
///
/// Returns `Ok` for **every** non-5xx status — including 4xx — so the
/// caller owns the classification; the only errors this function
/// produces are exhausted retries and an over-cap body.
pub async fn send_with_retry<F>(
    build: F,
    opts: &RequestOpts,
    redact: &(dyn Fn(&str) -> String + Sync),
) -> ProviderResult<Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt: u32 = 0;
    loop {
        let outcome = match build().send().await {
            Ok(resp) => {
                let status = resp.status();
                let headers = resp.headers().clone();
                match read_body(resp, opts.max_body_bytes).await {
                    Ok(body) if status.is_server_error() => {
                        let snippet = Response {
                            status,
                            headers,
                            body,
                        }
                        .snippet(256);
                        Err(Retryable::Status(status, snippet))
                    }
                    Ok(body) => Ok(Response {
                        status,
                        headers,
                        body,
                    }),
                    Err(BodyError::TooLarge(max)) => {
                        // Not retryable — the upstream will send the same
                        // oversized payload again.
                        return Err(ProviderError::InvalidResponse(format!(
                            "response body exceeds {max} bytes"
                        )));
                    }
                    Err(BodyError::Transport(e)) => Err(Retryable::Transport(redact(&e))),
                }
            }
            Err(e) => Err(Retryable::Transport(redact(&e.to_string()))),
        };

        let why = match outcome {
            Ok(resp) => return Ok(resp),
            Err(why) => why,
        };
        if attempt >= opts.policy.max_retries {
            return Err(why.into_error());
        }
        let delay = opts.policy.delay(attempt, rand::random::<f64>());
        if let Some(deadline) = opts.deadline
            && Instant::now() + delay >= deadline
        {
            tracing::debug!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                "provider http: retry would pass the deadline; giving up"
            );
            return Err(why.into_error());
        }
        tracing::debug!(
            attempt,
            delay_ms = delay.as_millis() as u64,
            "provider http: transient failure; backing off"
        );
        tokio::time::sleep(delay).await;
        attempt += 1;
    }
}

enum BodyError {
    TooLarge(usize),
    Transport(String),
}

/// Stream a body into memory with `max` enforced both on the declared
/// `Content-Length` and on the bytes actually received (a lying or
/// chunked upstream can't bypass the cap).
async fn read_body(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, BodyError> {
    if resp.content_length().is_some_and(|len| len > max as u64) {
        return Err(BodyError::TooLarge(max));
    }
    let mut stream = resp.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BodyError::Transport(e.to_string()))?;
        if out.len().saturating_add(chunk.len()) > max {
            return Err(BodyError::TooLarge(max));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Parse a `Retry-After` header (RFC 9110 §10.2.3) into whole seconds
/// from `now`. Accepts delta-seconds (`"7"`, `"7.5"`) and the HTTP-date
/// form (`"Wed, 21 Oct 2015 07:28:00 GMT"`). A date in the past yields
/// `Some(0)`; a missing / unparseable header yields `None`.
pub fn parse_retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<u64> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(secs);
    }
    if let Ok(secs) = raw.parse::<f64>()
        && secs.is_finite()
        && secs >= 0.0
    {
        return Some(secs.ceil() as u64);
    }
    // HTTP-date: IMF-fixdate is RFC 2822-compatible, including the
    // obsolete `GMT` zone name. chrono accepts `GMT`/`UT` but not the
    // asctime / RFC 850 legacy forms — rare enough in the wild that we
    // fall back to the default wait for those.
    let normalized = raw.strip_suffix(" GMT").map(|s| format!("{s} +0000"));
    let parsed = DateTime::parse_from_rfc2822(raw)
        .or_else(|e| match normalized {
            Some(ref n) => DateTime::parse_from_rfc2822(n),
            None => Err(e),
        })
        .ok()?;
    let delta = parsed.with_timezone(&Utc).timestamp() - now.timestamp();
    Some(delta.max(0) as u64)
}

/// `Retry-After` → `retry_after_secs`, with `fallback` when the header
/// is absent or unparseable, floored at 1 s so a `0` can't busy-loop
/// the quota park.
pub fn retry_after_secs(headers: &HeaderMap, fallback: u64) -> u64 {
    parse_retry_after(headers, Utc::now())
        .unwrap_or(fallback)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use reqwest::header::HeaderValue;

    fn headers(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn retry_after_parses_delta_seconds() {
        let now = Utc::now();
        assert_eq!(parse_retry_after(&headers("7"), now), Some(7));
        assert_eq!(parse_retry_after(&headers(" 42 "), now), Some(42));
        assert_eq!(parse_retry_after(&headers("7.2"), now), Some(8));
    }

    #[test]
    fn retry_after_parses_http_date_relative_to_now() {
        let now = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
        let h = headers("Sun, 01 Mar 2026 12:00:30 GMT");
        assert_eq!(parse_retry_after(&h, now), Some(30));
        // A date in the past clamps to zero rather than going negative.
        let past = headers("Sun, 01 Mar 2026 11:00:00 GMT");
        assert_eq!(parse_retry_after(&past, now), Some(0));
    }

    #[test]
    fn retry_after_missing_or_garbage_is_none() {
        let now = Utc::now();
        assert_eq!(parse_retry_after(&HeaderMap::new(), now), None);
        assert_eq!(parse_retry_after(&headers("soon"), now), None);
        assert_eq!(parse_retry_after(&headers(""), now), None);
        assert_eq!(retry_after_secs(&headers("soon"), 60), 60);
        // `0` floors to 1 so the park never spins.
        assert_eq!(retry_after_secs(&headers("0"), 60), 1);
    }

    #[test]
    fn backoff_is_exponential_capped_and_jittered() {
        let p = RetryPolicy::default();
        // jitter=1.0 → full delay; jitter=0.0 → half.
        assert_eq!(p.delay(0, 1.0), Duration::from_millis(200));
        assert_eq!(p.delay(0, 0.0), Duration::from_millis(100));
        assert_eq!(p.delay(1, 1.0), Duration::from_millis(400));
        assert_eq!(p.delay(2, 1.0), Duration::from_millis(800));
        // 200ms * 2^10 = 204 s → capped at 5 s.
        assert_eq!(p.delay(10, 1.0), Duration::from_secs(5));
        // Huge attempt numbers don't overflow.
        assert_eq!(p.delay(u32::MAX, 1.0), Duration::from_secs(5));
    }

    #[test]
    fn response_snippet_respects_char_boundaries() {
        let r = Response {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: "héllo".as_bytes().to_vec(),
        };
        // Byte 2 splits the two-byte `é`; snippet backs off to `h`.
        assert_eq!(r.snippet(2), "h");
        assert_eq!(r.snippet(100), "héllo");
    }
}
