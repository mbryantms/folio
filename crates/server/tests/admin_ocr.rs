//! Integration coverage for `GET /admin/ocr/models`
//! (text-detection-1.0 plan, M5).
//!
//! Tests cover:
//!  - admin-only ACL (RequireAdmin → 403 for non-admins, 401 unauth)
//!  - response shape: the three expected models in stable order,
//!    `total_bytes_on_disk` equals the sum across entries
//!
//! Byte-counting behavior is exercised by the `api::admin_ocr`
//! module-level unit tests against isolated tempdirs — they avoid
//! the process-wide-`HF_HOME` mutation that would race with other
//! tokio tests in this binary.

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

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
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

async fn get_models(app: &TestApp, auth: Option<&Authed>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri("/api/admin/ocr/models");
    if let Some(a) = auth {
        builder = builder.header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                a.session, a.csrf
            ),
        );
    }
    let resp = app
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn unauthenticated_request_is_rejected() {
    let app = TestApp::spawn().await;
    let (status, _) = get_models(&app, None).await;
    // RequireAdmin is layered after auth: no session → 401.
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "expected 401 or 403, got {status}"
    );
}

#[tokio::test]
async fn non_admin_user_gets_403() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (status, body) = get_models(&app, Some(&reader)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "auth.permission_denied");
}

#[tokio::test]
async fn admin_gets_full_model_list() {
    // We deliberately don't touch HF_HOME / TESSDATA_PREFIX here:
    // mutating process-wide env from a tokio test races with other
    // parallel tests in the same binary. The byte-sum behavior is
    // covered by the unit tests in `api::admin_ocr::tests` (which
    // operate on isolated tempdirs without touching env). What
    // this integration test pins is the wiring: admin reaches the
    // handler, the response carries the three expected models in
    // stable order, totals add up, and the env-probe fields are
    // populated.
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-models-admin@example.com").await;
    let (status, body) = get_models(&app, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "got {body}");

    let models = body["models"].as_array().expect("models array");
    assert_eq!(models.len(), 3, "expected 3 models, got {}", models.len());
    assert_eq!(models[0]["id"], "comic-text-detector");
    assert_eq!(models[1]["id"], "manga-ocr");
    assert_eq!(models[2]["id"], "tesseract-eng");

    // Required shape per model.
    for m in models {
        assert!(m["purpose"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(matches!(
            m["kind"].as_str(),
            Some("onnx") | Some("tessdata")
        ));
        assert!(m["cache_dir"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(m["present"].is_boolean());
        assert!(m["bytes_on_disk"].is_u64());
        assert!(m["expected_bytes_approx"].as_u64().unwrap() > 0);
        assert!(m["source"].as_str().is_some_and(|s| !s.is_empty()));
    }

    // `total_bytes_on_disk` == Σ per-model.
    let sum: u64 = models
        .iter()
        .map(|m| m["bytes_on_disk"].as_u64().unwrap())
        .sum();
    assert_eq!(body["total_bytes_on_disk"].as_u64().unwrap(), sum);

    // Env-probe fields are present and non-empty.
    assert!(body["hf_home"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(body["tessdata_dir"].as_str().is_some_and(|s| !s.is_empty()));
}
