//! Integration coverage for `POST /me/issues/{id}/ocr`
//! (text-detection-1.0 plan, M3).
//!
//! These tests exercise the validation + ACL + archive paths *before*
//! the OCR pipeline runs — the heavy recognizer e2e is covered
//! separately under `cargo test ... ocr_recognizer -- --ignored`.
//! Routing this way keeps the integration suite fast (Postgres
//! bring-up only, no Tesseract C++ on the hot path) while still
//! pinning the contract the client relies on.

mod common;

use std::io::Write;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use image::{DynamicImage, ImageFormat, RgbImage};
use sea_orm::{ActiveModelTrait, Database, Set};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

// ─── Test plumbing ───────────────────────────────────────────────

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    // axum's own deserialization errors come back as `text/plain`,
    // so don't blow up the test harness when a 400/422 isn't JSON.
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let session = extract("__Host-comic_session=");
    let csrf = extract("__Host-comic_csrf=");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn post_ocr(
    app: &TestApp,
    issue_id: &str,
    auth: Option<&Authed>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/api/me/issues/{issue_id}/ocr");
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = builder
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

// ─── Seed helpers ────────────────────────────────────────────────

/// Build a tiny CBZ at `path` containing a single entry. The caller
/// chooses what to write into the entry — for archive-decode tests
/// it's real PNG bytes; for the "not an image" path it's UTF-8 text.
fn build_cbz(path: &std::path::Path, entry_name: &str, bytes: &[u8]) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file(entry_name, opts).unwrap();
    zw.write_all(bytes).unwrap();
    zw.finish().unwrap();
}

