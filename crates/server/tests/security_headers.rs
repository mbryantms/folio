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
    assert!(csp.contains("require-trusted-types-for 'script'"));
    assert!(csp.contains("base-uri 'none'"));
    assert!(csp.contains("object-src 'none'"));
    // M3 (audit S-8): `'strict-dynamic'` without a per-request nonce is
    // either a no-op or actively disables script-src on modern browsers.
    // We dropped it in favor of strict `'self'`. Guard against regression.
    assert!(
        !csp.contains("'strict-dynamic'"),
        "CSP must not contain 'strict-dynamic' without nonce wiring: {csp}"
    );
    assert!(csp.contains("script-src 'self'"));
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
async fn root_returns_hello_with_headers() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    for h in REQUIRED_HEADERS {
        assert!(resp.headers().get(*h).is_some(), "missing on /: {h}");
    }
}
