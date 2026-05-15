//! Integration tests for the editable `/admin/auth` surface (M3 of
//! runtime-config-admin). Exercises:
//!   - dry-run validation rejects mode=oidc without OIDC creds (no DB
//!     write occurs)
//!   - happy path: setting all OIDC fields atomically updates Config +
//!     `/auth/config` flips `oidc_enabled` to true
//!   - secret rows are redacted on GET; plaintext never leaves the API
//!   - bool overlay maps registration_open + trust_unverified_email
//!   - POST /admin/auth/oidc/discover validates issuer URL shape and
//!     returns parsed endpoints when fetching against a wiremock OP

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::app_setting;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

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

async fn get(app: &TestApp, auth: &Authed, p: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(p)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_anon(app: &TestApp, p: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(Request::builder().uri(p).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn send_authed(
    app: &TestApp,
    auth: &Authed,
    m: Method,
    p: &str,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(m)
        .uri(p)
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

// ───────── dry-run validation ─────────

#[tokio::test]
async fn switching_to_oidc_without_creds_rejected_pre_write() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // The TestApp baseline is auth_mode=local. Try to flip to oidc
    // *without* providing OIDC creds — validation must reject before
    // any row hits the DB.
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({ "auth.mode": "oidc" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");

    // No DB row should have been written.
    let row = app_setting::Entity::find()
        .filter(app_setting::Column::Key.eq("auth.mode"))
        .one(&app.state().db)
        .await
        .unwrap();
    assert!(
        row.is_none(),
        "dry-run rejection must not persist auth.mode"
    );

    // And the live config still reflects the env baseline.
    assert_eq!(
        app.state().cfg().auth_mode.to_string(),
        "local",
        "Config::replace_cfg must not have fired"
    );
}

#[tokio::test]
async fn switching_to_oidc_with_all_creds_succeeds() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({
            "auth.mode": "both",
            "auth.oidc.issuer": "https://idp.example.test",
            "auth.oidc.client_id": "folio-test",
            "auth.oidc.client_secret": "super-secret-client-secret",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Live config reflects the change.
    let cfg = app.state().cfg();
    assert_eq!(cfg.auth_mode.to_string(), "both");
    assert_eq!(cfg.oidc_issuer.as_deref(), Some("https://idp.example.test"));
    assert_eq!(cfg.oidc_client_id.as_deref(), Some("folio-test"));
    assert_eq!(
        cfg.oidc_client_secret.as_deref(),
        Some("super-secret-client-secret")
    );

    // Public /auth/config reflects oidc_enabled=true.
    let resp = get_anon(&app, "/auth/config").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["auth_mode"], "both");
    assert_eq!(body["oidc_enabled"], true);
}

#[tokio::test]
async fn oidc_client_secret_redacted_in_get() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let plaintext = "rotated-client-secret-do-not-leak";
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({
            "auth.mode": "both",
            "auth.oidc.issuer": "https://idp.example.test",
            "auth.oidc.client_id": "folio-test",
            "auth.oidc.client_secret": plaintext,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get(&app, &admin, "/admin/settings").await;
    let body = body_json(resp.into_body()).await;
    let values = body["values"].as_array().unwrap();
    let secret = values
        .iter()
        .find(|v| v["key"] == "auth.oidc.client_secret")
        .expect("client_secret row missing");
    assert_eq!(secret["value"], "<set>");
    let s = serde_json::to_string(&body).unwrap();
    assert!(
        !s.contains(plaintext),
        "client secret plaintext leaked into GET response"
    );
}

#[tokio::test]
async fn toggling_registration_and_trust_persist() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({
            "auth.local.registration_open": false,
            "auth.oidc.trust_unverified_email": true,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let cfg = app.state().cfg();
    assert!(!cfg.local_registration_open);
    assert!(cfg.oidc_trust_unverified_email);
}

// ───────── /admin/auth/oidc/discover probe ─────────

#[tokio::test]
async fn discover_probe_rejects_bad_url() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/admin/auth/oidc/discover",
        Some(json!({ "issuer": "not-a-url" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "oidc.invalid_issuer");
}

#[tokio::test]
async fn discover_probe_502s_when_unreachable() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Pick a port unlikely to be open. The 5s timeout in the handler
    // ensures we don't hang forever even if something is bound.
    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/admin/auth/oidc/discover",
        Some(json!({ "issuer": "http://127.0.0.1:1" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn discover_probe_parses_endpoints_from_mock_op() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Stand up a minimal wiremock OP that returns a discovery doc.
    let server = MockServer::start().await;
    let issuer = server.uri();
    let doc = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "end_session_endpoint": format!("{issuer}/end_session"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "scopes_supported": ["openid", "email", "profile"],
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(doc))
        .mount(&server)
        .await;

    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/admin/auth/oidc/discover",
        Some(json!({ "issuer": issuer.clone() })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issuer"], issuer.clone());
    assert_eq!(
        body["authorization_endpoint"],
        format!("{issuer}/authorize")
    );
    assert_eq!(
        body["end_session_endpoint"],
        format!("{issuer}/end_session")
    );
    let scopes = body["scopes_supported"].as_array().unwrap();
    assert_eq!(scopes.len(), 3);
}

#[tokio::test]
async fn discover_probe_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &user,
        Method::POST,
        "/admin/auth/oidc/discover",
        Some(json!({ "issuer": "https://idp.example.test" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ───────── existing /admin/auth/config (read-only mirror) still works ─────────

#[tokio::test]
async fn admin_auth_config_reflects_db_overrides() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Baseline: local mode. Flip to both via PATCH and confirm
    // /admin/auth/config sees the new view.
    let resp = send_authed(
        &app,
        &admin,
        Method::PATCH,
        "/admin/settings",
        Some(json!({
            "auth.mode": "both",
            "auth.oidc.issuer": "https://idp.example.test",
            "auth.oidc.client_id": "folio-test",
            "auth.oidc.client_secret": "secret",
            "auth.oidc.trust_unverified_email": true,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get(&app, &admin, "/admin/auth/config").await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["auth_mode"], "both");
    assert_eq!(body["oidc"]["configured"], true);
    assert_eq!(body["oidc"]["issuer"], "https://idp.example.test");
    assert_eq!(body["oidc"]["client_id"], "folio-test");
    assert_eq!(body["oidc"]["trust_unverified_email"], true);
}
