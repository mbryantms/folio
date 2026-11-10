//! Email changes through `PATCH /me/account` need the current password and,
//! when mail delivery is configured, a confirmation link sent to the new
//! address (`GET /auth/local/confirm-email-change`).
//!
//! Covers: missing / wrong password refused (wrong ones count toward the
//! failed-auth lockout); with mail on, the address is unchanged until the
//! link is opened, the new address gets the link and the old one a notice;
//! the link works once, not after expiry, not after a newer request, not
//! when another account took the address; confirming signs other devices
//! out and verifies the address; with mail off the change applies at once;
//! identity-provider accounts still cannot edit their email.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use entity::{email_change_request, user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correctly-horse-battery";

struct Authed {
    session: String,
    csrf: String,
    refresh: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}; __Host-comic_refresh={}",
            self.session, self.csrf, self.refresh
        )
    }
}

fn set_cookies(resp: &Response<Body>) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect()
}

fn cookie_value(cookies: &[String], name: &str) -> String {
    let prefix = format!("{name}=");
    cookies
        .iter()
        .find_map(|s| {
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn auth_call(app: &TestApp, uri: &str, email: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn authed(resp: Response<Body>) -> Authed {
    let cookies = set_cookies(&resp);
    let body = body_json(resp.into_body()).await;
    Authed {
        session: cookie_value(&cookies, "__Host-comic_session"),
        csrf: cookie_value(&cookies, "__Host-comic_csrf"),
        refresh: cookie_value(&cookies, "__Host-comic_refresh"),
        user_id: Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

/// Register an active account (the first user, or any user with mail off).
async fn register(app: &TestApp, email: &str) -> Authed {
    let resp = auth_call(app, "/auth/local/register", email).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    authed(resp).await
}

/// A second device: a fresh login on the same account.
async fn login(app: &TestApp, email: &str) -> Authed {
    let resp = auth_call(app, "/auth/local/login", email).await;
    assert_eq!(resp.status(), StatusCode::OK);
    authed(resp).await
}

async fn patch_account(app: &TestApp, auth: &Authed, body: Value) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/me/account")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn get_me(app: &TestApp, auth: &Authed) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/me")
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
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn refresh_status(app: &TestApp, auth: &Authed) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_refresh={}; __Host-comic_csrf={}",
                        auth.refresh, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Open a confirmation link, optionally from a signed-in browser (its
/// refresh cookie rides along). Returns the redirect target.
async fn open_link(app: &TestApp, token: &str, browser: Option<&Authed>) -> String {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("/auth/local/confirm-email-change?token={token}"));
    if let Some(b) = browser {
        req = req.header(header::COOKIE, b.cookies());
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "expected a redirect, got {}",
        resp.status()
    );
    resp.headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

/// The token in the confirmation link of the newest email sent to `to`.
async fn link_token_sent_to(app: &TestApp, to: &str) -> String {
    let mail = app
        .email
        .outbox()
        .await
        .into_iter()
        .rev()
        .find(|m| m.to == to)
        .unwrap_or_else(|| panic!("no email sent to {to}"));
    let start = mail
        .body_text
        .find("confirm-email-change?token=")
        .expect("confirmation link in body")
        + "confirm-email-change?token=".len();
    mail.body_text[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

async fn user_row(app: &TestApp, id: Uuid) -> user::Model {
    user::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

const CONFIRMED: &str = "/settings/account?email_change=confirmed";
const INVALID: &str = "/settings/account?email_change=invalid";
const TAKEN: &str = "/settings/account?email_change=taken";

#[tokio::test]
async fn email_change_without_current_password_is_refused() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;

    let (status, body) = patch_account(&app, &a, json!({"email": "thief@example.com"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["field"], "current_password");

    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("owner@example.com"));
    assert!(app.email.outbox().await.is_empty(), "nothing may be mailed");
}

#[tokio::test]
async fn wrong_current_password_is_refused_and_counted() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;

    // FAIL_THRESHOLD wrong attempts…
    for _ in 0..server::auth::failed_auth::FAIL_THRESHOLD {
        let (status, body) = patch_account(
            &app,
            &a,
            json!({"email": "thief@example.com", "current_password": "not-the-password"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "auth.invalid");
        assert_eq!(body["error"]["details"][0]["field"], "current_password");
    }
    // …lock the account out: even the right password is now refused.
    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "thief@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "auth.locked_out");

    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("owner@example.com"));
    assert!(app.email.outbox().await.is_empty());
}

#[tokio::test]
async fn confirmed_change_applies_once_and_signs_other_devices_out() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let b = login(&app, "owner@example.com").await;

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "New@Example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "owner@example.com");
    assert_eq!(body["pending_email"], "new@example.com");

    // Nothing changed yet; /auth/me shows the pending address too.
    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("owner@example.com"));
    let (_, me) = get_me(&app, &a).await;
    assert_eq!(me["pending_email"], "new@example.com");

    // The new address gets the link, the old one a notice without it.
    let outbox = app.email.outbox().await;
    assert_eq!(outbox.len(), 2);
    let notice = outbox
        .iter()
        .find(|m| m.to == "owner@example.com")
        .expect("notice to old address");
    assert!(notice.body_text.contains("new@example.com"));
    assert!(!notice.body_text.contains("token="));
    let token = link_token_sent_to(&app, "new@example.com").await;

    // Device A opens the link in its own browser.
    assert_eq!(open_link(&app, &token, Some(&a)).await, CONFIRMED);
    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("new@example.com"));
    assert!(row.email_verified);

    // Device B is signed out; device A keeps its refresh session.
    assert_eq!(get_me(&app, &b).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &b).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &a).await, StatusCode::OK);

    // Single use.
    assert_eq!(open_link(&app, &token, None).await, INVALID);

    // No longer pending; the new address signs in.
    let c = login(&app, "new@example.com").await;
    let (_, me) = get_me(&app, &c).await;
    assert_eq!(me["email"], "new@example.com");
    assert!(me["pending_email"].is_null());
}

#[tokio::test]
async fn expired_link_is_refused() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let (status, _) = patch_account(
        &app,
        &a,
        json!({"email": "new@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = link_token_sent_to(&app, "new@example.com").await;

    let db = &app.state().db;
    let req = email_change_request::Entity::find()
        .filter(email_change_request::Column::UserId.eq(a.user_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am = req.into_active_model();
    am.expires_at = Set((chrono::Utc::now() - chrono::Duration::minutes(1)).fixed_offset());
    am.update(db).await.unwrap();

    assert_eq!(open_link(&app, &token, None).await, INVALID);
    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("owner@example.com"));
    // An expired request is not "pending".
    assert!(get_me(&app, &a).await.1["pending_email"].is_null());
}

#[tokio::test]
async fn a_second_request_replaces_the_first() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    for addr in ["first@example.com", "second@example.com"] {
        let (status, body) = patch_account(
            &app,
            &a,
            json!({"email": addr, "current_password": PASSWORD}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["pending_email"], addr);
    }
    let first = link_token_sent_to(&app, "first@example.com").await;
    let second = link_token_sent_to(&app, "second@example.com").await;

    assert_eq!(open_link(&app, &first, None).await, INVALID);
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("owner@example.com")
    );
    assert_eq!(open_link(&app, &second, None).await, CONFIRMED);
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("second@example.com")
    );
}

#[tokio::test]
async fn address_taken_meanwhile_is_refused() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let (status, _) = patch_account(
        &app,
        &a,
        json!({"email": "wanted@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = link_token_sent_to(&app, "wanted@example.com").await;

    // Someone registers the address before the link is opened (with mail on
    // a second registration is pending verification, but the row holds it).
    let resp = auth_call(&app, "/auth/local/register", "wanted@example.com").await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    assert_eq!(open_link(&app, &token, None).await, TAKEN);
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("owner@example.com")
    );
}

#[tokio::test]
async fn without_mail_the_change_applies_after_the_password_check() {
    let app = TestApp::spawn().await;
    let a = register(&app, "owner@example.com").await;
    let b = login(&app, "owner@example.com").await;

    let (status, _) = patch_account(&app, &a, json!({"email": "new@example.com"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("owner@example.com")
    );

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "new@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "new@example.com");
    assert!(body["pending_email"].is_null());
    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.email.as_deref(), Some("new@example.com"));
    assert!(
        !row.email_verified,
        "an unconfirmed address stays unverified"
    );

    // Other sessions end, the caller's stays.
    assert_eq!(refresh_status(&app, &b).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &a).await, StatusCode::OK);
    assert!(app.email.outbox().await.is_empty());
}

#[tokio::test]
async fn identity_provider_accounts_cannot_edit_email() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "sso@example.com").await;
    // Turn the account into a single-sign-on one, as an OIDC link does.
    let db = &app.state().db;
    let mut am = user_row(&app, a.user_id).await.into_active_model();
    am.external_id = Set("oidc:https://idp.example|sub-1".into());
    am.password_hash = Set(None);
    am.update(db).await.unwrap();

    let (_, me) = get_me(&app, &a).await;
    assert_eq!(me["email_editable"], false);
    for body in [
        json!({"email": "new@example.com"}),
        json!({"email": "new@example.com", "current_password": PASSWORD}),
    ] {
        let (status, resp) = patch_account(&app, &a, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{resp}");
        assert_eq!(resp["error"]["code"], "auth.email_managed_by_issuer");
    }
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("sso@example.com")
    );
    assert!(app.email.outbox().await.is_empty());
}
