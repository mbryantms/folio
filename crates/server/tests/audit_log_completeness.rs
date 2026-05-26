//! Regression guard for the audit-log completeness invariant
//! (audit-remediation M2, shipped 2026-05-23).
//!
//! Every admin mutation should write a row to `audit_log` via
//! [`crate::audit::record`] or the [`record_admin_action!`] macro.
//! Before M2, three handlers slipped through:
//!
//!   - `POST /libraries/{slug}/health-issues/{id}/dismiss` (admin
//!     marking a library-health issue as dismissed)
//!   - `POST /series/{series_slug}/issues/{issue_slug}/restore`
//!   - `POST /series/{series_slug}/issues/{issue_slug}/confirm-removal`
//!
//! Each test below exercises one of those endpoints end-to-end and
//! asserts the audit row appears. Future admin endpoints should add
//! similar coverage (or rely on the M10 AST-walking CI tool, which
//! statically verifies every `RequireAdmin` handler invokes some
//! `audit::record` / `record_admin_action!` form).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::library_health_issue;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use serde_json::Value;
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
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let extract = |needle: &str| -> String {
        cookies
            .iter()
            .find_map(|c| c.split(';').next()?.strip_prefix(needle).map(str::to_owned))
            .unwrap_or_default()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn admin_send(
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
    let body = if let Some(json) = body {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(b.body(body).unwrap())
        .await
        .unwrap()
}

async fn audit_entries_for(app: &TestApp, auth: &Authed, action_prefix: &str) -> Vec<Value> {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/admin/audit?action={action_prefix}"))
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    body["items"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn health_issue_dismiss_writes_audit_row() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let lib_id = common::seed::seed_library(&db, tmp.path()).await;
    let lib_slug = {
        use entity::library;
        let row = library::Entity::find_by_id(lib_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        row.slug
    };

    // Plant a health issue directly via the entity layer.
    let issue_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let am = library_health_issue::ActiveModel {
        id: Set(issue_id),
        library_id: Set(lib_id),
        scan_id: Set(None),
        kind: Set("ambiguous_folder".into()),
        payload: Set(serde_json::json!({})),
        severity: Set("warning".into()),
        fingerprint: Set("test-fingerprint".into()),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
        resolved_at: Set(None),
        dismissed_at: Set(None),
    };
    am.insert(&db).await.unwrap();

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/libraries/{lib_slug}/health-issues/{issue_id}/dismiss"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let entries = audit_entries_for(&app, &admin, "admin.library.health_issue.dismiss").await;
    let hit = entries
        .iter()
        .find(|e| e["target_id"] == issue_id.to_string())
        .expect("audit entry for health-issue dismiss");
    assert_eq!(hit["action"], "admin.library.health_issue.dismiss");
    assert_eq!(hit["target_type"], "library_health_issue");
    assert_eq!(hit["payload"]["library_id"], lib_id.to_string());
    assert_eq!(hit["payload"]["kind"], "ambiguous_folder");
}

#[tokio::test]
async fn reconcile_restore_writes_audit_row() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let lib_id = common::seed::seed_library(&db, tmp.path()).await;
    let series_id = common::seed::seed_series(&db, lib_id, "Restore Series").await;
    // A real on-disk file so the handler's `Path::exists` check passes.
    let file_path = tmp.path().join("issue-1.cbz");
    std::fs::write(&file_path, b"").unwrap();
    let issue_id = common::seed::IssueSeed::new(lib_id, series_id, &file_path, b"", 1.0)
        .insert(&db)
        .await;

    let (series_slug, issue_slug) = {
        use entity::{issue, series};
        let s = series::Entity::find_by_id(series_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let i = issue::Entity::find_by_id(issue_id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        (s.slug, i.slug)
    };

    // Mark removed_at so restore has something to clear.
    {
        use entity::issue;
        let row = issue::Entity::find_by_id(issue_id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut am: issue::ActiveModel = row.into();
        am.removed_at = Set(Some(Utc::now().fixed_offset()));
        am.update(&db).await.unwrap();
    }

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_slug}/issues/{issue_slug}/restore"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let entries = audit_entries_for(&app, &admin, "admin.issue.restore").await;
    let hit = entries
        .iter()
        .find(|e| e["target_id"] == issue_id)
        .expect("audit entry for issue restore");
    assert_eq!(hit["action"], "admin.issue.restore");
    assert_eq!(hit["target_type"], "issue");
    assert_eq!(hit["payload"]["series_slug"], series_slug);
    assert_eq!(hit["payload"]["issue_slug"], issue_slug);
}

#[tokio::test]
async fn reconcile_confirm_writes_audit_row() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let lib_id = common::seed::seed_library(&db, tmp.path()).await;
    let series_id = common::seed::seed_series(&db, lib_id, "Confirm Series").await;
    let file_path = tmp.path().join("issue-c.cbz");
    std::fs::write(&file_path, b"").unwrap();
    let issue_id = common::seed::IssueSeed::new(lib_id, series_id, &file_path, b"", 1.0)
        .insert(&db)
        .await;

    let (series_slug, issue_slug) = {
        use entity::{issue, series};
        let s = series::Entity::find_by_id(series_id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let i = issue::Entity::find_by_id(issue_id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        (s.slug, i.slug)
    };

    // Mark removed_at so confirm-removal accepts it.
    {
        use entity::issue;
        let row = issue::Entity::find_by_id(issue_id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut am: issue::ActiveModel = row.into();
        am.removed_at = Set(Some(Utc::now().fixed_offset()));
        am.update(&db).await.unwrap();
    }

    let resp = admin_send(
        &app,
        &admin,
        Method::POST,
        &format!("/api/series/{series_slug}/issues/{issue_slug}/confirm-removal"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let entries = audit_entries_for(&app, &admin, "admin.issue.confirm_removal").await;
    let hit = entries
        .iter()
        .find(|e| e["target_id"] == issue_id)
        .expect("audit entry for issue confirm-removal");
    assert_eq!(hit["action"], "admin.issue.confirm_removal");
    assert_eq!(hit["target_type"], "issue");
}
