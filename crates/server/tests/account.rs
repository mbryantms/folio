//! M4: `PATCH /me/account` — display name, email, and password change.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp, email: &str, password: &str) -> Authed {
    let body = format!(
        r#"{{"email":"{email}","password":"{password}"}}"#,
        email = email,
        password = password,
    );
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

async fn login(app: &TestApp, email: &str, password: &str) -> StatusCode {
    let body = format!(
        r#"{{"email":"{email}","password":"{password}"}}"#,
        email = email,
        password = password,
    );
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

async fn patch_account(
    app: &TestApp,
    auth: &Authed,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/me/account")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn display_name_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "rename@example.com", "correctly-horse-battery").await;
    let (status, body) = patch_account(&app, &auth, r#"{"display_name":"Power User"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["display_name"], "Power User");
}

#[tokio::test]
async fn empty_display_name_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "empty@example.com", "correctly-horse-battery").await;
    let (status, body) = patch_account(&app, &auth, r#"{"display_name":"   "}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.display_name");
}

#[tokio::test]
async fn change_password_requires_current() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pw1@example.com", "original-password-123").await;
    let (status, body) =
        patch_account(&app, &auth, r#"{"new_password":"new-password-stronger-9"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.current_password");
}

#[tokio::test]
async fn change_password_validates_current() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pw2@example.com", "original-password-123").await;
    let (status, body) = patch_account(
        &app,
        &auth,
        r#"{"current_password":"wrong-password-xx","new_password":"new-password-stronger-9"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "auth.invalid");
}

#[tokio::test]
async fn change_password_minimum_length() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pw3@example.com", "original-password-123").await;
    let (status, body) = patch_account(
        &app,
        &auth,
        r#"{"current_password":"original-password-123","new_password":"short"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.new_password");
}

#[tokio::test]
async fn change_password_round_trip_and_invalidates_old() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pw4@example.com", "original-password-123").await;
    let (status, _) = patch_account(
        &app,
        &auth,
        r#"{"current_password":"original-password-123","new_password":"second-password-456"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Old password no longer works.
    let bad = login(&app, "pw4@example.com", "original-password-123").await;
    assert_eq!(bad, StatusCode::UNAUTHORIZED);
    // New password works.
    let good = login(&app, "pw4@example.com", "second-password-456").await;
    assert_eq!(good, StatusCode::OK);
}

#[tokio::test]
async fn email_change_round_trip_and_uniqueness() {
    let app = TestApp::spawn().await;
    let _other = register(&app, "taken@example.com", "correctly-horse-battery").await;
    let auth = register(&app, "renamer@example.com", "correctly-horse-battery").await;

    // Conflict with another user's email
    let (status, body) = patch_account(&app, &auth, r#"{"email":"taken@example.com"}"#).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict");

    // Successful change
    let (status, body) = patch_account(&app, &auth, r#"{"email":"new-name@example.com"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "new-name@example.com");
}

#[tokio::test]
async fn email_validation_rejects_garbage() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "badmail@example.com", "correctly-horse-battery").await;
    let (status, body) = patch_account(&app, &auth, r#"{"email":"not-an-email"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.email");
}

#[tokio::test]
async fn empty_body_is_noop() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "noop@example.com", "correctly-horse-battery").await;
    let (status, _) = patch_account(&app, &auth, r#"{}"#).await;
    assert_eq!(status, StatusCode::OK);
}
