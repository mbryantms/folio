//! Matching-accuracy-1.0 M1 — operator-tunable match thresholds.
//!
//! Anchors the end-to-end overlay path:
//! `metadata.auto_apply_threshold` and `metadata.match_medium_threshold`
//! land in `app_setting` via `PATCH /admin/settings`, the live `Config`
//! reflects the new values, and the matcher's bucket helper buckets
//! against them. Pre-M1 the matcher hardcoded `95 / 70` so the
//! operator-side dial was a no-op; after M1 the dial is reachable.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::{Value, json};
use server::metadata::matcher::{Confidence, Score, Thresholds};
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

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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
                .uri("/api/admin/settings")
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_settings(app: &TestApp, auth: &Authed) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/settings")
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn patch_admin_settings_updates_threshold_overlay() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;

    // Baseline: post-M1 defaults from `Config::default` / test fixture.
    let cfg = app.state().cfg();
    assert_eq!(cfg.metadata_auto_apply_threshold, 80);
    assert_eq!(cfg.metadata_match_medium_threshold, 60);

    let resp = patch_settings(
        &app,
        &admin,
        json!({
            "metadata.auto_apply_threshold": 95,
            "metadata.match_medium_threshold": 75,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let cfg = app.state().cfg();
    assert_eq!(cfg.metadata_auto_apply_threshold, 95);
    assert_eq!(cfg.metadata_match_medium_threshold, 75);

    // The bucket helper now buckets against the new numbers.
    let t = Thresholds::new(
        cfg.metadata_auto_apply_threshold as f32,
        cfg.metadata_match_medium_threshold as f32,
    );
    let s = Score {
        total: 90.0,
        ..Default::default()
    };
    assert_eq!(s.bucket(t), Confidence::Medium);

    let s = Score {
        total: 70.0,
        ..Default::default()
    };
    assert_eq!(s.bucket(t), Confidence::Low);
}

#[tokio::test]
async fn patch_clamps_oversized_threshold_inputs() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({ "metadata.auto_apply_threshold": 9999 }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cfg = app.state().cfg();
    assert_eq!(cfg.metadata_auto_apply_threshold, 100);
}

#[tokio::test]
async fn read_endpoint_surfaces_threshold_values() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;

    let resp = patch_settings(
        &app,
        &admin,
        json!({
            "metadata.auto_apply_threshold": 85,
            "metadata.match_medium_threshold": 65,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get_settings(&app, &admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let values = body["values"].as_array().expect("values array");
    let high = values
        .iter()
        .find(|v| v["key"] == "metadata.auto_apply_threshold")
        .expect("auto_apply_threshold not in values");
    assert_eq!(high["value"].as_u64(), Some(85));
    let med = values
        .iter()
        .find(|v| v["key"] == "metadata.match_medium_threshold")
        .expect("match_medium_threshold not in values");
    assert_eq!(med["value"].as_u64(), Some(65));
}
