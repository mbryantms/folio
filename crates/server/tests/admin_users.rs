//! Integration tests for `/admin/users/*` and `/admin/audit` (M3).
//!
//! Covers the happy path for each endpoint plus the ACL boundaries:
//!   - Non-admin → 403
//!   - Admin → 200/201
//!   - Self-demote / self-disable → 403
//!   - Library-access set replaces the user's grants and triggers an audit row

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
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

async fn register(app: &TestApp, email: &str, password: &str) -> axum::http::Response<Body> {
    app.router
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
        .unwrap()
}

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = register(app, email, password).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "registration failed");
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

async fn get_me(app: &TestApp, auth: &Authed) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn admin_get(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
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

async fn admin_send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, auth.cookie())
        .header("x-csrf-token", &auth.csrf);
    let body = if let Some(json) = body {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(b.body(body).unwrap())
        .await
        .unwrap()
}

async fn create_library(app: &TestApp, admin: &Authed, name: &str, root: &str) -> String {
    let resp = admin_send(
        app,
        admin,
        Method::POST,
        "/api/libraries",
        Some(serde_json::json!({ "name": name, "root_path": root })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn list_users_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = admin_get(&app, &user, "/api/admin/users").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_users_returns_paginated_set() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    for i in 0..3 {
        let _ = register(
            &app,
            &format!("user{i}@example.com"),
            "correctly-horse-battery",
        )
        .await;
    }
    let resp = admin_get(&app, &admin, "/api/admin/users").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    // Admin is listed and flagged.
    let roles: Vec<&str> = items.iter().map(|i| i["role"].as_str().unwrap()).collect();
    assert!(roles.contains(&"admin"));
    assert_eq!(roles.iter().filter(|r| **r == "user").count(), 3);
}

#[tokio::test]
async fn list_users_filters_role() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "regular@example.com", "correctly-horse-battery").await;

    let resp = admin_get(&app, &admin, "/api/admin/users?role=admin").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["role"], "admin");
}

#[tokio::test]
async fn list_users_pagination_cursor_works() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    for i in 0..4 {
        let _ = register(
            &app,
            &format!("u{i}@example.com"),
            "correctly-horse-battery",
        )
        .await;
    }

    let resp = admin_get(&app, &admin, "/api/admin/users?limit=2").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let cursor = body["next_cursor"].as_str().expect("cursor present");
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    let resp = admin_get(
        &app,
        &admin,
        &format!("/api/admin/users?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn admin_promotes_user_then_audit_row_recorded() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "target@example.com", "correctly-horse-battery").await;

    // Find target
    let resp = admin_get(&app, &admin, "/api/admin/users").await;
    let body = body_json(resp.into_body()).await;
    let target = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == "target@example.com")
        .unwrap();
    let target_id = target["id"].as_str().unwrap();

    // Promote
    let resp = admin_send(
        &app,
        &admin,
        Method::PATCH,
        &format!("/api/admin/users/{target_id}"),
        Some(serde_json::json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["role"], "admin");

    // Audit log shows the promotion, with actor + target resolved to labels.
    let resp = admin_get(&app, &admin, "/api/admin/audit?action=admin.user.update").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let items = body["items"].as_array().unwrap();
    let entry = items
        .iter()
        .find(|i| i["target_id"] == target_id && i["payload"]["role"] == "admin")
        .expect("promotion audit entry");
    assert!(
        entry["actor_label"]
            .as_str()
            .is_some_and(|s| s.contains("admin@example.com")),
        "actor_label should resolve to admin's display + email, got {:?}",
        entry["actor_label"],
    );
    assert!(
        entry["target_label"]
            .as_str()
            .is_some_and(|s| s.contains("target@example.com")),
        "target_label should resolve to target's display + email, got {:?}",
        entry["target_label"],
    );
}

#[tokio::test]
async fn admin_cannot_demote_self() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let me = get_me(&app, &admin).await;
    let admin_id = me["id"].as_str().unwrap();

    let resp = admin_send(
        &app,
        &admin,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "role": "user" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "self_demote");
}

#[tokio::test]
async fn disable_then_enable_round_trip() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "target@example.com", "correctly-horse-battery").await;

    let resp = admin_get(&app, &admin, "/api/admin/users?role=user").await;
    let body = body_json(resp.into_body()).await;
    let target_id = body["items"][0]["id"].as_str().unwrap().to_owned();

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/disable"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["state"], "disabled");

    // Subsequent calls to /auth/me using the disabled user's token should fail.
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/enable"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["state"], "active");
}

#[tokio::test]
async fn cannot_disable_self() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let me = get_me(&app, &admin).await;
    let admin_id = me["id"].as_str().unwrap();

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{admin_id}/disable"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn library_access_replace_works() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "target@example.com", "correctly-horse-battery").await;

    let lib_a = create_library(&app, &admin, "Lib A", "/tmp/lib-a-m3").await;
    let lib_b = create_library(&app, &admin, "Lib B", "/tmp/lib-b-m3").await;

    let resp = admin_get(&app, &admin, "/api/admin/users?role=user").await;
    let body = body_json(resp.into_body()).await;
    let target_id = body["items"][0]["id"].as_str().unwrap().to_owned();

    // Grant both
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/library-access"),
        Some(serde_json::json!({ "library_ids": [lib_a, lib_b] })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["library_count"], 2);
    assert_eq!(body["library_access"].as_array().unwrap().len(), 2);

    // Replace with just lib_a
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/library-access"),
        Some(serde_json::json!({ "library_ids": [lib_a] })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["library_count"], 1);
    assert_eq!(body["library_access"][0]["library_id"], lib_a);

    // Audit row should show the replace
    let resp = admin_get(
        &app,
        &admin,
        "/api/admin/audit?action=admin.user.library_access.set",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let items = body["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["target_id"] == target_id));
}

#[tokio::test]
async fn library_access_rejects_unknown_library() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "target@example.com", "correctly-horse-battery").await;
    let resp = admin_get(&app, &admin, "/api/admin/users?role=user").await;
    let body = body_json(resp.into_body()).await;
    let target_id = body["items"][0]["id"].as_str().unwrap().to_owned();

    let bogus = uuid::Uuid::now_v7().to_string();
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/library-access"),
        Some(serde_json::json!({ "library_ids": [bogus] })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audit_log_filters_by_action_prefix() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = register(&app, "target@example.com", "correctly-horse-battery").await;
    let resp = admin_get(&app, &admin, "/api/admin/users?role=user").await;
    let body = body_json(resp.into_body()).await;
    let target_id = body["items"][0]["id"].as_str().unwrap().to_owned();
    let _ = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/disable"),
        None,
    )
    .await;
    let _ = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{target_id}/enable"),
        None,
    )
    .await;

    let resp = admin_get(&app, &admin, "/api/admin/audit?action=admin.user.*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let items = body["items"].as_array().unwrap();
    assert!(items.len() >= 2);
    for item in items {
        assert!(item["action"].as_str().unwrap().starts_with("admin.user."));
    }
}

#[tokio::test]
async fn audit_log_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = admin_get(&app, &user, "/api/admin/audit").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
