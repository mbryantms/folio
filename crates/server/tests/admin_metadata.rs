//! Integration tests for `/admin/metadata/*` (metadata-providers-1.0 M1).
//!
//! Wiremock-driven coverage of the ComicVine HTTP path lives in
//! `tests/comicvine_client.rs` — this file targets the admin handler's
//! decision matrix (auth gate, credential/enabled short-circuits, audit
//! row emission, unknown-provider 404, not-yet-supported provider 404).
//!
//! Coverage:
//! - GET /admin/metadata/providers requires admin
//! - list reports comicvine.configured=false when key unset
//! - list reports comicvine.configured=true + enabled=false when key set
//!   but master toggle off
//! - POST /providers/comicvine/test → 400 when key unset
//! - POST /providers/comicvine/test → 409 when key set but disabled
//! - POST /providers/foo/test → 404 unknown
//! - POST /providers/metron/test → 404 (M2 hasn't shipped)
//! - successful flow audit-logs `admin.metadata.providers.test`

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use sea_orm::EntityTrait;
use serde_json::Value;
use tower::ServiceExt;

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

#[tokio::test]
async fn list_providers_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &user, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_providers_unconfigured_when_no_credentials() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let providers = body["providers"].as_array().expect("providers array");
    assert!(!providers.is_empty(), "should include comicvine row");
    let cv = providers
        .iter()
        .find(|p| p["id"] == "comicvine")
        .expect("comicvine row");
    assert_eq!(cv["configured"], false);
    assert_eq!(cv["enabled"], false);
    assert_eq!(cv["quota"], Value::Null);
}

#[tokio::test]
async fn list_providers_configured_but_disabled() {
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let cv = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "comicvine")
        .unwrap()
        .clone();
    assert_eq!(cv["configured"], true);
    assert_eq!(cv["enabled"], false);
    // Quota snapshot resolves to a value (Redis bucket reports "full"
    // when no decrement has happened yet).
    assert!(cv["quota"].is_object());
}

#[tokio::test]
async fn test_provider_400_when_key_missing() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.no_credentials");
}

#[tokio::test]
async fn test_provider_409_when_disabled() {
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.disabled");
}

#[tokio::test]
async fn test_provider_404_for_unknown_id() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/notreal/test").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.unknown_provider");
}

#[tokio::test]
async fn test_provider_400_for_metron_without_credentials() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/metron/test").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.no_credentials");
}

#[tokio::test]
async fn test_provider_409_for_metron_when_disabled() {
    let app = TestApp::spawn_with_metron("u", "p", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = post(&app, &admin, "/api/admin/metadata/providers/metron/test").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.disabled");
}

// ───────── M6 admin surface ─────────

#[tokio::test]
async fn dashboard_returns_counts_and_provider_snapshot() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    // No series seeded → matched/unmatched both 0.
    assert_eq!(body["series_total"], 0);
    assert_eq!(body["series_matched"], 0);
    assert_eq!(body["series_unmatched"], 0);
    assert_eq!(body["applies_last_7_days"], 0);
    let providers = body["providers"].as_array().unwrap();
    assert!(providers.iter().any(|p| p["id"] == "comicvine"));
    assert!(providers.iter().any(|p| p["id"] == "metron"));
}

#[tokio::test]
async fn dashboard_requires_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &user, "/api/admin/metadata/dashboard").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn runs_list_empty_then_returns_seeded_row() {
    use sea_orm::{ActiveModelTrait, Set};
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/runs").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["runs"].as_array().unwrap().len(), 0);

    // Seed one run.
    let now = chrono::Utc::now().fixed_offset();
    let run_id = uuid::Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(uuid::Uuid::now_v7().to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(3),
        items_matched_high: Set(1),
        items_matched_medium: Set(1),
        items_matched_low: Set(1),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let resp = get(&app, &admin, "/api/admin/metadata/runs").await;
    let body = body_json(resp.into_body()).await;
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["scope"], "series");
    assert_eq!(runs[0]["items_total"], 3);

    // Detail.
    let resp = get(&app, &admin, &format!("/api/admin/metadata/runs/{run_id}")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["run"]["id"], run_id.to_string());
    assert_eq!(body["candidates"].as_array().unwrap().len(), 0);
}

