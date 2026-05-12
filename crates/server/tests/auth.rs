//! Local auth integration test.
//!
//! Covers register → first-user-admin bootstrap, login, /auth/me, CSRF
//! enforcement, refresh rotation, refresh-reuse rejection, logout.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn first_user_becomes_admin() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"first@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_session=")));
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_csrf=")));
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("__Secure-comic_refresh="))
    );

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["role"], "admin");
}

#[tokio::test]
async fn second_user_is_regular_when_no_smtp() {
    let app = TestApp::spawn().await;
    // First — admin
    let _ = register(&app, "first@example.com", "correctly-horse-battery").await;
    // Second — regular user
    let resp = register(&app, "second@example.com", "another-strong-password").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["role"], "user");
}

#[tokio::test]
async fn login_wrong_password_is_401() {
    let app = TestApp::spawn().await;
    let _ = register(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = login(&app, "user@example.com", "wrong-password").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_requires_auth() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_session_cookie_works() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["email"], "user@example.com");
    assert!(body["csrf_token"].is_string());
}

#[tokio::test]
async fn csrf_enforced_on_unsafe_verbs() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Secure-comic_refresh").unwrap();

    // Without CSRF header → 403
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Secure-comic_refresh={refresh}"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // With CSRF header → 200
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Secure-comic_refresh={refresh}"
                    ),
                )
                .header("x-csrf-token", csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn refresh_token_reuse_is_rejected() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let original_refresh = extract_cookie(&reg, "__Secure-comic_refresh").unwrap();

    // Rotate once.
    let rotated = refresh_call(&app, &original_refresh, &csrf).await;
    assert_eq!(rotated.status(), StatusCode::OK);

    // Try to use the original (now-rotated) token again.
    let replay = refresh_call(&app, &original_refresh, &csrf).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_clears_cookies_and_revokes_session() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Secure-comic_refresh").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/logout")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Secure-comic_refresh={refresh}"
                    ),
                )
                .header("x-csrf-token", csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Cookies should be cleared (Max-Age=0).
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("__Host-comic_session=") && c.contains("Max-Age=0"))
    );

    // Replaying the refresh after logout should fail.
    let replay = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

// ───── helpers ─────

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

async fn refresh_call(
    app: &TestApp,
    refresh_value: &str,
    csrf: &str,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!("__Secure-comic_refresh={refresh_value}; __Host-comic_csrf={csrf}"),
                )
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn extract_cookie(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
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
