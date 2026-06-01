//! Failed-auth IP lockout (§17.7 — auth.failed bucket).
//!
//! Counts failed `/auth/local/login` and `/auth/oidc/callback` attempts per
//! client IP in Redis. Once an IP crosses 10 failures in any 60-second
//! sliding window, set a sentinel `auth_lockout:{ip}` key with 15-minute
//! TTL — the login/callback handlers consult that key on entry and refuse
//! to proceed while it's set.
//!
//! This is in addition to the per-route tower_governor bucket: the bucket
//! limits incoming RPS, but a slow brute-forcer staying just under the
//! token-bucket replenishment rate would otherwise be uncapped. The
//! counter is in Redis so a server restart can't reset the window.
//!
//! Failures here never bubble up — if Redis is unavailable we log and
//! fail open, since the right answer for a degraded backend is to keep
//! letting real users in. The per-route bucket still protects against
//! the worst burst case.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::time::Duration;

use crate::middleware::RequestContext;
use crate::state::AppState;

/// Max failed attempts in the sliding window before the IP is locked out.
pub const FAIL_THRESHOLD: u32 = 10;
/// Sliding-window TTL on the failure counter. Each new failure refreshes it
/// so the counter only resets after a quiet minute.
pub const FAIL_WINDOW: Duration = Duration::from_secs(60);
/// Lockout duration once the threshold is crossed.
pub const LOCKOUT: Duration = Duration::from_secs(15 * 60);

fn counter_key(ip: IpAddr) -> String {
    format!("auth_fail:{ip}")
}

fn lockout_key(ip: IpAddr) -> String {
    format!("auth_lockout:{ip}")
}

/// Stable identifier for the email-keyed lockout bucket. SHA-256 of the
/// lower-cased trimmed email, truncated to 16 hex chars (64 bits — plenty
/// to distinguish buckets without enumerable plaintext sitting in Redis
/// keyspace). Lower-cased input is the same form already used for
/// `users.email` lookups, so the axis catches credential-stuffing
/// attempts regardless of how the attacker capitalises the address.
fn email_id(email: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(email.trim().to_lowercase().as_bytes());
    let digest = hasher.finalize();
    // 16 hex chars = 8 bytes
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn counter_key_email(email: &str) -> String {
    format!("auth_fail_email:{}", email_id(email))
}

fn lockout_key_email(email: &str) -> String {
    format!("auth_lockout_email:{}", email_id(email))
}

/// Check whether `ip` is currently in lockout. Returns `Ok(None)` to proceed
/// or `Ok(Some(seconds))` if locked out with the remaining TTL. Errors in
/// the underlying Redis call return `Ok(None)` — see the module-doc
/// fail-open rationale.
pub async fn check_lockout(
    mut redis: redis::aio::ConnectionManager,
    ip: IpAddr,
) -> Result<Option<u64>, redis::RedisError> {
    let key = lockout_key(ip);
    let ttl: i64 = match redis.ttl(&key).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, ip = %ip, "failed_auth: TTL check failed; fail-open");
            return Ok(None);
        }
    };
    // Redis TTL returns -2 if the key doesn't exist, -1 if it exists with no
    // expiry. We only set with PEXPIRE so -1 shouldn't happen, but treat
    // both as "not locked."
    if ttl > 0 {
        Ok(Some(ttl as u64))
    } else {
        Ok(None)
    }
}

/// Record an auth failure for `ip`. INCRs the counter, refreshes the window
/// TTL, and sets the lockout sentinel once the threshold is crossed.
/// Failures here are logged + swallowed.
pub async fn record_failure(mut redis: redis::aio::ConnectionManager, ip: IpAddr) {
    let counter = counter_key(ip);
    // INCR + EXPIRE in two commands. We could use a pipeline but the redis
    // crate's INCR returns the new value and EXPIRE is fire-and-forget;
    // a slight race is fine — worst case the lockout fires one attempt
    // late.
    let count: i64 = match redis.incr(&counter, 1).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, ip = %ip, "failed_auth: INCR failed; fail-open");
            return;
        }
    };
    let _: Result<(), _> = redis.expire(&counter, FAIL_WINDOW.as_secs() as i64).await;

    if count as u32 >= FAIL_THRESHOLD {
        let lk = lockout_key(ip);
        let secs = LOCKOUT.as_secs() as i64;
        // SET with EX (atomic set + expiry). Tolerate failure.
        let set: Result<(), _> = redis.set_ex(&lk, "1", LOCKOUT.as_secs()).await;
        if let Err(e) = set {
            tracing::warn!(error = %e, ip = %ip, "failed_auth: lockout SET failed; fail-open");
            return;
        }
        tracing::warn!(
            ip = %ip,
            count,
            ttl_secs = secs,
            "failed_auth: IP locked out after {} failures", count
        );
        metrics::counter!("folio_auth_lockout_total").increment(1);
    }
}

