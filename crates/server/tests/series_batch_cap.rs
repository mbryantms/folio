//! The 200-per-run cap on a series metadata batch is never silent: the
//! create response and the refresh-status estimate report what the scope
//! covers, what this run takes and what is left, and `scope=all` skips
//! issues searched in the last 24 hours so a second request takes the
//! next chunk instead of the same first 200.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::MockServer;

const CAP: usize = server::metadata::refresh::REFRESH_BATCH_CAP;

async fn register(app: &TestApp, email: &str) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": "correctly-horse-battery"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .filter(|c| c.starts_with("__Host-comic_session=") || c.starts_with("__Host-comic_csrf="))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn send(app: &TestApp, cookie: &str, m: Method, uri: &str) -> (StatusCode, Value) {
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap_or_default()
        .to_owned();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(m)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("X-CSRF-Token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn complete_batch(app: &TestApp, batch_id: &str) {
    app.state()
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE metadata_run SET status = 'completed', finished_at = now() WHERE batch_id = $1",
            [Uuid::parse_str(batch_id).unwrap().into()],
        ))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_scope_reports_the_cap_and_walks_on_after_it() {
    let (cv, metron, gcd) = (
        MockServer::start().await,
        MockServer::start().await,
        MockServer::start().await,
    );
    let app = TestApp::spawn_with_all_providers(cv.uri(), metron.uri(), gcd.uri()).await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib_id, "Big Run").insert(&db).await;
    let total = CAP + 5;
    for n in 1..=total {
        let file = tmp.path().join(format!("big-run-{n:03}.cbz"));
        IssueSeed::new(
            lib_id,
            series_id,
            &file,
            format!("big-run-{n}").as_bytes(),
            n as f64,
        )
        .insert(&db)
        .await;
    }
    let cookie = register(&app, "admin@example.com").await;
    let batch_uri = format!("/api/series/{series_id}/metadata/batch?scope=all");
    let status_uri = format!("/api/series/{series_id}/metadata/refresh-status");

    // Before anything ran: the estimate says what the cap leaves behind.
    let (st, body) = send(&app, &cookie, Method::GET, &status_uri).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let all = &body["fetch_estimate"][0];
    assert_eq!(all["scope"], "all");
    assert_eq!(all["issues"], CAP as i64);
    assert_eq!(all["eligible"], total as i64);
    assert_eq!(all["recently_fetched"], 0);
    assert_eq!(all["remainder"], 5);

    // First click: the first 200 by number, and the response says so.
    let (st, body) = send(&app, &cookie, Method::POST, &batch_uri).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["items_total"], CAP as i64);
    assert_eq!(body["eligible"], total as i64);
    assert_eq!(body["recently_fetched"], 0);
    assert_eq!(body["remainder"], 5);
    let first = body["batch_id"].as_str().unwrap().to_owned();
    complete_batch(&app, &first).await;

    // Second click: the five the cap left, not the same 200 again.
    let (_, body) = send(&app, &cookie, Method::GET, &status_uri).await;
    let all = &body["fetch_estimate"][0];
    assert_eq!(all["issues"], 5);
    assert_eq!(all["recently_fetched"], CAP as i64);
    assert_eq!(all["remainder"], 0);
    let (st, body) = send(&app, &cookie, Method::POST, &batch_uri).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["items_total"], 5);
    assert_eq!(body["recently_fetched"], CAP as i64);
    assert_eq!(body["remainder"], 0);
    let second = body["batch_id"].as_str().unwrap().to_owned();
    complete_batch(&app, &second).await;

    // Third click: everything was fetched in the last 24 h → nothing.
    let (st, body) = send(&app, &cookie, Method::POST, &batch_uri).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["items_total"], 0);
    assert_eq!(body["eligible"], total as i64);
    assert_eq!(body["recently_fetched"], total as i64);
    assert_eq!(body["remainder"], 0);
}