/// B14: the recent-applies feed lists only runs that wrote changes
/// (`items_applied > 0`), newest finish first, with resolved entity labels
/// and the `automatic` flag distinguishing weekly-refresh from manual.
#[tokio::test]
async fn recent_applies_lists_applied_runs_with_labels() {
    use sea_orm::{ActiveModelTrait, ColumnTrait, QueryFilter, Set};
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let db = &app.state().db;
    let admin_id = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("admin@example.com"))
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .id;
    let lib = LibrarySeed::new(std::path::Path::new("/tmp/folio-recent-applies"))
        .insert(db)
        .await;
    let series_id = SeriesSeed::new(lib, "Saga").insert(db).await;
    let now = chrono::Utc::now().fixed_offset();

    let mk = |applied: i32, triggered: Option<uuid::Uuid>, secs_ago: i64| {
        entity::metadata_run::ActiveModel {
            id: Set(uuid::Uuid::now_v7()),
            scope: Set("series".into()),
            scope_entity_id: Set(Some(series_id.to_string())),
            library_id: Set(Some(lib)),
            triggered_by: Set(triggered),
            trigger_kind: Set("manual".into()),
            providers: Set(vec!["comicvine".into()]),
            status: Set("completed".into()),
            started_at: Set(now - chrono::Duration::seconds(secs_ago + 1)),
            finished_at: Set(Some(now - chrono::Duration::seconds(secs_ago))),
            items_total: Set(applied.max(1)),
            items_matched_high: Set(0),
            items_matched_medium: Set(0),
            items_matched_low: Set(0),
            items_no_match: Set(0),
            items_applied: Set(applied),
            items_skipped: Set(0),
            items_failed: Set(0),
            error_summary: Set(None),
            resume_after: Set(None),
            batch_id: Set(None),
            query: Set(None),
        }
    };

    // Newest = a manual apply; older = an automatic (no triggered_by) apply;
    // and a 0-applies run that must be excluded.
    mk(1, Some(admin_id), 0).insert(db).await.unwrap();
    mk(2, None, 60).insert(db).await.unwrap();
    mk(0, Some(admin_id), 5).insert(db).await.unwrap();

    let resp = get(&app, &admin, "/api/admin/metadata/recent-applies").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let applies = body["applies"].as_array().unwrap();
    assert_eq!(applies.len(), 2, "the 0-applies run is excluded");
    // Newest finish first → manual, then automatic.
    assert_eq!(applies[0]["entity_label"], "Saga");
    assert_eq!(applies[0]["automatic"], false);
    assert_eq!(applies[0]["items_applied"], 1);
    assert_eq!(applies[1]["automatic"], true);
    assert_eq!(applies[1]["items_applied"], 2);
}

#[tokio::test]
async fn list_providers_includes_metron_row() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/admin/metadata/providers").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let providers = body["providers"].as_array().expect("providers");
    let metron = providers
        .iter()
        .find(|p| p["id"] == "metron")
        .expect("metron row");
    assert_eq!(metron["configured"], true);
    assert_eq!(metron["enabled"], true);
    assert!(metron["quota"].is_object());
}

#[tokio::test]
async fn test_provider_disabled_audit_does_not_fire() {
    // Audit row only writes on the "actually attempted" path; the
    // early 400 / 409 short-circuits exit before we reach the upstream
    // call. This isn't ideal — an operator clicking "Test" while
    // misconfigured should still leave a trail — but matches the
    // admin_email.test_send pattern. Capturing the current behavior
    // here makes future tightening greppable.
    let app = TestApp::spawn_with_comicvine("cv-test-key", false).await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let _ = post(&app, &admin, "/api/admin/metadata/providers/comicvine/test").await;
    let rows = entity::audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    assert!(
        !rows
            .iter()
            .any(|r| r.action == "admin.metadata.providers.test"),
        "audit row written even though provider was disabled (regression)"
    );
}

#[tokio::test]
async fn auto_synced_lists_only_unpaused_active_series() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempfile::tempdir().unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;
    // Auto-sync ON (paused = false).
    SeriesSeed::new(lib, "Saga").insert(&db).await;
    // Auto-sync OFF — must be excluded.
    SeriesSeed::new(lib, "Paused One")
        .with_metadata_sync_paused(true)
        .insert(&db)
        .await;

    let resp = get(&app, &admin, "/api/admin/metadata/auto-synced").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "only the unpaused series is auto-synced");
    assert_eq!(series[0]["name"], "Saga");
    assert!(series[0]["library_name"].as_str().is_some());
}
