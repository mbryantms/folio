//! Auto-derived series publication status from ComicInfo `<Count>`.
//!
//! Drives `library::scanner::reconcile_status::reconcile_series_status`
//! directly against a seeded DB rather than through the full scanner —
//! the scanner's role is just to populate `issues.comicinfo_count`,
//! which we set explicitly here so the tests stay focused on the
//! evaluation rule itself.
//!
//! Covers (in order):
//! - Real `<Count>` signal flips status to `"ended"` and refreshes
//!   `total_issues`.
//! - Manual PATCH override (`status_user_set_at IS NOT NULL`)
//!   prevents the status write but does NOT block the
//!   `total_issues` refresh.
//! - No `<Count>` signal (all NULL or `<= 0`) leaves status alone.
//! - Mixed Count values across a series: MAX wins.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use parsers::series_json::SeriesMetadata;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use server::library::scanner::reconcile_status::reconcile_series_status;
use uuid::Uuid;

async fn seed_library(app: &TestApp, name: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {name}")),
        root_path: Set(format!("/tmp/{name}-{lib_id}")),
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
    lib_id
}

async fn seed_series(
    app: &TestApp,
    lib_id: Uuid,
    name: &str,
    status: &str,
    status_user_set_at: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set(status.into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(format!("series-{}", series_id.simple())),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(status_user_set_at),
    }
    .insert(&db)
    .await
    .unwrap();
    series_id
}

