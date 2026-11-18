//! Series multi-select admin actions: `POST /metadata/coverage/selection`
//! (bulk "Analyze coverage") and `POST /metadata/sync/selection` (bulk
//! pause / resume metadata sync).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
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

async fn post(app: &TestApp, auth: &Authed, path: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
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

/// Two live series and one soft-removed one in a fresh library.
async fn seed(app: &TestApp, root: &std::path::Path) -> (Uuid, Uuid, Uuid) {
    let db = &app.state().db;
    let lib = LibrarySeed::new(root).insert(db).await;
    let a = SeriesSeed::new(lib, "Saga").insert(db).await;
    let b = SeriesSeed::new(lib, "Paper Girls").insert(db).await;
    let removed = SeriesSeed::new(lib, "Gone").insert(db).await;
    let row = entity::series::Entity::find_by_id(removed)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(db).await.unwrap();
    (a, b, removed)
}

async fn paused(app: &TestApp, id: Uuid) -> bool {
    entity::series::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .metadata_sync_paused
}

async fn audit_count(app: &TestApp, action: &str) -> usize {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn coverage_selection_queues_once_per_series_without_auto_accept() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let (a, b, removed) = seed(&app, dir.path()).await;

    let (st, body) = post(
        &app,
        &admin,
        "/api/metadata/coverage/selection",
        json!({ "series_ids": [a, b, a, removed, Uuid::now_v7()] }),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], 2, "{body}");
    assert_eq!(body["already_running"], 0, "{body}");
    assert_eq!(body["not_found"], 2, "{body}");

    let redis = &app.state().jobs.redis;
    for id in [a, b] {
        let rec = server::jobs::provider_coverage::latest_for_series(redis, id)
            .await
            .expect("job record");
        assert!(!rec.auto_accept, "bulk analysis must never auto-accept");
    }

    // Still queued: a second click joins them.
    let (st, body) = post(
        &app,
        &admin,
        "/api/metadata/coverage/selection",
        json!({ "series_ids": [a, b] }),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(body["queued"], 0, "{body}");
    assert_eq!(body["already_running"], 2, "{body}");

    assert_eq!(
        audit_count(&app, "admin.series.provider_coverage_analyze_selection").await,
        2
    );
}

#[tokio::test]
async fn sync_selection_pauses_and_resumes_in_one_update() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let (a, b, removed) = seed(&app, dir.path()).await;
    let db = &app.state().db;

    let (st, body) = post(
        &app,
        &admin,
        "/api/metadata/sync/selection",
        json!({ "series_ids": [a, removed], "paused": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 1, "{body}");
    assert_eq!(body["not_found"], 1, "{body}");
    assert!(paused(&app, a).await);
    assert!(!paused(&app, b).await);
    // A removed series is never touched.
    let gone = entity::series::Entity::find_by_id(removed)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(!gone.metadata_sync_paused);

    // `a` already paused → unchanged; `b` flips.
    let (_, body) = post(
        &app,
        &admin,
        "/api/metadata/sync/selection",
        json!({ "series_ids": [a, b], "paused": true }),
    )
    .await;
    assert_eq!(body["updated"], 1, "{body}");
    assert_eq!(body["unchanged"], 1, "{body}");

    let (_, body) = post(
        &app,
        &admin,
        "/api/metadata/sync/selection",
        json!({ "series_ids": [a, b], "paused": false }),
    )
    .await;
    assert_eq!(body["updated"], 2, "{body}");
    assert!(!paused(&app, a).await && !paused(&app, b).await);

    assert_eq!(
        audit_count(&app, "admin.series.metadata_pause_selection").await,
        2
    );
    assert_eq!(
        audit_count(&app, "admin.series.metadata_resume_selection").await,
        1
    );
}

#[tokio::test]
async fn series_selection_actions_refuse_empty_oversized_and_readers() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;

    for (path, extra) in [
        ("/api/metadata/coverage/selection", json!({})),
        ("/api/metadata/sync/selection", json!({ "paused": true })),
    ] {
        let mut empty = extra.clone();
        empty["series_ids"] = json!([]);
        let (st, body) = post(&app, &admin, path, empty).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
        assert_eq!(body["error"]["details"][0]["field"], "series_ids");

        let mut over = extra.clone();
        over["series_ids"] = json!((0..=100).map(|_| Uuid::now_v7()).collect::<Vec<_>>());
        let (st, _) = post(&app, &admin, path, over).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{path}");

        let mut one = extra.clone();
        one["series_ids"] = json!([Uuid::now_v7()]);
        let (st, _) = post(&app, &reader, path, one).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{path}");
    }
}
