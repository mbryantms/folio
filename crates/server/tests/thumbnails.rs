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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
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
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
        series_json_present: Set(None),
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
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
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
        state: Set(entity::issue::IssueState::Active),
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
        deck: Set(None),
        store_date: Set(None),
        foc_date: Set(None),
        price: Set(None),
        sku: Set(None),
        staff_rating: Set(None),
        aliases: Set(serde_json::json!([])),
        last_metadata_sync_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        superseded_by: Set(None),
        special_type: Set(None),
        slug: Set(uuid::Uuid::now_v7().to_string()),
        hash_algorithm: Set(1),
        metroninfo_present: Set(None),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        last_sidecar_rewrite_at: Set(None),
        metron_info_raw: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
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

/// Audit OP-7: a 200 carries `Cache-Control: private, max-age=300` and a
/// strong ETag that is the quoted blake3 digest of the body; a matching
/// `If-None-Match` answers 304 with the same validator headers and no body.
#[tokio::test]
async fn thumb_200_is_private_five_minute_cached_with_digest_etag_and_304_has_no_body() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("digest.cbz");
    build_cbz(&cbz, 2);
    let id = seed_issue(&app, &cbz).await;

    let first = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), None).await;
    assert_eq!(first.status(), StatusCode::OK);
    let cache_control = first
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .expect("cache-control")
        .to_owned();
    assert_eq!(
        cache_control, "private, max-age=300",
        "thumbs are reusable for five minutes, per user, never by a shared cache"
    );
    assert_eq!(
        first
            .headers()
            .get(header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some("Cookie, Authorization"),
        "one browser profile or OPDS client, two accounts in turn: the cache keys on the credentials"
    );
    let etag = first
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_owned();
    let content_length = first
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .expect("content-length");
    let body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.len(), content_length);
    assert!(!body.is_empty(), "a 200 carries the image bytes");
    assert_eq!(
        etag,
        format!("\"{}\"", blake3::hash(&body).to_hex()),
        "ETag is the strong digest of the served bytes, not an mtime stamp"
    );

    let second = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), Some(&etag)).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        second
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str()),
        "304 repeats the validator"
    );
    assert_eq!(
        second
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, max-age=300"),
        "304 refreshes the freshness lifetime"
    );
    assert!(
        second.headers().get(header::CONTENT_TYPE).is_none(),
        "304 carries no representation headers"
    );
    let body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "304 has no body");

    // A list of validators with the current one anywhere in it still matches.
    let third = fetch_thumb_response(
        &app,
        &session,
        &id,
        0,
        Some("cover"),
        Some(&format!("\"stale\", {etag}")),
    )
    .await;
    assert_eq!(third.status(), StatusCode::NOT_MODIFIED);

    // `If-None-Match` compares weakly: a proxy-weakened `W/` validator hits.
    let fourth = fetch_thumb_response(
        &app,
        &session,
        &id,
        0,
        Some("cover"),
        Some(&format!("W/{etag}")),
    )
    .await;
    assert_eq!(fourth.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn thumb_etag_changes_when_artifact_regenerates() {
    // Regression for the archive-edit staleness bug: the thumb URL is
    // stable across rewrites, so the response must (a) stay revalidatable
    // (a bounded `max-age`, never `immutable`) and (b) carry an ETag
    // derived from the on-disk bytes, so a regenerated thumbnail stops
    // matching the old validator once the five-minute window closes.
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("regen.cbz");
    build_cbz(&cbz, 2);
    let id = seed_issue(&app, &cbz).await;

    let first = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), None).await;
    assert_eq!(first.status(), StatusCode::OK);
    let cache_control = first
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .expect("cache-control")
        .to_owned();
    assert!(
        cache_control.contains("max-age=300")
            && cache_control.contains("private")
            && !cache_control.contains("immutable")
            && !cache_control.contains("public"),
        "thumb responses must stay revalidatable and per-user (got {cache_control:?})"
    );
    let etag = first
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_owned();

    // Simulate a post-edit regeneration: same path, different bytes.
    let cover = app
        ._data_dir
        .path()
        .join("thumbs")
        .join(format!("{id}.webp"));
    let mut bytes = std::fs::read(&cover).unwrap();
    bytes.extend_from_slice(b"x");
    std::fs::write(&cover, &bytes).unwrap();

    let second = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), Some(&etag)).await;
    assert_eq!(
        second.status(),
        StatusCode::OK,
        "a changed artifact must not 304 against the old ETag"
    );
    let etag2 = second
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag 2");
    assert_ne!(etag, etag2, "ETag must change with the artifact bytes");
}

/// The validator is the bytes' digest, never the file stamp: a restore that
/// keeps the length AND the mtime (`cp -p`, a backup replay) but swaps the
/// pixels must still miss the old ETag.
#[tokio::test]
async fn thumb_etag_ignores_mtime_and_length_when_bytes_change() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("restore.cbz");
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

    let cover = app
        ._data_dir
        .path()
        .join("thumbs")
        .join(format!("{id}.webp"));
    let mtime =
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(&cover).unwrap());
    let mut bytes = std::fs::read(&cover).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&cover, &bytes).unwrap();
    filetime::set_file_mtime(&cover, mtime).unwrap();

    let second = fetch_thumb_response(&app, &session, &id, 0, Some("cover"), Some(&etag)).await;
    assert_eq!(
        second.status(),
        StatusCode::OK,
        "same length + same mtime, different bytes: must not 304"
    );
    let etag2 = second
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("etag 2");
    assert_ne!(etag, etag2);
}

/// Inline (cold-path) cover generation applies the same wraparound crop as
/// the post-scan worker: page 0 of a 2:1 spread comes back as the front
/// half, while the strip variant keeps the whole spread.
#[tokio::test]
async fn inline_cover_thumb_crops_wraparound_but_strip_keeps_spread() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("wrap.cbz");
    {
        let wide: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(128, 96, |x, _| {
            if x < 64 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let mut png: Vec<u8> = Vec::new();
        wide.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let f = std::fs::File::create(&cbz).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-000.png", opts).unwrap();
        zw.write_all(&png).unwrap();
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(&solid_png([10, 100, 200, 255])).unwrap();
        zw.finish().unwrap();
    }
    let id = seed_issue(&app, &cbz).await;

    let resp = fetch_thumb_response(&app, &session, &id, 0, None, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let cover = image::load_from_memory(&body).unwrap().to_rgba8();
    assert_eq!(cover.dimensions(), (64, 96), "cover = front (right) half");
    let px = cover.get_pixel(32, 32).0;
    assert!(
        px[2] > 200 && px[0] < 60,
        "front half should be blue, got {px:?}"
    );

    let resp = fetch_thumb_response(&app, &session, &id, 0, Some("strip"), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let strip = image::load_from_memory(&body).unwrap().to_rgba8();
    assert_eq!(
        strip.dimensions(),
        (128, 96),
        "strip keeps the whole spread"
    );
}
