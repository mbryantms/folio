//! Integration coverage for the upstream-release lookup
//! (server-info-github-link 1.0 M4).
//!
//! Two test layers:
//!
//!  1. **`fetch_release` against wiremock** — exercises the HTTP +
//!     JSON-parsing path with a controllable upstream so we can hit
//!     success, 404, and malformed-body cases deterministically. The
//!     production code points at `https://api.github.com/...`; the
//!     wiremock test points at the mock's URL. The contract under
//!     test is the parse + error-handling logic, not the URL choice.
//!
//!  2. **`/admin/server/latest-release` via `TestApp`** — exercises
//!     the runtime toggle (`updates.check_upstream_releases = false`
//!     ⇒ 204) without making any outbound HTTP call.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::json;
use server::api::server_releases::fetch_release;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

// ──────────────────────────────────────────────────────────────────
// Layer 1: fetch_release against wiremock
// ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fetch_release_parses_a_well_formed_github_payload() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tag_name": "v0.1.9",
            "html_url": "https://github.com/owner/repo/releases/tag/v0.1.9",
            "published_at": "2026-05-17T10:00:00Z",
            // GitHub adds many other fields; serde ignores them.
            "name": "v0.1.9",
            "body": "Release notes…",
            "draft": false,
            "prerelease": false,
        })))
        .mount(&mock)
        .await;

    let url = format!("{}/repos/owner/repo/releases/latest", mock.uri());
    let parsed = fetch_release(&url).await.expect("fetch should succeed");
    assert_eq!(parsed.tag, "v0.1.9");
    assert_eq!(
        parsed.html_url,
        "https://github.com/owner/repo/releases/tag/v0.1.9"
    );
    assert_eq!(parsed.published_at, "2026-05-17T10:00:00Z");
}

#[tokio::test]
async fn fetch_release_handles_404_gracefully() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/releases/latest"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;

    let url = format!("{}/repos/owner/repo/releases/latest", mock.uri());
    let parsed = fetch_release(&url).await;
    assert!(
        parsed.is_none(),
        "404 should map to None, not panic or partial body"
    );
}

#[tokio::test]
async fn fetch_release_handles_malformed_body_gracefully() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/releases/latest"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("this is not JSON", "application/json"),
        )
        .mount(&mock)
        .await;

    let url = format!("{}/repos/owner/repo/releases/latest", mock.uri());
    let parsed = fetch_release(&url).await;
    assert!(parsed.is_none(), "malformed JSON should map to None");
}

#[tokio::test]
async fn fetch_release_handles_missing_required_field() {
    // GitHub-shaped envelope but missing `tag_name` — serde rejects
    // and the handler swallows.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/releases/latest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "html_url": "https://github.com/owner/repo/releases/tag/v0.1.9",
            "published_at": "2026-05-17T10:00:00Z",
        })))
        .mount(&mock)
        .await;

    let url = format!("{}/repos/owner/repo/releases/latest", mock.uri());
    let parsed = fetch_release(&url).await;
    assert!(
        parsed.is_none(),
        "missing tag_name should map to None, not crash"
    );
}

// ──────────────────────────────────────────────────────────────────
// Layer 2: runtime toggle via TestApp
// ──────────────────────────────────────────────────────────────────

/// Auth + headers boilerplate. The endpoint is admin-only — first
/// registered user becomes admin.
struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn latest_release_endpoint_is_admin_only() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = http(
        &app,
        Method::GET,
        "/api/admin/server/latest-release",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn latest_release_returns_204_when_setting_is_disabled() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    // Flip the runtime setting off via the admin settings PATCH.
    // The endpoint uses `#[serde(flatten)]` so values land at the
    // body root, not nested under `values`.
    let (s, _) = http(
        &app,
        Method::PATCH,
        "/api/admin/settings",
        Some(&admin),
        Some(json!({
            "updates.check_upstream_releases": false
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = http(
        &app,
        Method::GET,
        "/api/admin/server/latest-release",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NO_CONTENT,
        "endpoint must short-circuit before any HTTP fetch when disabled"
    );
}
