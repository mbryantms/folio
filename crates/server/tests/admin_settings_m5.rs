//! Integration tests for M5 of runtime-config-admin: cache + workers
//! operational tuning.
//!
//! M5 settings take effect on **next restart** (apalis pool size + ZIP
//! LRU capacity are fixed at boot). The tests below assert that overlay
//! and dry-run validation work; the live "applies on restart" behaviour
//! is exercised indirectly via the Config snapshot.

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

#[tokio::test]
async fn cache_and_workers_overlay_apply() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({
            "cache.zip_lru_capacity": 256,
            "workers.scan_count": 8,
            "workers.post_scan_count": 4,
            "workers.scan_batch_size": 250,
            "workers.scan_hash_buffer_kb": 2048,
            "workers.archive_work_parallel": 6,
            "workers.thumb_inline_parallel": 12,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cfg = app.state().cfg();
    assert_eq!(cfg.zip_lru_capacity, 256);
    assert_eq!(cfg.scan_worker_count, 8);
    assert_eq!(cfg.post_scan_worker_count, 4);
    assert_eq!(cfg.scan_batch_size, 250);
    assert_eq!(cfg.scan_hash_buffer_kb, 2048);
    assert_eq!(cfg.archive_work_parallel, 6);
    assert_eq!(cfg.thumb_inline_parallel, 12);
}

#[tokio::test]
async fn scan_worker_count_out_of_range_rejected() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(&app, &admin, json!({ "workers.scan_count": 9999 })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");

    // Live config unchanged.
    assert_eq!(app.state().cfg().scan_worker_count, 2);
}

#[tokio::test]
async fn hash_buffer_below_floor_rejected() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(&app, &admin, json!({ "workers.scan_hash_buffer_kb": 32 })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");
}

#[tokio::test]
async fn zip_lru_capacity_zero_rejected() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = patch_settings(&app, &admin, json!({ "cache.zip_lru_capacity": 0 })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");
}

#[tokio::test]
async fn negative_number_rejected_at_type_check() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Negative number fails the registry's Uint type check before the
    // range validation runs.
    let resp = patch_settings(&app, &admin, json!({ "workers.scan_count": -1 })).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_value");
}
