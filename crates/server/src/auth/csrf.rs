//! Double-submit CSRF middleware (§17.3).
//!
//! Required on every unsafe verb (POST/PUT/PATCH/DELETE) **when the request is
//! cookie-authenticated**. Bearer-authenticated requests bypass the check (no
//! ambient credential to forge).
//!
//! The token is the value of the `__Host-comic_csrf` cookie. It must match
//! either:
//!   - the `X-CSRF-Token` HTTP header (XHR happy path, in constant time), OR
//!   - a hidden `csrf_token=…` field in an `application/x-www-form-urlencoded`
//!     body (progressive-enhancement no-JS form path, also constant time).
//!
//! Endpoints exempted by path:
//!   - `/auth/oidc/callback`         (cross-origin redirect from issuer; state cookie gates it)
//!   - `/auth/local/login`           (no session yet — login establishes one)
//!   - `/auth/local/register`        (same)
//!   - `/auth/local/request-password-reset`  (no session)
//!   - `/auth/local/reset-password`  (token-gated)
//!   - `/auth/local/verify-email`    (token-gated, GET)
//!   - `/auth/local/resend-verification` (no session)
//!   - `/csp-report`                 (browser sends without CSRF)

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderName, Method, StatusCode, header::CONTENT_TYPE},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use constant_time_eq::constant_time_eq;

use super::cookies::CSRF_COOKIE;

const CSRF_HEADER: &str = "x-csrf-token";
/// Max body size we'll buffer in order to scan the form for a CSRF field.
/// Auth forms are tiny (<1 KB); this is just a guard so a misbehaving client
/// can't make us hold a multi-MB body in memory just to validate CSRF.
const MAX_FORM_BODY_FOR_CSRF: usize = 64 * 1024;

pub async fn require_csrf(req: Request, next: Next) -> Response {
    if !is_unsafe(req.method()) {
        return next.run(req).await;
    }
    if path_is_exempt(req.uri().path()) {
        return next.run(req).await;
    }
    if has_token_auth(req.headers()) {
        // Out-of-band credential (Bearer JWT/app-password, or Basic
        // carrying an `app_…` token — what OPDS clients send). No
        // session cookie is implicated, so CSRF is moot.
        return next.run(req).await;
    }

    let jar = CookieJar::from_headers(req.headers());
    let Some(cookie_value) = jar.get(CSRF_COOKIE).map(|c| c.value().to_owned()) else {
        return csrf_error();
    };
    if cookie_value.is_empty() {
        return csrf_error();
    }

    // Header path: XHR submits the token in `X-CSRF-Token`.
    let header_value = req
        .headers()
        .get(HeaderName::from_static(CSRF_HEADER))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Some(h) = header_value
        && constant_time_eq(cookie_value.as_bytes(), h.as_bytes())
    {
        return next.run(req).await;
    }

    // Form path: progressive-enhancement no-JS fallback submits the token
    // as a hidden `csrf_token=…` form field. Only buffer the body when
    // the request actually claims to be form-encoded — JSON bodies never
    // carry a CSRF field, so falling through here for them would be wasted
    // work and a potential body-size DoS surface.
    let is_form = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .map(|c| c.starts_with("application/x-www-form-urlencoded"))
        .unwrap_or(false);
    if !is_form {
        return csrf_error();
    }

    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_FORM_BODY_FOR_CSRF).await {
        Ok(b) => b,
        Err(_) => return csrf_error(),
    };

    // Extract `csrf_token=…` via `serde_urlencoded`. Tolerant of any extra
    // form fields — only this one drives the auth decision. Picking up
    // any value lets the handler downstream consume the same body.
    let mut form_token: Option<String> = None;
    for (k, v) in serde_urlencoded::from_bytes::<Vec<(String, String)>>(&bytes).unwrap_or_default()
    {
        if k == "csrf_token" {
            form_token = Some(v);
            break;
        }
    }

    let matched = form_token
        .as_deref()
        .map(|v| constant_time_eq(cookie_value.as_bytes(), v.as_bytes()))
        .unwrap_or(false);
    if !matched {
        return csrf_error();
    }

    // Re-attach the buffered bytes so the inner handler can read the body
    // as if we had never touched it. The token field is left in place —
    // `serde_urlencoded` ignores unknown keys, and `FormOrJson<T>` only
    // pulls the fields T declares.
    let req = Request::from_parts(parts, Body::from(bytes));
    next.run(req).await
}

fn is_unsafe(m: &Method) -> bool {
    matches!(
        m,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

/// True when the request carries an out-of-band credential the CSRF
/// middleware should consider equivalent to a Bearer token: either an
/// explicit `Authorization: Bearer …` OR an `Authorization: Basic …`
/// whose decoded password component is an `app_…` token. Basic carrying
/// a raw password (login flow only — and login is CSRF-exempt anyway)
/// or a JWT is *not* treated as token auth.
fn has_token_auth(h: &axum::http::HeaderMap) -> bool {
    let Some(raw) = h
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if raw.starts_with("Bearer ") {
        return true;
    }
    if let Some(b64) = raw.strip_prefix("Basic ") {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
            && let Ok(s) = std::str::from_utf8(&decoded)
            && let Some((_user, password)) = s.split_once(':')
        {
            return super::app_password::looks_like_app_password(password);
        }
    }
    false
}

/// Routes exempted from CSRF double-submit. These are either:
/// - cross-origin redirects from a trusted issuer (`/auth/oidc/callback`,
///   guarded by the state cookie)
/// - unauthenticated bootstrap endpoints (login/register — no session
///   exists yet, so there's no ambient credential to forge)
/// - token-gated endpoints (verify-email, reset-password — the token in
///   the URL is the credential)
/// - browser-injected reports (`/csp-report` — sent without CSRF by spec)
///
/// Kept here as a single const so additions touch one site instead of
/// drifting between csrf middleware + handler stubs.
pub const CSRF_EXEMPT_PATHS: &[&str] = &[
    "/auth/oidc/callback",
    "/auth/local/login",
    "/auth/local/register",
    "/auth/local/request-password-reset",
    "/auth/local/reset-password",
    "/auth/local/verify-email",
    "/auth/local/resend-verification",
    "/csp-report",
];

fn path_is_exempt(path: &str) -> bool {
    CSRF_EXEMPT_PATHS.contains(&path)
}

fn csrf_error() -> Response {
    let body = serde_json::json!({
        "error": {
            "code": "auth.csrf",
            "message": "CSRF token missing or mismatched"
        }
    });
    let mut resp = (StatusCode::FORBIDDEN, axum::Json(body)).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    resp
}

