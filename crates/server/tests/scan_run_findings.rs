//! Exact finding counts on scan-run rows (`health_issue_count`) and the
//! `scan_id` filter on `GET /admin/health-issues`.
//!
//! Runs and findings are inserted directly so each run's count is known:
//! the count is the run's open findings (neither resolved nor dismissed),
//! and the filter returns exactly those rows, paginated, with a first-page
//! `total` that respects it.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::library_health_issue::{self, HealthIssueKind};
use entity::scan_run::{self, ScanRunKind, ScanRunState};
use sea_orm::{ActiveModelTrait, Set};
use std::collections::HashMap;
use tower::ServiceExt;
use uuid::Uuid;

async fn register_admin(app: &TestApp) -> String {
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.split(';')
                .next()?
                .strip_prefix("__Host-comic_session=")
                .map(str::to_owned)
        })
        .expect("session cookie")
}

async fn get(app: &TestApp, session: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn insert_run(app: &TestApp, lib: Uuid, minutes_ago: i64) -> Uuid {
    let id = Uuid::now_v7();
    let started = (Utc::now() - Duration::minutes(minutes_ago)).fixed_offset();
    scan_run::ActiveModel {
        id: Set(id),
        library_id: Set(lib),
        state: Set(ScanRunState::Complete),
        started_at: Set(started),
        ended_at: Set(Some(started)),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set(ScanRunKind::Library),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

#[derive(Clone, Copy)]
enum Status {
    Open,
    Resolved,
    Dismissed,
}

async fn insert_finding(app: &TestApp, lib: Uuid, scan: Option<Uuid>, n: u32, status: Status) {
    let now = Utc::now().fixed_offset();
    library_health_issue::ActiveModel {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib),
        scan_id: Set(scan),
        kind: Set(HealthIssueKind::FileAtRoot),
        payload: Set(serde_json::json!({
            "kind": "file_at_root",
            "data": {"path": format!("/lib/stray-{n}-{}.cbz", Uuid::now_v7())}
        })),
        severity: Set("warning".into()),
        fingerprint: Set(Uuid::now_v7().simple().to_string()),
        first_seen_at: Set(now),
        last_seen_at: Set(now - Duration::seconds(i64::from(n))),
        resolved_at: Set(matches!(status, Status::Resolved).then_some(now)),
        dismissed_at: Set(matches!(status, Status::Dismissed).then_some(now)),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

fn counts_by_id(items: &[serde_json::Value]) -> HashMap<String, u64> {
    items
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_owned(),
                r["health_issue_count"]
                    .as_u64()
                    .expect("health_issue_count on every row"),
            )
        })
        .collect()
}

#[tokio::test]
async fn scan_run_rows_carry_their_open_finding_count() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = common::seed::seed_library(&app.state().db, tmp.path()).await;

    let three = insert_run(&app, lib, 30).await;
    let one = insert_run(&app, lib, 20).await;
    let zero = insert_run(&app, lib, 10).await;
    for n in 0..3 {
        insert_finding(&app, lib, Some(three), n, Status::Open).await;
    }
    // Resolved and dismissed findings are not counted.
    insert_finding(&app, lib, Some(three), 10, Status::Resolved).await;
    insert_finding(&app, lib, Some(three), 11, Status::Dismissed).await;
    insert_finding(&app, lib, Some(one), 20, Status::Open).await;
    // Rows with no run never count toward one.
    insert_finding(&app, lib, None, 30, Status::Open).await;

    let want = HashMap::from([
        (three.to_string(), 3u64),
        (one.to_string(), 1),
        (zero.to_string(), 0),
    ]);

    // Per-library history.
    let (status, body) = get(&app, &session, &format!("/api/libraries/{lib}/scan-runs")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(counts_by_id(body["items"].as_array().unwrap()), want);

    // Cross-library history.
    let (status, body) = get(&app, &session, "/api/admin/scan-runs").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(counts_by_id(body["items"].as_array().unwrap()), want);

    // Latest per library: the newest run, which has none.
    let (status, body) = get(&app, &session, "/api/admin/scan-runs/latest-per-library").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let latest = counts_by_id(body.as_array().unwrap());
    assert_eq!(latest, HashMap::from([(zero.to_string(), 0u64)]));

    // The count is what the filtered findings list returns.
    let (status, body) = get(
        &app,
        &session,
        &format!("/api/admin/health-issues?scan_id={three}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert_eq!(body["total"], 3);
}

#[tokio::test]
async fn health_issue_scan_filter_paginates_with_a_scoped_total() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = common::seed::seed_library(&app.state().db, tmp.path()).await;

    let target = insert_run(&app, lib, 20).await;
    let other = insert_run(&app, lib, 10).await;
    for n in 0..5 {
        insert_finding(&app, lib, Some(target), n, Status::Open).await;
    }
    for n in 10..14 {
        insert_finding(&app, lib, Some(other), n, Status::Open).await;
    }
    insert_finding(&app, lib, Some(target), 40, Status::Resolved).await;

    // Unfiltered: every open row, and the total says so.
    let (_, all) = get(&app, &session, "/api/admin/health-issues?limit=50").await;
    assert_eq!(all["total"], 9, "{all}");

    // Filtered, two per page.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut uri = format!("/api/admin/health-issues?scan_id={target}&limit=2");
        if let Some(c) = &cursor {
            uri.push_str(&format!("&cursor={}", urlencoding::encode(c)));
        }
        let (status, body) = get(&app, &session, &uri).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if pages == 0 {
            assert_eq!(body["total"], 5, "first page carries the scoped total");
        } else {
            assert!(body["total"].is_null(), "later pages carry no total");
        }
        for item in body["items"].as_array().unwrap() {
            assert_eq!(item["scan_id"], target.to_string());
            seen.push(item["id"].as_str().unwrap().to_owned());
        }
        pages += 1;
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 5);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 5, "no row repeats across pages");

    // The other status flags still apply on top of the scan filter.
    let (_, with_resolved) = get(
        &app,
        &session,
        &format!("/api/admin/health-issues?scan_id={target}&include_resolved=true"),
    )
    .await;
    assert_eq!(with_resolved["total"], 6);

    // A run with no findings: an empty page, total 0.
    let empty_run = insert_run(&app, lib, 5).await;
    let (_, empty) = get(
        &app,
        &session,
        &format!("/api/admin/health-issues?scan_id={empty_run}"),
    )
    .await;
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
    assert_eq!(empty["total"], 0);
}

#[tokio::test]
async fn malformed_scan_id_is_rejected() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let (status, _) = get(
        &app,
        &session,
        "/api/admin/health-issues?scan_id=not-a-uuid",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
