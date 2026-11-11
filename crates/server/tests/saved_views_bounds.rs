//! Saved-view filter bounds and the filter time box.
//!
//! - Size bounds (`views::dsl::MAX_FILTER_*`): create, update and both
//!   previews refuse a filter over them with a 422 whose `error.details`
//!   names the field; exactly at the cap is accepted. A stored view over
//!   the caps (written before they existed) still loads and runs.
//! - Time box (`views::timebox`): a filter query that overruns the
//!   statement timeout answers 422 `filter_too_slow` (OPDS serves the
//!   feed empty), and `SET LOCAL` never leaks the timeout onto the pooled
//!   connection. The HTTP cases shorten the timeout through the harness
//!   (`spawn_with_filter_statement_timeout`) and hold an ACCESS EXCLUSIVE
//!   lock on `series` from a second connection, so the query waits on the
//!   lock until Postgres cancels it: deterministic, no slow query, no 5 s
//!   sleep.

mod common;

use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use server::views::dsl::{MAX_FILTER_CONDITIONS, MAX_FILTER_LIST_VALUES, MAX_FILTER_VALUE_LEN};
use server::views::timebox::{
    self, FILTER_TOO_SLOW_CODE, FILTER_TOO_SLOW_MESSAGE, FilterQueryError,
};
use tower::ServiceExt;
use uuid::Uuid;

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
            .find_map(|c| {
                c.split(';')
                    .next()
                    .and_then(|kv| kv.strip_prefix(prefix))
                    .map(str::to_owned)
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http_raw(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, String) {
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
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, text) = http_raw(app, method, uri, auth, body).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// `n` cheap text conditions.
fn conditions(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| json!({"field": "name", "op": "contains", "value": format!("x{i}")}))
            .collect(),
    )
}

/// One `genres includes_any` condition listing `n` values.
fn list_condition(n: usize) -> Value {
    let values: Vec<String> = (0..n).map(|i| format!("g{i}")).collect();
    json!([{ "field": "genres", "op": "includes_any", "value": values }])
}

fn create_body(conds: Value) -> Value {
    json!({
        "kind": "filter_series",
        "name": "Bounded",
        "filter": { "match_mode": "any", "conditions": conds },
        "sort_field": "name",
        "sort_order": "asc",
        "result_limit": 12,
    })
}

fn preview_body(conds: Value) -> Value {
    json!({
        "filter": { "match_mode": "any", "conditions": conds },
        "sort_field": "created_at",
        "sort_order": "desc",
        "result_limit": 12,
    })
}

