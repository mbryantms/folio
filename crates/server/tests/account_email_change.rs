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
//!
//! Also: a pending link dies with a password change, a password reset or
//! sign-out-everywhere; the password branch shares the lockout; at most 5
//! requests an hour; the in-use check runs after the password and ignores
//! case; submitting the current address cancels; the browser opening the
//! link keeps a session only when it is the same user's; audit rows name
//! the addresses.

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
const NEW_PASSWORD: &str = "a-whole-new-passphrase";

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
const CONFIRMED_SIGNED_OUT: &str = "/settings/account?email_change=confirmed_signed_out";

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

    // The audit rows name the addresses: requested, then old → new.
    let requested = audit_payload(&app, "user.account.update").await;
    assert_eq!(requested["email_change"], "requested");
    assert_eq!(requested["pending_email"], "new@example.com");
    let confirmed = audit_payload(&app, "user.account.email_change.confirm").await;
    assert_eq!(confirmed["email"], "new@example.com");
    assert_eq!(confirmed["previous_email"], "owner@example.com");

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
    assert_eq!(open_link(&app, &second, None).await, CONFIRMED_SIGNED_OUT);
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

async fn audit_payload(app: &TestApp, action: &str) -> Value {
    use sea_orm::QueryOrder;
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .order_by_desc(entity::audit_log::Column::CreatedAt)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no {action} audit row"))
        .payload
}

