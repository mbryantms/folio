//! API decision-logic tests for `/series/{slug}/metadata/*`
//! (metadata-providers-1.0 M3).
//!
//! Scope: the HTTP layer — slug → entity resolution, ACL, the
//! providers-configured / coalescing / polling response shapes. The
//! orchestrator's fan-out behavior is covered by
//! `tests/metadata_orchestrator.rs` (which spins up wiremock servers
//! and drives the search directly). The apalis worker isn't running
//! in these tests, so POST handlers leave a `queued` row + a pushed
//! job that no worker dequeues; the GET polling tests insert
//! `completed` runs + candidate rows directly so the response shape
//! gets exercised end-to-end.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::{Value, json};
use std::path::Path;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn get(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn seed_series_in_library(app: &TestApp, root: &Path) -> (Uuid, Uuid) {
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(root).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .with_publisher("Image Comics")
        .insert(db)
        .await;
    (lib_id, series_id)
}

#[tokio::test]
async fn search_series_400_when_no_providers_configured() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.no_providers");
}

#[tokio::test]
async fn search_series_404_when_slug_unknown() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/series/no-such-slug/metadata/search").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_series_403_when_non_admin_lacks_library_access() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/search"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn search_series_returns_202_with_run_id_and_creates_run_row() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let run_id = body["run_id"].as_str().expect("run_id").to_owned();
    let run_uuid = Uuid::parse_str(&run_id).expect("uuid");
    assert_eq!(body["coalesced"], false);
    // Run row was created.
    let run = entity::metadata_run::Entity::find_by_id(run_uuid)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run row");
    assert_eq!(run.scope, "series");
    assert_eq!(run.scope_entity_id.as_deref(), Some(series_id.to_string().as_str()));
    assert_eq!(run.providers, vec!["comicvine"]);
    // Status starts at queued; the worker would flip it.
    assert!(run.status == "queued" || run.status == "searching" || run.status == "completed");
}

#[tokio::test]
async fn search_series_coalesces_second_click_to_same_run() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp1 = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
    )
    .await;
    assert_eq!(resp1.status(), StatusCode::ACCEPTED);
    let body1 = body_json(resp1.into_body()).await;
    let run1 = body1["run_id"].as_str().unwrap().to_owned();
    assert_eq!(body1["coalesced"], false);

    let resp2 = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
    )
    .await;
    assert_eq!(resp2.status(), StatusCode::ACCEPTED);
    let body2 = body_json(resp2.into_body()).await;
    assert_eq!(body2["coalesced"], true);
    assert_eq!(body2["run_id"], run1);
}

#[tokio::test]
async fn candidates_series_returns_completed_run_with_rows() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into(), "comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(Some(json!({
            "kind": "series",
            "name": "Saga",
            "year": 2012,
            "publisher": "Image Comics",
            "volume": null
        }))),
    }
    .insert(db)
    .await
    .unwrap();
    entity::metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("metron".into()),
        external_id: Set("123".into()),
        bucket: Set("high".into()),
        score: Set(85.0),
        score_breakdown: Set(json!({"name": 45.0, "year": 20.0, "publisher": 15.0, "issue_number": 0.0, "volume": 0.0})),
        candidate: Set(json!({"kind": "series", "name": "Saga"})),
        applied_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();

    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "completed");
    assert_eq!(body["items_matched_high"], 1);
    let candidates = body["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["source"], "metron");
    assert_eq!(candidates[0]["bucket"], "high");
}

#[tokio::test]
async fn candidates_series_404_when_run_id_belongs_to_different_series() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let other_series_id = SeriesSeed::new(_lib, "Other")
        .insert(&app.state().db)
        .await;
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let other_run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(other_run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(other_series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(0),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(1),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates?run_id={other_run_id}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.run_not_found");
}

#[tokio::test]
async fn candidates_series_404_when_no_run_exists() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_issue_succeeds_with_seeded_issue() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let cbz = dir.path().join("test.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&app.state().db)
        .await;
    let issue = entity::issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/issues/{}/metadata/search", issue.slug),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let run_id = body["run_id"].as_str().expect("run_id").to_owned();
    let run_uuid = Uuid::parse_str(&run_id).unwrap();
    let run = entity::metadata_run::Entity::find_by_id(run_uuid)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run row");
    assert_eq!(run.scope, "issue");
    assert_eq!(run.scope_entity_id.as_deref(), Some(issue_id.as_str()));
}

#[tokio::test]
async fn search_issue_400_when_issue_has_no_number_raw() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let cbz = dir.path().join("test.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&app.state().db)
        .await;
    // Clear number_raw to exercise the "issue without parsed number"
    // 400 path — IssueSeed populates it from sort_number by default.
    let issue = entity::issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let issue_slug = issue.slug.clone();
    let mut am: entity::issue::ActiveModel = issue.into();
    am.number_raw = Set(None);
    am.update(&app.state().db).await.unwrap();
    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/issues/{issue_slug}/metadata/search"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.no_issue_number");
}
