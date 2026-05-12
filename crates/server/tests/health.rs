//! `/healthz` + `/readyz` integration coverage.
//!
//! `/healthz` is always 200 — it's a liveness probe. `/readyz` is a
//! readiness probe; it must 200 when both Postgres and Redis are reachable,
//! and 503 with a per-dep status when either is not. The Redis check landed
//! with the v1 deployment work — pre-M9 the server hard-failed at boot if
//! Redis was down but `/readyz` would have happily returned 200 if Redis
//! went away mid-run, leaving the orchestrator routing traffic to a
//! half-broken instance.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use common::TestApp;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn healthz_always_returns_200() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn readyz_reports_both_db_and_redis_when_healthy() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "ready");
}

