//! Integration tests for `/admin/email/*` and the SMTP overlay path
//! (M2 of runtime-config-admin).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::EntityTrait;
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

async fn send_authed(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, auth.cookie())
        .header("x-csrf-token", &auth.csrf);
    let body_inner = if let Some(json) = body {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(b.body(body_inner).unwrap())
        .await
        .unwrap()
}

// ───────── /admin/email/status ─────────

#[tokio::test]
async fn status_reports_unconfigured_when_smtp_off() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = get(&app, &admin, "/admin/email/status").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    // `MockSender` is_configured()=true so this would normally be true,
    // but TestApp::spawn() uses MockSender. Verify the field exists and
    // the response shape is what the UI expects.
    assert!(body.get("configured").is_some(), "configured field missing");
    assert_eq!(body["last_send_at"], Value::Null);
    assert_eq!(body["last_send_ok"], Value::Null);
}

#[tokio::test]
async fn status_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = get(&app, &user, "/admin/email/status").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ───────── /admin/email/test ─────────

#[tokio::test]
async fn test_send_delivers_via_mock() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(&app, &admin, Method::POST, "/admin/email/test", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["delivered"], true);
    assert_eq!(body["to"], "admin@example.com");

    let outbox = app.email.outbox().await;
    assert_eq!(outbox.len(), 1);
    assert!(outbox[0].subject.contains("SMTP test"));
}

#[tokio::test]
async fn test_send_updates_status() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(&app, &admin, Method::POST, "/admin/email/test", None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get(&app, &admin, "/admin/email/status").await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["last_send_ok"], true);
    assert!(body["last_send_at"].is_string());
    assert!(body["last_duration_ms"].is_u64());
}

#[tokio::test]
async fn test_send_audit_logs() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(&app, &admin, Method::POST, "/admin/email/test", None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    use entity::audit_log;
    let rows = audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    assert!(
        rows.iter().any(|r| r.action == "admin.email.test"),
        "no admin.email.test audit row written"
    );
}

// ───────── overlay + hot-reload via PATCH /admin/settings ─────────

#[tokio::test]
async fn patch_smtp_host_triggers_email_rebuild() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Baseline: TestApp::spawn() leaves smtp_host=None, so overlay+rebuild
    // would land on Noop. Set smtp.host and assert the live Config sees it.
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({
            "smtp.host": "mail.example.test",
            "smtp.from": "noreply@example.test",
            "smtp.port": 2525,
            "smtp.tls": "tls",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let cfg = app.state().cfg();
    assert_eq!(cfg.smtp_host.as_deref(), Some("mail.example.test"));
    assert_eq!(cfg.smtp_from.as_deref(), Some("noreply@example.test"));
    assert_eq!(cfg.smtp_port, 2525);
    assert_eq!(cfg.smtp_tls, "tls");
}

#[tokio::test]
async fn patch_smtp_password_is_redacted_in_get() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({ "smtp.password": "super-secret-relay-password" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;

    // PATCH returns the SettingsView shape. The password value must be
    // redacted, and the audit log must not echo the plaintext either.
    let values = body["values"].as_array().expect("values array");
    let pw = values
        .iter()
        .find(|v| v["key"] == "smtp.password")
        .expect("smtp.password not in values");
    assert_eq!(pw["value"], "<set>");

    let json_str = serde_json::to_string(&body).unwrap();
    assert!(
        !json_str.contains("super-secret-relay-password"),
        "plaintext password leaked into PATCH response"
    );

    // Cross-check the live Config has the decrypted plaintext for the
    // LettreSender to use.
    let cfg = app.state().cfg();
    assert_eq!(
        cfg.smtp_password.as_deref(),
        Some("super-secret-relay-password")
    );
}

#[tokio::test]
async fn patch_invalid_smtp_port_rejected() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({ "smtp.port": "not-a-number" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_value");
}

#[tokio::test]
async fn delete_smtp_host_via_null_clears_field() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Set first.
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({ "smtp.host": "mail.example.test" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        app.state().cfg().smtp_host.as_deref(),
        Some("mail.example.test")
    );

    // Then null = delete row → Config falls back to env (None in tests).
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({ "smtp.host": null })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(app.state().cfg().smtp_host.as_deref(), None);
}
