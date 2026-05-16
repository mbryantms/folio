//! Security-headers middleware (§17.4).
//!
//! Sets CSP + companion headers on every response. As of v0.2 (rust-
//! public-origin), the Rust binary is the public origin for both API
//! and HTML responses — Next.js is an internal SSR upstream behind
//! the `upstream::proxy` fallback.
//!
//! CSP construction is per-request: a [`CspTemplate`] holds the
//! request-invariant bits (connect-src, frame-ancestors, etc.) and
//! [`build_csp`] slots in the per-request nonce from the [`Nonce`]
//! request extension. When the nonce is present (every request that
//! came through the [`nonce::set_nonce`](super::nonce::set_nonce)
//! middleware), `script-src` and `style-src` emit
//! `'self' 'nonce-XXX' 'strict-dynamic'`, which restores defence-in-
//! depth while still allowing the inline `<script>` tags Next.js
//! emits during SSR hydration (Next reads the same nonce from the
//! `x-csp-nonce` proxy header — see `web/proxy.ts`).
//!
//! Fallback when no nonce is present (e.g. a unit test that calls
//! `set_headers` directly without wiring `set_nonce`): emit the v0.2.1
//! relaxed policy with `'unsafe-inline'`. Dev builds keep
//! `'unsafe-eval'` for React Refresh source maps.

use crate::config::Config;
use crate::middleware::nonce::Nonce;
use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Request-invariant CSP scaffolding. Built once at boot from
/// [`Config`]; combined with the per-request nonce in [`build_csp`].
#[derive(Clone)]
pub struct CspTemplate {
    connect_src: String,
}

impl CspTemplate {
    pub fn new(cfg: &Config) -> Self {
        let oidc_origin = cfg
            .oidc_issuer
            .as_deref()
            .and_then(|s| url::Url::parse(s).ok())
            .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))
            .unwrap_or_default();
        let public_host = cfg
            .public_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        // WS scheme has to match the page scheme — `'self'` does not
        // cover the ws/wss-shaped origin in CSP. In dev that means
        // `ws://localhost:8080`; in prod `wss://comics.example.com`.
        let ws_scheme = if cfg.public_url.starts_with("https://") {
            "wss"
        } else {
            "ws"
        };
        let connect_src = if oidc_origin.is_empty() {
            format!("'self' {ws_scheme}://{public_host}")
        } else {
            format!("'self' {oidc_origin} {ws_scheme}://{public_host}")
        };
        Self { connect_src }
    }
}

/// Render the CSP header value for a single request. If `nonce` is
/// `Some`, the policy uses `'nonce-XXX' 'strict-dynamic'`; otherwise
/// it falls back to the v0.2.1 `'unsafe-inline'` policy so requests
/// that bypass `set_nonce` still get a coherent header.
pub fn build_csp(template: &CspTemplate, nonce: Option<&str>) -> HeaderValue {
    // Dev (`cargo run` + `next dev`) needs `'unsafe-eval'` for React
    // Refresh source maps. Release builds run the standalone bundle
    // and never need it.
    let unsafe_eval = if cfg!(debug_assertions) {
        " 'unsafe-eval'"
    } else {
        ""
    };
    let script_src = match nonce {
        Some(n) => {
            // `'strict-dynamic'` makes scripts loaded by the nonced
            // bootstrap inherit trust without each needing `'self'` or
            // a nonce of their own — the canonical CSP3 posture. In
            // dev we still bolt on `'unsafe-eval'` for React Refresh.
            format!("'self' 'nonce-{n}' 'strict-dynamic'{unsafe_eval}")
        }
        // No nonce wired (test-only path or any future caller that
        // bypasses `set_nonce`). Match v0.2.1 hotfix posture.
        None => format!("'self' 'unsafe-inline'{unsafe_eval}"),
    };
    // Style-src stays `'unsafe-inline'` regardless of nonce: Next.js
    // does not currently propagate nonces to `<style>` tags, and most
    // of the style violations come from inline `style="…"` attributes
    // which can't carry a nonce at all (CSP nonces are for elements,
    // not attributes). The script-src nonce is what restores defence-
    // in-depth against XSS, which is the dominant threat anyway.
    let style_src = "'self' 'unsafe-inline'";
    // Trusted Types: only enforce in release builds + only when the
    // nonce is wired. Dev `next dev` violates Trusted Types via React
    // Refresh's Function-constructor patches; an unnonced caller
    // (test-only) won't have nonced its inline scripts either, so the
    // policy would block them. Cloudflare's "Email Address
    // Obfuscation" must also be off — its `cdn-cgi/scripts/email-
    // decode.min.js` writes through innerHTML and trips this. See
    // docs/install/cloudflare.md.
    let trusted_types = if cfg!(debug_assertions) || nonce.is_none() {
        ""
    } else {
        "require-trusted-types-for 'script'; "
    };
    let csp = format!(
        "default-src 'self'; \
         script-src {script_src}; \
         style-src {style_src}; \
         img-src 'self' data: blob:; \
         font-src 'self'; \
         connect-src {connect_src}; \
         frame-ancestors 'none'; \
         form-action 'self'; \
         base-uri 'none'; \
         object-src 'none'; \
         worker-src 'self' blob:; \
         manifest-src 'self'; \
         {trusted_types}upgrade-insecure-requests; \
         report-to comic-csp",
        connect_src = template.connect_src,
    );
    // `from_maybe_shared` accepts a `String` without re-allocating.
    HeaderValue::from_str(&csp).expect("valid csp")
}

