//! `RequireAdmin` extractor coverage (M1).
//!
//! Confirms the structural admin guard rejects non-admins with 403
//! `auth.permission_denied` and accepts admins. Also verifies the request
//! context middleware persists `auth_sessions.ip` + `.user_agent`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

struct Authed {
    session: String,
    csrf: String,
    user_id: String,
}

async fn register(app: &TestApp, email: &str, user_agent: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::USER_AGENT, user_agent)
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
    let body = body_json(resp.into_body()).await;
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: body["user"]["id"].as_str().unwrap().to_owned(),
    }
}

async fn admin_get(app: &TestApp, auth: &Authed, uri: &str) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

#[tokio::test]
async fn require_admin_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com", "agent/admin").await;
    let user = register(&app, "user@example.com", "agent/user").await;
    // /admin/auth/config is the smallest admin-only handler that uses the new
    // extractor (no other side effects, no path params).
    let status = admin_get(&app, &user, "/api/admin/auth/config").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn require_admin_accepts_admin() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let status = admin_get(&app, &admin, "/api/admin/auth/config").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn require_admin_returns_envelope() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com", "agent/admin").await;
    let user = register(&app, "user@example.com", "agent/user").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/auth/config")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        user.session, user.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.permission_denied");
}

#[tokio::test]
async fn session_persists_user_agent_from_request() {
    let app = TestApp::spawn().await;
    let authed = register(&app, "ua@example.com", "Mozilla/5.0 (RequestContextTest)").await;

    let user_uuid = uuid::Uuid::parse_str(&authed.user_id).expect("valid user id");
    let state = app.state();
    let rows = entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(user_uuid))
        .all(&state.db)
        .await
        .expect("query auth_sessions");
    assert_eq!(rows.len(), 1, "expected exactly one session per register");
    let session = &rows[0];
    assert_eq!(
        session.user_agent.as_deref(),
        Some("Mozilla/5.0 (RequestContextTest)"),
        "user-agent header should be persisted on session insert",
    );
    // Without ConnectInfo wired through the test harness the fallback peer is
    // 127.0.0.1 — we just assert it's non-null so future XFF wiring is the
    // only thing left to test once the production listener is exercised.
    assert!(
        session.ip.is_some(),
        "ip should be persisted (fallback peer in tests)",
    );
}
