//! M5: thumbnail cleanup hooks + orphan sweep.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::{ActiveModel as IssueAM, Entity as IssueEntity},
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Set};
use server::library::thumbnails;
use std::path::Path;
use uuid::Uuid;

async fn seed(app: &TestApp, with_thumbs_for_state: &str) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    // Unique root_path per call so multiple seed()s in one test don't
    // collide on the libraries unique index.
    let root = format!("/tmp/cleanup-{lib_id}");
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Cleanup".into()),
        root_path: Set(root.clone()),
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
        name: Set("S".into()),
        normalized_name: Set(normalize_name("S")),
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

    // Unique 64-hex id per call (issues.id has a primary-key constraint
    // and several tests insert multiple seeds).
    let raw = blake3::hash(format!("{lib_id}").as_bytes())
        .to_hex()
        .to_string();
    let id = raw[..64].to_string();
    IssueAM {
        id: Set(id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set(format!("{root}/x.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set(with_thumbs_for_state.to_owned()),
        content_hash: Set(id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
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
        removed_at: Set(if with_thumbs_for_state == "active" {
            None
        } else {
            Some(now)
        }),
        removal_confirmed_at: Set(None),
        superseded_by: Set(None),
        special_type: Set(None),
        slug: Set(uuid::Uuid::now_v7().to_string()),
        hash_algorithm: Set(1),
        thumbnails_generated_at: Set(Some(now)),
        thumbnail_version: Set(thumbnails::THUMBNAIL_VERSION),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

fn put_fake_thumbs(data_dir: &Path, issue_id: &str) {
    let format = thumbnails::ThumbFormat::Webp;
    let cover = thumbnails::cover_path(data_dir, issue_id, format);
    std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
    std::fs::write(&cover, b"fake-cover").unwrap();
    let strip = thumbnails::strip_path(data_dir, issue_id, 0, format);
    std::fs::create_dir_all(strip.parent().unwrap()).unwrap();
    std::fs::write(&strip, b"fake-strip").unwrap();
}

#[tokio::test]
async fn wipe_issue_thumbs_removes_cover_and_strip_dir() {
    let app = TestApp::spawn().await;
    let id = seed(&app, "active").await;
    put_fake_thumbs(&app.state().cfg().data_path, &id);

    thumbnails::wipe_issue_thumbs(&app.state().cfg().data_path, &id);

    let cover = thumbnails::cover_path(
        &app.state().cfg().data_path,
        &id,
        thumbnails::ThumbFormat::Webp,
    );
    let dir = thumbnails::issue_thumbs_dir(&app.state().cfg().data_path, &id);
    assert!(!cover.exists(), "cover not wiped");
    assert!(!dir.exists(), "strip dir not wiped");
}

#[tokio::test]
async fn orphan_sweep_drops_artifacts_for_removed_issues() {
    let app = TestApp::spawn().await;
    // Two issues: one active, one in `removed` state.
    let active_id = seed(&app, "active").await;
    let removed_id = seed(&app, "removed").await;
    put_fake_thumbs(&app.state().cfg().data_path, &active_id);
    put_fake_thumbs(&app.state().cfg().data_path, &removed_id);

    // A third "stranger" id with no DB row at all — also orphaned.
    let stranger = "f".repeat(64);
    put_fake_thumbs(&app.state().cfg().data_path, &stranger);

    let wiped = server::jobs::orphan_sweep::run(&app.state()).await.unwrap();
    assert_eq!(wiped, 2, "should wipe 2 (removed + stranger)");

    let active_cover = thumbnails::cover_path(
        &app.state().cfg().data_path,
        &active_id,
        thumbnails::ThumbFormat::Webp,
    );
    let removed_cover = thumbnails::cover_path(
        &app.state().cfg().data_path,
        &removed_id,
        thumbnails::ThumbFormat::Webp,
    );
    let stranger_cover = thumbnails::cover_path(
        &app.state().cfg().data_path,
        &stranger,
        thumbnails::ThumbFormat::Webp,
    );
    assert!(
        active_cover.exists(),
        "active issue's cover must be preserved"
    );
    assert!(
        !removed_cover.exists(),
        "removed issue's cover should be gone"
    );
    assert!(!stranger_cover.exists(), "stranger's cover should be gone");
}

#[tokio::test]
async fn orphan_sweep_no_op_when_thumbs_dir_missing() {
    let app = TestApp::spawn().await;
    // Don't put any thumbs on disk; ensure sweep is happy.
    let n = server::jobs::orphan_sweep::run(&app.state()).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn list_issues_on_disk_finds_both_layouts() {
    let app = TestApp::spawn().await;
    let id1 = seed(&app, "active").await;
    let id2 = seed(&app, "active").await;
    put_fake_thumbs(&app.state().cfg().data_path, &id1);
    // Only the strip dir exists for id2 — exercise that case.
    let strip = thumbnails::strip_path(
        &app.state().cfg().data_path,
        &id2,
        0,
        thumbnails::ThumbFormat::Webp,
    );
    std::fs::create_dir_all(strip.parent().unwrap()).unwrap();
    std::fs::write(&strip, b"x").unwrap();

    let found = thumbnails::list_issues_on_disk(&app.state().cfg().data_path).unwrap();
    assert!(found.contains(&id1));
    assert!(found.contains(&id2));
}

// Suppress unused-import warning on IssueEntity for builds where the
// downstream queries are inlined into the helper.
#[allow(dead_code)]
fn _unused() {
    let _: Option<IssueEntity> = None;
}
