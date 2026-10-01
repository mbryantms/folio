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
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
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

/// B13: a completed run surfaces each configured provider's live quota so
/// the dialog can show remaining budget before the next batch.
#[tokio::test]
async fn candidates_completed_run_includes_provider_quota() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
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
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(0),
        items_matched_high: Set(0),
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

    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let providers = body["quota"]["providers"].as_array().unwrap();
    let cv = providers
        .iter()
        .find(|p| p["provider"] == "comicvine")
        .expect("comicvine quota present");
    assert!(
        cv["remaining_hour"].is_number(),
        "remaining_hour snapshot present: {cv}"
    );
    // WP-2.9: the headline budget rides along so the dialog can show
    // "N of M requests left" — ComicVine's is bucket-derived, hourly.
    assert_eq!(cv["budget"]["window"], "hour", "budget present: {cv}");
    assert_eq!(cv["budget"]["limit"], 200);
    // No quota-park, so no retry ETA.
    assert!(body["quota"]["retry_after_seconds"].is_null());
}

/// B13: a quota-parked run reports a concrete retry ETA (seconds), so the
/// dialog can say "retries in ~Xm" instead of "try again shortly".
#[tokio::test]
async fn candidates_awaiting_quota_reports_retry_eta() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let resume = now + chrono::Duration::minutes(30);
    let run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("awaiting_quota".into()),
        started_at: Set(now),
        finished_at: Set(None),
        items_total: Set(0),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(Some(resume)),
        batch_id: Set(None),
        query: Set(None),
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
    assert_eq!(body["status"], "awaiting_quota");
    let secs = body["quota"]["retry_after_seconds"]
        .as_u64()
        .expect("retry_after_seconds present");
    // ~30 min out; allow generous slack for test wall-clock.
    assert!(
        (1500..=1800).contains(&secs),
        "retry ETA near 30m, got {secs}s"
    );
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
    // SE-3 (WP-6.3): the structural `RequireAdmin` gate's code.
    assert_eq!(body["error"]["code"], "auth.permission_denied");
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

