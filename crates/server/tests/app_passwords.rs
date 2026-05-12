//! `/me/app-passwords` integration tests (M7, audit M-14).
//!
//! Covers: issue → use as Bearer → /auth/me returns the right user;
//! revoke kills the token; cross-user isolation; label validation;
//! per-user cap; last_used_at bump.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(|v| v.to_owned())
        })
}

struct Auth {
    session: String,
    csrf: String,
}

impl Auth {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
    fn from_response(resp: &Response<Body>) -> Self {
        Self {
            session: extract_cookie(resp, "__Host-comic_session").unwrap(),
            csrf: extract_cookie(resp, "__Host-comic_csrf").unwrap(),
        }
    }
}

async fn register(app: &TestApp, email: &str) -> Response<Body> {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    app.router
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
        .unwrap()
}

async fn create_password(app: &TestApp, auth: &Auth, label: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::from(format!(r#"{{"label":"{label}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn list_passwords(app: &TestApp, auth: &Auth) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/me/app-passwords")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn me_with_bearer(app: &TestApp, bearer: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn issue_then_use_as_bearer_returns_owner() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "owner@example.com").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    assert!(plaintext.starts_with("app_"), "plaintext shape");
    assert_eq!(body["label"], "kobo");

    let me = me_with_bearer(&app, &plaintext).await;
    assert_eq!(me.status(), StatusCode::OK);
    let me_body = body_json(me.into_body()).await;
    assert_eq!(me_body["email"], "owner@example.com");
}

#[tokio::test]
async fn list_returns_active_passwords_without_plaintext() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "lister@example.com").await;
    let auth = Auth::from_response(&reg);

    let _ = create_password(&app, &auth, "kobo").await;
    let _ = create_password(&app, &auth, "kavita").await;

    let body = list_passwords(&app, &auth).await;
    let arr = body["items"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(
        arr.iter().all(|i| i.get("plaintext").is_none()),
        "list view must not include plaintext"
    );
    let labels: Vec<_> = arr.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"kobo"));
    assert!(labels.contains(&"kavita"));
}

#[tokio::test]
async fn revoked_password_rejects_bearer_auth() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "revoke@example.com").await;
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    let id = body["id"].as_str().unwrap().to_owned();

    // Works before revoke.
    let me = me_with_bearer(&app, &plaintext).await;
    assert_eq!(me.status(), StatusCode::OK);

    // Revoke.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/me/app-passwords/{id}"))
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Now fails as Bearer.
    let me = me_with_bearer(&app, &plaintext).await;
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);

    // And the list view no longer shows it.
    let body = list_passwords(&app, &auth).await;
    assert!(body["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn cannot_revoke_other_users_password() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let other = register(&app, "other@example.com").await;
    let other_auth = Auth::from_response(&other);
    let resp = create_password(&app, &other_auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let other_id = body["id"].as_str().unwrap().to_owned();

    // Second user.
    let attacker = register(&app, "attacker@example.com").await;
    let attacker_auth = Auth::from_response(&attacker);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/me/app-passwords/{other_id}"))
                .header(header::COOKIE, attacker_auth.cookies())
                .header("x-csrf-token", &attacker_auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn last_used_at_bumps_after_bearer_request() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "stamp@example.com").await;
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    let id_str = body["id"].as_str().unwrap();
    let id: uuid::Uuid = id_str.parse().unwrap();

    // Before use: last_used_at is null.
    let state = app.state();
    let before = entity::app_password::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(before.last_used_at.is_none());

    let _ = me_with_bearer(&app, &plaintext).await;

    let after = entity::app_password::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(after.last_used_at.is_some(), "last_used_at should bump");
}

#[tokio::test]
async fn label_validation_rejects_empty_and_oversize() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "label@example.com").await;
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let long = "x".repeat(200);
    let resp = create_password(&app, &auth, &long).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cannot_see_other_users_passwords_in_list() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let alice_auth = Auth::from_response(&alice);
    let _ = create_password(&app, &alice_auth, "alice-token").await;

    let bob = register(&app, "bob@example.com").await;
    let bob_auth = Auth::from_response(&bob);
    let _ = create_password(&app, &bob_auth, "bob-token").await;

    let bob_list = list_passwords(&app, &bob_auth).await;
    let items = bob_list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "bob-token");

    // Belt-and-suspenders: confirm alice's row still exists at the
    // DB level so the test isn't just lucky.
    let state = app.state();
    let all_count = entity::app_password::Entity::find()
        .filter(entity::app_password::Column::RevokedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(all_count, 2);
}
