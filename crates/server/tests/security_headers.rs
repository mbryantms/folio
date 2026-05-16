//! Security-headers integration test (§17.4).
//!
//! Asserts every response carries the full set of security headers, even on
//! `/healthz` and `/csp-report`.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::TestApp;
use tower::ServiceExt;

const REQUIRED_HEADERS: &[&str] = &[
    "content-security-policy",
    "strict-transport-security",
    "x-content-type-options",
    "referrer-policy",
    "cross-origin-opener-policy",
    "cross-origin-embedder-policy",
    "cross-origin-resource-policy",
    "permissions-policy",
    "x-frame-options",
];

#[tokio::test]
async fn healthz_carries_all_security_headers() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    for h in REQUIRED_HEADERS {
        assert!(resp.headers().get(*h).is_some(), "missing header: {h}");
    }

    let csp = resp
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("base-uri 'none'"));
    assert!(csp.contains("object-src 'none'"));
    // `require-trusted-types-for 'script'` is dropped pending the
    // per-request nonce work: Next's hydration runtime + Cloudflare's
    // email-decode injection both patch innerHTML paths that violate
    // Trusted-Types enforcement before app code runs.
    assert!(!csp.contains("require-trusted-types-for"));
    // `'strict-dynamic'` only works alongside a per-request nonce. We
    // dropped it in M3 (audit S-8) and haven't re-introduced it yet.
    assert!(
        !csp.contains("'strict-dynamic'"),
        "CSP must not contain 'strict-dynamic' without nonce wiring: {csp}"
    );
    // `'unsafe-inline'` is required on both directives until nonces
    // land: Next's hydration scripts + framework-sprinkled style
    // attributes are neither hashable nor nonceable today. Dev also
    // adds `'unsafe-eval'` to script-src for React Refresh.
    assert!(csp.contains("'unsafe-inline'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
}

#[tokio::test]
async fn csp_report_endpoint_accepts_violation() {
    let app = TestApp::spawn().await;
    // Legacy form: { "csp-report": { ... } }
    let body = serde_json::json!({
        "csp-report": {
            "document-uri": "https://comics.example.com/",
            "violated-directive": "script-src",
            "blocked-uri": "https://evil.example.com/x.js"
        }
    });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/csp-report")
                .header("content-type", "application/csp-report")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // Headers still present on 204.
    assert!(resp.headers().get("content-security-policy").is_some());
}

#[tokio::test]
async fn healthz_returns_required_headers() {
    // Post v0.2 (rust-public-origin) `/` no longer has a Rust handler
    // — it falls through to the SSR proxy. `/healthz` is the simplest
    // remaining Rust-owned route to assert the security-headers
    // middleware wraps; the middleware is the same one wrapping the
    // fallback, so this is sufficient coverage for the layer itself.
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    for h in REQUIRED_HEADERS {
        assert!(resp.headers().get(*h).is_some(), "missing on /healthz: {h}");
    }
}
