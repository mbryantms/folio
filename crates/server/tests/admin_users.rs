//! Integration tests for `/admin/users/*` and `/admin/audit` (M3).
//!
//! Covers the happy path for each endpoint plus the ACL boundaries:
//!   - Non-admin → 403
//!   - Admin → 200/201
//!   - Self-demote / self-disable → 403
//!   - The last active admin can't be demoted / deactivated: a
//!     `pending_verification` admin is refused by the extractor (E15), and
//!     two admins demoting each other concurrently leave one (lock + 409)
//!   - Library-access set replaces the user's grants and triggers an audit row

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{audit_log, user};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
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

/// D9: the first page carries a `total` matching the active filters; later
/// pages omit it (so the count isn't recomputed on every page).
#[tokio::test]
async fn list_users_reports_filtered_total_on_first_page_only() {
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

    // First page → total counts every matching user (admin + 3).
    let resp = admin_get(&app, &admin, "/api/admin/users").await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total"].as_u64(), Some(4), "total counts every user");

    // The total tracks the active filter, not the unfiltered table.
    let resp = admin_get(&app, &admin, "/api/admin/users?role=admin").await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total"].as_u64(), Some(1), "total respects filters");

    // Later pages (cursor present) omit the total entirely.
    let resp = admin_get(&app, &admin, "/api/admin/users?limit=2").await;
    let body = body_json(resp.into_body()).await;
    let cursor = body["next_cursor"].as_str().expect("a second page");
    let resp = admin_get(
        &app,
        &admin,
        &format!("/api/admin/users?limit=2&cursor={cursor}"),
    )
    .await;
    let body = body_json(resp.into_body()).await;
    assert!(
        body["total"].is_null(),
        "total appears only on the first page"
    );
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
    // Audit-remediation M10 follow-up: referencing a well-formed UUID
    // that names a non-existent library is semantic-validation (422),
    // not parse-shape (400).
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
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

/// E9 (audit-2026-11): the `prefix.*` filter matches the prefix literally.
/// Action names carry `_`, which an unescaped `LIKE` read as "any one
/// character", so `admin.lib_x.*` also returned `admin.libXx.…` rows.
#[tokio::test]
async fn audit_log_prefix_filter_matches_underscore_literally() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    for action in ["admin.lib_x.set", "admin.libXx.set"] {
        server::audit::record(
            &db,
            server::audit::AuditEntry {
                actor_id: uuid::Uuid::now_v7(),
                action,
                target_type: None,
                target_id: None,
                payload: serde_json::json!({}),
                ip: None,
                user_agent: None,
            },
        )
        .await;
    }

    let resp = admin_get(&app, &admin, "/api/admin/audit?action=admin.lib_x.*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let actions: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, vec!["admin.lib_x.set"]);
}

#[tokio::test]
async fn audit_log_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = admin_get(&app, &user, "/api/admin/audit").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ───────── admin create-user (3.8 / audit D9) ─────────

/// Attempt a JSON login; returns the response so callers can assert on
/// status + Set-Cookie. No CSRF — the login form-action is pre-auth.
async fn login(app: &TestApp, email: &str, password: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn sets_session_cookie(resp: &axum::http::Response<Body>) -> bool {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|c| c.starts_with("__Host-comic_session=") && c.len() > "__Host-comic_session=;".len())
}

#[tokio::test]
async fn admin_create_user_returns_usable_temp_password() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({ "email": "Provisioned@Example.com" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    // Email is normalized lower; account is active + admin-vouched.
    assert_eq!(body["email"], "provisioned@example.com");
    assert_eq!(body["role"], "user");
    assert_eq!(body["state"], "active");
    assert_eq!(body["email_verified"], true);
    let temp = body["temp_password"]
        .as_str()
        .expect("temp_password present");
    assert!(
        temp.len() >= 12,
        "temp password meets the local-auth minimum"
    );

    // The returned temp password actually authenticates the new account.
    let ok = login(&app, "provisioned@example.com", temp).await;
    assert!(
        sets_session_cookie(&ok),
        "temp password should log the user in (status {:?})",
        ok.status()
    );
    // A wrong password does not.
    let bad = login(&app, "provisioned@example.com", "definitely-not-it-123").await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_create_user_honors_role_and_duplicate_conflicts() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // role=admin is honored.
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({ "email": "newadmin@example.com", "role": "admin" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert_eq!(body_json(resp.into_body()).await["role"], "admin");

    // Same email again → 409 conflict.
    let dup = admin_send(
        &app,
        &admin,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({ "email": "newadmin@example.com" })),
    )
    .await;
    assert_eq!(dup.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn admin_create_user_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = admin_send(
        &app,
        &user,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({ "email": "nope@example.com" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_create_user_rejects_bad_email() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({ "email": "not-an-email" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ───────── last active admin (audit-2026-11 E10) ─────────

const PW: &str = "correctly-horse-battery";

async fn user_id_of(app: &TestApp, auth: &Authed) -> String {
    get_me(app, auth).await["id"].as_str().unwrap().to_owned()
}

/// `(role, state)` straight from the row.
async fn role_state(app: &TestApp, id: &str) -> (user::UserRole, user::UserState) {
    let row = user::Entity::find_by_id(uuid::Uuid::parse_str(id).unwrap())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    (row.role, row.state)
}

async fn active_admin_count(app: &TestApp) -> u64 {
    user::Entity::find()
        .filter(user::Column::Role.eq(user::UserRole::Admin))
        .filter(user::Column::State.eq(user::UserState::Active))
        .count(&app.state().db)
        .await
        .unwrap()
}

async fn patch_user(app: &TestApp, actor: &Authed, id: &str, body: Value) -> StatusCode {
    admin_send(
        app,
        actor,
        Method::PATCH,
        &format!("/api/admin/users/{id}"),
        Some(body),
    )
    .await
    .status()
}

/// A signed-in admin whose state is `pending_verification`. It is not an
/// *active* admin, so the bootstrap admin stays the instance's only active
/// one; and since E15 it no longer passes `RequireAdmin` either, so with one
/// active admin no other signed-in admin can reach the last-admin check.
async fn pending_admin(app: &TestApp, admin: &Authed, email: &str) -> Authed {
    let other = register_authed(app, email, PW).await;
    let id = user_id_of(app, &other).await;
    assert_eq!(
        patch_user(app, admin, &id, serde_json::json!({ "role": "admin" })).await,
        StatusCode::OK
    );
    assert_eq!(
        patch_user(
            app,
            admin,
            &id,
            serde_json::json!({ "state": "pending_verification" })
        )
        .await,
        StatusCode::OK
    );
    other
}

/// The pending admin's attempt is refused by the extractor (E15) before the
/// handler's last-admin check runs.
async fn assert_pending_refused(resp: axum::http::Response<Body>) {
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_unverified", "{body}");
}

async fn audit_count(app: &TestApp, action: &str, target_id: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(action))
        .filter(audit_log::Column::TargetId.eq(target_id))
        .count(&app.state().db)
        .await
        .unwrap()
}

#[tokio::test]
async fn pending_admin_cannot_demote_the_last_active_admin() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", PW).await;
    let admin_id = user_id_of(&app, &admin).await;
    let pending = pending_admin(&app, &admin, "pending@example.com").await;

    let resp = admin_send(
        &app,
        &pending,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "role": "user" })),
    )
    .await;
    assert_pending_refused(resp).await;
    assert_eq!(
        role_state(&app, &admin_id).await,
        (user::UserRole::Admin, user::UserState::Active)
    );

    // A combined change (rename + demote) is refused whole: the name stays.
    let resp = admin_send(
        &app,
        &pending,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "role": "user", "display_name": "Renamed" })),
    )
    .await;
    assert_pending_refused(resp).await;
    assert_ne!(get_me(&app, &admin).await["display_name"], "Renamed");

    // No audit row for the refused changes.
    assert_eq!(audit_count(&app, "admin.user.update", &admin_id).await, 0);
}

#[tokio::test]
async fn pending_admin_cannot_disable_the_last_active_admin() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", PW).await;
    let admin_id = user_id_of(&app, &admin).await;
    let pending = pending_admin(&app, &admin, "pending@example.com").await;

    for state in ["disabled", "pending_verification"] {
        let resp = admin_send(
            &app,
            &pending,
            Method::PATCH,
            &format!("/api/admin/users/{admin_id}"),
            Some(serde_json::json!({ "state": state })),
        )
        .await;
        assert_pending_refused(resp).await;
    }
    let resp = admin_send(
        &app,
        &pending,
        Method::POST,
        &format!("/api/admin/users/{admin_id}/disable"),
        None,
    )
    .await;
    assert_pending_refused(resp).await;

    // No admin moves itself out of `active`: the self-disable 403 covers
    // `pending_verification` too, whoever else is an admin.
    let resp = admin_send(
        &app,
        &admin,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "state": "pending_verification" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "self_disable", "{body}");

    assert_eq!(
        role_state(&app, &admin_id).await,
        (user::UserRole::Admin, user::UserState::Active)
    );
    assert_eq!(audit_count(&app, "admin.user.disable", &admin_id).await, 0);
    // The session survived: no token_version bump on a refused change.
    get_me(&app, &admin).await;
}

#[tokio::test]
async fn demoting_one_of_two_admins_succeeds_then_the_remaining_one_stays() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", PW).await;
    let admin_id = user_id_of(&app, &admin).await;
    let second = register_authed(&app, "second@example.com", PW).await;
    let second_id = user_id_of(&app, &second).await;
    assert_eq!(
        patch_user(
            &app,
            &admin,
            &second_id,
            serde_json::json!({ "role": "admin" })
        )
        .await,
        StatusCode::OK
    );
    let pending = pending_admin(&app, &admin, "pending@example.com").await;
    assert_eq!(active_admin_count(&app).await, 2);

    // Two active admins: demoting one is fine, and audited.
    assert_eq!(
        patch_user(
            &app,
            &admin,
            &second_id,
            serde_json::json!({ "role": "user" })
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(role_state(&app, &second_id).await.0, user::UserRole::User);
    let resp = admin_get(&app, &admin, "/api/admin/audit?action=admin.user.update").await;
    let body = body_json(resp.into_body()).await;
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["target_id"] == second_id.as_str() && i["payload"]["role"] == "user"),
        "demotion audit entry: {body}"
    );

    // Now one active admin remains, and it can't be demoted: not by the
    // demoted admin (no longer an admin), not by the pending one.
    let resp = admin_send(
        &app,
        &second,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "role": "user" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = admin_send(
        &app,
        &pending,
        Method::PATCH,
        &format!("/api/admin/users/{admin_id}"),
        Some(serde_json::json!({ "role": "user" })),
    )
    .await;
    assert_pending_refused(resp).await;
    assert_eq!(active_admin_count(&app).await, 1);
}

#[tokio::test]
async fn disabling_one_of_two_admins_succeeds_and_is_audited() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", PW).await;
    let second = register_authed(&app, "second@example.com", PW).await;
    let second_id = user_id_of(&app, &second).await;
    assert_eq!(
        patch_user(
            &app,
            &admin,
            &second_id,
            serde_json::json!({ "role": "admin" })
        )
        .await,
        StatusCode::OK
    );
    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/admin/users/{second_id}/disable"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        role_state(&app, &second_id).await,
        (user::UserRole::Admin, user::UserState::Disabled)
    );
    assert_eq!(audit_count(&app, "admin.user.disable", &second_id).await, 1);
}

