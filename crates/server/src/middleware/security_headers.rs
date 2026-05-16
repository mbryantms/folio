//! Security-headers middleware (§17.4).
//!
//! Sets CSP + companion headers on every response. As of v0.2 (rust-
//! public-origin), the Rust binary is the public origin for both API
//! and HTML responses — Next.js is an internal SSR upstream behind
//! the `upstream::proxy` fallback. The CSP below is built for prod
//! HTML (Next 16 emits hashed external script tags, no inline scripts)
//! and dev `next dev` (which uses `eval()` for source maps + inline
//! scripts for HMR — relaxed via `debug_assertions` below).

use crate::config::Config;
use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SecurityHeaders {
    csp: HeaderValue,
}

impl SecurityHeaders {
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
        // Script + style source lists. Dev (`cargo run` + `next dev`)
        // needs `'unsafe-eval'` for React Refresh source maps; both dev
        // and release need `'unsafe-inline'` because Next emits inline
        // hydration `<script>` tags and framework-sprinkled
        // `style="…"` attributes in the SSR HTML, neither of which our
        // proxy can hash or nonce yet. The proper fix is a per-request
        // nonce threaded from Rust through the SSR proxy to Next so the
        // policy can return to `'self' 'nonce-…' 'strict-dynamic'`;
        // tracked separately.
        //
        // `require-trusted-types-for 'script'` is similarly dropped:
        // Next's hydration runtime + Cloudflare email-decode injection
        // patch `innerHTML` paths that violate Trusted-Types enforcement
        // before any app code runs. Re-enable alongside the nonce work.
        let script_src = if cfg!(debug_assertions) {
            "'self' 'unsafe-eval' 'unsafe-inline'"
        } else {
            "'self' 'unsafe-inline'"
        };
        let style_src = "'self' 'unsafe-inline'";
        let trusted_types = "";
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
             report-to comic-csp"
        );
        Self {
            csp: HeaderValue::from_str(&csp).expect("valid csp"),
        }
    }
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

pub async fn set_headers(
    State(state): State<Arc<SecurityHeaders>>,
    req: Request,
    next: Next,
) -> Response {
    let needs_no_referrer = path_needs_no_referrer(req.uri().path());
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
    h.insert(
        HeaderName::from_static("content-security-policy"),
        state.csp.clone(),
    );
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
