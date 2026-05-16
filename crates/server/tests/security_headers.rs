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
    "reporting-endpoints",
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
    assert!(csp.contains("frame-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("base-uri 'none'"));
    assert!(csp.contains("object-src 'none'"));
    // Both reporting mechanisms wired: legacy `report-uri` for older
    // browsers and modern `report-to comic-csp` + `Reporting-Endpoints`
    // for current Chromium / Edge. Verifying both prevents an accidental
    // drop of either path.
    assert!(csp.contains("report-uri /csp-report"));
    assert!(csp.contains("report-to comic-csp"));
    let reporting = resp
        .headers()
        .get("reporting-endpoints")
        .expect("reporting-endpoints header")
        .to_str()
        .unwrap();
    assert!(
        reporting.contains("comic-csp=") && reporting.contains("/csp-report"),
        "reporting-endpoints malformed: {reporting}"
    );
    // With the nonce middleware wired, every response carries a
    // per-request `'nonce-XXX'` plus `'strict-dynamic'`. `'unsafe-
    // inline'` falls away because nonced + strict-dynamic supersedes it
    // (modern browsers ignore `'unsafe-inline'` once strict-dynamic is
    // present, anyway).
    assert!(
        csp.contains("'strict-dynamic'"),
        "CSP missing 'strict-dynamic': {csp}"
    );
    let nonce_idx = csp
        .find("'nonce-")
        .unwrap_or_else(|| panic!("CSP missing per-request nonce: {csp}"));
    // Nonce shape: 22 base64url chars between `'nonce-` and the closing
    // `'`. Just spot-check the delimiter; full alphabet check lives in
    // the middleware::nonce unit tests.
    let after = &csp[nonce_idx + "'nonce-".len()..];
    let close = after.find('\'').expect("nonce closing quote");
    assert_eq!(close, 22, "nonce length {close} != 22 in {csp}");
    // script-src no longer needs `'unsafe-inline'` (nonce supersedes
    // it). style-src does — Next.js doesn't propagate nonces to
    // `<style>` tags and CSS attribute selectors can't carry a nonce.
    assert!(
        csp.contains("style-src 'self' 'unsafe-inline'"),
        "style-src must keep 'unsafe-inline': {csp}"
    );
    // `require-trusted-types-for 'script'` is still off — see M6 of the
    // csp-nonce plan. Re-add this assertion when M6 lands.
    assert!(!csp.contains("require-trusted-types-for"));
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
