//! `POST /scans/selection` — bulk rescan from a multi-select.
//!
//! The TestApp router runs no apalis monitor, so queued scans stay queued:
//! the `scan_run` rows and the coalescing outcome are what's asserted.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::scan_run::{self, ScanRunKind};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp, email: &str) -> Authed {
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
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|c| c.trim_start_matches(prefix).to_owned())
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn post(app: &TestApp, auth: &Authed, body: Value) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/scans/selection")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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

#[tokio::test]
async fn queues_series_and_issue_scans_and_folds_covered_issues() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = &app.state().db;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(db).await;
    let s1 = SeriesSeed::new(lib, "Saga").insert(db).await;
    let s2 = SeriesSeed::new(lib, "Paper Girls").insert(db).await;
    let removed = SeriesSeed::new(lib, "Gone").insert(db).await;
    let in_s1 = IssueSeed::new(lib, s1, &dir.path().join("a.cbz"), b"a", 1.0)
        .insert(db)
        .await;
    let in_s2 = IssueSeed::new(lib, s2, &dir.path().join("b.cbz"), b"b", 1.0)
        .insert(db)
        .await;
    let in_removed = IssueSeed::new(lib, removed, &dir.path().join("c.cbz"), b"c", 1.0)
        .insert(db)
        .await;
    let row = entity::series::Entity::find_by_id(removed)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(db).await.unwrap();

    let (st, body) = post(
        &app,
        &admin,
        json!({
            // `in_s1` is covered by the selected s1; `in_removed` drops.
            "series_ids": [s1, s1, removed, Uuid::now_v7()],
            "issue_ids": [in_s1, in_s2, in_removed, "nope"],
        }),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], 2, "{body}");
    assert_eq!(body["coalesced"], 1, "{body}");
    assert_eq!(body["not_found"], 4, "{body}");
    assert_eq!(body["failed"], 0, "{body}");

    let runs = scan_run::Entity::find()
        .filter(scan_run::Column::LibraryId.eq(lib))
        .all(db)
        .await
        .unwrap();
    assert_eq!(runs.len(), 2);
    assert!(
        runs.iter()
            .any(|r| r.kind == ScanRunKind::Series && r.series_id == Some(s1))
    );
    assert!(
        runs.iter()
            .any(|r| r.kind == ScanRunKind::Issue && r.issue_id.as_deref() == Some(in_s2.as_str()))
    );

    // A second click while those are still queued joins them.
    let (st, body) = post(
        &app,
        &admin,
        json!({ "series_ids": [s1], "issue_ids": [in_s2] }),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(body["queued"], 0, "{body}");
    assert_eq!(body["coalesced"], 2, "{body}");

    let audited = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.scan.selection"))
        .all(db)
        .await
        .unwrap();
    assert_eq!(audited.len(), 2);
}

#[tokio::test]
async fn refuses_empty_oversized_and_non_admin() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;

    let (st, _) = post(&app, &admin, json!({})).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    let too_many: Vec<Uuid> = (0..=server::api::scan_selection::SCAN_SELECTION_MAX_SERIES)
        .map(|_| Uuid::now_v7())
        .collect();
    let (st, body) = post(&app, &admin, json!({ "series_ids": too_many })).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["field"], "series_ids");

    let (st, _) = post(&app, &reader, json!({ "series_ids": [Uuid::now_v7()] })).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}
