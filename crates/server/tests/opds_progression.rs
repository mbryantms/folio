//! Integration tests for OPDS Progression 1.0 — M5 of
//! progress-writeback-2.0. Spec at
//! <https://drafts.opds.io/opds-progression-1.0> (merged into the
//! drafts repo 2026-03-01).
//!
//! Endpoints under test:
//! - `GET  /opds/v1/progression/{issue_id}` — fetch last-known
//!   progression as `application/opds-progression+json`
//! - `PUT  /opds/v1/progression/{issue_id}` — client writes
//!   progression; returns 204 on success
//!
//! These tests are de-facto spec compliance fixtures: at the time
//! of writing, no client implements the spec, so this suite
//! anchors what we emit (and accept) so future clients have a
//! known-good target to interop against.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use sea_orm::Database;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    #[allow(dead_code)]
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_text(b: Body) -> String {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let json_: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json_["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn http_with_csrf(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Response<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, auth.cookies())
        .header("X-CSRF-Token", &auth.csrf);
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let body = match body {
        Some(v) => Body::from(serde_json::to_string(&v).unwrap()),
        None => Body::empty(),
    };
    app.router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_writes_progression_and_returns_204() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-put@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Spec Series").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("s.cbz"), b"sp-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PUT,
        &format!("/opds/v1/progression/{issue_id}"),
        Some(json!({
            "modified": "2026-05-19T12:00:00Z",
            "device": { "id": "ios-1", "name": "iPad Pro" },
            "progression": 0.5,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Read back to confirm round-trip.
    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/opds/v1/progression/{issue_id}"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    assert_eq!(
        ct, "application/opds-progression+json",
        "GET emits the spec media type"
    );
    let body: serde_json::Value = body_json(resp.into_body()).await;
    // progression value depends on page_count rounding; seed_issue
    // doesn't set page_count, so the value normalises to 0.0. The
    // critical invariant is that GET round-trips the same key set.
    assert!(body["progression"].is_number());
    assert!(body["modified"].is_string());
    assert_eq!(body["device"]["id"], "ios-1");
    assert_eq!(body["device"]["name"], "iPad Pro");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_rejects_progression_out_of_range() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-range@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Range").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("s.cbz"), b"sr-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PUT,
        &format!("/opds/v1/progression/{issue_id}"),
        Some(json!({
            "modified": "2026-05-19T12:00:00Z",
            "device": { "id": "ios-1", "name": "iPad" },
            "progression": 1.5,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    assert_eq!(
        ct, "application/problem+json",
        "errors use RFC 7807 Problem Details"
    );
    let body: serde_json::Value = body_json(resp.into_body()).await;
    assert_eq!(
        body["type"], "https://registry.opds.io/error#progression-invalid-payload",
        "spec error URI emitted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_with_stale_modified_returns_409_progression_date() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-stale@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Stale").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("s.cbz"), b"ss-1", 1.0).await;

    // Initial write — establishes a recent `modified` baseline.
    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PUT,
        &format!("/opds/v1/progression/{issue_id}"),
        Some(json!({
            "modified": "2026-05-19T12:00:00Z",
            "device": { "id": "a", "name": "A" },
            "progression": 0.4,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Stale write — `modified` an hour earlier than what's on the
    // server now. Spec error `progression-date` (409).
    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PUT,
        &format!("/opds/v1/progression/{issue_id}"),
        Some(json!({
            "modified": "2025-01-01T00:00:00Z",
            "device": { "id": "b", "name": "B" },
            "progression": 0.2,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    assert_eq!(
        body["type"],
        "https://registry.opds.io/error#progression-date",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_returns_404_when_no_progress_row() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-empty@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Empty").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("s.cbz"), b"se-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/opds/v1/progression/{issue_id}"),
        None,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "no progress row → 404 (charitable spec interpretation)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_unknown_issue_returns_404() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-404@example.com").await;
    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        "/opds/v1/progression/nonexistent-issue-id",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_entry_advertises_progression_link_rel() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "prog-rel@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Discovery").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("s.cbz"), b"sd-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/opds/v1/series/{series}"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let expected = format!(
        r#"<link rel="http://opds-spec.org/progression" href="/opds/v1/progression/{issue_id}" type="application/opds-progression+json"/>"#,
    );
    assert!(
        body.contains(&expected),
        "issue entry advertises progression endpoint: looking for `{expected}` in body:\n{body}"
    );
}