/// 422 `validation` with an `error.details` entry for `field`.
fn assert_field_error(status: StatusCode, body: &Value, field: &str) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["error"]["code"], "validation", "{body:#}");
    let details = body["error"]["details"].as_array().expect("details");
    assert!(
        details
            .iter()
            .any(|d| d["field"] == field && d["message"].as_str().is_some_and(|m| !m.is_empty())),
        "no detail for {field}: {body:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn condition_count_is_bounded_on_every_write_and_preview() {
    let app = TestApp::spawn().await;
    let a = register(&app, "bounds@example.com").await;
    let over = conditions(MAX_FILTER_CONDITIONS + 1);
    let at = conditions(MAX_FILTER_CONDITIONS);

    // Create.
    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(over.clone())),
    )
    .await;
    assert_field_error(s, &b, "filter.conditions");
    let (s, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(at.clone())),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{view:#}");
    let id = view["id"].as_str().unwrap().to_owned();

    // Update.
    let uri = format!("/api/me/saved-views/{id}");
    let patch = |c: Value| json!({ "filter": { "match_mode": "all", "conditions": c } });
    let (s, b) = http(&app, Method::PATCH, &uri, &a, Some(patch(over.clone()))).await;
    assert_field_error(s, &b, "filter.conditions");
    let (s, b) = http(&app, Method::PATCH, &uri, &a, Some(patch(at.clone()))).await;
    assert_eq!(s, StatusCode::OK, "{b:#}");

    // Both previews.
    for path in [
        "/api/me/saved-views/preview",
        "/api/me/saved-views/preview-issues",
    ] {
        let (s, b) = http(
            &app,
            Method::POST,
            path,
            &a,
            Some(preview_body(over.clone())),
        )
        .await;
        assert_field_error(s, &b, "filter.conditions");
        let (s, b) = http(&app, Method::POST, path, &a, Some(preview_body(at.clone()))).await;
        assert_eq!(s, StatusCode::OK, "{path}: {b:#}");
    }

    // The cross-field rules still land in the same report: a blank name
    // and an oversize filter come back together.
    let mut body = create_body(over);
    body["name"] = json!("  ");
    let (s, b) = http(&app, Method::POST, "/api/me/saved-views", &a, Some(body)).await;
    assert_field_error(s, &b, "name");
    assert_field_error(s, &b, "filter.conditions");

    // Malformed JSON stays a 400.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/saved-views/preview")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                a.session, a.csrf
            ),
        )
        .header("X-CSRF-Token", &a.csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_values_and_value_length_are_bounded() {
    let app = TestApp::spawn().await;
    let a = register(&app, "values@example.com").await;
    let field = "filter.conditions[0].value";

    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(list_condition(MAX_FILTER_LIST_VALUES + 1))),
    )
    .await;
    assert_field_error(s, &b, field);
    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(list_condition(MAX_FILTER_LIST_VALUES))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b:#}");

    for path in [
        "/api/me/saved-views/preview",
        "/api/me/saved-views/preview-issues",
    ] {
        let (s, b) = http(
            &app,
            Method::POST,
            path,
            &a,
            Some(preview_body(list_condition(MAX_FILTER_LIST_VALUES + 1))),
        )
        .await;
        assert_field_error(s, &b, field);
        let (s, b) = http(
            &app,
            Method::POST,
            path,
            &a,
            Some(preview_body(list_condition(MAX_FILTER_LIST_VALUES))),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{path}: {b:#}");
    }

    // One string value over the byte cap; exactly at it is fine.
    let long = |n: usize| json!([{ "field": "name", "op": "contains", "value": "a".repeat(n) }]);
    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views/preview",
        &a,
        Some(preview_body(long(MAX_FILTER_VALUE_LEN + 1))),
    )
    .await;
    assert_field_error(s, &b, field);
    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views/preview",
        &a,
        Some(preview_body(long(MAX_FILTER_VALUE_LEN))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b:#}");
}

/// A view stored before the bounds existed is not rejected on read: it
/// loads in the list and its results run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_view_over_the_bounds_still_loads_and_runs() {
    let app = TestApp::spawn().await;
    let a = register(&app, "stored@example.com").await;
    let (s, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(conditions(1))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{view:#}");
    let id = Uuid::parse_str(view["id"].as_str().unwrap()).unwrap();

    // Write 60 conditions straight into the row, as an old view would hold.
    let db = Database::connect(&app.db_url).await.unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE saved_views SET conditions = $1 WHERE id = $2",
        [conditions(60).into(), id.into()],
    ))
    .await
    .unwrap();

    let (s, list) = http(&app, Method::GET, "/api/me/saved-views", &a, None).await;
    assert_eq!(s, StatusCode::OK);
    let stored = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == id.to_string())
        .expect("stored view listed");
    assert_eq!(stored["conditions"].as_array().unwrap().len(), 60);

    let (s, b) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/results"),
        &a,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b:#}");

    // A PATCH that leaves the filter alone (a rename) is not blocked by it.
    let (s, b) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/saved-views/{id}"),
        &a,
        Some(json!({ "name": "Renamed" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b:#}");
}

/// Hold an ACCESS EXCLUSIVE lock on `series` until the returned
/// transaction is dropped / rolled back: every filter query reads
/// `series`, so it waits on the lock until the statement timeout fires.
async fn lock_series(app: &TestApp) -> sea_orm::DatabaseTransaction {
    let db = Database::connect(&app.db_url).await.unwrap();
    let txn = db.begin().await.unwrap();
    txn.execute_unprepared("LOCK TABLE series IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    txn
}

fn assert_too_slow(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
    assert_eq!(body["error"]["code"], FILTER_TOO_SLOW_CODE, "{body:#}");
    assert_eq!(
        body["error"]["message"], FILTER_TOO_SLOW_MESSAGE,
        "{body:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_over_the_time_box_answers_422_and_opds_serves_empty() {
    let app = TestApp::spawn_with_filter_statement_timeout(Duration::from_millis(200)).await;
    let a = register(&app, "slow@example.com").await;
    let (s, series_view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(create_body(conditions(1))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let series_id = series_view["id"].as_str().unwrap().to_owned();
    let mut issue_body = create_body(conditions(1));
    issue_body["kind"] = json!("filter_issues");
    let (s, issue_view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &a,
        Some(issue_body),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{issue_view:#}");
    let issue_id = issue_view["id"].as_str().unwrap().to_owned();

    let lock = lock_series(&app).await;

    for path in [
        "/api/me/saved-views/preview",
        "/api/me/saved-views/preview-issues",
    ] {
        let (s, b) = http(
            &app,
            Method::POST,
            path,
            &a,
            Some(preview_body(conditions(1))),
        )
        .await;
        assert_too_slow(s, &b);
    }
    let (s, b) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{series_id}/results"),
        &a,
        None,
    )
    .await;
    assert_too_slow(s, &b);
    // First page: the rows and the `total` count share one time box.
    let (s, b) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{issue_id}/issue-results"),
        &a,
        None,
    )
    .await;
    assert_too_slow(s, &b);

    // OPDS has no error UI a reader would show: the feed is served empty.
    let (s, feed) = http_raw(
        &app,
        Method::GET,
        &format!("/opds/v1/views/{series_id}"),
        &a,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{feed}");
    assert!(!feed.contains("<entry>"), "{feed}");
    let (s, feed) = http(
        &app,
        Method::GET,
        &format!("/opds/v2/views/{issue_id}"),
        &a,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{feed:#}");
    assert_eq!(
        feed["publications"].as_array().map(Vec::len),
        Some(0),
        "{feed:#}"
    );

    lock.rollback().await.unwrap();

    // With the lock gone the same requests run normally.
    let (s, b) = http(
        &app,
        Method::POST,
        "/api/me/saved-views/preview",
        &a,
        Some(preview_body(conditions(1))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b:#}");
    let (s, b) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{issue_id}/issue-results"),
        &a,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b:#}");
    assert!(
        b["total"].is_number(),
        "first page still carries total: {b:#}"
    );
}

/// The helper itself, on a one-connection pool so the "next checkout" is
/// provably the connection that just timed out: the overrun is reported
/// as `TimedOut`, `SET LOCAL` did not leak onto the connection, and the
/// transaction is read-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn time_box_cancels_without_leaking_onto_the_pool() {
    let app = TestApp::spawn().await;
    let mut opts = ConnectOptions::new(app.db_url.clone());
    opts.max_connections(1)
        .min_connections(1)
        .sqlx_logging(false);
    let db = Database::connect(opts).await.unwrap();
    let show = |db: sea_orm::DatabaseConnection| async move {
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SHOW statement_timeout",
            ))
            .await
            .unwrap()
            .unwrap();
        row.try_get::<String>("", "statement_timeout").unwrap()
    };
    let before = show(db.clone()).await;
    assert_ne!(before, "100ms");

    let sleep = Statement::from_string(DbBackend::Postgres, "SELECT pg_sleep(1)");
    let started = std::time::Instant::now();
    let err = timebox::query_one_statement(&db, Duration::from_millis(100), sleep)
        .await
        .expect_err("pg_sleep(1) overruns a 100 ms box");
    assert!(matches!(err, FilterQueryError::TimedOut), "{err}");
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "cancelled early"
    );

    // Same (only) connection, next checkout: no timeout left behind.
    assert_eq!(show(db.clone()).await, before);

    // A statement that finishes in time returns its rows, and also leaves
    // nothing behind.
    let ok = Statement::from_string(DbBackend::Postgres, "SELECT 1 AS one");
    let rows = timebox::query_one_statement(&db, Duration::from_millis(100), ok)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(show(db.clone()).await, before);

    // Read-only: a write inside the box is refused (and is not a timeout).
    let write = Statement::from_string(
        DbBackend::Postgres,
        "CREATE TEMP TABLE timebox_probe (x int)",
    );
    let err = timebox::query_one_statement(&db, Duration::from_secs(5), write)
        .await
        .expect_err("read-only transaction refuses writes");
    assert!(matches!(err, FilterQueryError::Db(_)), "{err}");
    // The pool is still usable afterwards.
    assert_eq!(show(db).await, before);
}
