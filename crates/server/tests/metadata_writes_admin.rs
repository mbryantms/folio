//! Audit 2026-11 E16: the shared metadata writes are admin-only.
//!
//! Single + composite apply, batch create / batch apply, per-issue accept /
//! unaccept, the series Metadata tab's bulk accept, pause / resume, the
//! library-wide refresh and `PATCH /me/issues/bulk-metadata` take
//! `RequireAdmin`. A reader **with** library access gets 403
//! `auth.permission_denied` on every one and nothing changes; the read
//! endpoints (search, candidates) still answer the reader. The admin's
//! writes succeed and record their audit rows.
//!
//! The apalis workers don't run here, so the queued apply only enqueues
//! (its audit row is written by the job, covered in the job tests).

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
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

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

async fn register_authed(app: &TestApp, email: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
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
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, auth.cookie())
        .header("x-csrf-token", &auth.csrf);
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn grant_library(app: &TestApp, email: &str, lib_id: Uuid) {
    use entity::{library_user_access, user};
    let user_row = user::Entity::find()
        .filter(user::Column::Email.eq(email))
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
}

async fn audit_actions(app: &TestApp) -> Vec<String> {
    entity::audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect()
}

/// A completed series run with one high candidate (ordinal 0).
async fn seed_completed_series_run(app: &TestApp, series_id: Uuid) -> Uuid {
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
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    entity::metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set("12345".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({})),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

struct Fixture {
    app: TestApp,
    admin: Authed,
    reader: Authed,
    lib_id: Uuid,
    lib_slug: String,
    series_id: Uuid,
    issue_id: String,
    issue_slug: String,
    _dir: tempfile::TempDir,
}

/// Admin (first user) + a reader granted the library; one series with one
/// issue.
async fn fixture() -> Fixture {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let admin = register_authed(&app, "admin@example.com").await;
    let reader = register_authed(&app, "reader@example.com").await;
    let dir = tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path()).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .with_publisher("Image Comics")
        .insert(db)
        .await;
    grant_library(&app, "reader@example.com", lib_id).await;
    let issue_id = IssueSeed::new(lib_id, series_id, &dir.path().join("saga-1.cbz"), b"x", 1.0)
        .insert(db)
        .await;
    let issue_slug = entity::issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let lib_slug = entity::library::Entity::find_by_id(lib_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    Fixture {
        app,
        admin,
        reader,
        lib_id,
        lib_slug,
        series_id,
        issue_id,
        issue_slug,
        _dir: dir,
    }
}

fn assert_admin_only(status: StatusCode, body: &Value, what: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
    assert_eq!(body["error"]["code"], "auth.permission_denied", "{what}");
}

#[tokio::test]
async fn metadata_writes_require_admin_reads_stay_open() {
    let f = fixture().await;
    let app = &f.app;
    let db = &app.state().db;
    let series_id = f.series_id;
    let run_id = seed_completed_series_run(app, series_id).await;
    let issue_base = format!("/api/series/{series_id}/issues/{}", f.issue_slug);

    // Reads stay open to the reader.
    let (st, _) = send(
        app,
        &f.reader,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/search"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "reader search");
    let (st, body) = send(
        app,
        &f.reader,
        Method::GET,
        &format!("/api/series/{series_id}/metadata/candidates?run_id={run_id}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "reader candidates");
    assert_eq!(body["candidates"].as_array().map(Vec::len), Some(1));

    // Every write 403s for the reader before the handler runs.
    let audit_before = audit_actions(app).await;
    let bogus = Uuid::now_v7();
    let writes: Vec<(Method, String, Option<Value>)> = vec![
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/apply"),
            Some(json!({"run_id": run_id, "ordinal": 0})),
        ),
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/composite-apply"),
            Some(json!({"run_id": run_id, "field_sources": [], "included": [0]})),
        ),
        (
            Method::POST,
            format!("{issue_base}/metadata/apply"),
            Some(json!({"run_id": run_id, "ordinal": 0})),
        ),
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/batch"),
            None,
        ),
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/batch/selection"),
            Some(json!({"issue_ids": [f.issue_id]})),
        ),
        (
            Method::POST,
            "/api/metadata/batch/saved-view".to_owned(),
            Some(json!({"saved_view_id": bogus})),
        ),
        (
            Method::POST,
            format!("/api/metadata/batch/{bogus}/apply"),
            Some(json!({"filter": "all_strong"})),
        ),
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/pause"),
            None,
        ),
        (
            Method::POST,
            format!("/api/series/{series_id}/metadata/resume"),
            None,
        ),
        (Method::POST, format!("{issue_base}/metadata/accept"), None),
        (
            Method::DELETE,
            format!("{issue_base}/metadata/accept"),
            None,
        ),
        (
            Method::POST,
            format!("/api/libraries/{}/metadata/refresh?scope=all", f.lib_slug),
            None,
        ),
    ];
    for (method, path, body) in writes {
        let (st, resp) = send(app, &f.reader, method.clone(), &path, body).await;
        assert_admin_only(st, &resp, &format!("{method} {path}"));
    }

    // Nothing changed.
    assert_eq!(
        entity::metadata_batch::Entity::find()
            .count(db)
            .await
            .unwrap(),
        0,
        "no batch created"
    );
    let s = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(!s.metadata_sync_paused, "series not paused");
    let i = entity::issue::Entity::find_by_id(&f.issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        i.metadata_review_accepted_at.is_none(),
        "issue not accepted"
    );
    let cand = entity::metadata_run_candidate::Entity::find_by_id((run_id, 0))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(cand.applied_at.is_none(), "candidate not applied");
    assert_eq!(audit_actions(app).await, audit_before, "no audit rows");

    // The admin's writes succeed and record their audit rows.
    let (st, _) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/pause"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "admin pause");
    let (st, _) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("{issue_base}/metadata/accept"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "admin accept");
    let (st, created) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/batch"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "admin batch: {created}");
    let batch_id = created["batch_id"].as_str().unwrap().to_owned();
    let (st, _) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("/api/metadata/batch/{batch_id}/apply"),
        Some(json!({"filter": "all_strong"})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "admin batch apply");
    let (st, _) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("/api/libraries/{}/metadata/refresh?scope=all", f.lib_slug),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "admin refresh");
    // The single apply only enqueues; the job writes its audit row.
    let (st, _) = send(
        app,
        &f.admin,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/apply"),
        Some(json!({"run_id": run_id, "ordinal": 0})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "admin apply");

    let actions = audit_actions(app).await;
    for want in [
        "admin.series.metadata_pause",
        "metadata.issue.accept",
        "admin.metadata_batch.create",
        "admin.metadata_batch.apply",
        "admin.library.metadata_refresh",
    ] {
        assert!(actions.iter().any(|a| a == want), "{want} in {actions:?}");
    }
    let refresh = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.library.metadata_refresh"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refresh.target_id.as_deref(),
        Some(f.lib_id.to_string().as_str())
    );
    assert_eq!(refresh.payload["scope"], "all");
    let batch_row = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.metadata_batch.create"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch_row.target_id.as_deref(), Some(batch_id.as_str()));
    assert_eq!(batch_row.payload["source"], "series");
}

