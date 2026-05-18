//! Integration coverage for the manual scan-cancel endpoint
//! (`POST /libraries/{slug}/scan-runs/{id}/cancel`). The endpoint is the
//! operator's escape hatch for scans that have lost their worker —
//! typical cause: "Clear queues" mid-flight purges pending Redis jobs
//! but leaves the `scan_runs` row sitting at `state='running'`
//! forever because nothing transitions it to a terminal state.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{library, scan_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
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
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_library_and_scan_run(app: &TestApp, state: &str) -> (Uuid, String, Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let slug = lib_id.to_string();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test Library".into()),
        root_path: Set(format!("/tmp/scan-cancel-{lib_id}")),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(slug.clone()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".into()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    let scan_id = Uuid::now_v7();
    scan_run::ActiveModel {
        id: Set(scan_id),
        library_id: Set(lib_id),
        state: Set(state.into()),
        started_at: Set(now),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    (lib_id, slug, scan_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_flips_running_scan_to_cancelled() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-cancel@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "running").await;

    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:#?}");
    assert_eq!(body["state"], "cancelled");
    assert_eq!(body["error"], "Cancelled by admin");
    assert!(body["ended_at"].is_string());

    // DB row reflects the change.
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = scan_run::Entity::find_by_id(scan_id)
        .one(&db)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.state, "cancelled");
    assert!(row.ended_at.is_some());
    assert_eq!(row.error.as_deref(), Some("Cancelled by admin"));
    assert_eq!(row.library_id, lib_id);

    // Audit row landed.
    let audit_rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.scan_run.cancel"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(audit_rows.len(), 1, "exactly one cancel audit row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_terminal_scan_returns_409() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-409@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "complete").await;

    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "already_terminal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_requires_admin() {
    let app = TestApp::spawn().await;
    // First user auto-admin; second user is a regular reader.
    let _admin = register(&app, "first@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (_lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "running").await;

    let (status, _body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &reader,
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
        "non-admin should be rejected; got {status}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_unknown_scan_returns_404() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-404@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib_id, slug, _scan_id) = seed_library_and_scan_run(&app, "running").await;

    let bogus = Uuid::now_v7();
    let (status, _body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{bogus}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
