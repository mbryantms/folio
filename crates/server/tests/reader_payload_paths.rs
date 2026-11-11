//! Server paths and library settings are admin-only. A reader's raw JSON
//! leaves those keys out entirely; the admin still receives them with the
//! same values as before.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{library_health_issue, library_user_access, series, user::Entity as UserEntity};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, Unchanged};
use serde_json::{Value, json};
use server::api::libraries::LibraryView;
use tower::ServiceExt;
use uuid::Uuid;

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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("user row");
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: user_row.id,
    }
}

/// How a request authenticates: the browser's session cookie, or the
/// same session token sent as a Bearer JWT.
#[derive(Clone, Copy)]
enum Cred {
    Cookie,
    Bearer,
}

async fn send(
    app: &TestApp,
    user: &Authed,
    cred: Cred,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    req = match cred {
        Cred::Cookie => {
            let r = req.header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    user.session, user.csrf
                ),
            );
            if method == Method::GET {
                r
            } else {
                r.header("X-CSRF-Token", &user.csrf)
            }
        }
        Cred::Bearer => req.header(header::AUTHORIZATION, format!("Bearer {}", user.session)),
    };
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
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn get_as(app: &TestApp, user: &Authed, cred: Cred, path: &str) -> Value {
    let (status, bytes) = send(app, user, cred, Method::GET, path, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_json(app: &TestApp, user: &Authed, path: &str) -> Value {
    get_as(app, user, Cred::Cookie, path).await
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
    k.sort();
    k
}

struct Fixture {
    app: TestApp,
    admin: Authed,
    reader: Authed,
    lib_id: Uuid,
    root: String,
    series_slug: String,
    issue_slug: String,
    issue_path: String,
    folder: String,
    _tmp: tempfile::TempDir,
}

async fn setup() -> Fixture {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let db = &app.state().db;

    let tmp = tempfile::tempdir().unwrap();
    let lib_id = LibrarySeed::new(tmp.path()).insert(db).await;
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib_id),
        user_id: Set(reader.user_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    let folder = tmp.path().join("Saga");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.to_string_lossy().into_owned();
    series::ActiveModel {
        id: Unchanged(series_id),
        folder_path: Set(Some(folder.clone())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();

    let file = tmp.path().join("Saga").join("Saga 001.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &file, b"\x89PNG\r\n\x1a\nsaga", 1.0)
        .insert(db)
        .await;
    let issue = entity::issue::Entity::find_by_id(issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let issue_path = issue.file_path.clone();

    library_health_issue::ActiveModel {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib_id),
        scan_id: Set(None),
        kind: Set(library_health_issue::HealthIssueKind::SkippedArchiveEntries),
        payload: Set(json!({
            "kind": "skipped_archive_entries",
            "data": {"path": issue_path, "dropped": 1, "total": 3, "reason": "no image signature"}
        })),
        severity: Set("warning".into()),
        fingerprint: Set(format!(
            "SkippedArchiveEntries:{issue_path}:no image signature"
        )),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
        resolved_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();

    Fixture {
        admin,
        reader,
        lib_id,
        root: tmp.path().to_string_lossy().into_owned(),
        series_slug: series_id.to_string(),
        issue_slug: issue.slug,
        issue_path,
        folder,
        app,
        _tmp: tmp,
    }
}

#[tokio::test]
async fn reader_library_list_carries_only_id_name_slug() {
    let f = setup().await;
    for cred in [Cred::Cookie, Cred::Bearer] {
        let list = get_as(&f.app, &f.reader, cred, "/api/libraries").await;
        let lib = &list.as_array().expect("array")[0];
        assert_eq!(keys(lib), ["id", "name", "slug"]);

        let one = get_as(
            &f.app,
            &f.reader,
            cred,
            &format!("/api/libraries/{}", f.lib_id),
        )
        .await;
        assert_eq!(keys(&one), ["id", "name", "slug"]);
    }
}

/// Every key of the admin's library view: id, name, slug and the 27
/// admin-only fields.
const ADMIN_LIBRARY_KEYS: &[&str] = &[
    "allow_archive_writeback",
    "archive_backup_retain_count",
    "archive_backup_retain_days",
    "archive_writeback_jpeg_quality",
    "auto_convert_cb7_on_scan",
    "auto_convert_cbr_on_scan",
    "auto_search_new_series",
    "cbr_convert_confirmed_at",
    "dedupe_by_content",
    "default_language",
    "default_reading_direction",
    "file_watch_enabled",
    "filename_assume_issue_one",
    "filename_ignore_leading_numbers",
    "generate_page_thumbs_on_scan",
    "id",
    "ignore_globs",
    "last_scan_at",
    "metadata_auto_apply_strong_matches",
    "metadata_publisher_blacklist",
    "metadata_writeback_enabled",
    "name",
    "report_missing_comicinfo",
    "root_path",
    "root_path_writable",
    "scan_schedule_cron",
    "show_in_sidebar_by_default",
    "slug",
    "soft_delete_days",
    "trust_fingerprint_on_first_import",
];

#[tokio::test]
async fn admin_library_views_keep_paths_and_settings() {
    let f = setup().await;
    let model = entity::library::Entity::find_by_id(f.lib_id)
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap();
    let expected = serde_json::to_value(LibraryView::from(model)).unwrap();
    assert_eq!(expected["root_path"], json!(f.root));
    for v in [
        get_json(&f.app, &f.admin, "/api/libraries")
            .await
            .as_array()
            .unwrap()[0]
            .clone(),
        get_json(&f.app, &f.admin, &format!("/api/libraries/{}", f.lib_id)).await,
    ] {
        assert_eq!(keys(&v), ADMIN_LIBRARY_KEYS, "admin view key set");
        assert_eq!(v, expected, "admin view equals the full serialized view");
    }
}

#[tokio::test]
async fn issue_detail_hides_path_and_library_flags_from_reader() {
    let f = setup().await;
    let path = format!("/api/series/{}/issues/{}", f.series_slug, f.issue_slug);

    for cred in [Cred::Cookie, Cred::Bearer] {
        let reader = get_as(&f.app, &f.reader, cred, &path).await;
        for key in [
            "file_path",
            "allow_archive_writeback",
            "library_cbr_convert_confirmed",
        ] {
            assert!(reader.get(key).is_none(), "reader issue detail has {key}");
        }
        assert!(
            !reader.to_string().contains(&f.issue_path),
            "reader issue detail names the archive path"
        );
    }

    let admin = get_json(&f.app, &f.admin, &path).await;
    assert_eq!(admin["file_path"], json!(f.issue_path));
    assert_eq!(admin["allow_archive_writeback"], json!(false));
    assert_eq!(admin["library_cbr_convert_confirmed"], json!(false));
}

#[tokio::test]
async fn metadata_overview_hides_series_folder_from_reader() {
    let f = setup().await;
    let path = format!("/api/series/{}/metadata/overview", f.series_slug);

    let reader = get_json(&f.app, &f.reader, &path).await;
    assert!(reader["files"].is_object());
    assert_eq!(reader["files"]["folder_path"], Value::Null);
    assert!(!reader.to_string().contains(&f.folder));

    let admin = get_json(&f.app, &f.admin, &path).await;
    assert_eq!(admin["files"]["folder_path"], json!(f.folder));
}

#[tokio::test]
async fn issue_health_hides_paths_from_reader() {
    let f = setup().await;
    let path = format!(
        "/api/series/{}/issues/{}/health-issues",
        f.series_slug, f.issue_slug
    );

    let reader = get_json(&f.app, &f.reader, &path).await;
    let row = &reader.as_array().expect("array")[0];
    assert!(row.get("fingerprint").is_none());
    assert!(!row.to_string().contains(&f.issue_path));
    assert_eq!(row["payload"]["data"]["dropped"], json!(1));
    assert_eq!(row["payload"]["data"]["total"], json!(3));
    assert!(row["payload"]["data"].get("path").is_none());

    let admin = get_json(&f.app, &f.admin, &path).await;
    let row = &admin.as_array().expect("array")[0];
    assert_eq!(row["payload"]["data"]["path"], json!(f.issue_path));
    assert_eq!(
        row["fingerprint"],
        json!(format!(
            "SkippedArchiveEntries:{}:no image signature",
            f.issue_path
        ))
    );
}

/// The admin-only PATCH answers with the same library flags GET carries.
#[tokio::test]
async fn admin_patch_response_keeps_library_flags() {
    let f = setup().await;
    let path = format!("/api/series/{}/issues/{}", f.series_slug, f.issue_slug);
    let get = get_json(&f.app, &f.admin, &path).await;
    let (status, bytes) = send(
        &f.app,
        &f.admin,
        Cred::Cookie,
        Method::PATCH,
        &path,
        Some(json!({"title": "Renamed"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let patched: Value = serde_json::from_slice(&bytes).unwrap();
    for key in [
        "file_path",
        "allow_archive_writeback",
        "library_cbr_convert_confirmed",
    ] {
        assert!(patched.get(key).is_some(), "PATCH response lacks {key}");
        assert_eq!(
            patched[key], get[key],
            "{key} differs between PATCH and GET"
        );
    }
}

/// A failed metadata run whose stored error names a server path, for
/// `scope` (`"series"` / `"issue"`) and `entity_id`.
async fn seed_failed_run(app: &TestApp, scope: &str, entity_id: &str, secret: &str) {
    let now = Utc::now().fixed_offset();
    entity::metadata_run::ActiveModel {
        id: Set(Uuid::now_v7()),
        scope: Set(scope.into()),
        scope_entity_id: Set(Some(entity_id.into())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("failed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(1),
        error_summary: Set(Some(format!("sidecar writeback failed: io at {secret}"))),
        resume_after: Set(None),
        batch_id: Set(None),
        provider_status: Set(Some(json!([{
            "source": "comicvine",
            "state": "failed",
            "candidates": 0,
            "error": format!("could not read {secret}")
        }]))),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// A run's stored error is the job's own text: a reader learns only that
/// the last update failed, with no path and no raw error; the admin still
/// reads the summary and the provider error.
#[tokio::test]
async fn candidates_hide_job_errors_from_reader() {
    let f = setup().await;
    let series_id = f.series_slug.clone();
    let issue_id = entity::issue::Entity::find()
        .filter(entity::issue::Column::Slug.eq(f.issue_slug.clone()))
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap()
        .id;
    seed_failed_run(&f.app, "series", &series_id, &f.issue_path).await;
    seed_failed_run(&f.app, "issue", &issue_id, &f.issue_path).await;

    for path in [
        format!("/api/series/{}/metadata/candidates", f.series_slug),
        format!(
            "/api/series/{}/issues/{}/metadata/candidates",
            f.series_slug, f.issue_slug
        ),
    ] {
        let reader = get_json(&f.app, &f.reader, &path).await;
        assert_eq!(
            reader["error_summary"],
            json!(server::api::metadata_search::READER_RUN_ERROR),
            "{path}"
        );
        let text = reader.to_string();
        assert!(!text.contains(&f.issue_path), "{path}: {text}");
        assert!(!text.contains("sidecar writeback failed"), "{path}: {text}");
        assert!(!text.contains("could not read"), "{path}: {text}");
        assert_eq!(reader["provider_status"][0]["state"], json!("failed"));

        let admin = get_json(&f.app, &f.admin, &path).await;
        assert_eq!(
            admin["error_summary"],
            json!(format!("sidecar writeback failed: io at {}", f.issue_path)),
            "{path}"
        );
        assert_eq!(
            admin["provider_status"][0]["error"],
            json!(format!("could not read {}", f.issue_path))
        );
    }
}