/// The series Metadata tab's bulk "Mark complete" is admin-only like the
/// per-issue accept.
#[tokio::test]
async fn series_tab_bulk_accept_requires_admin() {
    let f = fixture().await;
    let app = &f.app;
    let path = format!("/api/series/{}/metadata/issues/accept", f.series_id);
    let body = json!({"issue_ids": [f.issue_id], "accepted": true});
    let audit_before = audit_actions(app).await;

    let (st, resp) = send(app, &f.reader, Method::POST, &path, Some(body.clone())).await;
    assert_admin_only(st, &resp, "reader bulk accept");
    let i = entity::issue::Entity::find_by_id(&f.issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(i.metadata_review_accepted_at.is_none());
    assert_eq!(audit_actions(app).await, audit_before, "no audit rows");

    let (st, resp) = send(app, &f.admin, Method::POST, &path, Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert_eq!(resp["updated"], 1);
    assert!(
        audit_actions(app)
            .await
            .iter()
            .any(|a| a == "metadata.issue.accept_bulk")
    );
}

/// `PATCH /me/issues/bulk-metadata` writes the same columns + user pins as
/// the admin-only issue PATCH, so it takes the same gate.
#[tokio::test]
async fn bulk_metadata_edit_requires_admin() {
    let f = fixture().await;
    let app = &f.app;
    let body = json!({
        "issue_ids": [f.issue_id],
        "patch": {"language_code": "ja"},
    });
    let audit_before = audit_actions(app).await;

    let (st, resp) = send(
        app,
        &f.reader,
        Method::PATCH,
        "/api/me/issues/bulk-metadata",
        Some(body.clone()),
    )
    .await;
    assert_admin_only(st, &resp, "reader bulk edit");
    let i = entity::issue::Entity::find_by_id(&f.issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(i.language_code.as_deref(), Some("ja"));
    assert_eq!(audit_actions(app).await, audit_before, "no audit rows");

    let (st, resp) = send(
        app,
        &f.admin,
        Method::PATCH,
        "/api/me/issues/bulk-metadata",
        Some(body),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert_eq!(resp["updated"], 1);
    assert_eq!(resp["forbidden"], 0);
    let i = entity::issue::Entity::find_by_id(&f.issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(i.language_code.as_deref(), Some("ja"));
    assert!(
        audit_actions(app)
            .await
            .iter()
            .any(|a| a == "admin.issue.bulk_metadata_update")
    );
}