#[tokio::test]
async fn selection_batch_searches_only_the_chosen_in_series_issues() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (lib_id, series_id) = seed_series_in_library(&app, dir.path()).await;
    // Three active issues in the target series; we pick two.
    let i1 = IssueSeed::new(lib_id, series_id, &dir.path().join("a.cbz"), b"a", 1.0)
        .insert(&app.state().db)
        .await;
    let i2 = IssueSeed::new(lib_id, series_id, &dir.path().join("b.cbz"), b"b", 2.0)
        .insert(&app.state().db)
        .await;
    let _i3 = IssueSeed::new(lib_id, series_id, &dir.path().join("c.cbz"), b"c", 3.0)
        .insert(&app.state().db)
        .await;
    // A real issue in a *different* series — including its id must NOT widen
    // the batch past the series the caller is operating on.
    let dir2 = tempdir().unwrap();
    let (lib2, series2) = seed_series_in_library(&app, dir2.path()).await;
    let foreign = IssueSeed::new(lib2, series2, &dir2.path().join("z.cbz"), b"z", 1.0)
        .insert(&app.state().db)
        .await;

    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/batch/selection"),
        serde_json::json!({ "issue_ids": [i1, i2, foreign, "not-a-real-id"] }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    // Only the two in-series picks fan out; the foreign + bogus ids drop.
    assert_eq!(body["jobs_enqueued"].as_u64().unwrap(), 2);
    let batch_id = Uuid::parse_str(body["batch_id"].as_str().expect("batch_id")).unwrap();

    let batch = entity::metadata_batch::Entity::find_by_id(batch_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("batch row");
    assert_eq!(batch.scope, "series_issues");
    assert_eq!(batch.items_total, 2);

    let children = entity::metadata_run::Entity::find()
        .filter(entity::metadata_run::Column::BatchId.eq(batch_id))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(children.len(), 2);
    assert!(children.iter().all(|r| r.scope == "issue"));
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

// ───────── WP-2.8: query overrides + lookup-by-URL ─────────

fn cv_ok(results: Value) -> Value {
    json!({"status_code": 1, "error": "OK", "results": results})
}

fn cv_volume_detail_json() -> Value {
    json!({
        "id": 12345,
        "name": "Saga",
        "start_year": "2012",
        "publisher": {"id": 99, "name": "Image Comics"},
        "deck": "Sci-fi epic.",
        "description": "Full description body.",
        "image": null,
        "count_of_issues": 60,
        "site_detail_url": "https://comicvine.gamespot.com/volume/4050-12345/",
        "date_last_updated": "2024-01-15 12:34:56",
        "aliases": null,
    })
}

fn cv_issue_detail_json() -> Value {
    json!({
        "id": 67890,
        "name": "First Issue",
        "issue_number": "1",
        "cover_date": "2012-03-14",
        "store_date": "2012-03-12",
        "deck": "Short blurb",
        "description": "Full HTML body.",
        "image": null,
        "person_credits": [],
        "character_credits": [],
        "team_credits": [],
        "location_credits": [],
        "concept_credits": [],
        "object_credits": [],
        "story_arc_credits": [],
        "associated_images": [],
        "first_appearance_characters": [],
        "volume": {
            "id": 12345,
            "name": "Saga",
            "start_year": "2012",
            "site_detail_url": null,
            "publisher": null,
            "deck": null,
            "description": null,
            "image": null,
            "count_of_issues": null,
            "date_last_updated": null,
            "aliases": null,
        },
        "site_detail_url": "https://comicvine.gamespot.com/issue/4000-67890/",
        "date_last_updated": "2024-02-20 08:00:00",
        "aliases": null,
    })
}

async fn run_row(app: &TestApp, run_id: &str) -> entity::metadata_run::Model {
    entity::metadata_run::Entity::find_by_id(Uuid::parse_str(run_id).unwrap())
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run row")
}

#[tokio::test]
async fn search_series_overrides_replace_facts_for_the_run_only() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;

    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
        json!({"name": "  Saga Deluxe Edition ", "year": 2014}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let run_id = body["run_id"].as_str().unwrap().to_owned();

    // The stored query carries the *effective* facts + a note of which
    // fields were overridden.
    let run = run_row(&app, &run_id).await;
    let q = run.query.expect("stored query");
    assert_eq!(q["kind"], "series");
    assert_eq!(q["name"], "Saga Deluxe Edition");
    assert_eq!(q["year"], 2014);
    assert_eq!(q["publisher"], "Image Comics", "untouched facts stay local");
    assert_eq!(q["overrides"]["name"], "Saga Deluxe Edition");
    assert_eq!(q["overrides"]["year"], 2014);
    assert!(q["overrides"].get("publisher").is_none());

    // The series row itself is untouched.
    let s = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.name, "Saga");
    assert_ne!(s.year, Some(2014), "override must not write the series row");

    // The polling endpoint surfaces the effective query + the flag so
    // the dialog can render "searched as …".
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates?run_id={run_id}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["query"]["name"], "Saga Deluxe Edition");
    assert_eq!(body["query"]["year"], 2014);
    assert_eq!(body["query"]["overridden"], true);
    assert_eq!(body["query"]["year_gate_relaxed"], false);
    assert_eq!(body["query"]["lookup"], false);
    assert_eq!(body["query"]["label"], "Saga Deluxe Edition");
}

#[tokio::test]
async fn search_series_without_body_still_works_and_reports_no_override() {
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
    let run = run_row(&app, body["run_id"].as_str().unwrap()).await;
    let q = run.query.unwrap();
    assert_eq!(q["name"], "Saga");
    assert!(q.get("overrides").is_none());
}

