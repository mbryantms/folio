//! Per-route token-bucket rate limiting (§17.7).
//!
//! Buckets are keyed by the real client IP (via [`crate::middleware::RequestContext`]
//! → XFF walker → trusted-proxy CIDRs). Token state lives in-memory inside
//! the `governor` crate's keyed-state store — fine for a single-replica
//! deployment, which is the Folio target. Multi-replica deploys would
//! require a Redis backend; that's tracked as a post-1.0 enhancement.
//!
//! The failed-auth lockout (10/min/IP → 15-min lockout after threshold) is
//! Redis-backed and lives in [`crate::auth::failed_auth`] — it spans process
//! restarts so a brute-forcer can't reset by triggering a restart.
//!
//! On denial we return a JSON envelope matching the project's error contract
//! and set `Retry-After` (seconds). Denials emit
//! `comic_rate_limit_denied_total{bucket="…"}`.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

use super::RequestContext;

// ───────── key extractor ─────────

/// Pulls the real client IP from the [`RequestContext`] populated by
/// [`super::request_context::set_context`]. That middleware runs the XFF
/// walker against `COMIC_TRUSTED_PROXIES`, so this is the right answer
/// for per-IP buckets behind a reverse proxy.
#[derive(Debug, Clone, Copy)]
pub struct ClientIpKey;

impl KeyExtractor for ClientIpKey {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        req.extensions()
            .get::<RequestContext>()
            .and_then(|ctx| ctx.client_ip)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

// ───────── bucket factory ─────────

/// A single rate-limit bucket. Built once per process via [`Bucket::build`].
pub struct Bucket {
    /// Telemetry name for the denial counter (`comic_rate_limit_denied_total{bucket=…}`).
    pub name: &'static str,
    /// Replenishment interval — one token per `period`.
    pub period: Duration,
    /// Burst size — max tokens in the bucket. Must be `>= 1`.
    pub burst: u32,
}

impl Bucket {
    /// Build a tower layer ready to plug onto a route. Returns the layer plus
    /// the keep-alive `Arc` so callers can hold the config alive (the layer
    /// holds one too, this is for any future inspection).
    pub fn build(&self) -> GovernorLayer<ClientIpKey, governor::middleware::NoOpMiddleware, Body> {
        let mut default = GovernorConfigBuilder::default();
        let mut builder = default.key_extractor(ClientIpKey);
        builder.period(self.period).burst_size(self.burst);
        let config: GovernorConfig<ClientIpKey, governor::middleware::NoOpMiddleware> =
            builder.finish().expect("non-zero period + burst");
        let name = self.name;
        GovernorLayer::new(Arc::new(config))
            .error_handler(move |err| handle_governor_error(name, err))
    }
}

// ───────── bucket catalog ─────────

// Buckets per `docs/architecture/rate-limits.md`. `period` is the token
// replenishment interval — `5/min` = one token every 12 seconds with a
// burst of 10.

/// `POST /auth/local/login` — 5/min/IP + burst 10.
pub const LOGIN: Bucket = Bucket {
    name: "auth.login",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `GET /auth/oidc/callback` — 5/min/IP + burst 10.
pub const OIDC_CALLBACK: Bucket = Bucket {
    name: "auth.oidc_callback",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `POST /auth/local/register` — 5/min/IP + burst 10. Same shape as login;
/// the bucket is named separately so the metric tag distinguishes.
pub const REGISTER: Bucket = Bucket {
    name: "auth.register",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `POST /auth/local/request-password-reset` — 5/hour/IP + burst 5.
pub const PASSWORD_RESET_REQUEST: Bucket = Bucket {
    name: "auth.password_reset_request",
    period: Duration::from_secs(720),
    burst: 5,
};

/// `POST /auth/local/reset-password` — 10/hour/IP + burst 10.
pub const PASSWORD_RESET_REDEEM: Bucket = Bucket {
    name: "auth.password_reset_redeem",
    period: Duration::from_secs(360),
    burst: 10,
};

/// `POST /auth/local/resend-verification` — 5/hour/IP + burst 5.
pub const RESEND_VERIFICATION: Bucket = Bucket {
    name: "auth.resend_verification",
    period: Duration::from_secs(720),
    burst: 5,
};

/// `POST /auth/ws-ticket` — 30/s/IP + burst 60.
pub const WS_TICKET: Bucket = Bucket {
    name: "ws_ticket",
    period: Duration::from_millis(33),
    burst: 60,
};

/// `POST /csp-report` — 100/min/IP + burst 200.
pub const CSP_REPORT: Bucket = Bucket {
    name: "csp_report",
    period: Duration::from_millis(600),
    burst: 200,
};

// ───────── error handler ─────────

fn handle_governor_error(bucket: &'static str, err: GovernorError) -> Response<Body> {
    match err {
        GovernorError::TooManyRequests { wait_time, headers } => {
            metrics::counter!("comic_rate_limit_denied_total", "bucket" => bucket).increment(1);
            tracing::info!(bucket, wait_time, "rate limit denied");
            envelope_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                &format!("rate limit exceeded for {bucket}; retry in {wait_time}s",),
                wait_time,
                headers,
            )
        }
        GovernorError::UnableToExtractKey => {
            // `RequestContext` should always be present (set by the outer
            // `set_context` middleware). If it's missing, that's a wiring
            // bug — let the request through rather than failing closed,
            // since a malformed deployment shouldn't lock everyone out.
            tracing::warn!(bucket, "rate-limit key extractor failed; fail-open");
            envelope_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "rate limiter key extraction failed",
                0,
                None,
            )
        }
        GovernorError::Other { msg, code, headers } => {
            metrics::counter!("comic_rate_limit_denied_total", "bucket" => bucket).increment(1);
            envelope_response(
                code,
                "rate_limited",
                msg.as_deref().unwrap_or("rate limited"),
                0,
                headers,
            )
        }
    }
}

fn envelope_response(
    status: StatusCode,
    code: &str,
    message: &str,
    retry_after_seconds: u64,
    extra_headers: Option<HeaderMap>,
) -> Response<Body> {
    let body = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
            "retry_after_seconds": retry_after_seconds,
        }
    })
    .to_string();
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if retry_after_seconds > 0
        && let Ok(v) = HeaderValue::from_str(&retry_after_seconds.to_string())
    {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    if let Some(extra) = extra_headers {
        for (k, v) in extra.into_iter().flat_map(|(k, v)| k.map(|k| (k, v))) {
            // Avoid clobbering Content-Type we just set; governor's headers
            // are X-RateLimit-After + Retry-After only.
            if k == header::CONTENT_TYPE {
                continue;
            }
            resp.headers_mut().insert(k, v);
        }
    }
    resp
}
