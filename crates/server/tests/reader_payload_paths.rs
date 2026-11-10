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

async fn get_json(app: &TestApp, user: &Authed, path: &str) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        user.session, user.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
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
    let list = get_json(&f.app, &f.reader, "/api/libraries").await;
    let lib = &list.as_array().expect("array")[0];
    assert_eq!(keys(lib), ["id", "name", "slug"]);

    let one = get_json(&f.app, &f.reader, &format!("/api/libraries/{}", f.lib_id)).await;
    assert_eq!(keys(&one), ["id", "name", "slug"]);
}

#[tokio::test]
async fn admin_library_views_keep_paths_and_settings() {
    let f = setup().await;
    for v in [
        get_json(&f.app, &f.admin, "/api/libraries")
            .await
            .as_array()
            .unwrap()[0]
            .clone(),
        get_json(&f.app, &f.admin, &format!("/api/libraries/{}", f.lib_id)).await,
    ] {
        assert_eq!(v["root_path"], json!(f.root));
        for key in [
            "default_language",
            "default_reading_direction",
            "ignore_globs",
            "soft_delete_days",
            "allow_archive_writeback",
            "metadata_writeback_enabled",
            "metadata_publisher_blacklist",
            "root_path_writable",
            "show_in_sidebar_by_default",
            "last_scan_at",
            "scan_schedule_cron",
            "cbr_convert_confirmed_at",
        ] {
            assert!(v.get(key).is_some(), "admin view lacks {key}");
        }
    }
}

#[tokio::test]
async fn issue_detail_hides_path_and_library_flags_from_reader() {
    let f = setup().await;
    let path = format!("/api/series/{}/issues/{}", f.series_slug, f.issue_slug);

    let reader = get_json(&f.app, &f.reader, &path).await;
    for key in [
        "file_path",
        "allow_archive_writeback",
        "library_cbr_convert_confirmed",
    ] {
        assert!(reader.get(key).is_none(), "reader issue detail has {key}");
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
    assert!(reader["files"].get("folder_path").is_none());

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
