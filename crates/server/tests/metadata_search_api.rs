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
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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
    assert_eq!(
        run.scope_entity_id.as_deref(),
        Some(series_id.to_string().as_str())
    );
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
        batch_id: Set(None),
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
    let other_series_id = SeriesSeed::new(_lib, "Other").insert(&app.state().db).await;
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
        batch_id: Set(None),
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
        &format!(
            "/api/series/{series_id}/issues/{}/metadata/search",
            issue.slug
        ),
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

// ───────── apply endpoint decision logic ─────────

async fn seed_completed_series_run(
    app: &TestApp,
    series_id: Uuid,
    source: &str,
    external_id: &str,
) -> (Uuid, i32) {
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
        providers: Set(vec![source.into()]),
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
        batch_id: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    entity::metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set(source.into()),
        external_id: Set(external_id.into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({})),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    (run_id, 0)
}

async fn post_json(
    app: &TestApp,
    auth: &Authed,
    path: &str,
    body: Value,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn apply_series_returns_202_when_run_and_candidate_exist() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let (run_id, ordinal) = seed_completed_series_run(&app, series_id, "comicvine", "12345").await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/apply"),
        json!({"run_id": run_id, "ordinal": ordinal}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["run_id"], run_id.to_string());
    assert_eq!(body["ordinal"], ordinal);
    assert_eq!(body["status"], "queued");
}

#[tokio::test]
async fn apply_series_400_when_candidate_ordinal_unknown() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let (run_id, _ord) = seed_completed_series_run(&app, series_id, "comicvine", "12345").await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/apply"),
        json!({"run_id": run_id, "ordinal": 99}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.candidate_not_found");
}

#[tokio::test]
async fn apply_series_404_when_run_belongs_to_different_series() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let other_series_id = SeriesSeed::new(lib_id, "Other")
        .insert(&app.state().db)
        .await;
    let (other_run_id, _ord) =
        seed_completed_series_run(&app, other_series_id, "comicvine", "12345").await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/apply"),
        json!({"run_id": other_run_id, "ordinal": 0}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.run_not_found");
}

