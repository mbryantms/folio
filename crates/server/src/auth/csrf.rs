//! Double-submit CSRF middleware (§17.3).
//!
//! Required on every unsafe verb (POST/PUT/PATCH/DELETE) **when the request is
//! cookie-authenticated**. Requests carrying an out-of-band credential skip
//! the check, by [`token_exemption`]:
//!
//!   - `Authorization: Bearer <jwt>` — exempt on every route (a browser
//!     never attaches a Bearer header on its own, so there is no ambient
//!     credential to forge).
//!   - An app password (`Bearer app_…`, or `Basic` whose password is
//!     `app_…`) — exempt **only on the reader-client routes**
//!     ([`crate::auth::extractor::is_reader_route`]), the one surface where
//!     an app password authenticates. Elsewhere a request that also carries
//!     a session or refresh cookie gets the full cookie check like any
//!     browser request; one with no such cookie has no ambient credential
//!     to forge and passes through, so the extractor can refuse the app
//!     password with its own message ("app passwords only work with reader
//!     apps") instead of a misleading CSRF error.
//!   - App password via `Basic` on a reader route — exempt only when the
//!     request does not look like it came from a browser page on another
//!     site: no `Origin` header and no `Sec-Fetch-Site: cross-site` /
//!     `same-site`. A browser that has cached Basic credentials attaches
//!     them by itself; reader apps (Panels, Chunky, KOReader, Komga-style
//!     clients) are native HTTP clients and send neither header.
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
//!
//! The exempt sign-in POSTs (login, register, request-password-reset,
//! reset-password, resend-verification) carry no session to forge, but a
//! cross-site form could still sign a visitor into an account the attacker
//! controls (login CSRF). They get a cross-site guard instead of the token
//! check, [`crate::auth::origin::cross_site_reason`]: refused on
//! `Sec-Fetch-Site: cross-site` / `same-site`, or on an `Origin` that is
//! neither the configured public origin nor the host and port of the
//! request's own `Host` header (so same-origin posts over a LAN address or
//! another host name pass); requests with neither header pass. The refusal
//! is the standard 403 `auth.csrf` envelope, the same whichever check
//! failed. The OIDC callback (a cross-site redirect by design, gated by its
//! state cookie), `verify-email` (a GET) and `/csp-report` are not guarded.

