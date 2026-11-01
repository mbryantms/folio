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
    SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await
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
    assert!(
        body["external_url"]
            .as_str()
            .unwrap()
            .contains("4050-12345")
    );

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

/// Audit 2026-11 N-A4: the add body validates through `Validated<T>`, so
/// an unknown source / empty id is a 422 whose `error.details` names the
/// field (the `<ExternalIdsCard>` add form binds it to the input), not
/// the old 400 `metadata.invalid_source`. Malformed JSON stays 400.
#[tokio::test]
async fn external_ids_add_422_names_the_field() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let series_id = seed_series(&app).await;
    let path = format!("/api/series/{series_id}/external-ids");

    let resp = send(
        &app,
        &admin,
        Method::POST,
        &path,
        Some(json!({"source": "wat", "external_id": "x"})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation", "{body:#?}");
    assert_eq!(body["error"]["message"], "source: unknown source");
    assert_eq!(body["error"]["details"][0]["field"], "source");
    assert_eq!(body["error"]["details"][0]["message"], "unknown source");

    let resp = send(
        &app,
        &admin,
        Method::POST,
        &path,
        Some(json!({"source": "metron", "external_id": "   "})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["details"][0]["field"], "external_id");
    assert_eq!(
        body["error"]["details"][0]["message"],
        "external_id required"
    );

    // Parse-shape failure stays 400 (no field to bind).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&path)
                .header(header::COOKIE, admin.cookie())
                .header("x-csrf-token", &admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation");
    assert!(body["error"]["details"].is_null(), "{body:#?}");
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

async fn user_id_by_email(app: &TestApp, email: &str) -> Uuid {
    entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("user row")
        .id
}

async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    use sea_orm::{ActiveModelTrait, Set};
    let now = chrono::Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// `(source, external_id, set_by)` for every `external_ids` row on the
/// entity, sorted — the "row is unchanged" probe.
async fn id_rows(
    app: &TestApp,
    entity_type: &str,
    entity_id: &str,
) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq(entity_type))
        .filter(entity::external_id::Column::EntityId.eq(entity_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.source, r.external_id, r.set_by))
        .collect();
    rows.sort();
    rows
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

/// Audit 2026-11 E13: external-id add / unlink are admin writes, like the
/// series and issue `PATCH`. A reader with library access gets 403 on all
/// four (and the rows stay as they were, with no audit row), while the
/// list endpoints still answer them. The admin path keeps working and
/// records its `admin.*.external_id_*` audit rows.
#[tokio::test]
async fn external_id_writes_require_admin_and_lists_stay_open_to_readers() {
    use common::seed::IssueSeed;
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let reader = register_authed(&app, "reader@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("issue.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&app.state().db)
        .await;
    let issue_slug = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let reader_id = user_id_by_email(&app, "reader@example.com").await;
    grant_library(&app, reader_id, lib_id).await;

    let series_path = format!("/api/series/{series_id}/external-ids");
    let issue_path = format!("/api/series/{series_id}/issues/{issue_slug}/external-ids");

    // Admin links one id on each entity: still allowed, still audited.
    for path in [&series_path, &issue_path] {
        let resp = send(
            &app,
            &admin,
            Method::POST,
            path,
            Some(json!({"source": "comicvine", "external_id": "12345"})),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED, "admin add {path}");
    }
    let series_before = id_rows(&app, "series", &series_id.to_string()).await;
    let issue_before = id_rows(&app, "issue", &issue_id).await;
    assert_eq!(series_before.len(), 1);
    assert_eq!(issue_before.len(), 1);
    let audit_before = audit_actions(&app).await;
    assert!(audit_before.contains(&"admin.series.external_id_set".to_owned()));
    assert!(audit_before.contains(&"admin.issue.external_id_set".to_owned()));

    // The reader can see the library, so the lists answer them.
    for path in [&series_path, &issue_path] {
        let resp = get(&app, &reader, path).await;
        assert_eq!(resp.status(), StatusCode::OK, "reader list {path}");
        let body = body_json(resp.into_body()).await;
        let rows = body["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{body:#?}");
        assert_eq!(rows[0]["external_id"], "12345");
    }

    // Each write is refused before the handler body runs: 403 in the
    // canonical envelope, whether it would add, re-point or unlink.
    let writes = [
        (
            Method::POST,
            series_path.clone(),
            Some(json!({"source": "comicvine", "external_id": "99999"})),
        ),
        (
            Method::POST,
            series_path.clone(),
            Some(json!({"source": "metron", "external_id": "7"})),
        ),
        (Method::DELETE, format!("{series_path}/comicvine"), None),
        (
            Method::POST,
            issue_path.clone(),
            Some(json!({"source": "comicvine", "external_id": "99999"})),
        ),
        (
            Method::POST,
            issue_path.clone(),
            Some(json!({"source": "metron", "external_id": "7"})),
        ),
        (Method::DELETE, format!("{issue_path}/comicvine"), None),
    ];
    for (method, path, body) in writes {
        let resp = send(&app, &reader, method.clone(), &path, body).await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "reader {method} {path}"
        );
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"]["code"], "auth.permission_denied", "{body:#?}");
    }
    assert_eq!(
        id_rows(&app, "series", &series_id.to_string()).await,
        series_before
    );
    assert_eq!(id_rows(&app, "issue", &issue_id).await, issue_before);
    assert_eq!(audit_actions(&app).await.len(), audit_before.len());

    // The admin's unlinks still go through and are audited.
    for path in [&series_path, &issue_path] {
        let resp = send(
            &app,
            &admin,
            Method::DELETE,
            &format!("{path}/comicvine"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "admin unlink {path}");
    }
    assert!(
        id_rows(&app, "series", &series_id.to_string())
            .await
            .is_empty()
    );
    assert!(id_rows(&app, "issue", &issue_id).await.is_empty());
    let audit_after = audit_actions(&app).await;
    assert!(audit_after.contains(&"admin.series.external_id_unlink".to_owned()));
    assert!(audit_after.contains(&"admin.issue.external_id_unlink".to_owned()));
}

// ───────── /issues/{id}/covers ─────────

#[tokio::test]
async fn issue_covers_lists_active_rows_with_fallback_url() {
    use common::seed::IssueSeed;
    use sea_orm::{ActiveModelTrait, Set};
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("issue.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&app.state().db)
        .await;

    // Seed two issue_cover rows — one primary, one variant.
    let now = chrono::Utc::now().fixed_offset();
    entity::issue_cover::ActiveModel {
        id: Set(Uuid::now_v7()),
        issue_id: Set(issue_id.clone()),
        kind: Set(entity::issue_cover::IssueCoverKind::Primary),
        ordinal: Set(0),
        source_provider: Set(Some("comicvine".into())),
        source_external_id: Set(Some("67890".into())),
        source_url: Set(Some("https://cdn/super.jpg".into())),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set(format!("thumbs/issues/{issue_id}/covers/00.jpg")),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(now),
        is_active: Set(true),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    entity::issue_cover::ActiveModel {
        id: Set(Uuid::now_v7()),
        issue_id: Set(issue_id.clone()),
        kind: Set(entity::issue_cover::IssueCoverKind::Variant),
        ordinal: Set(1),
        source_provider: Set(Some("comicvine".into())),
        source_external_id: Set(Some("67891".into())),
        source_url: Set(Some("https://cdn/variant-b.jpg".into())),
        variant_label: Set(Some("Cover B".into())),
        variant_artist_person_id: Set(None),
        local_path: Set(format!("thumbs/issues/{issue_id}/covers/01.jpg")),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(now),
        is_active: Set(true),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let resp = get(&app, &admin, &format!("/api/issues/{issue_id}/covers")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let covers = body["covers"].as_array().unwrap();
    assert_eq!(covers.len(), 2);
    // Primary first (sorted by kind, ordinal).
    let primary = covers.iter().find(|c| c["kind"] == "primary").unwrap();
    assert_eq!(primary["source_url"], "https://cdn/super.jpg");
    let variant = covers.iter().find(|c| c["kind"] == "variant").unwrap();
    assert_eq!(variant["variant_label"], "Cover B");
    // Fallback URL points at page 0 thumb.
    let fb = body["fallback_primary_url"].as_str().unwrap();
    assert!(fb.contains("/pages/0/thumb"));
}

#[tokio::test]
async fn issue_covers_404_when_issue_missing() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let resp = get(&app, &admin, "/api/issues/no-such-issue/covers").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
