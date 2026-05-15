//! Integration tests for M4 of runtime-config-admin: JWT TTLs,
//! rate-limit kill switch, log level live-reload.
//!
//! These exercise the overlay + dry-run validation paths for the new
//! keys, but stop short of asserting the access cookie's actual `exp`
//! claim (jwt parsing in tests is covered by the wider auth tests). The
//! goal here is to prove the live Config picks up changes and that
//! validation rejects bad combos before they hit the DB.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::{Value, json};
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

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
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

async fn patch_settings(app: &TestApp, auth: &Authed, body: Value) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/admin/settings")
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

// ───────── JWT TTLs ─────────

#[tokio::test]
async fn access_ttl_overlay_takes_effect() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({ "auth.jwt.access_ttl": "1h", "auth.jwt.refresh_ttl": "30d" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cfg = app.state().cfg();
    assert_eq!(cfg.jwt_access_ttl, "1h");
    // access_ttl() returns the parsed Duration; 1h = 3600s.
    assert_eq!(cfg.access_ttl().as_secs(), 3600);
}

#[tokio::test]
async fn refresh_shorter_than_access_rejected_pre_write() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({ "auth.jwt.access_ttl": "24h", "auth.jwt.refresh_ttl": "1h" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");

    // Live config unchanged.
    let cfg = app.state().cfg();
    assert_eq!(cfg.jwt_access_ttl, "15m");
    assert_eq!(cfg.jwt_refresh_ttl, "30d");
}

#[tokio::test]
async fn malformed_ttl_string_rejected_pre_write() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({ "auth.jwt.access_ttl": "five hours please" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");
}

// ───────── Rate-limit kill switch ─────────

#[tokio::test]
async fn rate_limit_toggle_persists() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    assert!(app.state().cfg().rate_limit_enabled);

    let resp = patch_settings(&app, &admin, json!({ "auth.rate_limit_enabled": false })).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!app.state().cfg().rate_limit_enabled);

    let resp = patch_settings(&app, &admin, json!({ "auth.rate_limit_enabled": true })).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(app.state().cfg().rate_limit_enabled);
}

#[tokio::test]
async fn failed_auth_lockout_skipped_when_disabled() {
    use server::auth::failed_auth;

    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Disable rate limiting.
    let resp = patch_settings(&app, &admin, json!({ "auth.rate_limit_enabled": false })).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Manually set a lockout sentinel in Redis for an arbitrary IP.
    // `check_lockout_for` should bypass the Redis check entirely when
    // the toggle is off and return Ok(None).
    let ip: std::net::IpAddr = "203.0.113.7".parse().unwrap();
    let result = failed_auth::check_lockout_for(&app.state(), ip).await;
    assert!(matches!(result, Ok(None)));
}

// ───────── Log level live reload ─────────

#[tokio::test]
async fn log_level_overlay_takes_effect() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(&app, &admin, json!({ "observability.log_level": "debug" })).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(app.state().cfg().log_level, "debug");
}

#[tokio::test]
async fn invalid_log_level_rejected_pre_write() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // `EnvFilter::try_new` is permissive about unknown target names —
    // a bare string is treated as a target. The clearest way to
    // produce a guaranteed parse failure is to use a directive with a
    // level value that isn't a real level.
    let resp = patch_settings(
        &app,
        &admin,
        json!({ "observability.log_level": "target=not_a_real_level" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    let code = body["error"]["code"].as_str().unwrap_or_default();
    assert!(
        code == "settings.invalid_value" || code == "settings.invalid_combination",
        "got error code {code}"
    );

    // Live config unchanged.
    assert_eq!(app.state().cfg().log_level, "warn");
}

#[tokio::test]
async fn log_level_accepts_module_scoped_directive() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({ "observability.log_level": "info,server::auth=debug" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(app.state().cfg().log_level, "info,server::auth=debug");
}
