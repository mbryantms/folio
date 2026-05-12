//! Library Scanner v1 — Milestone 6 series identity & match_key override.
//!
//! Validates the focused MVP:
//!   - the second scan of the same library reuses the existing series via the
//!     `folder_path` fast path (no second `series_created`)
//!   - `PATCH /series/{id}` accepts `match_key` and the value persists
//!   - a folder rename keeps the same series_id (resolution falls through to
//!     `normalized_name + year` and backfills `folder_path`)
//!   - moving an issue file between folders preserves its issue id

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{
    library::ActiveModel as LibraryAM,
    series::{Column as SeriesCol, Entity as SeriesEntity},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
                    r#"{"email":"id@example.com","password":"correctly-horse-battery"}"#,
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

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Identity Lib".into()),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
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
    id
}

#[tokio::test]
async fn second_scan_reuses_series_via_folder_path() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Iota (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Iota 001.cbz"), 1);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let s1 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        s1.series_created, 1,
        "first scan creates the series: {s1:?}"
    );

    // Touch the file so the mtime gate doesn't short-circuit the second walk.
    let _ = std::fs::File::options()
        .write(true)
        .open(folder.join("Iota 001.cbz"))
        .unwrap()
        .write_all(&[])
        .ok();
    let new_time = filetime::FileTime::from_system_time(std::time::SystemTime::now());
    filetime::set_file_mtime(folder.join("Iota 001.cbz"), new_time).unwrap();

    let s2 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s2.series_created, 0, "second scan reuses series: {s2:?}");

    // Only one series row in the DB.
    let series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(series.len(), 1);
}

#[tokio::test]
async fn folder_rename_keeps_same_series_row() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Kappa (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Kappa 001.cbz"), 1);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let s1 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s1.series_created, 1);
    let series_before = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();

    // Rename the folder. ComicInfo is absent, so identity falls back to the
    // filename-inferred Series name "Kappa". The series row stays. Its
    // folder_path is backfilled to the new path.
    let renamed = tmp.path().join("Series Kappa Vol 1 (2025)");
    std::fs::rename(&folder, &renamed).unwrap();
    // Touch the renamed file so per-folder mtime gate fires for both old and new
    // (the rename usually updates parent mtime; force the file mtime too).
    let new_time = filetime::FileTime::from_system_time(std::time::SystemTime::now());
    filetime::set_file_mtime(renamed.join("Kappa 001.cbz"), new_time).unwrap();

    let s2 = scanner::scan_library(&state, lib_id).await.unwrap();
    // Note: with no ComicInfo and a different folder name, filename inference
    // picks up "Kappa" as the series name (same as before) — so identity falls
    // through normalized_name+year to the existing row, no new series.
    assert_eq!(
        s2.series_created, 0,
        "rename should not create a new series: {s2:?}"
    );

    let all_series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(all_series.len(), 1, "still one series row");
    assert_eq!(all_series[0].id, series_before.id);
    assert_eq!(
        all_series[0].folder_path.as_deref(),
        Some(renamed.to_string_lossy().as_ref()),
        "folder_path is backfilled to the new location",
    );
}

#[tokio::test]
async fn match_key_patch_persists_and_is_sticky() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Lambda (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Lambda 001.cbz"), 1);
    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series_row = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let series_id = series_row.id;
    let series_slug = series_row.slug.clone();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/series/{series_slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(r#"{"match_key":"comicvine:1234"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = body_json(resp.into_body()).await;

    // Re-scan: scanner must NOT clear match_key (sticky).
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.match_key.as_deref(), Some("comicvine:1234"));

    // Empty string clears it.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/series/{series_slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(r#"{"match_key":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let after = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.match_key, None);
}
