//! Observability split M7 — `/admin/scan-batches` read endpoints.
//!
//! Asserts the list returns batches with member-run tallies, and the detail
//! returns member runs + aggregated `ScanStats` totals + the library-event
//! drill-down count.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::LibrarySeed;
use entity::library_event::ActiveModel as EventAM;
use entity::scan_batch::ActiveModel as BatchAM;
use entity::scan_run::ActiveModel as ScanRunAM;
use sea_orm::{ActiveModelTrait, Set};
use server::library::scanner::ScanStats;
use tower::ServiceExt;
use uuid::Uuid;

async fn admin_session(app: &TestApp) -> String {
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
        .find(|c| c.starts_with("__Host-comic_session="))
        .map(|c| c.split(';').next().unwrap().to_owned())
        .expect("session cookie")
}

async fn get_json(app: &TestApp, uri: &str, session: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn scan_batches_list_and_detail() {
    let app = TestApp::spawn().await;
    let session = admin_session(&app).await;
    let db = app.state().db.clone();
    let dir = tempfile::tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;

    // One complete batch with a single completed member run + one event.
    let batch_id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    BatchAM {
        id: Set(batch_id),
        kind: Set("scan_all".into()),
        actor_id: Set(None),
        force: Set(false),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        library_count: Set(1),
        state: Set("complete".into()),
    }
    .insert(&db)
    .await
    .unwrap();

    let run_id = Uuid::now_v7();
    ScanRunAM {
        id: Set(run_id),
        library_id: Set(lib),
        state: Set("complete".into()),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        stats: Set(serde_json::to_value(ScanStats {
            files_added: 3,
            series_created: 1,
            ..Default::default()
        })
        .unwrap()),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(Some(batch_id)),
    }
    .insert(&db)
    .await
    .unwrap();

    EventAM {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib),
        scan_run_id: Set(Some(run_id)),
        batch_id: Set(Some(batch_id)),
        category: Set("issue".into()),
        entity_type: Set(Some("issue".into())),
        entity_id: Set(Some("i1".into())),
        entity_label: Set(Some("Saga #1".into())),
        action: Set("added".into()),
        severity: Set("info".into()),
        summary: Set("Added issue Saga #1".into()),
        detail: Set(None),
        created_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    // List.
    let (status, body) = get_json(&app, "/api/admin/scan-batches", &session).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], batch_id.to_string());
    assert_eq!(items[0]["state"], "complete");
    assert_eq!(items[0]["runs"]["complete"], 1);

    // Detail.
    let (status, body) = get_json(
        &app,
        &format!("/api/admin/scan-batches/{batch_id}"),
        &session,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["id"], batch_id.to_string());
    assert_eq!(body["member_runs"].as_array().unwrap().len(), 1);
    assert_eq!(body["totals"]["files_added"], 3);
    assert_eq!(body["totals"]["series_created"], 1);
    assert_eq!(body["event_count"], 1);

    // Unknown batch → 404.
    let (status, _) = get_json(
        &app,
        &format!("/api/admin/scan-batches/{}", Uuid::now_v7()),
        &session,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
