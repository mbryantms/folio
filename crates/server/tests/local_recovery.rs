//! Local recovery flow integration tests (M4, audit M-1).
//!
//! Exercises the four endpoints + their interaction with the MockSender:
//!   - Register w/ SMTP on → 202 + verify-email captured + state =
//!     pending_verification
//!   - verify-email follows the token and flips the user to active
//!   - request-password-reset captures a reset email + reset-password
//!     succeeds with the captured token and bumps token_version
//!   - reset-password rejects an expired-purpose or tampered token
//!   - resend-verification is a no-op for an already-active user, but
//!     re-issues for a still-pending user

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_json(
    app: &TestApp,
    uri: &str,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Register a throwaway admin so subsequent test registrations follow the
/// non-first-user path (which under `spawn_with_smtp` lands in
/// `pending_verification` and exercises the verify-email flow).
async fn seed_admin(app: &TestApp) {
    let resp = post_json(
        app,
        "/auth/local/register",
        serde_json::json!({
            "email": "admin@example.test",
            "password": "correctly-horse-battery"
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "first-user-admin bootstrap should succeed"
    );
    // Clear the outbox — the admin bootstrap doesn't send (no smtp branch),
    // but a future schema change could; explicit clear keeps subsequent
    // assertions deterministic.
    app.email.clear().await;
}

async fn get(app: &TestApp, uri: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Pull the `?token=...` value out of the email body. Both templates
/// embed the link as plain text plus an HTML anchor; the plain-text
/// version is unambiguous.
fn extract_token(body_text: &str) -> String {
    let needle = "token=";
    let start = body_text.find(needle).expect("email contains token=");
    let rest = &body_text[start + needle.len()..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    rest[..end].to_owned()
}

async fn register_active_user(app: &TestApp, email: &str, password: &str) {
    let resp = post_json(
        app,
        "/auth/local/register",
        serde_json::json!({
            "email": email,
            "password": password
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}

async fn find_user(app: &TestApp, email: &str) -> entity::user::Model {
    entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("user exists")
}

async fn request_reset_token(app: &TestApp, email: &str) -> String {
    app.email.clear().await;
    let resp = post_json(
        app,
        "/auth/local/request-password-reset",
        serde_json::json!({ "email": email }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let outbox = app.email.outbox().await;
    assert_eq!(outbox.len(), 1, "exactly one reset email");
    extract_token(&outbox[0].body_text)
}

async fn login_status(app: &TestApp, email: &str, password: &str) -> StatusCode {
    post_json(
        app,
        "/auth/local/login",
        serde_json::json!({
            "email": email,
            "password": password
        }),
    )
    .await
    .status()
}

#[tokio::test]
async fn register_with_smtp_returns_202_and_sends_verify_email() {
    let app = TestApp::spawn_with_smtp().await;
    seed_admin(&app).await;

    let resp = post_json(
        &app,
        "/auth/local/register",
        serde_json::json!({
            "email": "verify-me@example.com",
            "password": "correctly-horse-battery"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "pending_verification");

    let outbox = app.email.outbox().await;
    assert_eq!(outbox.len(), 1, "expected one email (verify-email)");
    let msg = &outbox[0];
    assert_eq!(msg.to, "verify-me@example.com");
    assert!(msg.subject.contains("Verify"));
    assert!(msg.body_text.contains("/auth/local/verify-email?token="));
}

#[tokio::test]
async fn verify_email_activates_pending_user() {
    let app = TestApp::spawn_with_smtp().await;
    seed_admin(&app).await;
    let resp = post_json(
        &app,
        "/auth/local/register",
        serde_json::json!({
            "email": "click-me@example.com",
            "password": "correctly-horse-battery"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let msg = app.email.last().await;
    let token = extract_token(&msg.body_text);

    let resp = get(&app, &format!("/auth/local/verify-email?token={}", token)).await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "verify-email should 302"
    );
    assert_eq!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/sign-in?verified=1")
    );

    // The user's row in the DB should now be active + email_verified.
    let state = app.state();
    let row = entity::user::Entity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.email.as_deref() == Some("click-me@example.com"))
        .expect("user exists");
    assert_eq!(row.state, "active");
    assert!(row.email_verified);
}

#[tokio::test]
async fn password_reset_round_trip_bumps_token_version() {
    let app = TestApp::spawn().await;

    // Register a normal account (no SMTP, so it activates immediately).
    register_active_user(&app, "reset-me@example.com", "correctly-horse-battery").await;

    // Pre-reset state — record token_version.
    let state = app.state();
    let before = find_user(&app, "reset-me@example.com").await;
    let tv_before = before.token_version;

    let token = request_reset_token(&app, "reset-me@example.com").await;

    let resp = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": token.clone(),
            "new_password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // token_version bumped → existing sessions invalidated.
    let after = entity::user::Entity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.email.as_deref() == Some("reset-me@example.com"))
        .expect("user exists");
    assert_eq!(
        after.token_version,
        tv_before + 1,
        "token_version should bump"
    );

    // Confirmation email landed too.
    let outbox = app.email.outbox().await;
    assert_eq!(outbox.len(), 2, "reset + confirmation");
    assert!(outbox[1].subject.contains("password was changed"));

    // New password works for login.
    let resp = post_json(
        &app,
        "/auth/local/login",
        serde_json::json!({
            "email": "reset-me@example.com",
            "password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Reusing the same reset link must fail and must not change the password.
    let replay = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": token,
            "new_password": "replayed-passphrase"
        }),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);

    let old_password = post_json(
        &app,
        "/auth/local/login",
        serde_json::json!({
            "email": "reset-me@example.com",
            "password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(old_password.status(), StatusCode::OK);

    let replayed_password = post_json(
        &app,
        "/auth/local/login",
        serde_json::json!({
            "email": "reset-me@example.com",
            "password": "replayed-passphrase"
        }),
    )
    .await;
    assert_eq!(replayed_password.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reset_password_rejects_malformed_and_missing_token_rows() {
    let app = TestApp::spawn().await;
    register_active_user(&app, "missing-row@example.com", "correctly-horse-battery").await;

    let malformed = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": "not-a-valid-token",
            "new_password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let user = find_user(&app, "missing-row@example.com").await;
    let token = request_reset_token(&app, "missing-row@example.com").await;
    entity::password_reset_use::Entity::delete_many()
        .filter(entity::password_reset_use::Column::UserId.eq(user.id))
        .exec(&app.state().db)
        .await
        .unwrap();

    let missing = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": token,
            "new_password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        login_status(&app, "missing-row@example.com", "correctly-horse-battery").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn reset_password_rejects_expired_and_wrong_user_token_rows() {
    let app = TestApp::spawn().await;
    register_active_user(&app, "expired-row@example.com", "correctly-horse-battery").await;
    register_active_user(&app, "wrong-row@example.com", "correctly-horse-battery").await;
    register_active_user(&app, "other-user@example.com", "correctly-horse-battery").await;

    let expired_user = find_user(&app, "expired-row@example.com").await;
    let expired_token = request_reset_token(&app, "expired-row@example.com").await;
    let expired_row = entity::password_reset_use::Entity::find()
        .filter(entity::password_reset_use::Column::UserId.eq(expired_user.id))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("reset row exists");
    let mut expired_row = expired_row.into_active_model();
    expired_row.expires_at = Set((Utc::now() - Duration::minutes(1)).fixed_offset());
    expired_row.update(&app.state().db).await.unwrap();

    let expired = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": expired_token,
            "new_password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::BAD_REQUEST);

    let wrong_user = find_user(&app, "wrong-row@example.com").await;
    let other_user = find_user(&app, "other-user@example.com").await;
    let wrong_user_token = request_reset_token(&app, "wrong-row@example.com").await;
    let wrong_row = entity::password_reset_use::Entity::find()
        .filter(entity::password_reset_use::Column::UserId.eq(wrong_user.id))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("reset row exists");
    let mut wrong_row = wrong_row.into_active_model();
    wrong_row.user_id = Set(other_user.id);
    wrong_row.update(&app.state().db).await.unwrap();

    let wrong = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": wrong_user_token,
            "new_password": "wrong-row-new-passphrase"
        }),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        login_status(&app, "wrong-row@example.com", "correctly-horse-battery").await,
        StatusCode::OK
    );
    assert_eq!(
        login_status(&app, "wrong-row@example.com", "wrong-row-new-passphrase").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reset_password_submissions_consume_token_once() {
    let app = TestApp::spawn().await;
    register_active_user(&app, "reset-race@example.com", "correctly-horse-battery").await;
    let before = find_user(&app, "reset-race@example.com").await;
    let token = request_reset_token(&app, "reset-race@example.com").await;

    let attempts = (0..4).map(|idx| {
        let app = &app;
        let token = token.clone();
        async move {
            let password = format!("concurrent-passphrase-{idx}");
            let resp = post_json(
                app,
                "/auth/local/reset-password",
                serde_json::json!({
                    "token": token,
                    "new_password": password
                }),
            )
            .await;
            (password, resp.status())
        }
    });
    let results = futures::future::join_all(attempts).await;
    let winners: Vec<_> = results
        .iter()
        .filter(|(_, status)| *status == StatusCode::NO_CONTENT)
        .collect();
    assert_eq!(winners.len(), 1, "exactly one concurrent reset succeeds");

    let after = find_user(&app, "reset-race@example.com").await;
    assert_eq!(
        after.token_version,
        before.token_version + 1,
        "token_version should bump exactly once"
    );

    let winning_password = winners[0].0.as_str();
    assert_eq!(
        login_status(&app, "reset-race@example.com", winning_password).await,
        StatusCode::OK
    );
    for (password, status) in results {
        if status != StatusCode::NO_CONTENT {
            assert_eq!(
                login_status(&app, "reset-race@example.com", &password).await,
                StatusCode::UNAUTHORIZED
            );
        }
    }
}

#[tokio::test]
async fn request_password_reset_does_not_leak_unknown_email() {
    let app = TestApp::spawn().await;
    let resp = post_json(
        &app,
        "/auth/local/request-password-reset",
        serde_json::json!({ "email": "nobody@example.com" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(app.email.outbox().await.is_empty());
}

#[tokio::test]
async fn reset_password_rejects_wrong_purpose_token() {
    let app = TestApp::spawn_with_smtp().await;
    seed_admin(&app).await;
    // Get a verify-email token.
    let resp = post_json(
        &app,
        "/auth/local/register",
        serde_json::json!({
            "email": "wrong-purpose@example.com",
            "password": "correctly-horse-battery"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let token = extract_token(&app.email.last().await.body_text);

    // Submit the verify-email token to reset-password — should 400.
    let resp = post_json(
        &app,
        "/auth/local/reset-password",
        serde_json::json!({
            "token": token,
            "new_password": "much-better-passphrase"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.token_invalid");
}

#[tokio::test]
async fn resend_verification_only_sends_when_pending() {
    let app = TestApp::spawn_with_smtp().await;
    seed_admin(&app).await;
    let resp = post_json(
        &app,
        "/auth/local/register",
        serde_json::json!({
            "email": "pending@example.com",
            "password": "correctly-horse-battery"
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let pre = app.email.outbox().await.len();
    assert_eq!(pre, 1, "one welcome email");

    // Resend → another verify email.
    let resp = post_json(
        &app,
        "/auth/local/resend-verification",
        serde_json::json!({ "email": "pending@example.com" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.email.outbox().await.len(), 2);

    // Verify the user to flip state to active.
    let token = extract_token(&app.email.last().await.body_text);
    let resp = get(&app, &format!("/auth/local/verify-email?token={}", token)).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Now resend should NOT send anything — user is active.
    let count_before = app.email.outbox().await.len();
    let resp = post_json(
        &app,
        "/auth/local/resend-verification",
        serde_json::json!({ "email": "pending@example.com" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.email.outbox().await.len(),
        count_before,
        "no new email for already-active user"
    );
}
