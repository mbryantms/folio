//! `DELETE /libraries/{id}` — hard delete with on-disk + cascade cleanup.
//!
//! Verifies:
//!   - non-admin gets 403, unknown id gets 404
//!   - cascade wipes series + issues + scan_runs + library_health_issues
//!   - manual cleanup of `library_user_access` (no FK)
//!   - on-disk thumbnails for the library's issues are wiped
//!   - audit log row survives the delete (audit is append-only)

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    audit_log,
    issue::{ActiveModel as IssueAM, Entity as IssueEntity},
    library::{self, ActiveModel as LibraryAM, Entity as LibraryEntity},
    library_user_access::{ActiveModel as AccessAM, Entity as AccessEntity},
    scan_run::{ActiveModel as ScanRunAM, Entity as ScanRunEntity},
    series::{ActiveModel as SeriesAM, Entity as SeriesEntity, normalize_name},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use server::library::thumbnails::{self, ThumbFormat};
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"lib-delete-admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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

async fn register_regular_user(app: &TestApp, email: &str) -> Authed {
    // First user becomes admin; second is a regular user.
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

/// Seed a library with one series, one active issue, one scan run, one
/// library_user_access grant, and on-disk cover/strip thumbs for the
/// issue. Returns the library id and the issue id.
async fn seed(app: &TestApp) -> (Uuid, String) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();

    LibraryAM {
        id: Set(lib_id),
        name: Set("Doomed Lib".into()),
        root_path: Set(format!("/tmp/doomed-{lib_id}")),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(lib_id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Doomed Series".into()),
        normalized_name: Set(normalize_name("Doomed Series")),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("ongoing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("eng".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(series_id.to_string()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let issue_id = format!("{:0>64}", "deadbeef");
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set("/tmp/doomed/issue1.cbz".into()),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(0.0)),
        number_raw: Set(None),
        volume: Set(None),
        year: Set(None),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(Some(1)),
        pages: Set(serde_json::json!([])),
        comic_info_raw: Set(serde_json::json!({})),
        alternate_series: Set(None),
        story_arc: Set(None),
        story_arc_number: Set(None),
        characters: Set(None),
        teams: Set(None),
        locations: Set(None),
        tags: Set(None),
        genre: Set(None),
        writer: Set(None),
        penciller: Set(None),
        inker: Set(None),
        colorist: Set(None),
        letterer: Set(None),
        cover_artist: Set(None),
        editor: Set(None),
        translator: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        scan_information: Set(None),
        community_rating: Set(None),
        review: Set(None),
        web_url: Set(None),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        superseded_by: Set(None),
        special_type: Set(None),
        slug: Set(uuid::Uuid::now_v7().to_string()),
        hash_algorithm: Set(1),
        thumbnails_generated_at: Set(Some(now)),
        thumbnail_version: Set(1),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    // Scan run row — cascades on library delete.
    ScanRunAM {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib_id),
        state: Set("completed".into()),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        error: Set(None),
        stats: Set(serde_json::json!({})),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    // Drop a fake cover thumb on disk. We only need a file at the right
    // path for the wipe sweep to have something to remove.
    let cover_path =
        thumbnails::cover_path(&app.state().cfg.data_path, &issue_id, ThumbFormat::Webp);
    std::fs::create_dir_all(cover_path.parent().unwrap()).unwrap();
    std::fs::write(&cover_path, b"fake-cover").unwrap();

    (lib_id, issue_id)
}

async fn delete_library(
    app: &TestApp,
    auth: &Authed,
    lib_id: Uuid,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/libraries/{lib_id}"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn delete_library_cascades_and_wipes_thumbs() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, issue_id) = seed(&app).await;

    // Add a library_user_access row pointing at the doomed library so we
    // can verify the manual cleanup.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let actor_user_id = entity::user::Entity::find()
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .id;
    AccessAM {
        library_id: Set(lib_id),
        user_id: Set(actor_user_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        updated_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await
    .unwrap();

    let cover_path =
        thumbnails::cover_path(&app.state().cfg.data_path, &issue_id, ThumbFormat::Webp);
    assert!(cover_path.exists(), "fake cover should exist before delete");

    let (status, body) = delete_library(&app, &auth, lib_id).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["deleted_issues"], 1);
    assert_eq!(body["deleted_series"], 1);
    assert_eq!(body["thumbs_swept"], 1);

    // Library row gone.
    assert!(
        LibraryEntity::find_by_id(lib_id)
            .one(&db)
            .await
            .unwrap()
            .is_none(),
        "library should be deleted",
    );
    // Cascades: series, issues, scan_runs.
    let series_count = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(series_count, 0, "series should cascade delete");
    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(issue_count, 0, "issues should cascade delete");
    let scan_count = ScanRunEntity::find()
        .filter(entity::scan_run::Column::LibraryId.eq(lib_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(scan_count, 0, "scan_runs should cascade delete");
    let access_count = AccessEntity::find()
        .filter(entity::library_user_access::Column::LibraryId.eq(lib_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(
        access_count, 0,
        "library_user_access should be manually purged"
    );

    // On-disk cover wiped.
    assert!(!cover_path.exists(), "cover thumb should be wiped");

    // Audit row survives.
    let audit_count = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.delete"))
        .filter(audit_log::Column::TargetId.eq(lib_id.to_string()))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(audit_count, 1, "audit row must survive delete");
}

#[tokio::test]
async fn delete_library_404_for_unknown() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (status, _) = delete_library(&app, &auth, Uuid::now_v7()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_library_403_for_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_admin(&app).await;
    let user = register_regular_user(&app, "regular@example.com").await;
    let (lib_id, _) = seed(&app).await;
    let (status, _) = delete_library(&app, &user, lib_id).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// Suppress unused-import warnings for helpers used only by some tests.
#[allow(dead_code)]
fn _unused(_: &Path) {}

#[allow(dead_code)]
fn _unused_lib(_: library::Model) {}
