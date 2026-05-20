//! SSRF guard for `POST /me/cbl-lists` (auth-hardening Phase B B1).
//!
//! Closes [security-audit.md H-1](../../../docs/dev/security-audit.md):
//! authenticated callers must not be able to make the server fetch
//! loopback / RFC-1918 / cloud-metadata addresses. The guard rejects
//! before the outbound `reqwest` call so we can assert behaviour without
//! standing up an actual HTTP target.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
    #[allow(dead_code)]
    user_id: Uuid,
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
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn post_create_from_url(
    app: &TestApp,
    auth: &Authed,
    url: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "kind": "url", "url": url }).to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/cbl-lists")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_loopback_url() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-loopback@example.com").await;
    let (status, body) = post_create_from_url(&app, &auth, "https://127.0.0.1/list.cbl").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["error"]["code"], "invalid_url");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_aws_metadata_url() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-metadata@example.com").await;
    // 169.254.169.254 — AWS / GCP / Azure / OpenStack instance metadata.
    // Single most-cited SSRF target.
    let (status, body) = post_create_from_url(
        &app,
        &auth,
        "https://169.254.169.254/latest/meta-data/iam/security-credentials/",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["error"]["code"], "invalid_url");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_rfc1918_private_ranges() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-private@example.com").await;
    for raw in [
        "https://10.0.0.1/list.cbl",
        "https://172.16.5.5/list.cbl",
        "https://192.168.1.1/list.cbl",
        "https://[::1]/list.cbl",
        "https://[fc00::1]/list.cbl",
    ] {
        let (status, body) = post_create_from_url(&app, &auth, raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw} body={body}");
        assert_eq!(body["error"]["code"], "invalid_url", "{raw}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_non_https_schemes() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-scheme@example.com").await;
    for raw in [
        "http://example.com/list.cbl",
        "ftp://example.com/list.cbl",
        "file:///etc/passwd",
    ] {
        let (status, body) = post_create_from_url(&app, &auth, raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw} body={body}");
        assert_eq!(body["error"]["code"], "invalid_url", "{raw}");
    }
}
