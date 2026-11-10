//! Cookie shapes for the web session (§17.2, §17.3).
//!
//! All cookies use the `__Host-` prefix (browser-enforced: requires Secure, no Domain,
//! Path=/). The session and refresh cookies are HttpOnly; the CSRF cookie is not — JS
//! reads it for the double-submit header.

use axum_extra::extract::cookie::{Cookie, SameSite};
use std::time::Duration;

pub const SESSION_COOKIE: &str = "__Host-comic_session";
/// next-intl reads this cookie when the URL has no `[locale]` segment
/// (post-Human-URLs M3). Not HttpOnly — the client also needs to read it
/// to update the locale immediately on language-preference change. Lasts
/// one year so unauthenticated visitors don't get locale flapping on
/// every Accept-Language reparse.
pub const LOCALE_COOKIE: &str = "NEXT_LOCALE";
pub const REFRESH_COOKIE: &str = "__Host-comic_refresh";
pub const LEGACY_REFRESH_COOKIE: &str = "__Secure-comic_refresh";
pub const CSRF_COOKIE: &str = "__Host-comic_csrf";
/// The current access token's expiry as epoch seconds, readable by
/// JavaScript so the web client can refresh before the token lapses. Holds
/// no secret; set and cleared with the session cookie.
pub const ACCESS_EXP_COOKIE: &str = "__Host-comic_access_exp";

pub const REFRESH_PATH: &str = "/";

pub fn session_cookie(value: String, max_age: Duration) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, value);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time_from_std(max_age));
    c
}

pub fn refresh_cookie(value: String, max_age: Duration) -> Cookie<'static> {
    let mut c = Cookie::new(REFRESH_COOKIE, value);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path(REFRESH_PATH);
    c.set_max_age(time_from_std(max_age));
    c
}

pub fn csrf_cookie(value: String, max_age: Duration) -> Cookie<'static> {
    let mut c = Cookie::new(CSRF_COOKIE, value);
    // NOT HttpOnly — JS reads this for the X-CSRF-Token header.
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time_from_std(max_age));
    c
}

/// The [`ACCESS_EXP_COOKIE`]: `<exp>.<lifetime>` — the access token's `exp`
/// (epoch seconds) and its lifetime in seconds — with that same lifetime as
/// Max-Age. The lifetime lets the client space its refreshes without
/// trusting its own clock. Not HttpOnly, by design.
pub fn access_exp_cookie(exp: i64, max_age: Duration) -> Cookie<'static> {
    let mut c = Cookie::new(ACCESS_EXP_COOKIE, format!("{exp}.{}", max_age.as_secs()));
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time_from_std(max_age));
    c
}

/// Max-Age of the CSRF cookie: what the refresh cookie gets (the refresh TTL,
/// clamped to `remaining_age` when the session's creation time is known),
/// but never less than the access lifetime — every mutation the access
/// cookie still authorizes needs the CSRF cookie too.
pub fn csrf_max_age(
    access_ttl: Duration,
    refresh_ttl: Duration,
    remaining_age: Option<Duration>,
) -> Duration {
    remaining_age
        .map_or(refresh_ttl, |r| refresh_ttl.min(r))
        .max(access_ttl)
}

