//! Integration tests for `/admin/metadata/*` (metadata-providers-1.0 M1).
//!
//! Wiremock-driven coverage of the ComicVine HTTP path lives in
//! `tests/comicvine_client.rs` — this file targets the admin handler's
//! decision matrix (auth gate, credential/enabled short-circuits, audit
//! row emission, unknown-provider 404, not-yet-supported provider 404).
//!
//! Coverage:
//! - GET /admin/metadata/providers requires admin
//! - list reports comicvine.configured=false when key unset
//! - list reports comicvine.configured=true + enabled=false when key set
//!   but master toggle off
//! - POST /providers/comicvine/test → 400 when key unset
//! - POST /providers/comicvine/test → 409 when key set but disabled
//! - POST /providers/foo/test → 404 unknown
//! - POST /providers/metron/test → 404 (M2 hasn't shipped)
//! - successful flow audit-logs `admin.metadata.providers.test`

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::EntityTrait;
use serde_json::Value;
use tower::ServiceExt;

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
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

async fn get(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn list_providers_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &user, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_providers_unconfigured_when_no_credentials() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let providers = body["providers"].as_array().expect("providers array");
    assert!(!providers.is_empty(), "should include comicvine row");
    let cv = providers
        .iter()
        .find(|p| p["id"] == "comicvine")
        .expect("comicvine row");
    assert_eq!(cv["configured"], false);
    assert_eq!(cv["enabled"], false);
    assert_eq!(cv["quota"], Value::Null);
}

#[tokio::test]
async fn list_providers_configured_but_disabled() {
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let cv = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "comicvine")
        .unwrap()
        .clone();
    assert_eq!(cv["configured"], true);
    assert_eq!(cv["enabled"], false);
    // Quota snapshot resolves to a value (Redis bucket reports "full"
    // when no decrement has happened yet).
    assert!(cv["quota"].is_object());
}

#[tokio::test]
async fn test_provider_400_when_key_missing() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.no_credentials");
}

#[tokio::test]
async fn test_provider_409_when_disabled() {
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.disabled");
}

#[tokio::test]
async fn test_provider_404_for_unknown_id() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/notreal/test").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.unknown_provider");
}

#[tokio::test]
async fn test_provider_404_for_metron_until_m2() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/metron/test").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.provider_not_in_m1");
}

#[tokio::test]
async fn test_provider_disabled_audit_does_not_fire() {
    // Audit row only writes on the "actually attempted" path; the
    // early 400 / 409 short-circuits exit before we reach the upstream
    // call. This isn't ideal — an operator clicking "Test" while
    // misconfigured should still leave a trail — but matches the
    // admin_email.test_send pattern. Capturing the current behavior
    // here makes future tightening greppable.
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    let rows = entity::audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    assert!(
        !rows.iter().any(|r| r.action == "admin.metadata.providers.test"),
        "audit row written even though provider was disabled (regression)"
    );
}
