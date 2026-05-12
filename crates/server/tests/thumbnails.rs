//! Per-page thumbnail generation through the API + LRU.
//!
//! Builds a CBZ with real PNG pages so the `image` crate decoder accepts them,
//! then asserts the dual-path scheme: cover at `<id>.webp`, per-page at
//! `<id>/<n>.webp`.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use image::{ImageBuffer, ImageFormat, Rgba};
use sea_orm::{ActiveModelTrait, Set};
use std::io::{Cursor, Write};
use tower::ServiceExt;
use uuid::Uuid;

fn solid_png(color: [u8; 4]) -> Vec<u8> {
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(64, 64, |_, _| Rgba(color));
    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .unwrap();
    buf
}

fn build_cbz(path: &std::path::Path, pages: usize) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for n in 0..pages {
        zw.start_file(format!("page-{n:03}.png"), opts).unwrap();
        let color = [(n * 30) as u8, 100, 200, 255];
        zw.write_all(&solid_png(color)).unwrap();
    }
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
                    r#"{"email":"thumb-admin@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
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

async fn seed_issue(app: &TestApp, file_path: &std::path::Path) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Thumb Lib".into()),
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
        name: Set("Thumb Series".into()),
        normalized_name: Set(normalize_name("Thumb Series")),
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

    let bytes = std::fs::read(file_path).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();

    IssueAM {
        id: Set(hash.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set(file_path.to_string_lossy().into_owned()),
        file_size: Set(std::fs::metadata(file_path).unwrap().len() as i64),
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
        page_count: Set(Some(3)),
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

async fn fetch_thumb(app: &TestApp, session: &str, issue_id: &str, n: u32) -> StatusCode {
    fetch_thumb_with(app, session, issue_id, n, None).await
}

async fn fetch_thumb_with(
    app: &TestApp,
    session: &str,
    issue_id: &str,
    n: u32,
    variant: Option<&str>,
) -> StatusCode {
    let q = variant.map(|v| format!("?variant={v}")).unwrap_or_default();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{issue_id}/pages/{n}/thumb{q}"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

async fn fetch_thumb_response(
    app: &TestApp,
    session: &str,
    issue_id: &str,
    n: u32,
    variant: Option<&str>,
    if_none_match: Option<&str>,
) -> axum::response::Response {
    let q = variant.map(|v| format!("?variant={v}")).unwrap_or_default();
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("/issues/{issue_id}/pages/{n}/thumb{q}"))
        .header(header::COOKIE, format!("__Host-comic_session={session}"));
    if let Some(etag) = if_none_match {
        req = req.header(header::IF_NONE_MATCH, etag);
    }
    app.router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn cover_thumb_lands_at_root_path() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("c.cbz");
    build_cbz(&cbz, 3);
    let id = seed_issue(&app, &cbz).await;

    assert_eq!(fetch_thumb(&app, &session, &id, 0).await, StatusCode::OK);

    let cover = app
        ._data_dir
        .path()
        .join("thumbs")
        .join(format!("{id}.webp"));
    assert!(
        cover.exists(),
        "cover thumb should be at <thumbs>/<id>.webp"
    );
}

#[tokio::test]
async fn per_page_thumb_lands_at_strip_subdir() {
    // Post-M2: the un-paramed legacy URL defaults to the strip variant
    // for n > 0. Files now live under `<id>/s/<n>.webp` instead of the
    // old `<id>/<n>.webp` layout (cleaner separation from any future
    // variants).
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("p.cbz");
    build_cbz(&cbz, 3);
    let id = seed_issue(&app, &cbz).await;

    for n in 1..=2u32 {
        assert_eq!(fetch_thumb(&app, &session, &id, n).await, StatusCode::OK);
        let p = app
            ._data_dir
            .path()
            .join("thumbs")
            .join(&id)
            .join("s")
            .join(format!("{n}.webp"));
        assert!(
            p.exists(),
            "page {n} thumb should land at <thumbs>/<id>/s/{n}.webp"
        );
    }
}

#[tokio::test]
async fn thumb_for_missing_page_returns_404() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("oob.cbz");
    build_cbz(&cbz, 3);
    let id = seed_issue(&app, &cbz).await;

    assert_eq!(
        fetch_thumb(&app, &session, &id, 99).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn variant_strip_lands_at_strip_subdir() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("v.cbz");
    build_cbz(&cbz, 4);
    let id = seed_issue(&app, &cbz).await;

    assert_eq!(
        fetch_thumb_with(&app, &session, &id, 2, Some("strip")).await,
        StatusCode::OK
    );
    let strip = app
        ._data_dir
        .path()
        .join("thumbs")
        .join(&id)
        .join("s")
        .join("2.webp");
    assert!(
        strip.exists(),
        "strip thumb should land at <thumbs>/<id>/s/<n>.webp"
    );
}

#[tokio::test]
async fn variant_cover_serves_from_legacy_path() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("c.cbz");
    build_cbz(&cbz, 3);
    let id = seed_issue(&app, &cbz).await;

    // Default (no variant param) is cover.
    assert_eq!(fetch_thumb(&app, &session, &id, 0).await, StatusCode::OK);
    let cover_legacy = app
        ._data_dir
        .path()
        .join("thumbs")
        .join(format!("{id}.webp"));
    assert!(
        cover_legacy.exists(),
        "cover should land at backwards-compat path"
    );

    // Explicit ?variant=cover too.
    assert_eq!(
        fetch_thumb_with(&app, &session, &id, 0, Some("cover")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn variant_unknown_falls_back_to_cover() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("fallback.cbz");
    build_cbz(&cbz, 2);
    let id = seed_issue(&app, &cbz).await;

    // ?variant=nonsense — handler shouldn't 400, should silently use cover.
    assert_eq!(
        fetch_thumb_with(&app, &session, &id, 0, Some("nonsense")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn thumb_serves_conditional_etag_304() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("etag.cbz");
    build_cbz(&cbz, 2);
    let id = seed_issue(&app, &cbz).await;

    let first = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), None).await;
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_owned();

    let second = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), Some(&etag)).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
}
