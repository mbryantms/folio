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
            // a nonce of their own — the canonical CSP3 posture.
            // `'self'` is omitted on purpose: browsers ignore it once
            // `'strict-dynamic'` is present and emit a console warning
            // if it's listed. In dev we still bolt on `'unsafe-eval'`
            // for React Refresh.
            //
            // `'sha256-CZ6sxw5J…'` whitelists one specific inline
            // script Next.js emits without a nonce (observed in prod;
            // content is stable across requests). Hash-based, so if
            // Next ever changes that script content the entry stops
            // matching and the block re-surfaces — visible regression
            // rather than silent accumulation. The bulk of inline
            // scripts (flight payload pushes etc.) get nonces via
            // `getScriptNonceFromHeader` parsing our request CSP.
            format!(
                "'nonce-{n}' 'strict-dynamic' 'sha256-CZ6sxw5J9fnBmFLteUj3ajNhEVqMElLD+l/3BBNAhys='{unsafe_eval}"
            )
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
    // Trusted Types stays off. The React 19 / Next 16 runtime emits
    // chunks that call `Element.innerHTML = …` directly (observed in
    // prod via the v0.3.2 console: "Sink type mismatch violation
    // blocked by CSP" from `_next/static/chunks/...`). Until React
    // ships first-class Trusted Types support, enforcing this
    // directive breaks hydration entirely — page goes blank. Tracked
    // for a future re-enable once upstream is ready; for now the
    // script-src nonce + `'strict-dynamic'` is the meaningful XSS
    // defence.
    let trusted_types = "";
    // Notes on directives we *don't* set explicitly:
    //   - `script-src-elem` / `script-src-attr` fall back to `script-src`
    //     above. Folio renders zero inline event handlers (no `onclick=`)
    //     so a tighter `-attr 'none'` is feasible — defer until we wire
    //     a regression check that catches accidental introductions.
    //   - `style-src-elem` / `style-src-attr` could split nonce-able
    //     `<style>` tags from un-nonceable `style="…"` attributes.
    //     Verified-Next-noncing-every-style is the gate; defer.
    //   - `prefetch-src` and `navigate-to` are removed from CSP 3 and
    //     never had universal browser support — don't add.
    //
    // `report-uri` is the legacy mechanism (Firefox, Safari, older
    // Chromiums); `report-to` plus the matching `Reporting-Endpoints`
    // response header is the modern one (Chrome 96+, Edge). We ship
    // both so violations are visible everywhere.
    let csp = format!(
        "default-src 'self'; \
         script-src {script_src}; \
         style-src {style_src}; \
         img-src 'self' data: blob:; \
         font-src 'self'; \
         connect-src {connect_src}; \
         frame-src 'none'; \
         frame-ancestors 'none'; \
         form-action 'self'; \
         base-uri 'none'; \
         object-src 'none'; \
         worker-src 'self' blob:; \
         manifest-src 'self'; \
         {trusted_types}upgrade-insecure-requests; \
         report-uri /csp-report; \
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
    // Wire the `report-to comic-csp` group named in the CSP above to
    // the actual delivery endpoint. Without this header modern Chrome /
    // Edge browsers drop reports on the floor. The bare relative path
    // is fine — same-origin reporting is supported in CSP 3.
    set!("reporting-endpoints", "comic-csp=\"/csp-report\"");
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
