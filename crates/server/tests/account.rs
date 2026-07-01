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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.display_name");
}

#[tokio::test]
async fn change_password_requires_current() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pw1@example.com", "original-password-123").await;
    let (status, body) =
        patch_account(&app, &auth, r#"{"new_password":"new-password-stronger-9"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.email");
}

#[tokio::test]
async fn empty_body_is_noop() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "noop@example.com", "correctly-horse-battery").await;
    let (status, _) = patch_account(&app, &auth, r#"{}"#).await;
    assert_eq!(status, StatusCode::OK);
}

// ---- AUTH-1: a password change must revoke every OTHER session (so a stolen
// refresh token can't outlive the change) while keeping the caller signed in. ----

fn cookie_value(cookies: &[String], prefix: &str) -> String {
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
        .unwrap_or_else(|| panic!("missing cookie {prefix}"))
}

/// POST an email/password auth request (register or login), returning the
/// status and every `Set-Cookie` value so the caller can pull the refresh token.
async fn auth_post(
    app: &TestApp,
    uri: &str,
    email: &str,
    password: &str,
) -> (StatusCode, Vec<String>) {
    let body = format!(r#"{{"email":"{email}","password":"{password}"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let cookies = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    (status, cookies)
}

/// Attempt to rotate a refresh token. CSRF is double-submit, so any matching
/// cookie+header value satisfies the middleware (it compares the two, not
/// server state) — the refresh cookie is the credential under test.
async fn refresh_status(app: &TestApp, refresh: &str) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_refresh={refresh}; __Host-comic_csrf=csrf-x"),
                )
                .header("X-CSRF-Token", "csrf-x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

#[tokio::test]
async fn password_change_revokes_other_sessions_but_keeps_caller() {
    let app = TestApp::spawn().await;
    let email = "auth1-sessions@example.com";
    let pw = "original-password-123";

    // Session A (the caller that will change the password) and session B (a
    // second device), both on the same account.
    let (sa, cookies_a) = auth_post(&app, "/auth/local/register", email, pw).await;
    assert_eq!(sa, StatusCode::CREATED);
    let (sb, cookies_b) = auth_post(&app, "/auth/local/login", email, pw).await;
    assert_eq!(sb, StatusCode::OK);

    let refresh_a = cookie_value(&cookies_a, "__Host-comic_refresh=");
    let refresh_b = cookie_value(&cookies_b, "__Host-comic_refresh=");

    // Session A changes its password. A real browser sends its refresh cookie
    // on this request (Path=/), which is how the handler identifies and spares
    // the caller's own session while revoking the rest.
    let session_a = cookie_value(&cookies_a, "__Host-comic_session=");
    let csrf_a = cookie_value(&cookies_a, "__Host-comic_csrf=");
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
                        "__Host-comic_session={session_a}; __Host-comic_csrf={csrf_a}; __Host-comic_refresh={refresh_a}"
                    ),
                )
                .header("X-CSRF-Token", &csrf_a)
                .body(Body::from(
                    r#"{"current_password":"original-password-123","new_password":"second-password-456"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The other device's refresh token is now revoked (it can no longer rotate)...
    assert_eq!(
        refresh_status(&app, &refresh_b).await,
        StatusCode::UNAUTHORIZED,
        "other-device refresh must be revoked after a password change",
    );
    // ...while the caller's own session survives so their browser stays signed
    // in (its access token, invalidated by the token_version bump, re-mints here).
    assert_eq!(
        refresh_status(&app, &refresh_a).await,
        StatusCode::OK,
        "the caller's own refresh must survive the password change",
    );
}