/// Real PNG bytes for a `w × h` flat-color image. Used by tests that
/// need `image::load_from_memory` to succeed so the handler reaches
/// the post-decode validation steps.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])));
    let mut buf = std::io::Cursor::new(Vec::<u8>::new());
    img.write_to(&mut buf, ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// Insert a library + series + issue tied to `file_path`. Returns
/// `(library_id, issue_id)`. The handler resolves the archive by
/// `issue.file_path`, so the caller controls whether the path
/// points at a real CBZ or a bogus location (for the
/// archive-unreadable test).
async fn seed_issue(app: &TestApp, file_path: &str) -> (Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test Library".into()),
        root_path: Set(std::path::Path::new(file_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/tmp".into())),
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
        thumbnail_format: Set("webp".into()),
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
    }
    .insert(&db)
    .await
    .unwrap();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Series".into()),
        normalized_name: Set(normalize_name("Series")),
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
        folder_path: Set(None),
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
    let issue_id = Uuid::now_v7().to_string();
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(Uuid::now_v7().to_string()),
        file_path: Set(file_path.to_string()),
        file_size: Set(0),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(Some("Issue".into())),
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
        hash_algorithm: Set(1),
        metroninfo_present: Set(None),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    (lib_id, issue_id)
}

async fn grant_access(app: &TestApp, lib_id: Uuid, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    library_user_access::ActiveModel {
        library_id: Set(lib_id),
        user_id: Set(user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
}

// ─── Existing validation / routing tests ─────────────────────────

const VALID_BODY: fn() -> serde_json::Value = || {
    json!({
        "page": 0,
        "region": { "x": 100, "y": 100, "w": 200, "h": 80 },
        "lang": "western"
    })
};

#[tokio::test]
async fn unauthenticated_request_is_rejected() {
    // The CSRF middleware fires before the auth extractor: a POST
    // with no `__Host-comic_csrf` cookie / `X-CSRF-Token` header
    // is rejected with 403 regardless of whether a session exists.
    // We assert "rejected" rather than a specific code so the test
    // stays meaningful if the layering ever shifts.
    let app = TestApp::spawn().await;
    let (status, _) = post_ocr(
        &app,
        "00000000-0000-0000-0000-000000000000",
        None,
        VALID_BODY(),
    )
    .await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "expected 401 or 403 for unauthed POST, got {status}"
    );
}

#[tokio::test]
async fn missing_issue_returns_404() {
    let app = TestApp::spawn().await;
    let user = register(&app, "ocr-missing@example.com").await;
    let (status, body) = post_ocr(
        &app,
        // Real UUID shape so route parsing accepts it; just no row.
        "11111111-1111-1111-1111-111111111111",
        Some(&user),
        VALID_BODY(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn zero_size_region_returns_400() {
    let app = TestApp::spawn().await;
    let user = register(&app, "ocr-zero@example.com").await;
    let (status, body) = post_ocr(
        &app,
        "11111111-1111-1111-1111-111111111111",
        Some(&user),
        json!({
            "page": 0,
            "region": { "x": 100, "y": 100, "w": 0, "h": 80 },
            "lang": "western"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "invalid_region");
}

#[tokio::test]
async fn unknown_language_returns_400() {
    let app = TestApp::spawn().await;
    let user = register(&app, "ocr-lang@example.com").await;
    let (status, body) = post_ocr(
        &app,
        "11111111-1111-1111-1111-111111111111",
        Some(&user),
        json!({
            "page": 0,
            "region": { "x": 100, "y": 100, "w": 200, "h": 80 },
            "lang": "klingon"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "invalid_lang");
}

#[tokio::test]
async fn default_language_is_western() {
    // No `lang` field → should be accepted, fall through to ACL
    // 404 (no such issue) rather than 400 invalid_lang.
    let app = TestApp::spawn().await;
    let user = register(&app, "ocr-default@example.com").await;
    let (status, body) = post_ocr(
        &app,
        "11111111-1111-1111-1111-111111111111",
        Some(&user),
        json!({
            "page": 0,
            "region": { "x": 100, "y": 100, "w": 200, "h": 80 }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn malformed_body_is_rejected() {
    let app = TestApp::spawn().await;
    let user = register(&app, "ocr-bad@example.com").await;
    let (status, _) = post_ocr(
        &app,
        "11111111-1111-1111-1111-111111111111",
        Some(&user),
        // Missing `region` field — axum's Json extractor rejects.
        json!({ "page": 0 }),
    )
    .await;
    // axum 0.8 returns 422 for Json deserialization failures by
    // default; older codepaths return 400. Accept either so the
    // test isn't brittle against extractor-layer revs.
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected 400 or 422 for malformed body, got {status}"
    );
}

// ─── ACL + archive + post-decode coverage ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_without_library_access_returns_404() {
    // First user becomes admin; the reader is a separate non-admin
    // account that owns no `library_user_access` row.
    //
    // The handler's `visible()` guard collapses "no row" into a 404
    // intentionally — leaking 403 here would reveal which issue
    // IDs are real on the server.
    let app = TestApp::spawn().await;
    let _admin = register(&app, "ocr-admin@example.com").await;
    let reader = register(&app, "ocr-reader@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("acl.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(80, 80));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let (status, body) = post_ocr(&app, &issue_id, Some(&reader), VALID_BODY()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_unreadable_returns_500() {
    // Issue row points at a path that doesn't exist on disk.
    // `zip_lru.get_or_open` errors, and the handler maps that to
    // 500 archive_unreadable.
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-archive-bad@example.com").await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/path/missing.cbz").await;

    let (status, body) = post_ocr(&app, &issue_id, Some(&admin), VALID_BODY()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["code"], "archive_unreadable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_out_of_range_returns_404() {
    // CBZ has one entry; request page index 5 → 404 page not found.
    // Exercises the `pages.get(page_index)` None branch.
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-page-oob@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("oob.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(80, 80));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 5,
            "region": { "x": 0, "y": 0, "w": 40, "h": 40 },
            "lang": "western"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_image_page_returns_415() {
    // CBZ entry is UTF-8 text, not an image. `image::load_from_memory`
    // fails → 415 decode_failed. The validator that this maps to
    // UNSUPPORTED_MEDIA_TYPE (not 500) is part of M3's contract: a
    // bad page is "this server can't OCR that", not "internal error".
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-decode-fail@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("garbage.cbz");
    // The archive crate's `pages()` filter is extension-based — we
    // need a `.png` name so the entry is enumerated as page 0, but
    // the bytes themselves must be invalid so the `image` crate's
    // decode in the handler bails with `decode_failed`.
    build_cbz(&cbz, "page-001.png", b"this is plainly not an image");
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 0, "y": 0, "w": 10, "h": 10 },
            "lang": "western"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body["error"]["code"], "decode_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn region_outside_page_bounds_returns_400() {
    // 80×80 image; requested region (50, 50, 100×100) extends past
    // the right + bottom edges → 400 invalid_region. The handler
    // performs this check *after* decoding so it can validate
    // against real page dimensions, not the row's possibly-stale
    // `pages` JSON.
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-region-oob@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("bounds.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(80, 80));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 50, "y": 50, "w": 100, "h": 100 },
            "lang": "western"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "invalid_region");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_with_library_access_reaches_pipeline() {
    // Once ACL passes and the page decodes cleanly + the region is
    // valid, the handler hands off to `run_ocr`. That fires up the
    // detector + recognizer singletons, which is exactly the heavy
    // work we don't want in the per-PR test loop (HF download +
    // Tesseract C++ build).
    //
    // We assert one thing only: the response **is not** the
    // pre-pipeline 404 / 400 / 415 the earlier tests pinned. Either
    // a 200 (pipeline ran end-to-end on a hot machine) or a 500
    // `ocr_failed` (pipeline failed cleanly because the models
    // aren't staged on this box) is fine — both prove the request
    // reached the dispatch.
    let app = TestApp::spawn().await;
    let _admin = register(&app, "ocr-reach-admin@example.com").await;
    let reader = register(&app, "ocr-reach-reader@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("reach.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(120, 120));
    let (lib_id, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;
    grant_access(&app, lib_id, reader.user_id).await;

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&reader),
        json!({
            "page": 0,
            "region": { "x": 10, "y": 10, "w": 60, "h": 40 },
            "lang": "western"
        }),
    )
    .await;
    // 200 if pipeline ran; 500 ocr_failed if models / Tesseract
    // aren't staged. Anything else means we never made it past the
    // validation / ACL / archive / decode gates — i.e. a regression.
    assert!(
        matches!(status, StatusCode::OK | StatusCode::INTERNAL_SERVER_ERROR),
        "expected 200 or 500 (pipeline reached), got {status}: {body}",
    );
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        assert_eq!(
            body["error"]["code"], "ocr_failed",
            "500 must be pipeline failure, not an earlier short-circuit"
        );
    }
}

// ─── M4: cache + rate-limit coverage ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_hit_short_circuits_pipeline() {
    // Pre-populate Redis with a sentinel payload for the exact key
    // the handler will compute. The issue's `file_path` points at
    // nowhere — if the cache short-circuit failed, the request
    // would reach `zip_lru.get_or_open` and 500 with
    // `archive_unreadable`. A 200 with our sentinel text proves the
    // hit path bypasses both archive load and the OCR pipeline.
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-cache-hit@example.com").await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/cache-hit.cbz").await;
    // `seed_issue` sets content_hash == issue_id, so the key the
    // handler computes uses `issue_id` for the content_hash slot.
    let region_hash = server::ocr::cache::region_hash(10, 20, 30, 40);
    // Default `detect` is `false` post-v0.3.26 — the seed must match
    // the key shape the handler will compute for a request without
    // an explicit `detect` field.
    let key = server::ocr::cache::cache_key(&issue_id, 0, "western", false, &region_hash);
    let payload = serde_json::json!({
        "text": "sentinel-cached-text",
        "confidence": 0.42_f64,
    })
    .to_string();
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&key, payload, 60).await.unwrap();

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 10, "y": 20, "w": 30, "h": 40 },
            "lang": "western"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "cache hit should return 200, got {body}"
    );
    assert_eq!(body["text"], "sentinel-cached-text");
    // serde_json's f64 round-trip on `0.42` is stable.
    assert!((body["confidence"].as_f64().unwrap() - 0.42).abs() < 1e-6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_miss_then_pipeline_writes_cache() {
    // Reach the pipeline with an empty cache; if it ran to
    // completion (200), assert the response was persisted under
    // the same key the handler builds. If the pipeline isn't
    // available on this box (500 ocr_failed), we skip the assert
    // — same tolerance as `reader_with_library_access_reaches_pipeline`.
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-cache-write@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("write.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(120, 120));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let region = serde_json::json!({ "x": 10, "y": 10, "w": 60, "h": 40 });
    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": region,
            "lang": "western"
        }),
    )
    .await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::INTERNAL_SERVER_ERROR),
        "expected 200 or 500 (pipeline reached), got {status}: {body}",
    );
    if status != StatusCode::OK {
        // Pipeline unavailable on this CI box; the rest of the
        // assertion is a no-op. The cache-hit test still pins the
        // read-side contract on every box.
        return;
    }

    let region_hash = server::ocr::cache::region_hash(10, 10, 60, 40);
    let key = server::ocr::cache::cache_key(&issue_id, 0, "western", false, &region_hash);
    let mut redis = app.state().jobs.redis.clone();
    let cached: Option<String> = redis.get(&key).await.unwrap();
    let cached = cached.expect("pipeline success should have populated the cache");
    let parsed: serde_json::Value = serde_json::from_str(&cached).unwrap();
    assert_eq!(parsed["text"], body["text"]);
    assert_eq!(parsed["confidence"], body["confidence"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_lookup_keyed_by_lang() {
    // Same region, different `lang` hint must miss — manga and
    // western recognizers can produce wildly different text for
    // the same pixels, so they cache independently. We pre-seed
    // the western cache slot only and verify the manga request
    // skipped past it (and 500s because the bogus file path means
    // the archive load fails, which is exactly the post-cache
    // path we want to hit).
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-cache-lang@example.com").await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/lang.cbz").await;
    let region_hash = server::ocr::cache::region_hash(0, 0, 50, 50);
    let western_key = server::ocr::cache::cache_key(&issue_id, 0, "western", false, &region_hash);
    let payload = serde_json::json!({ "text": "western-only", "confidence": 0.9_f64 }).to_string();
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&western_key, payload, 60).await.unwrap();

    // Manga lookup → different key → miss → archive load → 500.
    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 0, "y": 0, "w": 50, "h": 50 },
            "lang": "manga"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "manga request must miss the western-only cache and fall through; got {body}"
    );
    assert_eq!(body["error"]["code"], "archive_unreadable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_cache_hit_skips_detector_for_different_region() {
    // v0.3.25: the detector now runs on the **full page** and its
    // bbox list is cached per `(content_hash, page)`. Two OCR calls
    // on the same page with *different* user regions should both
    // run the recognizer but share the detector output — second
    // call short-circuits the heavy detect stage.
    //
    // We pre-seed the detect cache with a single bbox that overlaps
    // one region but not the other. The handler will:
    //  - call A: detect-cache HIT → bbox picked (covers user rect) →
    //    recognize → result cache MISS → run recognizer → 200
    //  - call B: same content_hash + page → detect-cache HIT →
    //    no bbox overlaps → fall back to user rect → recognize →
    //    200
    // Both calls return 200 without ever invoking the detector — the
    // detect bytes-cache served them. We can't directly assert
    // "detector didn't run" (no probe), but observing two distinct
    // recognize results on bogus archive paths would mean the
    // pipeline got past detect — which it can't if our cache reads
    // happen before page load. (The archive *does* load here because
    // we need the recognizer to run, so we use a real CBZ.)
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-detect-cache@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("detect.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(200, 200));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    // Pre-seed the detect cache with one bbox at (50, 50, 100, 100),
    // in the v2 `CachedDetection` shape (page dims + bbox list).
    let detect_key = server::ocr::cache::detect_cache_key(&issue_id, 0);
    let detection = serde_json::json!({
        "page_w": 200_u32, "page_h": 200_u32,
        "bboxes": [{
            "xmin": 50.0_f64, "ymin": 50.0_f64,
            "xmax": 150.0_f64, "ymax": 150.0_f64,
            "confidence": 0.9_f64, "class": 0_u32,
        }],
    })
    .to_string();
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&detect_key, detection, 60).await.unwrap();

    // Call A: user region overlaps the seeded bbox. Post-v0.3.26 we
    // must opt into the detector explicitly — without `detect: true`
    // the handler skips both the detect cache and the detector run.
    let (status_a, body_a) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 60, "y": 60, "w": 40, "h": 40 },
            "lang": "western",
            "detect": true,
        }),
    )
    .await;
    // Skip when pipeline isn't available on this box, same tolerance
    // as the other pipeline-reach tests.
    if status_a != StatusCode::OK {
        assert_eq!(
            body_a["error"]["code"], "ocr_failed",
            "non-200 must be ocr_failed, not an earlier short-circuit",
        );
        return;
    }

    // Detect cache must still be present after the call.
    let cached_after: Option<String> = redis.get(&detect_key).await.unwrap();
    assert!(
        cached_after.is_some(),
        "detect cache entry should survive the OCR call",
    );

    // Call B: user region misses the seeded bbox → recognizer runs
    // on the user's rect verbatim. Still 200. `detect: true` again
    // so the handler consults the detect cache.
    let (status_b, body_b) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 0, "y": 0, "w": 30, "h": 30 },
            "lang": "western",
            "detect": true,
        }),
    )
    .await;
    assert_eq!(
        status_b,
        StatusCode::OK,
        "second OCR on cached-detect page should succeed: {body_b}",
    );
}