#[tokio::test]
async fn apply_series_403_when_override_user_edits_requested_by_non_admin() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    // Grant the user library access via direct row insert (the test
    // harness has no helper for this, so we do it raw).
    use entity::library_user_access;
    use entity::user;
    use sea_orm::{ColumnTrait, QueryFilter};
    let user_row = user::Entity::find()
        .filter(user::Column::Email.eq("user@example.com"))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_row.id),
        library_id: Set(lib_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    let (run_id, ord) = seed_completed_series_run(&app, series_id, "comicvine", "12345").await;
    let resp = post_json(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/apply"),
        json!({"run_id": run_id, "ordinal": ord, "override_user_edits": true}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.forbidden");
}

#[tokio::test]
async fn apply_series_403_when_user_lacks_library_access() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let (run_id, ord) = seed_completed_series_run(&app, series_id, "comicvine", "12345").await;
    let resp = post_json(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/apply"),
        json!({"run_id": run_id, "ordinal": ord}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
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

// ───────── composite (multi-provider) endpoints ─────────

#[tokio::test]
async fn composite_diff_series_403_when_non_admin_lacks_access() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let run = Uuid::now_v7();
    let resp = get(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/composite-diff?run_id={run}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn composite_diff_series_404_when_run_unknown() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let run = Uuid::now_v7();
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/composite-diff?run_id={run}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn composite_apply_series_403_when_non_admin_lacks_access() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let body = json!({ "run_id": Uuid::now_v7(), "field_sources": [], "included": [] });
    let resp = post_json(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/composite-apply"),
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn composite_apply_series_404_when_run_unknown() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let body = json!({ "run_id": Uuid::now_v7(), "field_sources": [], "included": [] });
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/composite-apply"),
        body,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn series_batch_groups_per_issue_runs_and_holds_for_review() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    // Two active, numbered issues → two searchable children.
    let cbz1 = dir.path().join("saga-1.cbz");
    let cbz2 = dir.path().join("saga-2.cbz");
    let _i1 = IssueSeed::new(lib_id, series_id, &cbz1, b"x", 1.0)
        .insert(&app.state().db)
        .await;
    let _i2 = IssueSeed::new(lib_id, series_id, &cbz2, b"y", 2.0)
        .insert(&app.state().db)
        .await;

    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/batch"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let batch_id = Uuid::parse_str(body["batch_id"].as_str().expect("batch_id")).unwrap();
    assert_eq!(body["jobs_enqueued"].as_u64().unwrap(), 2);

    // One batch row, scoped + manual + correct denominator.
    let batch = entity::metadata_batch::Entity::find_by_id(batch_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("batch row");
    assert_eq!(batch.scope, "series_issues");
    assert_eq!(batch.trigger_kind, "manual");
    assert_eq!(batch.items_total, 2);

    // Both child runs carry the batch_id, are issue-scoped, and run as
    // `manual` so nothing auto-applies (the queue is the accept surface).
    let children = entity::metadata_run::Entity::find()
        .filter(entity::metadata_run::Column::BatchId.eq(batch_id))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(children.len(), 2);
    assert!(
        children
            .iter()
            .all(|r| r.scope == "issue" && r.trigger_kind == "manual")
    );
}

/// Seed a one-child batch whose child is a `multi_good` (needs-review) run
/// with two providers' candidates. `applied` flips both candidates' applied_at
/// so the run looks already-applied. Returns the batch id.
async fn seed_needs_review_batch(
    app: &TestApp,
    lib_id: Uuid,
    issue_id: &str,
    applied: bool,
) -> Uuid {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let batch_id = Uuid::now_v7();
    entity::metadata_batch::ActiveModel {
        id: Set(batch_id),
        library_id: Set(Some(lib_id)),
        scope: Set("series_issues".into()),
        trigger_kind: Set("manual".into()),
        status: Set("completed".into()),
        items_total: Set(1),
        created_by: Set(None),
        created_at: Set(now),
        ended_at: Set(Some(now)),
    }
    .insert(db)
    .await
    .unwrap();

    let run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(issue_id.to_string())),
        library_id: Set(Some(lib_id)),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into(), "metron".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(0),
        items_matched_medium: Set(1),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(Some(batch_id)),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();

    let applied_at = if applied { Some(now) } else { None };
    for (ord, src) in [(0i32, "comicvine"), (1, "metron")] {
        entity::metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(ord),
            source: Set(src.into()),
            external_id: Set(format!("ext-{ord}")),
            bucket: Set("medium".into()),
            score: Set(70.0),
            score_breakdown: Set(json!({})),
            candidate: Set(json!({})),
            applied_at: Set(applied_at),
        }
        .insert(db)
        .await
        .unwrap();
    }
    entity::metadata_match_outcome::ActiveModel {
        id: Set(Uuid::now_v7()),
        run_id: Set(run_id),
        scope: Set("issue".into()),
        outcome_kind: Set("multi_good".into()),
        top_score: Set(70.0),
        top_hamming: Set(None),
        second_score: Set(Some(68.0)),
        second_hamming: Set(None),
        candidate_count: Set(2),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    batch_id
}

#[tokio::test]
async fn batch_apply_all_needs_review_enqueues_one_composite_per_run() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"x", 1.0)
        .insert(&app.state().db)
        .await;
    let batch_id = seed_needs_review_batch(&app, lib_id, &issue_id, false).await;

    // "All" → the one needs-review run is enqueued for composite apply.
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/metadata/batch/{batch_id}/apply"),
        json!({"filter": "all_needs_review", "mode": "fill_missing"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["enqueued"].as_u64().unwrap(), 1);
    assert_eq!(body["skipped_already_applied"].as_u64().unwrap(), 0);

    // `run_ids: []` (Selected scope with nothing picked) → enqueues nothing.
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/metadata/batch/{batch_id}/apply"),
        json!({"filter": "all_needs_review", "mode": "fill_missing", "run_ids": []}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["enqueued"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn batch_apply_all_needs_review_skips_fully_applied_runs() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"x", 1.0)
        .insert(&app.state().db)
        .await;
    // Both candidates already applied → the run is skipped, not re-enqueued.
    let batch_id = seed_needs_review_batch(&app, lib_id, &issue_id, true).await;

    let resp = post_json(
        &app,
        &admin,
        &format!("/api/metadata/batch/{batch_id}/apply"),
        json!({"filter": "all_needs_review", "mode": "replace_all"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["enqueued"].as_u64().unwrap(), 0);
    assert_eq!(body["skipped_already_applied"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn create_series_batch_incomplete_scope_skips_complete_issues() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();

    // Issue 1 → COMPLETE: title + page count from the seed, then the remaining
    // core fields (cover date / summary / a credit) + a matched external_id.
    let c1 = dir.path().join("c1.cbz");
    let complete_id = IssueSeed::new(lib_id, series_id, &c1, b"a", 1.0)
        .with_title("Chapter One")
        .with_page_count(22)
        .insert(db)
        .await;
    let mut am: entity::issue::ActiveModel = entity::issue::Entity::find_by_id(&complete_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.year = Set(Some(2011));
    am.summary = Set(Some("A complete summary.".into()));
    am.writer = Set(Some("Jonathan Hickman".into()));
    am.update(db).await.unwrap();
    entity::external_id::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(complete_id.clone()),
        source: Set("comicvine".into()),
        external_id: Set("12345".into()),
        external_url: Set(None),
        set_by: Set("comicvine".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    // Issue 2 → bare (needs_metadata).
    let c2 = dir.path().join("c2.cbz");
    let bare_id = IssueSeed::new(lib_id, series_id, &c2, b"b", 2.0)
        .insert(db)
        .await;

    // scope=incomplete fans out over the bare issue only.
    let resp = post(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/batch?scope=incomplete"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(
        body["items_total"].as_u64().unwrap(),
        1,
        "only the incomplete issue should be queued"
    );

    // A child run exists for the bare issue, none for the complete one.
    let runs = entity::metadata_run::Entity::find()
        .filter(
            entity::metadata_run::Column::ScopeEntityId
                .is_in([bare_id.clone(), complete_id.clone()]),
        )
        .all(db)
        .await
        .unwrap();
    assert_eq!(
        runs.iter()
            .filter(|r| r.scope_entity_id.as_deref() == Some(bare_id.as_str()))
            .count(),
        1,
        "bare issue gets a run"
    );
    assert_eq!(
        runs.iter()
            .filter(|r| r.scope_entity_id.as_deref() == Some(complete_id.as_str()))
            .count(),
        0,
        "complete issue is skipped"
    );
}
