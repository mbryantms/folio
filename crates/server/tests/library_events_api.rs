//! Observability split M10 — `GET /admin/library-events` filters + pagination.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::LibrarySeed;
use entity::library_event::ActiveModel as EventAM;
use entity::scan_batch::ActiveModel as BatchAM;
use sea_orm::{ActiveModelTrait, Set};
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

#[allow(clippy::too_many_arguments)]
async fn seed_event(
    db: &sea_orm::DatabaseConnection,
    lib: Uuid,
    batch: Option<Uuid>,
    category: &str,
    action: &str,
    severity: &str,
) {
    EventAM {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib),
        scan_run_id: Set(None),
        batch_id: Set(batch),
        category: Set(category.into()),
        entity_type: Set(None),
        entity_id: Set(None),
        entity_label: Set(None),
        action: Set(action.into()),
        severity: Set(severity.into()),
        summary: Set(format!("{category}/{action}")),
        detail: Set(None),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn get_items(app: &TestApp, uri: &str, session: &str) -> Vec<serde_json::Value> {
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
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["items"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn library_events_filters() {
    let app = TestApp::spawn().await;
    let session = admin_session(&app).await;
    let db = app.state().db.clone();
    let dir = tempfile::tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;
    let batch = Uuid::now_v7();
    // The library_events.batch_id FK requires a real scan_batch row.
    BatchAM {
        id: Set(batch),
        kind: Set("scan_all".into()),
        actor_id: Set(None),
        force: Set(false),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        ended_at: Set(None),
        library_count: Set(1),
        state: Set("running".into()),
    }
    .insert(&db)
    .await
    .unwrap();

    // Mixed events: 2 in the batch (issue/added info, thumbnail/errored warn),
    // 1 outside the batch (series/updated info).
    seed_event(&db, lib, Some(batch), "issue", "added", "info").await;
    seed_event(&db, lib, Some(batch), "thumbnail", "errored", "warning").await;
    seed_event(&db, lib, None, "series", "updated", "info").await;

    // No filter → all 3.
    let all = get_items(&app, "/api/admin/library-events", &session).await;
    assert_eq!(all.len(), 3);
    // Each row carries the resolved library name.
    assert!(all.iter().all(|e| e["library_name"].is_string()));

    // batch_id → only the 2 in the batch.
    let in_batch = get_items(
        &app,
        &format!("/api/admin/library-events?batch_id={batch}"),
        &session,
    )
    .await;
    assert_eq!(in_batch.len(), 2);

    // category filter (csv) → issue + thumbnail = 2.
    let cats = get_items(
        &app,
        "/api/admin/library-events?category=issue,thumbnail",
        &session,
    )
    .await;
    assert_eq!(cats.len(), 2);

    // severity=warning → only the thumbnail/errored row.
    let warns = get_items(&app, "/api/admin/library-events?severity=warning", &session).await;
    assert_eq!(warns.len(), 1);
    assert_eq!(warns[0]["category"], "thumbnail");
}

#[tokio::test]
async fn library_events_cursor_paginates() {
    let app = TestApp::spawn().await;
    let session = admin_session(&app).await;
    let db = app.state().db.clone();
    let dir = tempfile::tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;

    for _ in 0..3 {
        seed_event(&db, lib, None, "issue", "added", "info").await;
    }

    // limit=2 → first page of 2 + a next_cursor; walking yields all 3 with no
    // dupes (the no-silent-truncation invariant).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/library-events?limit=2")
                .header(header::COOKIE, &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let page1: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cursor = page1["next_cursor"].as_str().expect("next_cursor");

    let page2 = get_items(
        &app,
        &format!("/api/admin/library-events?limit=2&cursor={cursor}"),
        &session,
    )
    .await;
    assert_eq!(page2.len(), 1, "second page has the remaining row");
}