async fn post_authed(app: &TestApp, auth: &Authed, uri: &str, body: Value) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Request a change to `addr` (must succeed) and return the link's token.
async fn request_change(app: &TestApp, auth: &Authed, addr: &str) -> String {
    let (status, body) = patch_account(
        app,
        auth,
        json!({"email": addr, "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pending_email"], addr);
    link_token_sent_to(app, addr).await
}

async fn pending_rows(app: &TestApp, user_id: Uuid) -> usize {
    email_change_request::Entity::find()
        .filter(email_change_request::Column::UserId.eq(user_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

/// The link of a request made before a credential change answers `invalid`,
/// leaves the email alone and its row is gone.
async fn assert_link_void(app: &TestApp, token: &str, user_id: Uuid, password: &str) {
    // The account no longer reports the change as pending (a fresh sign-in,
    // since the credential change ended the old sessions or their tokens).
    let fresh = login_with(app, "owner@example.com", password).await;
    assert!(get_me(app, &fresh).await.1["pending_email"].is_null());
    assert_eq!(open_link(app, token, None).await, INVALID);
    assert_eq!(
        user_row(app, user_id).await.email.as_deref(),
        Some("owner@example.com")
    );
    // The row stays (expired) so its hourly count is kept.
    assert_eq!(pending_rows(app, user_id).await, 1);
    assert!(get_me(app, &fresh).await.1["pending_email"].is_null());
}

async fn login_with(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    authed(resp).await
}

#[tokio::test]
async fn password_change_voids_a_pending_link() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let token = request_change(&app, &a, "new@example.com").await;

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"current_password": PASSWORD, "new_password": "a-whole-new-passphrase"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_link_void(&app, &token, a.user_id, NEW_PASSWORD).await;
}

#[tokio::test]
async fn password_reset_voids_a_pending_link() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let token = request_change(&app, &a, "new@example.com").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/request-password-reset")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": "owner@example.com"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let mail = app
        .email
        .outbox()
        .await
        .into_iter()
        .rev()
        .find(|m| m.to == "owner@example.com" && m.body_text.contains("reset-password?token="))
        .expect("reset email");
    let start =
        mail.body_text.find("reset-password?token=").unwrap() + "reset-password?token=".len();
    let reset_token = mail.body_text[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/reset-password")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"token": reset_token, "new_password": "a-whole-new-passphrase"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "reset: {}", resp.status());
    assert_link_void(&app, &token, a.user_id, NEW_PASSWORD).await;
}

#[tokio::test]
async fn sign_out_everywhere_voids_a_pending_link() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let token = request_change(&app, &a, "new@example.com").await;

    let status = post_authed(&app, &a, "/api/me/sessions/revoke-all", json!({})).await;
    assert!(status.is_success(), "revoke-all: {status}");
    assert_link_void(&app, &token, a.user_id, PASSWORD).await;
}

#[tokio::test]
async fn password_branch_counts_failures_and_shares_the_lockout() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let hash_before = user_row(&app, a.user_id).await.password_hash;
    for _ in 0..server::auth::failed_auth::FAIL_THRESHOLD {
        let (status, body) = patch_account(
            &app,
            &a,
            json!({"current_password": "not-the-password", "new_password": "a-whole-new-passphrase"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "auth.invalid");
        assert_eq!(body["error"]["details"][0]["field"], "current_password");
    }
    // Locked: the right password is refused on both branches.
    for body in [
        json!({"current_password": PASSWORD, "new_password": "a-whole-new-passphrase"}),
        json!({"email": "new@example.com", "current_password": PASSWORD}),
    ] {
        let (status, resp) = patch_account(&app, &a, body).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{resp}");
        assert_eq!(resp["error"]["code"], "auth.locked_out");
    }
    // Neither the password nor the email changed.
    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.password_hash, hash_before);
    assert_eq!(row.email.as_deref(), Some("owner@example.com"));
    assert!(app.email.outbox().await.is_empty());
}

#[tokio::test]
async fn at_most_five_requests_an_hour() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    for n in 1..=5 {
        request_change(&app, &a, &format!("new{n}@example.com")).await;
    }
    assert_eq!(
        app.email.outbox().await.len(),
        10,
        "link + notice per request"
    );

    let sixth = || async {
        app.router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/me/account")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, a.cookies())
                    .header("X-CSRF-Token", &a.csrf)
                    .body(Body::from(
                        json!({"email": "new6@example.com", "current_password": PASSWORD})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    let resp = sixth().await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = resp.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry > 0 && retry <= 3600, "Retry-After {retry}");
    // The rate-limit middleware's envelope, `retry_after_seconds` included.
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "rate_limited");
    assert_eq!(body["error"]["retry_after_seconds"], json!(retry));

    // Cancelling does not reset the count.
    let (status, body) = patch_account(&app, &a, json!({"email": "owner@example.com"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(sixth().await.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(app.email.outbox().await.len(), 10, "nothing more mailed");

    // Once the hour is over, requests work again and the count restarts.
    let db = &app.state().db;
    let row = email_change_request::Entity::find()
        .filter(email_change_request::Column::UserId.eq(a.user_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am = row.into_active_model();
    am.window_started_at = Set((chrono::Utc::now() - chrono::Duration::minutes(61)).fixed_offset());
    am.update(db).await.unwrap();
    assert_eq!(sixth().await.status(), StatusCode::OK);
    let row = email_change_request::Entity::find()
        .filter(email_change_request::Column::UserId.eq(a.user_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.request_count, 1);
}

#[tokio::test]
async fn address_in_use_is_reported_only_after_the_password() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    // A second account holds the address (pending verification with mail on).
    let resp = auth_call(&app, "/auth/local/register", "taken@example.com").await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    app.email.clear().await;

    let (status, body) = patch_account(&app, &a, json!({"email": "taken@example.com"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["field"], "current_password");

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "taken@example.com", "current_password": "not-the-password"}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "TAKEN@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(app.email.outbox().await.is_empty());
}

#[tokio::test]
async fn in_use_check_ignores_case_without_mail() {
    let app = TestApp::spawn().await;
    let other = register(&app, "other@example.com").await;
    // A row stored with capitals (older data, or an admin edit).
    let db = &app.state().db;
    let mut am = user_row(&app, other.user_id).await.into_active_model();
    am.email = Set(Some("Other@Example.com".into()));
    am.update(db).await.unwrap();

    let a = register(&app, "owner@example.com").await;
    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "other@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("owner@example.com")
    );
}

#[tokio::test]
async fn submitting_the_current_address_cancels_a_pending_change() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let token = request_change(&app, &a, "new@example.com").await;

    let (status, body) = patch_account(&app, &a, json!({"email": "Owner@example.com"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["pending_email"].is_null(), "{body}");
    assert!(get_me(&app, &a).await.1["pending_email"].is_null());
    let cancelled = audit_payload(&app, "user.account.update").await;
    assert_eq!(cancelled["email_change"], "cancelled");
    assert_eq!(cancelled["pending_email"], "new@example.com");

    assert_eq!(open_link(&app, &token, None).await, INVALID);
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("owner@example.com")
    );
}

#[tokio::test]
async fn link_opened_without_a_session_signs_every_device_out() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let b = login(&app, "owner@example.com").await;
    let token = request_change(&app, &a, "new@example.com").await;

    assert_eq!(open_link(&app, &token, None).await, CONFIRMED_SIGNED_OUT);
    assert_eq!(refresh_status(&app, &a).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &b).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn link_opened_as_another_user_keeps_only_that_users_session() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    // A second, active account signed in in the browser that opens the link.
    let resp = auth_call(&app, "/auth/local/register", "other@example.com").await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let db = &app.state().db;
    let other = user::Entity::find()
        .filter(user::Column::Email.eq("other@example.com"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am = other.into_active_model();
    am.state = Set(user::UserState::Active);
    am.email_verified = Set(true);
    am.update(db).await.unwrap();
    let x = login(&app, "other@example.com").await;

    let token = request_change(&app, &a, "new@example.com").await;
    assert_eq!(
        open_link(&app, &token, Some(&x)).await,
        CONFIRMED_SIGNED_OUT
    );
    assert_eq!(
        user_row(&app, a.user_id).await.email.as_deref(),
        Some("new@example.com")
    );
    assert_eq!(refresh_status(&app, &a).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &x).await, StatusCode::OK);
}

/// PATCH /me/account as a JSON request, for parallel use.
async fn patch_status(app: &TestApp, auth: &Authed, body: Value) -> StatusCode {
    patch_account(app, auth, body).await.0
}

async fn request_count(app: &TestApp, user_id: Uuid) -> i32 {
    email_change_request::Entity::find()
        .filter(email_change_request::Column::UserId.eq(user_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("request row")
        .request_count
}

/// Fire `n` email-change requests at once; return (accepted, limited).
async fn burst(app: &TestApp, auth: &Authed, tag: &str, n: usize) -> (usize, usize) {
    let statuses = futures::future::join_all((0..n).map(|i| {
        patch_status(
            app,
            auth,
            json!({"email": format!("{tag}{i}@example.com"), "current_password": PASSWORD}),
        )
    }))
    .await;
    for st in &statuses {
        assert!(
            *st == StatusCode::OK || *st == StatusCode::TOO_MANY_REQUESTS,
            "unexpected {st}"
        );
    }
    let ok = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    (ok, n - ok)
}

#[tokio::test]
async fn parallel_requests_are_counted_under_a_lock() {
    // No row yet: the first requests race to create it.
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let (ok, limited) = burst(&app, &a, "fresh", 8).await;
    assert_eq!((ok, limited), (5, 3));
    assert_eq!(request_count(&app, a.user_id).await, 5);
    assert_eq!(app.email.outbox().await.len(), 10, "two mails per accepted");

    // An existing row with two requests in its window.
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    request_change(&app, &a, "one@example.com").await;
    request_change(&app, &a, "two@example.com").await;
    let (ok, limited) = burst(&app, &a, "more", 6).await;
    assert_eq!((ok, limited), (3, 3));
    assert_eq!(request_count(&app, a.user_id).await, 5);
}

async fn audit_rows(app: &TestApp, action: &str) -> Vec<Value> {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.payload)
        .collect()
}

#[tokio::test]
async fn combined_password_and_email_change_over_the_limit_changes_nothing() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let b = login(&app, "owner@example.com").await;
    for n in 1..=5 {
        request_change(&app, &a, &format!("new{n}@example.com")).await;
    }
    let audits_before = audit_rows(&app, "user.account.update").await.len();
    let hash_before = user_row(&app, a.user_id).await.password_hash;

    let (status, body) = patch_account(
        &app,
        &a,
        json!({
            "email": "new6@example.com",
            "current_password": PASSWORD,
            "new_password": NEW_PASSWORD,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");

    let row = user_row(&app, a.user_id).await;
    assert_eq!(row.password_hash, hash_before, "password unchanged");
    assert_eq!(
        refresh_status(&app, &b).await,
        StatusCode::OK,
        "other session alive"
    );
    assert_eq!(
        audit_rows(&app, "user.account.update").await.len(),
        audits_before,
        "nothing audited"
    );
    login_with(&app, "owner@example.com", PASSWORD).await;
}

#[tokio::test]
async fn combined_password_and_email_change_applies_both() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let b = login(&app, "owner@example.com").await;

    let (status, body) = patch_account(
        &app,
        &a,
        json!({
            "email": "new@example.com",
            "current_password": PASSWORD,
            "new_password": NEW_PASSWORD,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pending_email"], "new@example.com");

    assert_eq!(refresh_status(&app, &b).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &a).await, StatusCode::OK);
    assert_eq!(login_status(&app, PASSWORD).await, StatusCode::UNAUTHORIZED);
    login_with(&app, "owner@example.com", NEW_PASSWORD).await;

    let audit = audit_payload(&app, "user.account.update").await;
    assert_eq!(audit["password"], "changed");
    assert_eq!(audit["email_change"], "requested");
    assert_eq!(audit["pending_email"], "new@example.com");

    // The link was made after the password change, so it still works.
    let token = link_token_sent_to(&app, "new@example.com").await;
    assert_eq!(open_link(&app, &token, None).await, CONFIRMED_SIGNED_OUT);
}

async fn login_status(app: &TestApp, password: &str) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": "owner@example.com", "password": password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_lone_current_password_is_ignored() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let hash_before = user_row(&app, a.user_id).await.password_hash;
    // A password manager filled the profile form's password field.
    let (status, body) = patch_account(
        &app,
        &a,
        json!({
            "display_name": "Renamed",
            "email": "owner@example.com",
            "current_password": PASSWORD,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["display_name"], "Renamed");
    assert_eq!(user_row(&app, a.user_id).await.password_hash, hash_before);
}

#[tokio::test]
async fn oidc_only_mode_refuses_the_request_up_front() {
    let app = TestApp::spawn_with_smtp().await;
    let a = register(&app, "owner@example.com").await;
    let mut cfg = (*app.state().cfg()).clone();
    cfg.auth_mode = server::config::AuthMode::Oidc;
    app.state().replace_cfg(cfg);

    let (status, body) = patch_account(
        &app,
        &a,
        json!({"email": "new@example.com", "current_password": PASSWORD}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "auth.email_managed_by_issuer");
    assert!(app.email.outbox().await.is_empty(), "no mail");
    assert_eq!(pending_rows(&app, a.user_id).await, 0);
}