use axum::{
    body::{Body, to_bytes},
    extract::{MatchedPath, Request, State},
    http::{
        HeaderMap, HeaderName, Method, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use constant_time_eq::constant_time_eq;

use super::cookies::{CSRF_COOKIE, LEGACY_REFRESH_COOKIE, REFRESH_COOKIE, SESSION_COOKIE};
use super::extractor::is_reader_route;
use crate::state::AppState;

const CSRF_HEADER: &str = "x-csrf-token";
const SEC_FETCH_SITE: &str = "sec-fetch-site";
/// Max body size we'll buffer in order to scan the form for a CSRF field.
/// Auth forms are tiny (<1 KB); this is just a guard so a misbehaving client
/// can't make us hold a multi-MB body in memory just to validate CSRF.
const MAX_FORM_BODY_FOR_CSRF: usize = 64 * 1024;

pub async fn require_csrf(State(app): State<AppState>, req: Request, next: Next) -> Response {
    if !is_unsafe(req.method()) {
        return next.run(req).await;
    }
    if path_is_exempt(req.uri().path()) {
        let peer_trusted = req
            .extensions()
            .get::<crate::middleware::RequestContext>()
            .is_some_and(|c| c.peer_trusted);
        if CROSS_SITE_GUARDED_PATHS.contains(&req.uri().path())
            && let Some(reason) = super::origin::cross_site_reason(
                req.headers(),
                req.uri().authority().map(|a| a.as_str()),
                &app.cfg().public_url,
                peer_trusted,
            )
        {
            return cross_site_refusal(req, reason).await;
        }
        return next.run(req).await;
    }
    if token_exemption(&req) {
        return next.run(req).await;
    }

    // M7 diagnostics — capture method + path up front so rejection
    // logs can include them after the request is consumed by
    // `into_parts`. Without this, CSRF rejections were silent and
    // indistinguishable from "no request arrived" in /admin/logs.
    let method = req.method().clone();
    let path = req.uri().path().to_owned();

    let jar = CookieJar::from_headers(req.headers());
    let Some(cookie_value) = jar.get(CSRF_COOKIE).map(|c| c.value().to_owned()) else {
        return csrf_error(&method, &path, "missing-cookie");
    };
    if cookie_value.is_empty() {
        return csrf_error(&method, &path, "empty-cookie");
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
        return csrf_error(&method, &path, "no-token");
    }

    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_FORM_BODY_FOR_CSRF).await {
        Ok(b) => b,
        Err(_) => return csrf_error(&method, &path, "body-too-large"),
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
        return csrf_error(&method, &path, "form-mismatch");
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

/// The out-of-band credential an `Authorization` header carries, as far as
/// the CSRF decision cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenCredential {
    /// `Bearer <anything but app_…>` — a JWT (or junk the extractor will
    /// refuse; it never falls back to the session cookie).
    BearerJwt,
    /// `Bearer app_…`.
    AppPasswordBearer,
    /// `Basic <b64(user:app_…)>` — what OPDS clients send.
    AppPasswordBasic,
}

fn token_credential(h: &HeaderMap) -> Option<TokenCredential> {
    let raw = h.get(AUTHORIZATION).and_then(|v| v.to_str().ok())?;
    if let Some(token) = raw.strip_prefix("Bearer ") {
        return Some(
            if super::app_password::looks_like_app_password(token.trim()) {
                TokenCredential::AppPasswordBearer
            } else {
                TokenCredential::BearerJwt
            },
        );
    }
    if let Some(b64) = raw.strip_prefix("Basic ") {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
            && let Ok(s) = std::str::from_utf8(&decoded)
            && let Some((_user, password)) = s.split_once(':')
            && super::app_password::looks_like_app_password(password)
        {
            return Some(TokenCredential::AppPasswordBasic);
        }
    }
    // Basic carrying a raw password (or a JWT) is not token auth: the
    // extractor refuses it, and it gets no CSRF exemption.
    None
}

/// True when the request is exempt from the double-submit check because of
/// the credential it carries (module docs). The route decision reads the
/// matched route template, which `Router::layer` middleware sees after
/// routing; a request no explicit route claimed (the Next.js fallback) has
/// none and is never a reader route.
fn token_exemption(req: &Request) -> bool {
    let Some(cred) = token_credential(req.headers()) else {
        return false;
    };
    let on_reader_route = || {
        req.extensions()
            .get::<MatchedPath>()
            .is_some_and(|p| is_reader_route(p.as_str()))
    };
    match cred {
        TokenCredential::BearerJwt => true,
        TokenCredential::AppPasswordBearer => {
            on_reader_route() || !has_session_cookie(req.headers())
        }
        TokenCredential::AppPasswordBasic => {
            if on_reader_route() {
                !looks_like_browser_cross_site(req.headers())
            } else {
                !has_session_cookie(req.headers())
            }
        }
    }
}

/// True when the request carries a cookie that could authenticate it: the
/// session cookie or a refresh cookie. Without one there is nothing ambient
/// for a cross-site page to ride on.
fn has_session_cookie(h: &HeaderMap) -> bool {
    let jar = CookieJar::from_headers(h);
    [SESSION_COOKIE, REFRESH_COOKIE, LEGACY_REFRESH_COOKIE]
        .iter()
        .any(|name| jar.get(name).is_some_and(|c| !c.value().is_empty()))
}

/// True when the headers show a browser page made this request on its own
/// behalf rather than a native reader app: any `Origin` header (browsers
/// send it on every non-GET fetch / form post; native HTTP clients don't),
/// or a `Sec-Fetch-Site` naming another site.
fn looks_like_browser_cross_site(h: &HeaderMap) -> bool {
    if h.contains_key(ORIGIN) {
        return true;
    }
    h.get(SEC_FETCH_SITE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.eq_ignore_ascii_case("cross-site") || v.eq_ignore_ascii_case("same-site")
        })
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

/// The CSRF-exempt POSTs that still refuse a cross-site browser request
/// (module docs). Every entry is also in [`CSRF_EXEMPT_PATHS`].
pub const CROSS_SITE_GUARDED_PATHS: &[&str] = &[
    "/auth/local/login",
    "/auth/local/register",
    "/auth/local/request-password-reset",
    "/auth/local/reset-password",
    "/auth/local/resend-verification",
];

/// The refusal for a cross-site sign-in POST. A form navigation (a form
/// content type, or `Sec-Fetch-Mode: navigate`) gets a 303 back to the page
/// the form lives on with `error=auth.csrf`, which those pages show as a
/// banner; any other caller gets the 403 `auth.csrf` envelope. Either way
/// the answer is the same whichever check failed (the reason is logged).
async fn cross_site_refusal(req: Request, reason: &'static str) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let content_type = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let urlencoded = content_type.starts_with("application/x-www-form-urlencoded");
    let is_form = urlencoded
        || content_type.starts_with("multipart/form-data")
        || req
            .headers()
            .get("sec-fetch-mode")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|m| m.eq_ignore_ascii_case("navigate"));
    if !is_form {
        return csrf_error(&method, &path, reason);
    }
    tracing::info!(method = %method, path = %path, reason = %reason, "csrf rejection");
    const CODE: &str = "auth.csrf";
    const MESSAGE: &str = "request could not be verified";
    let target = match path.as_str() {
        "/auth/local/request-password-reset" => {
            crate::api::form_or_json::redirect_with_error("/forgot-password", CODE, MESSAGE, None)
        }
        // The reset page needs its token back; it travels in the form body.
        "/auth/local/reset-password" => {
            let token = if urlencoded {
                to_bytes(req.into_body(), MAX_FORM_BODY_FOR_CSRF)
                    .await
                    .ok()
                    .and_then(|b| {
                        serde_urlencoded::from_bytes::<Vec<(String, String)>>(&b)
                            .ok()?
                            .into_iter()
                            .find_map(|(k, v)| (k == "token").then_some(v))
                    })
            } else {
                None
            };
            match token {
                Some(t) => format!(
                    "/reset-password?token={}&error={CODE}&message={}",
                    urlencoding::encode(&t),
                    urlencoding::encode(MESSAGE)
                ),
                None => crate::api::form_or_json::redirect_with_error(
                    "/forgot-password",
                    CODE,
                    MESSAGE,
                    None,
                ),
            }
        }
        _ => crate::api::form_or_json::redirect_with_error("/sign-in", CODE, MESSAGE, None),
    };
    axum::response::Redirect::to(&target).into_response()
}

fn path_is_exempt(path: &str) -> bool {
    CSRF_EXEMPT_PATHS.contains(&path)
}

fn csrf_error(method: &Method, path: &str, reason: &'static str) -> Response {
    // M7 (v0.3.41): log every rejection at info so /admin/logs makes
    // CSRF failures visible. Without this, a Panels-style PATCH with a
    // non-app-password `Authorization: Basic …` header was indistinguishable
    // from no request at all when investigating the Komga compat
    // writeback failure.
    tracing::info!(
        method = %method,
        path = %path,
        reason = %reason,
        "csrf rejection",
    );
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
