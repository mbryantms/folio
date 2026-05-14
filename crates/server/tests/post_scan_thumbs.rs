//! M1: post-scan thumbs worker.
//!
//! Asserts the worker pre-generates covers, lazily backfills per-page strip
//! thumbs, stamps the success columns, and stays idempotent across re-runs. Exercises the
//! worker function directly rather than driving apalis end-to-end — keeps
//! tests fast and removes Redis from the test loop for this layer.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::{ActiveModel as IssueAM, Entity as IssueEntity},
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use image::{ImageBuffer, ImageFormat, Rgba};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::jobs::post_scan::{ThumbsJob, handle_thumbs};
use server::library::thumbnails;
use std::io::{Cursor, Write};
use std::path::Path;
use uuid::Uuid;

fn solid_png(color: [u8; 4]) -> Vec<u8> {
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(64, 64, |_, _| Rgba(color));
    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .unwrap();
    buf
}

fn build_cbz(path: &Path, pages: usize) {
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

async fn seed_issue(app: &TestApp, file_path: &Path, pages: usize) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Thumbs M1".into()),
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
        page_count: Set(Some(pages as i32)),
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

#[tokio::test]
async fn cover_worker_generates_cover_without_eager_strips() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("issue.cbz");
    build_cbz(&cbz, 5);
    let id = seed_issue(&app, &cbz, 5).await;

    let state = app.state();
    handle_thumbs(
        ThumbsJob::cover(id.clone()),
        apalis::prelude::Data::new(state.clone()),
    )
    .await
    .unwrap();

    // Cover lives at the legacy backwards-compat path.
    let cover = thumbnails::cover_path(&state.cfg().data_path, &id, thumbnails::ThumbFormat::Webp);
    assert!(cover.exists(), "cover thumb missing: {}", cover.display());

    // Strip thumbnails are generated lazily by the reader catchup job, not by
    // the scan/admin cover job.
    for n in 0..5 {
        let strip =
            thumbnails::strip_path(&state.cfg().data_path, &id, n, thumbnails::ThumbFormat::Webp);
        assert!(!strip.exists(), "strip page {n} should not be eager");
    }

    // DB row stamped done at current version, no error.
    let row = IssueEntity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.thumbnails_generated_at.is_some());
    assert_eq!(row.thumbnail_version, thumbnails::THUMBNAIL_VERSION);
    assert!(row.thumbnails_error.is_none());
}

#[tokio::test]
async fn strip_worker_generates_strip_for_every_page() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("issue.cbz");
    build_cbz(&cbz, 5);
    let id = seed_issue(&app, &cbz, 5).await;

    let state = app.state();
    handle_thumbs(
        ThumbsJob::strip(id.clone()),
        apalis::prelude::Data::new(state.clone()),
    )
    .await
    .unwrap();

    for n in 0..5 {
        let strip =
            thumbnails::strip_path(&state.cfg().data_path, &id, n, thumbnails::ThumbFormat::Webp);
        assert!(
            strip.exists(),
            "strip page {n} missing: {}",
            strip.display()
        );
    }
}

#[tokio::test]
async fn worker_is_idempotent_across_reruns() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("idem.cbz");
    build_cbz(&cbz, 3);
    let id = seed_issue(&app, &cbz, 3).await;

    let state = app.state();
    let job = ThumbsJob::cover(id.clone());

    handle_thumbs(job.clone(), apalis::prelude::Data::new(state.clone()))
        .await
        .unwrap();
    let row1 = IssueEntity::find_by_id(id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let stamp1 = row1.thumbnails_generated_at.unwrap();

    // Capture mtime of the cover file so we can prove the second
    // pass didn't rewrite it.
    let cover = thumbnails::cover_path(&state.cfg().data_path, &id, thumbnails::ThumbFormat::Webp);
    let mtime1 = std::fs::metadata(&cover).unwrap().modified().unwrap();

    // Sleep so the next stamp can't tie on second-resolution timestamps.
    std::thread::sleep(std::time::Duration::from_millis(50));

    handle_thumbs(job, apalis::prelude::Data::new(state.clone()))
        .await
        .unwrap();
    let row2 = IssueEntity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();

    // Stamp is bumped (we always stamp on success), but the file wasn't
    // re-encoded — generate() short-circuits when the file already exists.
    assert!(row2.thumbnails_generated_at.unwrap() >= stamp1);
    let mtime2 = std::fs::metadata(&cover).unwrap().modified().unwrap();
    assert_eq!(mtime1, mtime2, "cover file should not have been rewritten");
}

#[tokio::test]
async fn worker_marks_error_on_unreadable_archive() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("broken.cbz");
    // Write garbage that isn't a valid ZIP — the LRU's open will fail.
    std::fs::write(&cbz, b"not a zip file").unwrap();

    // Seed a row with a fake hash and the correct file path so the worker
    // tries to open it.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Broken Lib".into()),
        root_path: Set(dir.path().to_string_lossy().into_owned()),
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
        name: Set("Broken".into()),
        normalized_name: Set(normalize_name("Broken")),
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
        folder_path: Set(Some(dir.path().to_string_lossy().into_owned())),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let id = "deadbeef".repeat(8); // 64 hex chars
    IssueAM {
        id: Set(id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set(cbz.to_string_lossy().into_owned()),
        file_size: Set(std::fs::metadata(&cbz).unwrap().len() as i64),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(id.clone()),
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

    let state = app.state();
    handle_thumbs(
        ThumbsJob::cover(id.clone()),
        apalis::prelude::Data::new(state.clone()),
    )
    .await
    .unwrap();

    let row = IssueEntity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.thumbnails_error.is_some(), "should record the error");
    // Error path stamps `generated_at` too so the post-scan enqueue
    // query (`generated_at IS NULL OR version < CURRENT`) skips this
    // row on the next pass — see `stamp_error` rationale in post_scan.rs.
    // Operators retry via admin "Force recreate" (clears both columns)
    // or a global THUMBNAIL_VERSION bump.
    assert!(
        row.thumbnails_generated_at.is_some(),
        "error path should stamp generated_at to break retry loop"
    );
    assert_eq!(
        row.thumbnail_version,
        server::library::thumbnails::THUMBNAIL_VERSION,
        "error path bumps version to current sentinel"
    );
}

#[tokio::test]
async fn worker_skips_non_active_issue() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("x.cbz");
    build_cbz(&cbz, 2);
    let id = seed_issue(&app, &cbz, 2).await;

    // Flip state to non-active (e.g. encrypted / removed).
    let row = IssueEntity::find_by_id(id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: IssueAM = row.into();
    am.state = Set("removed".into());
    am.update(&app.state().db).await.unwrap();

    let state = app.state();
    handle_thumbs(
        ThumbsJob::cover(id.clone()),
        apalis::prelude::Data::new(state.clone()),
    )
    .await
    .unwrap();

    let cover = thumbnails::cover_path(&state.cfg().data_path, &id, thumbnails::ThumbFormat::Webp);
    assert!(!cover.exists(), "non-active issue should not gen thumbs");
    let row = IssueEntity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.thumbnails_generated_at.is_none());
}