async fn seed_issue_with_count(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    title: &str,
    comicinfo_count: Option<i32>,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    // BLAKE3-shaped id (64 hex chars).
    let id = format!("{:0>62}{:02x}", Uuid::now_v7().simple(), rand_byte());
    IssueAM {
        id: Set(id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("issue-{id}")),
        file_path: Set(format!("/tmp/{title}-{id}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(id.clone()),
        title: Set(Some(title.into())),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(Some(2020)),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(Some(20)),
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
        hash_algorithm: Set(1),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(comicinfo_count),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

fn rand_byte() -> u8 {
    use std::sync::atomic::{AtomicU8, Ordering};
    static N: AtomicU8 = AtomicU8::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

async fn fetch_series(app: &TestApp, series_id: Uuid) -> entity::series::Model {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn count_signal_flips_status_to_ended_and_refreshes_total() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    // Default state: "continuing", no manual override stamp.
    let series = seed_series(&app, lib, "Wrapped Series", "continuing", None).await;

    // Two issues each tagged with `<Count>3</Count>`. The publisher
    // claims three issues exist.
    seed_issue_with_count(&app, lib, series, "iss-a", Some(3)).await;
    seed_issue_with_count(&app, lib, series, "iss-b", Some(3)).await;

    reconcile_series_status(&db, series, None).await.unwrap();

    let after = fetch_series(&app, series).await;
    assert_eq!(after.status, "ended");
    assert_eq!(after.total_issues, Some(3));
    assert!(after.status_user_set_at.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_override_blocks_status_write_but_total_issues_still_refreshes() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    // User PATCHed the series to "hiatus" earlier — the timestamp is
    // what `update_series` would have set.
    let stamp = Utc::now().fixed_offset();
    let series = seed_series(&app, lib, "Pinned Series", "hiatus", Some(stamp)).await;

    seed_issue_with_count(&app, lib, series, "iss-a", Some(12)).await;

    reconcile_series_status(&db, series, None).await.unwrap();

    let after = fetch_series(&app, series).await;
    // Status preserved.
    assert_eq!(after.status, "hiatus");
    // Override still recorded.
    assert!(after.status_user_set_at.is_some());
    // But the count refresh is independent — the UI's
    // Complete/Incomplete badge reads off `total_issues` regardless.
    assert_eq!(after.total_issues, Some(12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_count_signal_leaves_status_unchanged() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let series = seed_series(&app, lib, "Mystery Series", "continuing", None).await;

    // All issues either lack ComicInfo Count or have it as 0/NULL —
    // no signal to act on.
    seed_issue_with_count(&app, lib, series, "iss-a", None).await;
    seed_issue_with_count(&app, lib, series, "iss-b", Some(0)).await;

    reconcile_series_status(&db, series, None).await.unwrap();

    let after = fetch_series(&app, series).await;
    // Default sticks until we have a real Count to act on.
    assert_eq!(after.status, "continuing");
    assert_eq!(after.total_issues, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_count_values_take_max() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let series = seed_series(&app, lib, "Relaunched Series", "continuing", None).await;

    // Three issues with disagreeing counts. The relaunch took the
    // total from 6 → 12; MAX correctly captures the latest known
    // total without needing per-issue update timestamps.
    seed_issue_with_count(&app, lib, series, "iss-a", Some(6)).await;
    seed_issue_with_count(&app, lib, series, "iss-b", Some(6)).await;
    seed_issue_with_count(&app, lib, series, "iss-c", Some(12)).await;

    reconcile_series_status(&db, series, None).await.unwrap();

    let after = fetch_series(&app, series).await;
    assert_eq!(after.status, "ended");
    assert_eq!(after.total_issues, Some(12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_issues_excluded_from_count_max() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let series = seed_series(&app, lib, "Pruned Series", "continuing", None).await;

    let kept = seed_issue_with_count(&app, lib, series, "iss-a", Some(6)).await;
    let removed = seed_issue_with_count(&app, lib, series, "iss-b", Some(99)).await;
    // Soft-delete one issue. Its mis-tagged Count must not contaminate
    // the MAX.
    let row = entity::issue::Entity::find_by_id(removed.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();

    reconcile_series_status(&db, series, None).await.unwrap();

    let after = fetch_series(&app, series).await;
    assert_eq!(after.total_issues, Some(6));
    assert_eq!(after.status, "ended");
    let _ = kept; // silence unused warning
}

// ───── series.json sidecar tests ─────

fn sidecar(
    status: Option<&str>,
    total_issues: Option<i32>,
    description_text: Option<&str>,
    description_formatted: Option<&str>,
    comicid: Option<i64>,
) -> SeriesMetadata {
    SeriesMetadata {
        kind: Some("comicSeries".into()),
        name: None,
        description_text: description_text.map(str::to_owned),
        description_formatted: description_formatted.map(str::to_owned),
        publisher: None,
        imprint: None,
        comic_image: None,
        year_began: None,
        year_end: None,
        total_issues,
        publication_run: None,
        status: status.map(str::to_owned),
        booktype: None,
        age_rating: None,
        comicid,
        volume: None,
        extra: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_status_and_total_authoritative_without_count() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let series = seed_series(&app, lib, "Black Science", "continuing", None).await;
    // No issues at all — the sidecar must be the only signal.
    let meta = sidecar(Some("Ended"), Some(43), None, None, None);

    reconcile_series_status(&db, series, Some(&meta))
        .await
        .unwrap();

    let after = fetch_series(&app, series).await;
    assert_eq!(after.status, "ended");
    assert_eq!(after.total_issues, Some(43));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_total_wins_over_comicinfo_count() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let series = seed_series(&app, lib, "Series", "continuing", None).await;
    // Issues report Count=20, but the sidecar says 43 — sidecar
    // should win because it's per-series intent vs. per-issue
    // inference.
    seed_issue_with_count(&app, lib, series, "iss-a", Some(20)).await;
    seed_issue_with_count(&app, lib, series, "iss-b", Some(20)).await;
    let meta = sidecar(Some("Ended"), Some(43), None, None, None);

    reconcile_series_status(&db, series, Some(&meta))
        .await
        .unwrap();

    let after = fetch_series(&app, series).await;
    assert_eq!(after.total_issues, Some(43));
    assert_eq!(after.status, "ended");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_override_freezes_status_but_total_and_summary_still_flow() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    let stamp = Utc::now().fixed_offset();
    let series = seed_series(&app, lib, "Pinned", "hiatus", Some(stamp)).await;
    let meta = sidecar(
        Some("Ended"),
        Some(12),
        Some("Sidecar summary."),
        None,
        Some(54321),
    );

    reconcile_series_status(&db, series, Some(&meta))
        .await
        .unwrap();

    let after = fetch_series(&app, series).await;
    // Status preserved (user pinned it).
    assert_eq!(after.status, "hiatus");
    assert!(after.status_user_set_at.is_some());
    // But the other sidecar fields still propagate — manual override
    // only freezes status.
    assert_eq!(after.total_issues, Some(12));
    assert_eq!(after.summary.as_deref(), Some("Sidecar summary."));
    assert_eq!(after.comicvine_id, Some(54321));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_signal_does_not_overwrite_existing_total_with_null() {
    // Regression for the v1 bug: reconcile used to write
    // `total_issues = max_count` unconditionally, which nuked any
    // sidecar-derived total whenever a later scan ran without a
    // signal (tombstone path, or no Count on issues).
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    // Pretend an earlier sidecar scan set total_issues=12 directly.
    let series = seed_series(&app, lib, "PriorTotal", "continuing", None).await;
    let prior = sidecar(None, Some(12), None, None, None);
    reconcile_series_status(&db, series, Some(&prior))
        .await
        .unwrap();
    assert_eq!(fetch_series(&app, series).await.total_issues, Some(12));

    // Now: no sidecar, no issues with Count. Reconcile must NOT
    // erase the prior total.
    reconcile_series_status(&db, series, None).await.unwrap();
    assert_eq!(fetch_series(&app, series).await.total_issues, Some(12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn description_text_seeds_summary_with_html_fallback() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;

    // Plain text wins when present.
    let s1 = seed_series(&app, lib, "Plain", "continuing", None).await;
    let plain = sidecar(
        None,
        None,
        Some("Plain summary."),
        Some("<p>Rich.</p>"),
        None,
    );
    reconcile_series_status(&db, s1, Some(&plain))
        .await
        .unwrap();
    assert_eq!(
        fetch_series(&app, s1).await.summary.as_deref(),
        Some("Plain summary.")
    );

    // Falls back to formatted when text absent.
    let s2 = seed_series(&app, lib, "OnlyHtml", "continuing", None).await;
    let html_only = sidecar(None, None, None, Some("<p>Only HTML.</p>"), None);
    reconcile_series_status(&db, s2, Some(&html_only))
        .await
        .unwrap();
    assert_eq!(
        fetch_series(&app, s2).await.summary.as_deref(),
        Some("<p>Only HTML.</p>")
    );

    // Whitespace-only inputs do NOT clobber an empty summary.
    let s3 = seed_series(&app, lib, "Blank", "continuing", None).await;
    let blank = sidecar(None, None, Some("   "), Some("\n\t"), None);
    reconcile_series_status(&db, s3, Some(&blank))
        .await
        .unwrap();
    assert_eq!(fetch_series(&app, s3).await.summary, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comicvine_id_only_backfills_when_null() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib = seed_library(&app, "lib").await;
    // Series row arrives with no comicvine_id — sidecar fills it in.
    let s_empty = seed_series(&app, lib, "EmptyCv", "continuing", None).await;
    let meta = sidecar(None, None, None, None, Some(69537));
    reconcile_series_status(&db, s_empty, Some(&meta))
        .await
        .unwrap();
    assert_eq!(fetch_series(&app, s_empty).await.comicvine_id, Some(69537));

    // Pre-existing id (e.g. set by a richer source) is left alone.
    let s_set = seed_series(&app, lib, "PreSetCv", "continuing", None).await;
    let row = entity::series::Entity::find_by_id(s_set)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.comicvine_id = Set(Some(11111));
    am.update(&db).await.unwrap();
    reconcile_series_status(&db, s_set, Some(&meta))
        .await
        .unwrap();
    // Sidecar's 69537 must NOT clobber the pre-existing 11111.
    assert_eq!(fetch_series(&app, s_set).await.comicvine_id, Some(11111));
}