#[tokio::test]
async fn search_series_override_validation_lands_422_with_field_details() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
        json!({"name": "   ", "year": 1800}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation");
    let fields: Vec<&str> = body["error"]["details"]
        .as_array()
        .expect("details")
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    assert!(fields.contains(&"name"), "{fields:?}");
    assert!(fields.contains(&"year"), "{fields:?}");
    // No run row was created for a rejected request.
    let n = entity::metadata_run::Entity::find()
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn search_issue_overrides_number_and_year_persist_on_run() {
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
    let resp = post_json(
        &app,
        &admin,
        &format!(
            "/api/series/{series_id}/issues/{}/metadata/search",
            issue.slug
        ),
        json!({"issue_number": "Annual 1", "year": 2013, "publisher": "Image"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let run = run_row(&app, body["run_id"].as_str().unwrap()).await;
    let q = run.query.unwrap();
    assert_eq!(q["kind"], "issue");
    assert_eq!(q["issue_number"], "Annual 1");
    assert_eq!(q["series_year"], 2013);
    assert_eq!(q["publisher"], "Image");
    assert_eq!(q["overrides"]["issue_number"], "Annual 1");
    // Issue row untouched.
    let after = entity::issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.number_raw.as_deref(), Some("1"));
}

/// The override must reach the provider: replay the persisted query
/// through the orchestrator (exactly what the worker does with the job
/// payload) against a wiremock ComicVine that only answers a `filter`
/// carrying the overridden name.
#[tokio::test]
async fn search_series_override_changes_the_provider_query_string() {
    use server::metadata::orchestrator::{self, PreFilter, StoredQuery};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .and(query_param("filter", "name:Overridden Name"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_ok(json!([{
            "id": 555,
            "name": "Overridden Name",
            "start_year": "2012",
            "publisher": {"id": 1, "name": "Image Comics"},
            "deck": null,
            "description": null,
            "image": null,
            "count_of_issues": null,
            "site_detail_url": null,
            "date_last_updated": null,
            "aliases": null,
        }]))))
        .expect(1)
        .mount(&cv_mock)
        .await;
    let app = TestApp::spawn_with_comicvine_at("k", cv_mock.uri()).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;

    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/search"),
        json!({"name": "Overridden Name"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = body_json(resp.into_body()).await;
    let run_id = Uuid::parse_str(body["run_id"].as_str().unwrap()).unwrap();

    // Replay the stored query through the orchestrator with the
    // production provider factory (pointed at wiremock via config).
    let run = run_row(&app, &run_id.to_string()).await;
    let StoredQuery::Series(facts) = serde_json::from_value(run.query.unwrap()).unwrap() else {
        panic!("expected series query")
    };
    assert_eq!(facts.name, "Overridden Name");
    let state = app.state();
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    let ranked = orchestrator::run_series_search(
        &state.db,
        run_id,
        &providers,
        &facts,
        server::metadata::matcher::Thresholds::new(80.0, 60.0),
        &PreFilter::default(),
        3,
        None,
    )
    .await
    .expect("search");
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].external_id, "555");
    // wiremock's `.expect(1)` verifies the filter reached the provider.
}

#[tokio::test]
async fn lookup_series_rejects_bad_urls_with_422_bound_to_url_field() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let path = format!("/api/series/{series_id}/metadata/lookup");

    for (body, needle) in [
        (
            json!({"url": "https://www.comics.org/series/1/"}),
            "not a supported provider host",
        ),
        (
            json!({"url": "https://comicvine.gamespot.com/saga/"}),
            "no series or issue id",
        ),
        (
            json!({"url": "https://metron.cloud/series/saga-2012/"}),
            "slug",
        ),
        (
            json!({"url": "https://comicvine.gamespot.com/x/4000-1/"}),
            "issue link",
        ),
        (json!({}), "paste a provider URL"),
    ] {
        let resp = post_json(&app, &admin, &path, body.clone()).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let out = body_json(resp.into_body()).await;
        assert_eq!(out["error"]["code"], "validation");
        assert!(
            out["error"]["message"].as_str().unwrap().contains(needle),
            "{body} → {}",
            out["error"]["message"]
        );
        assert_eq!(out["error"]["details"][0]["field"], "url", "{body}");
    }
    let n = entity::metadata_run::Entity::find()
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(n, 0, "rejected lookups create no run rows");
}

#[tokio::test]
async fn lookup_series_rejects_unconfigured_provider_with_422() {
    // ComicVine is the only configured provider; a Metron URL must be
    // refused before any network call.
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/lookup"),
        json!({"url": "https://metron.cloud/api/series/1234/"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let out = body_json(resp.into_body()).await;
    assert_eq!(out["error"]["code"], "validation");
    assert!(
        out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("metron is not configured"),
        "{}",
        out["error"]["message"]
    );
    assert_eq!(out["error"]["details"][0]["field"], "url");

    // Explicit pair form binds to `source` instead.
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/lookup"),
        json!({"source": "metron", "external_id": "1234"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let out = body_json(resp.into_body()).await;
    assert_eq!(out["error"]["details"][0]["field"], "source");
}

#[tokio::test]
async fn lookup_series_403_when_non_admin_lacks_library_access() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post_json(
        &app,
        &user,
        &format!("/api/series/{series_id}/metadata/lookup"),
        json!({"url": "https://comicvine.gamespot.com/volume/4050-12345/"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lookup_series_creates_one_high_candidate_and_apply_works_on_it() {
    use server::jobs::metadata_apply::apply_series_inline;
    use server::metadata::apply::{ApplyArgs, ApplyMode};
    use server::metadata::writers::CoverOverwritePolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-12345"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_ok(cv_volume_detail_json())))
        // Exactly one provider hit: the apply must reuse the cache the
        // lookup populated instead of re-fetching.
        .expect(1)
        .mount(&cv_mock)
        .await;
    let app = TestApp::spawn_with_comicvine_at("k", cv_mock.uri()).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Some Other Name")
        .insert(&app.state().db)
        .await;
    // Clear the seed's year + deck so fill_missing has slots to write.
    {
        let row = entity::series::Entity::find_by_id(series_id)
            .one(&app.state().db)
            .await
            .unwrap()
            .unwrap();
        let mut am: entity::series::ActiveModel = row.into();
        am.year = Set(None);
        am.deck = Set(None);
        am.update(&app.state().db).await.unwrap();
    }

    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/lookup"),
        json!({"url": "https://comicvine.gamespot.com/saga/4050-12345/"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["source"], "comicvine");
    assert_eq!(body["external_id"], "12345");
    let run_id = body["run_id"].as_str().unwrap().to_owned();

    // The run is already completed with exactly one HIGH candidate and
    // no matcher outcome row (the matcher never ran).
    let run = run_row(&app, &run_id).await;
    assert_eq!(run.status, "completed");
    assert_eq!(run.items_total, 1);
    assert_eq!(run.items_matched_high, 1);
    assert_eq!(run.providers, vec!["comicvine"]);
    assert_eq!(
        run.scope_entity_id.as_deref(),
        Some(series_id.to_string().as_str())
    );
    let q = run.query.clone().unwrap();
    assert_eq!(q["lookup"]["source"], "comicvine");
    assert_eq!(q["lookup"]["external_id"], "12345");
    assert_eq!(
        q["lookup"]["url"],
        "https://comicvine.gamespot.com/saga/4050-12345/"
    );
    let outcomes = entity::metadata_match_outcome::Entity::find()
        .filter(entity::metadata_match_outcome::Column::RunId.eq(run.id))
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(outcomes, 0, "lookup runs don't skew match-quality stats");

    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/candidates?run_id={run_id}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "completed");
    assert_eq!(body["candidates"].as_array().unwrap().len(), 1);
    let c = &body["candidates"][0];
    assert_eq!(c["bucket"], "high");
    assert_eq!(c["score"], 100.0);
    assert_eq!(c["score_breakdown"]["lookup"], true);
    assert_eq!(c["candidate"]["kind"], "series");
    assert_eq!(c["candidate"]["name"], "Saga");
    assert_eq!(c["candidate"]["year"], 2012);
    assert_eq!(c["candidate"]["publisher"], "Image Comics");
    assert_eq!(
        c["candidate"]["external_url"],
        "https://comicvine.gamespot.com/volume/4050-12345/"
    );
    assert_eq!(body["match_outcome"]["kind"], "single_good");
    assert_eq!(body["query"]["lookup"], true);
    assert_eq!(body["query"]["overridden"], false);

    // Apply the lookup candidate through the ordinary apply path.
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        ApplyArgs {
            run_id: run.id,
            ordinal: 0,
            mode: ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            actor_id: None,
            selected_fields: None,
            override_external_id_sources: std::collections::HashSet::new(),
        },
    )
    .await
    .expect("apply on lookup candidate");
    assert!(outcome.applied_fields.contains(&"year_began".to_owned()));
    let after = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.year, Some(2012));
    assert_eq!(after.deck.as_deref(), Some("Sci-fi epic."));
    // The real CV mapper also carries the publisher's CV id as an
    // identifier (written alongside on the series entity), so match on
    // the set rather than `.one()`.
    let ids: Vec<String> = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.external_id)
        .collect();
    assert!(ids.contains(&"12345".to_owned()), "external ids: {ids:?}");
}

#[tokio::test]
async fn lookup_issue_by_explicit_source_and_id_creates_issue_candidate() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/4000-67890"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_ok(cv_issue_detail_json())))
        .expect(1)
        .mount(&cv_mock)
        .await;
    let app = TestApp::spawn_with_comicvine_at("k", cv_mock.uri()).await;
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

    let resp = post_json(
        &app,
        &admin,
        &format!(
            "/api/series/{series_id}/issues/{}/metadata/lookup",
            issue.slug
        ),
        // CV type prefix tolerated + stripped.
        json!({"source": "comicvine", "external_id": "4000-67890"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["external_id"], "67890");
    let run_id = body["run_id"].as_str().unwrap().to_owned();
    let run = run_row(&app, &run_id).await;
    assert_eq!(run.scope, "issue");
    assert_eq!(run.scope_entity_id.as_deref(), Some(issue_id.as_str()));
    let q = run.query.unwrap();
    assert!(
        q["lookup"].get("url").is_none(),
        "no url in the explicit-pair form"
    );

    let resp = get(
        &app,
        &admin,
        &format!(
            "/api/series/{series_id}/issues/{}/metadata/candidates?run_id={run_id}",
            issue.slug
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let c = &body["candidates"][0];
    assert_eq!(c["bucket"], "high");
    assert_eq!(c["candidate"]["kind"], "issue");
    assert_eq!(c["candidate"]["issue_number"], "1");
    assert_eq!(c["candidate"]["series_name"], "Saga");
    assert_eq!(c["candidate"]["series_external_id"], "12345");
    assert_eq!(body["match_outcome"]["kind"], "single_good");
}

#[tokio::test]
async fn lookup_series_404_when_provider_has_no_such_record() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 101,
            "error": "Object Not Found",
            "results": [],
        })))
        .mount(&cv_mock)
        .await;
    let app = TestApp::spawn_with_comicvine_at("k", cv_mock.uri()).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let (_lib, series_id) = seed_series_in_library(&app, dir.path()).await;
    let resp = post_json(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/lookup"),
        json!({"url": "https://comicvine.gamespot.com/volume/4050-1/"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let out = body_json(resp.into_body()).await;
    assert_eq!(out["error"]["code"], "metadata.lookup_not_found");
    let n = entity::metadata_run::Entity::find()
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}
