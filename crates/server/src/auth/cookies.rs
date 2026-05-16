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
// Refresh cookie uses `__Secure-` (not `__Host-`) so it can carry any Path. We
// still set Path=/ here — the original narrower `Path=/auth/refresh` was an
// over-tightening that broke the dev proxy: Next rewrites client-side
// `/api/auth/refresh` → `/auth/refresh` on the wire, but the browser checks
// the cookie's Path against the *original* request URL `/api/auth/refresh`,
// which doesn't match `/auth/refresh`. Result: refresh fired without the
// refresh cookie attached, the server returned 401, and the user was
// hard-bounced every access-cookie expiry. SameSite=Lax + HttpOnly already
// give us the cross-site / JS-read protections; the Path narrowing was
// belt-and-suspenders that never paid off.
pub const REFRESH_COOKIE: &str = "__Secure-comic_refresh";
pub const CSRF_COOKIE: &str = "__Host-comic_csrf";

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

pub fn clear(name: &'static str, path: &'static str) -> Cookie<'static> {
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

fn time_from_std(d: Duration) -> time::Duration {
    time::Duration::seconds(d.as_secs().min(i64::MAX as u64) as i64)
}

/// Generate a fresh CSRF token (32 bytes, base64url, no padding).
pub fn new_csrf_token() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a fresh refresh-token raw value (32 bytes, base64url, no padding).
pub fn new_refresh_token_raw() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}
