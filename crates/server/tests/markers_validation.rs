//! Roadmap WP-5.3 (audit UX-12, SE-5) — marker request validation,
//! editable captured text, and the marker-write rate limit.
//!
//! Create and update take `Validated<T>`: field-level failures are 422s in
//! the canonical envelope with `error.details = [{field, message}]`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{SeriesSeed, seed_issue, seed_library};
use sea_orm::Database;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Authed {
    session: String,
    csrf: String,
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
    Authed {
        session: extract_cookie(&resp, "__Host-comic_session"),
        csrf: extract_cookie(&resp, "__Host-comic_csrf"),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let req = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// First registered user is the admin, so no grant is needed.
async fn setup() -> (TestApp, Authed, String, tempfile::TempDir) {
    let app = TestApp::spawn().await;
    let auth = register(&app, "validator@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = SeriesSeed::new(lib, "Validated").insert(&db).await;
    let issue = seed_issue(&db, lib, series, &tmp.path().join("v.cbz"), b"v", 1.0).await;
    (app, auth, issue, tmp)
}

/// Assert a 422 validation envelope whose `details` names `field`.
fn assert_field_error(status: StatusCode, body: &Value, field: &str) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body:#}");
    assert_eq!(body["error"]["code"], "validation", "body: {body:#}");
    let fields: Vec<&str> = body["error"]["details"]
        .as_array()
        .unwrap_or_else(|| panic!("expected details: {body:#}"))
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    assert!(fields.contains(&field), "expected {field} in {fields:?}");
}

fn highlight(issue: &str, region: Value) -> Value {
    json!({
        "issue_id": issue,
        "page_index": 0,
        "kind": "highlight",
        "region": region,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_rejects_bad_fields_with_field_details() {
    let (app, auth, issue, _tmp) = setup().await;
    let post = |body: Value| http(&app, Method::POST, "/api/me/markers", &auth, Some(body));

    let (s, b) = post(
        json!({"issue_id": issue, "page_index": 0, "kind": "bookmark", "color": "x".repeat(33)}),
    )
    .await;
    assert_field_error(s, &b, "color");

    let (s, b) = post(json!({"issue_id": issue, "page_index": -1, "kind": "bookmark"})).await;
    assert_field_error(s, &b, "page_index");

    let (s, b) = post(json!({"issue_id": issue, "page_index": 0, "kind": "scribble"})).await;
    assert_field_error(s, &b, "kind");

    let (s, b) = post(json!({"issue_id": "", "page_index": 0, "kind": "bookmark"})).await;
    assert_field_error(s, &b, "issue_id");

    let (s, b) = post(json!({"issue_id": issue, "page_index": 0, "kind": "note", "body": "x".repeat(10 * 1024 + 1)})).await;
    assert_field_error(s, &b, "body");

    let (s, b) = post(json!({
        "issue_id": issue, "page_index": 0, "kind": "bookmark",
        "selection": {"text": "x".repeat(8 * 1024 + 1)},
    }))
    .await;
    assert_field_error(s, &b, "selection");

    let (s, b) = post(json!({
        "issue_id": issue, "page_index": 0, "kind": "bookmark",
        "selection": {"text": 42},
    }))
    .await;
    assert_field_error(s, &b, "selection");

    // Region smaller than 0.5% on either side — including one that only
    // becomes degenerate after clamping (negative width).
    for region in [
        json!({"x": 10, "y": 10, "w": 0.4, "h": 20}),
        json!({"x": 10, "y": 10, "w": 20, "h": 0}),
        json!({"x": 10, "y": 10, "w": -5, "h": 20}),
        json!({"x": 10, "y": 10, "w": 20}),
        json!({"x": 10, "y": 10, "w": 20, "h": 20, "shape": "blob"}),
    ] {
        let (s, b) = post(highlight(&issue, region.clone())).await;
        assert_field_error(s, &b, "region");
    }

    // Boundaries are inclusive and the old ~1.1 KB selection cap is gone.
    let (s, b) = post(json!({
        "issue_id": issue, "page_index": 0, "kind": "highlight",
        "region": {"x": 10, "y": 10, "w": 0.5, "h": 0.5},
        "color": "y".repeat(32),
        "selection": {"text": "é".repeat(4 * 1024)},
    }))
    .await;
    assert_eq!(s, StatusCode::CREATED, "body: {b:#}");
    assert_eq!(b["selection"]["text"].as_str().unwrap().len(), 8 * 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_rejects_bad_fields_with_field_details() {
    let (app, auth, issue, _tmp) = setup().await;
    let (s, m) = http(
        &app,
        Method::POST,
        "/api/me/markers",
        &auth,
        Some(highlight(&issue, json!({"x": 1, "y": 1, "w": 10, "h": 10}))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let url = format!("/api/me/markers/{}", m["id"].as_str().unwrap());
    let patch = |body: Value| http(&app, Method::PATCH, &url, &auth, Some(body));

    let (s, b) = patch(json!({"color": "x".repeat(33)})).await;
    assert_field_error(s, &b, "color");
    let (s, b) = patch(json!({"body": "x".repeat(10 * 1024 + 1)})).await;
    assert_field_error(s, &b, "body");
    let (s, b) = patch(json!({"region": {"x": 1, "y": 1, "w": 10, "h": 0.2}})).await;
    assert_field_error(s, &b, "region");
    let (s, b) = patch(json!({"selection": {"text": "x".repeat(8 * 1024 + 1)}})).await;
    assert_field_error(s, &b, "selection");

    // Clearing stays allowed.
    let (s, b) = patch(json!({"color": null, "selection": null})).await;
    assert_eq!(s, StatusCode::OK, "body: {b:#}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_text_is_editable_and_round_trips() {
    let (app, auth, issue, _tmp) = setup().await;
    let (s, m) = http(
        &app,
        Method::POST,
        "/api/me/markers",
        &auth,
        Some(json!({
            "issue_id": issue,
            "page_index": 0,
            "kind": "highlight",
            "region": {"x": 5, "y": 5, "w": 30, "h": 10, "shape": "text"},
            "selection": {"text": "WE ARE ALL B0RN", "ocr_confidence": 0.61, "image_hash": "abc"},
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{m:#}");
    let id = m["id"].as_str().unwrap().to_owned();

    // The editor sends the whole selection back with only `text` changed.
    let fixed = "We are all born\nof the stars.";
    let (s, u) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/markers/{id}"),
        &auth,
        Some(json!({"selection": {"text": fixed, "ocr_confidence": 0.61, "image_hash": "abc"}})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{u:#}");
    assert_eq!(u["selection"]["text"], fixed);

    let (s, list) = http(
        &app,
        Method::GET,
        &format!("/api/me/issues/{issue}/markers"),
        &auth,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let row = &list["items"][0];
    assert_eq!(row["selection"]["text"], fixed);
    assert_eq!(row["selection"]["image_hash"], "abc");
    // Region untouched by a selection-only patch.
    assert_eq!(row["region"]["w"], 30.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marker_writes_share_a_rate_limit_bucket() {
    let (app, auth, _issue, _tmp) = setup().await;
    // An empty bulk delete is the cheapest write (no marker query), so it
    // drains the bucket quickly. Burst is 600; the bucket refills at
    // 10/s while the loop runs, so the 429 lands somewhere after that.
    let mut allowed = 0;
    let mut limited = None;
    for _ in 0..3000 {
        let (s, b) = http(
            &app,
            Method::POST,
            "/api/me/markers/bulk-delete",
            &auth,
            Some(json!({"marker_ids": []})),
        )
        .await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(b);
            break;
        }
        assert_eq!(s, StatusCode::OK);
        allowed += 1;
    }
    let body = limited.expect("marker write bucket never tripped");
    assert!(allowed >= 600, "tripped after only {allowed} writes");
    assert_eq!(body["error"]["code"], "rate_limited");

    // The bucket is shared by every write route…
    let (s, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/markers/{}", uuid::Uuid::now_v7()),
        &auth,
        Some(json!({"color": "red"})),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    // …and does not throttle reads.
    let (s, _) = http(&app, Method::GET, "/api/me/markers", &auth, None).await;
    assert_eq!(s, StatusCode::OK);
}