/// Convenience used by `local::login` and `oidc::callback` on every auth
/// failure path. No-op when we don't have a client IP (which shouldn't
/// happen given the `set_context` middleware) or when the operator has
/// flipped `auth.rate_limit_enabled = false` via /admin/server.
pub async fn record_failure_for(app: &AppState, ctx: &RequestContext) {
    if !app.cfg().rate_limit_enabled {
        return;
    }
    if let Some(ip) = ctx.client_ip {
        record_failure(app.jobs.redis.clone(), ip).await;
    }
}

/// AppState-aware wrapper around [`check_lockout`] that short-circuits
/// to `Ok(None)` when the operator has disabled rate limiting. Use this
/// in preference to calling [`check_lockout`] directly so the kill
/// switch covers both write (record) and read (check) paths uniformly.
pub async fn check_lockout_for(
    app: &AppState,
    ip: IpAddr,
) -> Result<Option<u64>, redis::RedisError> {
    if !app.cfg().rate_limit_enabled {
        return Ok(None);
    }
    check_lockout(app.jobs.redis.clone(), ip).await
}

// ─────────────── email-keyed axis (Phase B B3) ──────────────────
// IP-keyed lockout catches "one IP spraying many usernames" but leaves
// "many IPs targeting one user" (credential stuffing from a botnet)
// uncovered. The email axis fills that gap. Keys are SHA-256-truncated
// digests of the lower-cased email so the Redis keyspace doesn't leak
// plaintext addresses to anyone with KEYS/SCAN access.

/// Record a failure for `email` (any string the user supplied — we
/// hash it before storing so unknown-account attempts can't enumerate
/// real addresses via a Redis dump). Mirrors [`record_failure`].
pub async fn record_failure_for_email_value(mut redis: redis::aio::ConnectionManager, email: &str) {
    let counter = counter_key_email(email);
    let count: i64 = match redis.incr(&counter, 1).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "failed_auth(email): INCR failed; fail-open");
            return;
        }
    };
    let _: Result<(), _> = redis.expire(&counter, FAIL_WINDOW.as_secs() as i64).await;
    if count as u32 >= FAIL_THRESHOLD {
        let lk = lockout_key_email(email);
        let set: Result<(), _> = redis.set_ex(&lk, "1", LOCKOUT.as_secs()).await;
        if let Err(e) = set {
            tracing::warn!(error = %e, "failed_auth(email): lockout SET failed; fail-open");
            return;
        }
        tracing::warn!(
            count,
            "failed_auth(email): account locked out after {} failures",
            count
        );
        metrics::counter!("folio_auth_lockout_email_total").increment(1);
    }
}

/// Check email-keyed lockout. Same fail-open semantics as
/// [`check_lockout`] — Redis flakes never bounce real users.
pub async fn check_lockout_for_email(
    app: &AppState,
    email: &str,
) -> Result<Option<u64>, redis::RedisError> {
    if !app.cfg().rate_limit_enabled {
        return Ok(None);
    }
    let mut redis = app.jobs.redis.clone();
    let key = lockout_key_email(email);
    let ttl: i64 = match redis.ttl(&key).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "failed_auth(email): TTL check failed; fail-open");
            return Ok(None);
        }
    };
    if ttl > 0 {
        Ok(Some(ttl as u64))
    } else {
        Ok(None)
    }
}

/// Convenience: record a failure on the email axis. Honours the operator
/// kill switch. Pair with [`record_failure_for`] so every failed attempt
/// counts against both axes.
pub async fn record_failure_for_email(app: &AppState, email: &str) {
    if !app.cfg().rate_limit_enabled {
        return;
    }
    record_failure_for_email_value(app.jobs.redis.clone(), email).await;
}

/// Build the 429 response returned when an IP is in lockout. Shares the
/// envelope shape with the rate-limit middleware so clients can parse one
/// response format for both.
pub fn lockout_response(retry_after_seconds: u64) -> Response {
    let body = serde_json::json!({
        "error": {
            "code": "auth.locked_out",
            "message": "too many failed sign-in attempts; try again later",
            "retry_after_seconds": retry_after_seconds,
        }
    });
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    if let Ok(v) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    resp
}