/// Two admins demote each other at the same moment. Without the lock both
/// requests pass the extractor and the check, and the instance ends with no
/// admin; with it the second waits, re-reads, and is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cross_demotion_leaves_an_admin() {
    let app = TestApp::spawn().await;
    let a = register_authed(&app, "a@example.com", PW).await;
    let a_id = user_id_of(&app, &a).await;
    let b = register_authed(&app, "b@example.com", PW).await;
    let b_id = user_id_of(&app, &b).await;
    assert_eq!(
        patch_user(&app, &a, &b_id, serde_json::json!({ "role": "admin" })).await,
        StatusCode::OK
    );
    assert_eq!(active_admin_count(&app).await, 2);

    let demote = |actor: &Authed, target: &str| {
        let req = Request::builder()
            .method(Method::PATCH)
            .uri(format!("/api/admin/users/{target}"))
            .header(header::COOKIE, actor.cookie())
            .header("x-csrf-token", &actor.csrf)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"role":"user"}"#))
            .unwrap();
        let router = app.router.clone();
        tokio::spawn(async move { router.oneshot(req).await.unwrap().status() })
    };
    let a_demotes_b = demote(&a, &b_id);
    let b_demotes_a = demote(&b, &a_id);
    let (sa, sb) = (a_demotes_b.await.unwrap(), b_demotes_a.await.unwrap());

    let ok = [sa, sb].iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(ok, 1, "exactly one demotion lands: {sa} / {sb}");
    for s in [sa, sb] {
        assert!(
            [StatusCode::OK, StatusCode::FORBIDDEN, StatusCode::CONFLICT].contains(&s),
            "unexpected status {s}"
        );
    }
    assert_eq!(active_admin_count(&app).await, 1);
}
