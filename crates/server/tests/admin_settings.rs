//! Integration tests for `/admin/settings` (M1 of runtime-config-admin plan).
//!
//! Covers the M1 surface only:
//!   - Non-admin → 403
//!   - Empty registry: `GET` returns no rows on a fresh install
//!   - `PATCH` with any key returns 400 since the registry is empty
//!   - End-to-end secret roundtrip through `settings::read_all` / `write`
//!     using a stub key directly (bypasses the empty registry)
//!
//! Once M2 adds real keys (`smtp.*`) the same fixtures here will be reused
//! for end-to-end "set then read back" assertions.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::Value;
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

async fn get(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn patch_json(
    app: &TestApp,
    auth: &Authed,
    path: &str,
    body: Value,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

// ───────── HTTP API surface ─────────

#[tokio::test]
async fn settings_get_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = get(&app, &user, "/admin/settings").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn settings_get_returns_registry_and_no_values_on_fresh_install() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = get(&app, &admin, "/admin/settings").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;

    // Registry surface grows milestone-by-milestone; M2 added the SMTP
    // block. Assert the smtp.* keys are present and that no DB rows
    // exist yet on a fresh install.
    let reg = body["registry"].as_array().expect("registry array");
    let keys: Vec<&str> = reg
        .iter()
        .map(|e| e["key"].as_str().expect("key string"))
        .collect();
    for expected in [
        "smtp.host",
        "smtp.port",
        "smtp.tls",
        "smtp.username",
        "smtp.password",
        "smtp.from",
    ] {
        assert!(keys.contains(&expected), "registry missing {expected}");
    }
    assert_eq!(
        body["values"].as_array().unwrap().len(),
        0,
        "fresh TestApp must have no app_setting rows"
    );
}

#[tokio::test]
async fn settings_patch_rejects_unknown_key() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_json(
        &app,
        &admin,
        "/admin/settings",
        serde_json::json!({ "bogus.future.key": "value" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.unknown_key");
}

#[tokio::test]
async fn settings_patch_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = patch_json(
        &app,
        &user,
        "/admin/settings",
        serde_json::json!({ "bogus.future.key": "value" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ───────── settings module ─────────

#[tokio::test]
async fn secret_roundtrips_through_db() {
    use server::settings::{self, crypto};

    let app = TestApp::spawn().await;
    let state = app.state();

    // Seal a value with the live encryption key, write it directly to
    // `app_setting`, then read it back through `settings::read_all` and
    // confirm we get the plaintext.
    let plaintext = "hunter2-extra-special";
    let sealed = crypto::seal(&state.secrets.settings_encryption_key, plaintext.as_bytes())
        .expect("seal");

    use entity::app_setting;
    use sea_orm::{ActiveValue::Set, EntityTrait};
    let am = app_setting::ActiveModel {
        key: Set("smtp.password".into()),
        value: Set(serde_json::to_value(sealed).unwrap()),
        is_secret: Set(true),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
        updated_by: Set(None),
    };
    app_setting::Entity::insert(am)
        .exec(&state.db)
        .await
        .expect("insert");

    let rows = settings::read_all(&state.db, &state.secrets)
        .await
        .expect("read_all");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_secret);
    assert_eq!(rows[0].value, Value::String(plaintext.into()));
}

#[tokio::test]
async fn get_redacts_secret_rows() {
    use server::settings::crypto;

    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let state = app.state();

    // Manually seed a secret row so we exercise the GET-side redaction
    // path even though the M1 registry is empty.
    let sealed = crypto::seal(
        &state.secrets.settings_encryption_key,
        b"super-secret-smtp-password",
    )
    .expect("seal");

    use entity::app_setting;
    use sea_orm::{ActiveValue::Set, EntityTrait};
    let am = app_setting::ActiveModel {
        key: Set("smtp.password".into()),
        value: Set(serde_json::to_value(sealed).unwrap()),
        is_secret: Set(true),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
        updated_by: Set(None),
    };
    app_setting::Entity::insert(am)
        .exec(&state.db)
        .await
        .expect("insert");

    let resp = get(&app, &admin, "/admin/settings").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let values = body["values"].as_array().expect("values");
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["key"], "smtp.password");
    assert_eq!(values[0]["value"], "<set>", "secret must be redacted");
    assert_eq!(values[0]["is_secret"], true);
    // The actual plaintext must NOT appear anywhere in the body.
    let json_str = serde_json::to_string(&body).unwrap();
    assert!(
        !json_str.contains("super-secret-smtp-password"),
        "plaintext leaked into GET response"
    );
}
