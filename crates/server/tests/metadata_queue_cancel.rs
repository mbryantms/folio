//! Cancelling provider metadata work: `POST /admin/queue/clear` with
//! `target = metadata` closes the pending runs (and their batches),
//! `POST /admin/metadata/batches/{id}/cancel` does it for one batch, and a
//! job whose run was cancelled does no provider work when it comes up.

mod common;

use apalis::prelude::Data;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::{TestApp, seed};
use entity::{metadata_batch, metadata_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::jobs::metadata_search::{SearchSeriesJob, handle_series};
use server::metadata::matcher::SeriesQueryFacts;
use tower::ServiceExt;
use uuid::Uuid;

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

fn csrf(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned()
}

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf(cookie))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn seed_batch(app: &TestApp, items: i32) -> Uuid {
    let id = Uuid::now_v7();
    metadata_batch::ActiveModel {
        id: Set(id),
        library_id: Set(None),
        scope: Set("series_issues".into()),
        trigger_kind: Set("manual".into()),
        status: Set("running".into()),
        items_total: Set(items),
        created_by: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        ended_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

async fn seed_run(app: &TestApp, batch: Option<Uuid>, status: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(Uuid::now_v7().to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into()]),
        status: Set(status.into()),
        // A second in the past so a "started before the clear" scope sees it.
        started_at: Set(now - chrono::Duration::seconds(1)),
        finished_at: Set((status == "completed").then_some(now)),
        items_total: Set(1),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(batch),
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

async fn run_status(app: &TestApp, id: Uuid) -> String {
    metadata_run::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .status
}

async fn batch_status(app: &TestApp, id: Uuid) -> (String, bool) {
    let b = metadata_batch::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    (b.status, b.ended_at.is_some())
}

#[tokio::test]
async fn clearing_the_metadata_queue_closes_pending_runs_and_their_batches() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let batch = seed_batch(&app, 3).await;
    let queued = seed_run(&app, Some(batch), "queued").await;
    let parked = seed_run(&app, Some(batch), "awaiting_quota").await;
    let done = seed_run(&app, Some(batch), "completed").await;
    let loose = seed_run(&app, None, "searching").await;

    let (st, body) = post(
        &app,
        &admin,
        "/api/admin/queue/clear",
        json!({"target": "metadata"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["cancelled_metadata_runs"], 3, "{body}");
    assert_eq!(body["cancelled_scan_runs"], 0);

    assert_eq!(run_status(&app, queued).await, "cancelled");
    assert_eq!(run_status(&app, parked).await, "cancelled");
    assert_eq!(run_status(&app, loose).await, "cancelled");
    assert_eq!(run_status(&app, done).await, "completed");
    // One member completed: the batch reads as completed, and it is over.
    assert_eq!(
        batch_status(&app, batch).await,
        ("completed".to_owned(), true)
    );
    // The parked run lost its resume.
    let parked_row = metadata_run::Entity::find_by_id(parked)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(parked_row.resume_after.is_none());
    assert_eq!(
        parked_row.error_summary.as_deref(),
        Some("queue cleared by admin")
    );

    // Scans-only clears leave metadata alone.
    let again = seed_run(&app, None, "queued").await;
    let (_, body) = post(
        &app,
        &admin,
        "/api/admin/queue/clear",
        json!({"target": "scans"}),
    )
    .await;
    assert_eq!(body["cancelled_metadata_runs"], 0);
    assert_eq!(run_status(&app, again).await, "queued");
}

#[tokio::test]
async fn cancelling_one_batch_closes_only_its_runs() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let batch = seed_batch(&app, 2).await;
    let mine = seed_run(&app, Some(batch), "queued").await;
    let other_batch = seed_batch(&app, 1).await;
    let theirs = seed_run(&app, Some(other_batch), "queued").await;

    let (st, body) = post(
        &app,
        &admin,
        &format!("/api/admin/metadata/batches/{batch}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["cancelled_runs"], 1);
    assert_eq!(run_status(&app, mine).await, "cancelled");
    assert_eq!(run_status(&app, theirs).await, "queued");
    // Nothing in it completed: the batch is cancelled and over.
    assert_eq!(
        batch_status(&app, batch).await,
        ("cancelled".to_owned(), true)
    );
    assert_eq!(batch_status(&app, other_batch).await.0, "running");

    // Audited.
    let rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.metadata.batch.cancel"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].target_id.as_deref(),
        Some(batch.to_string().as_str())
    );

    // Idempotent; unknown batch 404; members 403.
    let (_, again) = post(
        &app,
        &admin,
        &format!("/api/admin/metadata/batches/{batch}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(again["cancelled_runs"], 0);
    let (st, _) = post(
        &app,
        &admin,
        &format!("/api/admin/metadata/batches/{}/cancel", Uuid::now_v7()),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let member = register(&app, "member@example.com").await;
    let (st, _) = post(
        &app,
        &member,
        &format!("/api/admin/metadata/batches/{batch}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// A job whose run was cancelled before a worker reached it does nothing:
/// no provider work, and the run keeps `cancelled` (it is not failed as
/// "no providers configured" — the guard runs first).
#[tokio::test]
async fn a_job_for_a_cancelled_run_is_a_no_op() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    let series_id = seed::seed_series(db, lib, "Saga").await;
    let run = seed_run(&app, None, "cancelled").await;
    let job = SearchSeriesJob {
        run_id: run,
        series_id,
        library_id: Some(lib),
        facts: SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            volume: None,
            format: None,
        },
        year_asserted: false,
    };
    handle_series(job, Data::new(app.state().clone()))
        .await
        .expect("handler returns Ok");
    assert_eq!(run_status(&app, run).await, "cancelled");
}
