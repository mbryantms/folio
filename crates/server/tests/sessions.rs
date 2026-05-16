//! `/me/sessions` integration tests (M5, audit M-8).
//!
//! Each test registers one or more accounts, logs in to spawn extra
//! auth_session rows, then exercises list / revoke / revoke-all and
//! verifies database side effects.

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
    refresh: String,
}

impl Auth {
    fn cookie_header(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}; __Secure-comic_refresh={}",
            self.session, self.csrf, self.refresh
        )
    }
    fn from_response(resp: &Response<Body>) -> Self {
        Self {
            session: extract_cookie(resp, "__Host-comic_session").unwrap(),
            csrf: extract_cookie(resp, "__Host-comic_csrf").unwrap(),
            refresh: extract_cookie(resp, "__Secure-comic_refresh").unwrap(),
        }
    }
}

async fn register(app: &TestApp, email: &str, password: &str) -> Response<Body> {
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

async fn login(app: &TestApp, email: &str, password: &str) -> Response<Body> {
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

async fn list_sessions(app: &TestApp, auth: &Auth) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/me/sessions")
                .header(header::COOKIE, auth.cookie_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn revoke_session(app: &TestApp, auth: &Auth, id: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/me/sessions/{id}"))
                .header(header::COOKIE, auth.cookie_header())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn revoke_all(app: &TestApp, auth: &Auth) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/sessions/revoke-all")
                .header(header::COOKIE, auth.cookie_header())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn list_includes_current_session_flag() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "list-current@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);

    // Open a second session via /login.
    let _second = login(&app, "list-current@example.com", "correctly-horse-battery").await;

    let resp = list_sessions(&app, &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let arr = body["sessions"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "two sessions visible after a second login");

    let current_count = arr
        .iter()
        .filter(|s| s["current"].as_bool() == Some(true))
        .count();
    assert_eq!(
        current_count, 1,
        "exactly one session should be flagged current"
    );

    // Sanity: id, created_at, last_used_at, expires_at are present.
    let s = &arr[0];
    assert!(s["id"].is_string());
    assert!(s["created_at"].is_string());
    assert!(s["last_used_at"].is_string());
    assert!(s["expires_at"].is_string());
}

#[tokio::test]
async fn revoke_one_kills_only_that_session() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "revoke-one@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);
    let _ = login(&app, "revoke-one@example.com", "correctly-horse-battery").await;

    let resp = list_sessions(&app, &auth).await;
    let body = body_json(resp.into_body()).await;
    let arr = body["sessions"].as_array().unwrap().clone();
    assert_eq!(arr.len(), 2);

    // Pick the non-current one to revoke.
    let target = arr
        .iter()
        .find(|s| s["current"].as_bool() == Some(false))
        .expect("non-current session");
    let target_id = target["id"].as_str().unwrap().to_owned();

    let resp = revoke_session(&app, &auth, &target_id).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // List should now show only the current session.
    let resp = list_sessions(&app, &auth).await;
    let body = body_json(resp.into_body()).await;
    let arr = body["sessions"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["current"].as_bool(), Some(true));
}

#[tokio::test]
async fn revoke_one_404_when_not_owned() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "correctly-horse-battery").await;
    assert_eq!(admin.status(), StatusCode::CREATED);
    let other = register(&app, "other@example.com", "correctly-horse-battery").await;
    assert_eq!(other.status(), StatusCode::CREATED);
    let admin_auth = Auth::from_response(&admin);

    // Find other user's session id from the DB.
    let state = app.state();
    let other_user = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("other@example.com"))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let other_session = entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(other_user.id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("other user has a session");

    // Admin tries to revoke other's session — 404, not 403, because we
    // don't leak existence.
    let resp = revoke_session(&app, &admin_auth, &other_session.id.to_string()).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn revoke_all_revokes_every_session_and_bumps_tv() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "revoke-all@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);
    let _ = login(&app, "revoke-all@example.com", "correctly-horse-battery").await;
    let _ = login(&app, "revoke-all@example.com", "correctly-horse-battery").await;

    // Pre-state.
    let state = app.state();
    let user_row = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("revoke-all@example.com"))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let tv_before = user_row.token_version;

    let resp = revoke_all(&app, &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["revoked"].as_u64(), Some(3));

    // List should be empty now (all revoked).
    let resp = list_sessions(&app, &auth).await;
    // The /auth/me access token still verifies for ~15 min unless tv
    // bumped. The bump should kill it immediately.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let user_after = entity::user::Entity::find_by_id(user_row.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        user_after.token_version,
        tv_before + 1,
        "token_version should bump on revoke-all"
    );

    let revoked_count = entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(user_row.id))
        .filter(entity::auth_session::Column::RevokedAt.is_not_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(revoked_count, 3);
}

#[tokio::test]
async fn revoke_one_clears_current_marks_only_other_session() {
    // Variant of the above: revoke the *current* session and confirm it
    // disappears from the list (and the cookie is therefore stale).
    let app = TestApp::spawn().await;
    let reg = register(&app, "revoke-cur@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);
    let _ = login(&app, "revoke-cur@example.com", "correctly-horse-battery").await;

    let resp = list_sessions(&app, &auth).await;
    let body = body_json(resp.into_body()).await;
    let arr = body["sessions"].as_array().unwrap().clone();
    let current_id = arr
        .iter()
        .find(|s| s["current"].as_bool() == Some(true))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = revoke_session(&app, &auth, &current_id).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = list_sessions(&app, &auth).await;
    let body = body_json(resp.into_body()).await;
    let arr = body["sessions"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["current"].as_bool(),
        Some(false),
        "remaining session is not the caller's"
    );
}
