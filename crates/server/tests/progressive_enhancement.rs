//! Progressive-enhancement tests for the auth handlers (M9).
//!
//! These exercise the form-encoded fallback path that browsers take when JS
//! hasn't hydrated or has failed. Each test confirms that:
//!   - submitting `application/x-www-form-urlencoded` produces a 303 redirect
//!     (NOT a 200 JSON envelope and NOT a leak into the response URL),
//!   - the redirect target is the validated `next` field on success,
//!   - failure cases land at the form's origin page with `?error=&message=`,
//!   - `__Host-`/`__Secure-` cookies are set on the form path's success.
//!
//! The JSON contract is covered separately in `auth.rs`; this file only
//! cares about the form half of the dual-mode contract.

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

fn form(body: &str) -> Body {
    Body::from(body.to_owned())
}

async fn register_json(app: &TestApp, email: &str, password: &str) {
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
}

#[tokio::test]
async fn form_login_success_303_with_cookies() {
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form(
                    "email=user%40example.com&password=correctly-horse-battery",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/",
        "default redirect target on the form path is /"
    );
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies.iter().any(|c| c.contains("__Host-comic_session=")),
        "session cookie should land on the 303 so the next request is authed"
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("__Secure-comic_refresh=")),
        "refresh cookie should also land"
    );
}

#[tokio::test]
async fn form_login_honors_safe_next_target() {
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form(
                    "email=user%40example.com&password=correctly-horse-battery&next=%2Flibrary",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/library",
        "validated `next` should drive the redirect target"
    );
}

#[tokio::test]
async fn form_login_rejects_open_redirect() {
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form(
                    // Protocol-relative URL would target evil.com if accepted.
                    "email=user%40example.com&password=correctly-horse-battery&next=%2F%2Fevil.com",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers().get(header::LOCATION).unwrap().to_str().unwrap();
    assert_eq!(
        loc, "/",
        "unsafe `next` is dropped and the default redirect target (`/`) is used"
    );
}

#[tokio::test]
async fn form_login_bad_credentials_303_to_sign_in() {
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form(
                    "email=user%40example.com&password=wrong-password",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers().get(header::LOCATION).unwrap().to_str().unwrap();
    assert!(
        loc.starts_with("/sign-in?error=auth.invalid"),
        "form-fallback failure must 303 back to /sign-in with an error code, got {loc}"
    );
}

#[tokio::test]
async fn form_register_success_303_with_cookies() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form(
                    "email=first%40example.com&password=correctly-horse-battery",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_session=")));
}

#[tokio::test]
async fn form_register_validation_failure_303_to_sign_in() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form("email=first%40example.com&password=tooshort"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers().get(header::LOCATION).unwrap().to_str().unwrap();
    assert!(loc.starts_with("/sign-in?error=validation"), "got {loc}");
}

#[tokio::test]
async fn form_request_password_reset_303_to_forgot_sent() {
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/request-password-reset")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form("email=user%40example.com"))
                .unwrap(),
        )
        .await
        .unwrap();

    // The JSON contract returns 204; the form contract redirects to the
    // post-submit confirmation view. Anti-enumeration: same redirect even
    // when the email doesn't match a user.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/forgot-password?sent=1"
    );
}

#[tokio::test]
async fn json_login_still_returns_json() {
    // Regression guard: form-path additions must not have changed the
    // existing JSON contract. M8's mutation hooks rely on the 200/JSON
    // envelope.
    let app = TestApp::spawn().await;
    register_json(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"user@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get(header::LOCATION).is_none(),
        "JSON path must NOT 303"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["email"], "user@example.com");
}

#[tokio::test]
async fn csrf_middleware_accepts_hidden_form_field() {
    // Authenticated POST /me/account with the CSRF token in a hidden form
    // field (no `X-CSRF-Token` header). This is the no-JS path for the
    // account settings page; the CSRF middleware has to read the body.
    let app = TestApp::spawn().await;

    // Register + capture cookies for the auth session.
    let reg = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"first@example.com","password":"correctly-horse-battery","display_name":"First"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);

    let mut session = String::new();
    let mut csrf_value = String::new();
    for c in reg.headers().get_all(header::SET_COOKIE).iter() {
        let s = c.to_str().unwrap();
        if let Some(v) = s.strip_prefix("__Host-comic_session=")
            && let Some(end) = v.find(';')
        {
            session = format!("__Host-comic_session={}", &v[..end]);
        }
        if let Some(v) = s.strip_prefix("__Host-comic_csrf=")
            && let Some(end) = v.find(';')
        {
            csrf_value = v[..end].to_owned();
        }
    }
    assert!(!session.is_empty(), "session cookie set");
    assert!(!csrf_value.is_empty(), "csrf cookie set");

    // Submit form-encoded with csrf_token in the body (NO X-CSRF-Token header).
    let cookie_header = format!("{session}; __Host-comic_csrf={csrf_value}");
    let body_str = format!(
        "csrf_token={token}&display_name=Renamed",
        token = urlencoding::encode(&csrf_value)
    );
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/me/account")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(header::COOKIE, cookie_header)
                .body(Body::from(body_str))
                .unwrap(),
        )
        .await
        .unwrap();

    // CSRF should pass via the form-field path, account update should
    // succeed, and we should land at the form-success redirect target.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/settings/account?ok=1"
    );
}

#[tokio::test]
async fn csrf_middleware_rejects_form_field_mismatch() {
    let app = TestApp::spawn().await;
    let reg = app
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
    let mut session = String::new();
    let mut csrf_value = String::new();
    for c in reg.headers().get_all(header::SET_COOKIE).iter() {
        let s = c.to_str().unwrap();
        if let Some(v) = s.strip_prefix("__Host-comic_session=")
            && let Some(end) = v.find(';')
        {
            session = format!("__Host-comic_session={}", &v[..end]);
        }
        if let Some(v) = s.strip_prefix("__Host-comic_csrf=")
            && let Some(end) = v.find(';')
        {
            csrf_value = v[..end].to_owned();
        }
    }

    let cookie_header = format!("{session}; __Host-comic_csrf={csrf_value}");
    // Tamper with the form-field value so it doesn't match the cookie.
    let body_str = "csrf_token=NOPE&display_name=Renamed";
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/me/account")
                .header(
                    header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(header::COOKIE, cookie_header)
                .body(Body::from(body_str))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.csrf");
}