/// What is left of a session's absolute age at `now` (zero once past it).
/// Never panics or wraps, whatever `max_age` holds.
pub fn remaining_session_age(
    created_at: chrono::DateTime<chrono::FixedOffset>,
    max_age: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> Duration {
    let Ok(max_age_delta) = chrono::TimeDelta::from_std(max_age) else {
        return max_age;
    };
    match created_at.checked_add_signed(max_age_delta) {
        Some(end) => (end.with_timezone(&chrono::Utc) - now)
            .to_std()
            .unwrap_or_default(),
        None => max_age,
    }
}

/// `std` → `chrono` duration, saturating instead of wrapping.
pub fn chrono_delta(d: Duration) -> chrono::TimeDelta {
    chrono::TimeDelta::from_std(d).unwrap_or(chrono::TimeDelta::MAX)
}

/// Whole seconds of `d` as `i64`, saturating.
pub fn secs_i64(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

/// The caller's CSRF value when its cookie is present, else a fresh one;
/// the flag is true when it was minted here. Only sign-in, registration,
/// refresh and the single sign-on callback always mint: rotating the value
/// under a request already in flight fails that request with 403.
pub fn existing_or_new_csrf(jar: &axum_extra::extract::CookieJar) -> (String, bool) {
    match jar.get(CSRF_COOKIE).map(|c| c.value().to_owned()) {
        Some(v) if !v.is_empty() => (v, false),
        _ => (new_csrf_token(), true),
    }
}

/// `NEXT_LOCALE` cookie — readable by both server (via `Accept-Language`
/// fallback) and the client locale-switcher. Long-lived (1 year) since
/// language is a stable user preference, not a session-scoped value.
/// Cannot use `__Host-` prefix because next-intl looks for the bare name.
pub fn locale_cookie(value: String) -> Cookie<'static> {
    let mut c = Cookie::new(LOCALE_COOKIE, value);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(time::Duration::days(365));
    c
}

pub fn clear(
    name: impl Into<std::borrow::Cow<'static, str>>,
    path: &'static str,
) -> Cookie<'static> {
    let mut c = Cookie::new(name, "");
    c.set_path(path);
    c.set_max_age(time::Duration::seconds(0));
    // CRITICAL: `__Host-` and `__Secure-` prefixed cookies require the
    // `Secure` attribute on every Set-Cookie that touches them — INCLUDING
    // the deletion. Browsers (Chrome, Firefox, Safari) silently reject any
    // clear-cookie that omits `Secure`, leaving the original cookie in
    // place. Without this, /auth/logout's response cleared no cookies and
    // the user appeared to remain signed in.
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c
}

/// Expire every auth cookie on `jar` — session, access expiry, CSRF,
/// refresh and the legacy refresh name. The one list both sign-out and
/// account deletion clear, so a cookie added here is cleared by both.
pub fn clear_auth(jar: axum_extra::extract::CookieJar) -> axum_extra::extract::CookieJar {
    jar.remove(clear(SESSION_COOKIE, "/"))
        .remove(clear(ACCESS_EXP_COOKIE, "/"))
        .remove(clear(CSRF_COOKIE, "/"))
        .remove(clear(REFRESH_COOKIE, REFRESH_PATH))
        .remove(clear(LEGACY_REFRESH_COOKIE, REFRESH_PATH))
}

fn time_from_std(d: Duration) -> time::Duration {
    time::Duration::seconds(d.as_secs().min(i64::MAX as u64) as i64)
}

/// Generate a fresh CSRF token (32 bytes, base64url, no padding).
pub fn new_csrf_token() -> String {
    use base64::Engine;
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a fresh refresh-token raw value (32 bytes, base64url, no padding).
pub fn new_refresh_token_raw() -> String {
    use base64::Engine;
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    // digest 0.11's output array no longer implements `LowerHex`.
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    const H: Duration = Duration::from_secs(3600);
    const D: Duration = Duration::from_secs(86_400);

    #[test]
    fn csrf_never_outlived_by_the_access_cookie() {
        // Last minutes of the absolute age: still covers the access cookie.
        assert_eq!(csrf_max_age(H, 30 * D, Some(Duration::ZERO)), H);
        assert_eq!(csrf_max_age(H, 30 * D, Some(Duration::from_secs(60))), H);
        // Mid-session: the refresh lifetime, clamped to what is left.
        assert_eq!(csrf_max_age(H, 30 * D, Some(2 * D)), 2 * D);
        assert_eq!(csrf_max_age(H, 30 * D, Some(60 * D)), 30 * D);
        // Creation time not at hand: the refresh lifetime.
        assert_eq!(csrf_max_age(H, 30 * D, None), 30 * D);
    }

    #[test]
    fn remaining_age_never_panics() {
        let now = chrono::Utc::now();
        let created = now.fixed_offset() - chrono::TimeDelta::days(89);
        let left = remaining_session_age(created, 90 * D, now);
        assert!(left <= D && left > D - Duration::from_secs(60));
        assert_eq!(remaining_session_age(created, 10 * D, now), Duration::ZERO);
        assert_eq!(
            remaining_session_age(created, Duration::from_secs(u64::MAX), now),
            Duration::from_secs(u64::MAX)
        );
        assert_eq!(secs_i64(Duration::from_secs(u64::MAX)), i64::MAX);
    }

    #[test]
    fn access_exp_cookie_carries_the_lifetime() {
        let c = access_exp_cookie(1_700_000_000, H);
        assert_eq!(c.value(), "1700000000.3600");
        assert_eq!(c.http_only(), None);
    }
}
