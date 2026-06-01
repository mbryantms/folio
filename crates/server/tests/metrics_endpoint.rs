//! Metrics-observability expansion: `/metrics` exposes the new `folio_*`
//! families (HTTP RED / process / job-queue) and honors the optional
//! `COMIC_METRICS_TOKEN` bearer gate. (M1/M2/M3/M5.)

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::TestApp;
use tower::ServiceExt;

async fn body_text(b: Body) -> String {
    let bytes = axum::body::to_bytes(b, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn scrape(app: &TestApp, bearer: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().method("GET").uri("/metrics");
    if let Some(t) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_exposes_http_process_and_queue_families() {
    let app = TestApp::spawn().await;

    // Exercise a real route so the HTTP middleware records a sample.
    let _ = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Publish the job-queue-depth gauges directly — the scheduler that does
    // this on a 30s timer doesn't run under the bare test router.
    server::jobs::scheduler::refresh_job_queue_depth_gauges(&app.state()).await;

    let (status, body) = scrape(&app, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("folio_http_requests_total"),
        "missing HTTP RED counter:\n{body}"
    );
    assert!(
        body.contains("folio_process_"),
        "missing process metrics (collect() on scrape):\n{body}"
    );
    assert!(
        body.contains("folio_jobs_queue_depth"),
        "missing job-queue depth gauge:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_bearer_gate_enforced_when_token_set() {
    let app = TestApp::spawn_with_metrics_token("s3cret-scrape-token").await;

    let (no_token, _) = scrape(&app, None).await;
    assert_eq!(
        no_token,
        StatusCode::UNAUTHORIZED,
        "missing bearer must be rejected"
    );

    let (wrong_token, _) = scrape(&app, Some("not-the-token")).await;
    assert_eq!(
        wrong_token,
        StatusCode::UNAUTHORIZED,
        "wrong bearer must be rejected"
    );

    let (ok, body) = scrape(&app, Some("s3cret-scrape-token")).await;
    assert_eq!(ok, StatusCode::OK, "correct bearer must be accepted");
    assert!(body.contains("folio_"), "scrape body should expose metrics");
}
