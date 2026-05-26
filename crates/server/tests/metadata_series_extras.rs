//! Integration tests for the M5 metadata series extras
//! (`/metadata/pause`, `/metadata/resume`, `/metadata/status`,
//! and the external-ids CRUD endpoints).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
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

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, auth.cookie())
        .header("x-csrf-token", &auth.csrf);
    let body_inner = if let Some(json) = body {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(b.body(body_inner).unwrap())
        .await
        .unwrap()
}

async fn get(app: &TestApp, auth: &Authed, path: &str) -> axum::http::Response<Body> {
    send(app, auth, Method::GET, path, None).await
}

async fn seed_series(app: &TestApp) -> Uuid {
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await
}

// ───────── pause / resume ─────────

#[tokio::test]
async fn pause_then_resume_flips_metadata_sync_paused() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;

    let resp = send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/pause"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["paused"], true);

    // DB row reflects the change.
    let row = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.metadata_sync_paused);

    let resp = send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/resume"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["paused"], false);
    let row = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.metadata_sync_paused);

    // Audit rows emitted.
    let rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::TargetType.eq("series"))
        .all(&app.state().db)
        .await
        .unwrap();
    let actions: Vec<_> = rows.iter().map(|r| r.action.as_str()).collect();
    assert!(actions.contains(&"admin.series.metadata_pause"));
    assert!(actions.contains(&"admin.series.metadata_resume"));
}

#[tokio::test]
async fn sync_status_reports_paused_and_linked_count() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;

    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/status"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["paused"], false);
    assert_eq!(body["linked_source_count"], 0);
}

// ───────── external_ids CRUD ─────────

#[tokio::test]
async fn external_ids_add_then_list_then_delete_round_trip() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;

    // Empty by default.
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/external-ids"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 0);

    // Add a CV identifier.
    let resp = send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_id}/external-ids"),
        Some(json!({"source": "comicvine", "external_id": "12345"})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["source"], "comicvine");
    assert_eq!(body["external_id"], "12345");
    assert_eq!(body["set_by"], "user");
    assert!(body["external_url"].as_str().unwrap().contains("4050-12345"));

    // List shows it.
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/external-ids"),
    )
    .await;
    let body = body_json(resp.into_body()).await;
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);

    // Status reports linked_source_count=1.
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/metadata/status"),
    )
    .await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["linked_source_count"], 1);

    // Delete.
    let resp = send(
        &app,
        &admin,
        Method::DELETE,
        &format!("/api/series/{series_id}/external-ids/comicvine"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Gone.
    let resp = get(
        &app,
        &admin,
        &format!("/api/series/{series_id}/external-ids"),
    )
    .await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn external_ids_add_400_when_source_unknown() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;
    let resp = send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_id}/external-ids"),
        Some(json!({"source": "wat", "external_id": "x"})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "metadata.invalid_source");
}

#[tokio::test]
async fn external_ids_delete_404_when_no_link() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;
    let resp = send(
        &app,
        &admin,
        Method::DELETE,
        &format!("/api/series/{series_id}/external-ids/comicvine"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