// ─── OCR rework 1.0: text-regions endpoint ───────────────────────

async fn get_text_regions(
    app: &TestApp,
    issue_id: &str,
    page: u32,
    auth: Option<&Authed>,
) -> (StatusCode, serde_json::Value) {
    let uri = format!("/api/me/issues/{issue_id}/pages/{page}/text-regions");
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(a) = auth {
        builder = builder.header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                a.session, a.csrf
            ),
        );
    }
    let resp = app
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_regions_requires_auth() {
    let app = TestApp::spawn().await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/regions-auth.cbz").await;
    let (status, _) = get_text_regions(&app, &issue_id, 0, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_regions_unknown_issue_is_404() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "regions-404@example.com").await;
    let (status, body) = get_text_regions(&app, &Uuid::new_v4().to_string(), 0, Some(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_regions_cache_hit_converts_to_percent_without_archive() {
    // Pre-seed the detect cache; the issue's file_path points at
    // nowhere, so a 200 proves the hit path never touched the
    // archive — and the payload pins the px→percent conversion.
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "regions-hit@example.com").await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/regions-hit.cbz").await;

    let detect_key = server::ocr::cache::detect_cache_key(&issue_id, 3);
    let detection = serde_json::json!({
        "page_w": 1000_u32, "page_h": 2000_u32,
        "bboxes": [
            {
                "xmin": 100.0_f64, "ymin": 400.0_f64,
                "xmax": 350.0_f64, "ymax": 600.0_f64,
                "confidence": 0.9_f64, "class": 0_u32,
            },
            {
                // Collapsed after clamping → must be filtered out.
                "xmin": 1200.0_f64, "ymin": 100.0_f64,
                "xmax": 1300.0_f64, "ymax": 200.0_f64,
                "confidence": 0.8_f64, "class": 1_u32,
            },
        ],
    })
    .to_string();
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&detect_key, detection, 60).await.unwrap();

    let (status, body) = get_text_regions(&app, &issue_id, 3, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["page_w"], 1000);
    assert_eq!(body["page_h"], 2000);
    let regions = body["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 1, "off-page bbox must be dropped: {body}");
    let r = &regions[0];
    assert!((r["x"].as_f64().unwrap() - 10.0).abs() < 1e-4);
    assert!((r["y"].as_f64().unwrap() - 20.0).abs() < 1e-4);
    assert!((r["w"].as_f64().unwrap() - 25.0).abs() < 1e-4);
    assert!((r["h"].as_f64().unwrap() - 10.0).abs() < 1e-4);
    assert_eq!(r["class"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_regions_treats_v1_payload_as_miss() {
    // A bare bbox array (the pre-v2 payload shape) under the v2 key
    // must deserialize-fail and fall through to the archive load —
    // which 500s here because the file path is bogus. That proves
    // the malformed entry wasn't served.
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "regions-v1@example.com").await;
    let (_lib, issue_id) = seed_issue(&app, "/nonexistent/regions-v1.cbz").await;

    let detect_key = server::ocr::cache::detect_cache_key(&issue_id, 0);
    let v1 = serde_json::json!([{
        "xmin": 1.0_f64, "ymin": 2.0_f64, "xmax": 3.0_f64, "ymax": 4.0_f64,
        "confidence": 0.9_f64, "class": 0_u32,
    }])
    .to_string();
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&detect_key, v1, 60).await.unwrap();

    let (status, body) = get_text_regions(&app, &issue_id, 0, Some(&admin)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "archive_unreadable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detect_disabled_skips_detector_cache() {
    // v0.3.26: when `detect` is omitted (defaults to false) the
    // handler must bypass the detect cache entirely — no `ocr:detect:*`
    // key should appear in Redis after the call. We verify by polling
    // the key after a successful OCR; absence proves the detector
    // didn't run.
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-detect-off@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("nodetect.cbz");
    build_cbz(&cbz, "page-001.png", &png_bytes(120, 120));
    let (_lib, issue_id) = seed_issue(&app, cbz.to_str().unwrap()).await;

    let (status, body) = post_ocr(
        &app,
        &issue_id,
        Some(&admin),
        json!({
            "page": 0,
            "region": { "x": 10, "y": 10, "w": 60, "h": 40 },
            "lang": "western",
            // Note: no `detect` field — should default to false.
        }),
    )
    .await;
    // Recognize is far cheaper than detect+recognize; we expect 200
    // on any host with tessdata staged. 500 is tolerated for parity
    // with the other pipeline-reach tests but indicates a bad config.
    if status != StatusCode::OK {
        assert_eq!(body["error"]["code"], "ocr_failed");
        return;
    }

    // The detect cache key for this page should be ABSENT — proof
    // the detector path was skipped.
    let detect_key = server::ocr::cache::detect_cache_key(&issue_id, 0);
    let mut redis = app.state().jobs.redis.clone();
    let cached: Option<String> = redis.get(&detect_key).await.unwrap();
    assert!(
        cached.is_none(),
        "detect cache should be untouched when detect=false (default); got {cached:?}",
    );

    // The result cache should hold the recognize-only output under
    // the `detect=false` key shape.
    let region_hash = server::ocr::cache::region_hash(10, 10, 60, 40);
    let key = server::ocr::cache::cache_key(&issue_id, 0, "western", false, &region_hash);
    let result_cached: Option<String> = redis.get(&key).await.unwrap();
    assert!(
        result_cached.is_some(),
        "result cache should be populated under the no-detect key",
    );
}