/// Page paths (locale-aware) that must use a stricter Referrer-Policy: never
/// emit a `Referer` header on outbound navigations. Credentials accidentally
/// landed in the URL must not leak to the next site the user visits.
///
/// Match is a suffix check on the path (so `/en/sign-in` and `/sign-in` both
/// hit). next-intl drops the prefix when only one locale is active, so both
/// shapes occur in practice.
fn path_needs_no_referrer(path: &str) -> bool {
    const SUFFIXES: &[&str] = &["/sign-in", "/forgot-password", "/reset-password"];
    SUFFIXES.iter().any(|s| path == *s || path.ends_with(s))
}

/// CSP header value, stashed on the request extensions so inner
/// consumers (notably [`crate::upstream::proxy`]) can forward the
/// same value to Next.js. Next's app-render reads it via
/// `headers['content-security-policy']` and extracts the per-request
/// `'nonce-XXX'` substring to stamp onto its own framework-emitted
/// `<script>` tags — see
/// `node_modules/next/dist/server/app-render/get-script-nonce-from-header.js`.
#[derive(Clone)]
pub struct CspHeader(pub HeaderValue);

pub async fn set_headers(
    State(template): State<Arc<CspTemplate>>,
    mut req: Request,
    next: Next,
) -> Response {
    let needs_no_referrer = path_needs_no_referrer(req.uri().path());
    let nonce = req.extensions().get::<Nonce>().map(|n| n.0.clone());
    let csp = build_csp(&template, nonce.as_deref());

    // Stash the built CSP on the request so the proxy fallback can
    // forward it as a request header to Next.js. Build it once here so
    // request + response always carry the same nonce.
    req.extensions_mut().insert(CspHeader(csp.clone()));

    let mut resp = next.run(req).await;
    let h = resp.headers_mut();

    macro_rules! set {
        ($name:expr, $val:expr) => {
            h.insert(
                HeaderName::from_static($name),
                HeaderValue::from_static($val),
            );
        };
    }
    h.insert(HeaderName::from_static("content-security-policy"), csp);
    set!(
        "strict-transport-security",
        "max-age=63072000; includeSubDomains"
    );
    set!("x-content-type-options", "nosniff");
    if needs_no_referrer {
        // Auth pages: zero referrer leakage. If the user's URL ever
        // contained credentials (today's progressive-enhancement guard
        // makes this impossible for new submits, but old browser
        // history entries may still hold one), the next page they
        // navigate to won't receive it.
        set!("referrer-policy", "no-referrer");
    } else {
        set!("referrer-policy", "strict-origin-when-cross-origin");
    }
    set!("cross-origin-opener-policy", "same-origin");
    set!("cross-origin-embedder-policy", "credentialless");
    set!("cross-origin-resource-policy", "same-origin");
    set!(
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), usb=(), bluetooth=(), payment=()"
    );
    set!("x-frame-options", "DENY");

    resp
}
