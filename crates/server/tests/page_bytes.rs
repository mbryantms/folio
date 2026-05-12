//! Range / sniff / ETag edge cases for `GET /issues/{id}/pages/{n}`.
//!
//! Builds a real CBZ on disk, inserts a library + series + issue, registers
//! an admin user (becomes ACL-cleared via the role check), and exercises the
//! full byte stream and Range responses.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Set};
use std::io::Write;
use tower::ServiceExt;
use uuid::Uuid;

const PNG_HEADER: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Build a 256-byte payload that starts with the PNG magic so the server's
/// content-type sniff returns `image/png`. The remaining bytes are a
/// counter so range slices are easy to verify.
fn page_payload() -> Vec<u8> {
    let mut v = PNG_HEADER.to_vec();
    while v.len() < 256 {
        v.push((v.len() & 0xFF) as u8);
    }
    v
}

fn build_cbz(path: &std::path::Path, payload: &[u8]) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(payload).unwrap();
    zw.finish().unwrap();
}

async fn register_admin(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Pull out the session cookie value; that's our JWT.
    let session = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .expect("session cookie")
        .to_owned();
    session
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_session=")
        .to_owned()
}

async fn seed_issue(app: &TestApp, file_path: &std::path::Path, file_size: i64) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test Library".into()),
        root_path: Set(file_path.parent().unwrap().to_string_lossy().into_owned()),
        default_language: Set("en".into()),
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
        name: Set("Test Series".into()),
        normalized_name: Set(normalize_name("Test Series")),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(series_id.to_string()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(file_path.parent().map(|p| p.to_string_lossy().into_owned())),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    // BLAKE3 of the file is the issue id.
    let bytes = std::fs::read(file_path).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();

    IssueAM {
        id: Set(hash.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set(file_path.to_string_lossy().into_owned()),
        file_size: Set(file_size),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(hash.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
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
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    hash
}

async fn body_bytes(b: Body) -> Vec<u8> {
    to_bytes(b, usize::MAX).await.unwrap().to_vec()
}

#[tokio::test]
async fn full_body_returns_200_with_sniffed_content_type() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("test.cbz");
    let payload = page_payload();
    build_cbz(&cbz, &payload);
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
    let cd = resp.headers().get(header::CONTENT_DISPOSITION).unwrap();
    assert!(cd.to_str().unwrap().contains("page-0.png"));
    let body = body_bytes(resp.into_body()).await;
    assert_eq!(body, payload);
}

#[tokio::test]
async fn range_request_returns_206_with_content_range() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("range.cbz");
    let payload = page_payload();
    build_cbz(&cbz, &payload);
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    // Mid-range
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::RANGE, "bytes=100-109")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let cr = resp
        .headers()
        .get(header::CONTENT_RANGE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(cr, format!("bytes 100-109/{}", payload.len()));
    let body = body_bytes(resp.into_body()).await;
    assert_eq!(body, payload[100..110]);
}

#[tokio::test]
async fn unsatisfiable_range_returns_416() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("416.cbz");
    let payload = page_payload();
    build_cbz(&cbz, &payload);
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::RANGE, "bytes=99999-100000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let cr = resp
        .headers()
        .get(header::CONTENT_RANGE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cr, format!("bytes */{}", payload.len()));
}

#[tokio::test]
async fn suffix_range_returns_last_n_bytes() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("suffix.cbz");
    let payload = page_payload();
    build_cbz(&cbz, &payload);
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::RANGE, "bytes=-10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let body = body_bytes(resp.into_body()).await;
    assert_eq!(body, payload[(payload.len() - 10)..]);
}

#[tokio::test]
async fn unsupported_media_type_returns_415() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("svg.cbz");
    // CBZ contains an .svg-named entry — but our sniffer also recognizes the
    // angle-bracket prefix, so it would reject either way. Use plain text bytes
    // (no allowlisted magic) to trigger 415.
    let f = std::fs::File::create(&cbz).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    // .png extension so it appears in `pages()`, but bytes don't match any allowlisted magic.
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(b"not a real image, just plain text bytes")
        .unwrap();
    zw.finish().unwrap();
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn etag_round_trip_with_if_range() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("etag.cbz");
    let payload = page_payload();
    build_cbz(&cbz, &payload);
    let size = std::fs::metadata(&cbz).unwrap().len() as i64;
    let id = seed_issue(&app, &cbz, size).await;

    // First request: pull the ETag.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let etag = resp
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Range with matching If-Range → honored (206).
    let resp2 = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::RANGE, "bytes=0-9")
                .header(header::IF_RANGE, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::PARTIAL_CONTENT);

    // Range with mismatched If-Range → ignored (200, full body).
    let resp3 = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{id}/pages/0"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::RANGE, "bytes=0-9")
                .header(header::IF_RANGE, "\"different-etag\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp3.status(), StatusCode::OK);
}
