//! Integration coverage for the reading-direction resolution chain
//! introduced in `manga-and-bulk-metadata-1.0` M1 + M2.
//!
//! The reader picks reading direction at mount via the chain
//!   1. ComicInfo `<Manga>YesAndRightToLeft</Manga>` on the issue
//!   2. `series.reading_direction`            (M2 — new column)
//!   3. `users.default_reading_direction`     (existing)
//!   4. `library.default_reading_direction`   (M1 — newly consulted)
//!   5. `"ltr"`
//!
//! Server-side these are exposed on the issue-detail response: the
//! issue carries its own `manga` field; `series_reading_direction`
//! and `library_default_reading_direction` are looked up by the
//! handler. This test verifies the lookup + PATCH /series round-trip.

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

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
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
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery-staple"}"#,
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
    let extract = |prefix: &str| {
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

/// Seed a library + series + single issue. `library_dir` and
/// `series_dir` are the values to plant on each respective column;
/// `None` for series means "no override" (Auto).
async fn seed(
    app: &TestApp,
    series_slug: &str,
    library_dir: &str,
    series_dir: Option<&str>,
) -> (Uuid, String) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {series_slug}")),
        root_path: Set(format!("/tmp/{series_slug}-{lib_id}")),
        default_language: Set("en".into()),
        default_reading_direction: Set(library_dir.to_owned()),
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
    }
    .insert(&db)
    .await
    .unwrap();

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(format!("Series {series_slug}")),
        normalized_name: Set(normalize_name(&format!("Series {series_slug}"))),
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
        series_group: Set(None),
        slug: Set(series_slug.to_owned()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
        reading_direction: Set(series_dir.map(str::to_owned)),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let issue_id = format!("{:0>64}", format!("{:x}", lib_id.as_u128()));
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set("issue-1".into()),
        file_path: Set(format!("/tmp/{series_slug}/1.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
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
        hash_algorithm: Set(0),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
    }
    .insert(&db)
    .await
    .unwrap();

    (lib_id, series_slug.to_owned())
}

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn patch(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", auth.csrf.clone())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// M1 — Issue detail response carries the library's
/// `default_reading_direction` so the reader's resolution chain has
/// it available as a fallback below the user pref.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_detail_exposes_library_default_reading_direction() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    seed(&app, "lib-rtl", "rtl", None).await;

    let (status, body) = get(&app, &auth, "/api/series/lib-rtl/issues/issue-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["library_default_reading_direction"], "rtl");
    // Series didn't pin → field should be absent (skip_serializing_if).
    assert!(
        body.get("series_reading_direction").is_none()
            || body["series_reading_direction"].is_null(),
        "series_reading_direction should be omitted when null: {body:#?}",
    );
}

/// M2 — Issue detail also carries `series_reading_direction` when
/// the parent series has pinned a value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_detail_exposes_series_reading_direction() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    seed(&app, "series-rtl", "ltr", Some("rtl")).await;

    let (status, body) = get(&app, &auth, "/api/series/series-rtl/issues/issue-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["series_reading_direction"], "rtl");
    assert_eq!(body["library_default_reading_direction"], "ltr");
}

/// M2 — PATCH /series/{slug} accepts `reading_direction` and the
/// value round-trips through GET /series/{slug}.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_series_reading_direction_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    seed(&app, "patch-dir", "ltr", None).await;

    let (status, _) = patch(
        &app,
        &auth,
        "/api/series/patch-dir",
        serde_json::json!({ "reading_direction": "rtl" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = get(&app, &auth, "/api/series/patch-dir").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reading_direction"], "rtl");

    // Clear back to NULL.
    let (status, _) = patch(
        &app,
        &auth,
        "/api/series/patch-dir",
        serde_json::json!({ "reading_direction": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = get(&app, &auth, "/api/series/patch-dir").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.get("reading_direction").is_none() || body["reading_direction"].is_null(),
        "after clearing, reading_direction should be omitted: {body:#?}",
    );
}

/// M2 — PATCH /series/{slug} rejects unknown reading-direction
/// values so a typo doesn't silently disable the cascade.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_series_rejects_unknown_reading_direction() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    seed(&app, "patch-bad", "ltr", None).await;

    let (status, body) = patch(
        &app,
        &auth,
        "/api/series/patch-bad",
        serde_json::json!({ "reading_direction": "diagonal" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.reading_direction");
}

/// M2 — `"ttb"` is accepted for future webtoon support (R6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_series_accepts_ttb_for_webtoon() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    seed(&app, "patch-ttb", "ltr", None).await;

    let (status, _) = patch(
        &app,
        &auth,
        "/api/series/patch-ttb",
        serde_json::json!({ "reading_direction": "ttb" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = get(&app, &auth, "/api/series/patch-ttb").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reading_direction"], "ttb");
}

// ────────────── M3 — scanner heuristic ──────────────

/// Plant N issues on the series with the given `manga` value.
async fn seed_manga_issues(
    db: &sea_orm::DatabaseConnection,
    library_id: Uuid,
    series_id: Uuid,
    count: u32,
    manga: Option<&str>,
    id_prefix: &str,
) {
    use entity::issue::ActiveModel as IssueAM;
    let now = Utc::now().fixed_offset();
    for i in 0..count {
        let issue_id = format!("{id_prefix}{i:0>62}");
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(library_id),
            series_id: Set(series_id),
            slug: Set(format!("{id_prefix}-{i}")),
            file_path: Set(format!("/tmp/{id_prefix}/{i}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some(f64::from(i))),
            number_raw: Set(Some(i.to_string())),
            volume: Set(None),
            year: Set(None),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            manga: Set(manga.map(str::to_owned)),
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
            hash_algorithm: Set(0),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
            cover_page_index: Set(0),
        }
        .insert(db)
        .await
        .unwrap();
    }
}

/// Look up the current series row from a slug.
async fn fetch_series_by_slug(
    db: &sea_orm::DatabaseConnection,
    slug: &str,
) -> entity::series::Model {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    entity::series::Entity::find()
        .filter(entity::series::Column::Slug.eq(slug))
        .one(db)
        .await
        .unwrap()
        .expect("series exists")
}

/// M3 — when every issue carries the manga flag and the series row
/// has no override, the rollup pins `reading_direction = rtl`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_auto_marks_pure_manga_series_rtl() {
    let app = TestApp::spawn().await;
    let _ = register_admin(&app).await;
    let (lib_id, _) = seed(&app, "auto-rtl", "ltr", None).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    // 4 issues, all marked YesAndRightToLeft.
    let series = fetch_series_by_slug(&db, "auto-rtl").await;
    seed_manga_issues(&db, lib_id, series.id, 4, Some("YesAndRightToLeft"), "m3a").await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series.id)
        .await
        .unwrap();

    let refreshed = fetch_series_by_slug(&db, "auto-rtl").await;
    assert_eq!(refreshed.reading_direction.as_deref(), Some("rtl"));
}

/// M3 — series with mixed manga + non-manga issues (50/50) does NOT
/// flip — the 80% threshold gates the heuristic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_does_not_flip_mixed_series_below_threshold() {
    let app = TestApp::spawn().await;
    let _ = register_admin(&app).await;
    let (lib_id, _) = seed(&app, "auto-mixed", "ltr", None).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let series = fetch_series_by_slug(&db, "auto-mixed").await;
    // 2 manga + 2 plain = 50% manga.
    seed_manga_issues(&db, lib_id, series.id, 2, Some("YesAndRightToLeft"), "m3ba").await;
    seed_manga_issues(&db, lib_id, series.id, 2, None, "m3bb").await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series.id)
        .await
        .unwrap();

    let refreshed = fetch_series_by_slug(&db, "auto-mixed").await;
    assert!(
        refreshed.reading_direction.is_none(),
        "below-threshold series should not be flipped: got {:?}",
        refreshed.reading_direction,
    );
}

/// M3 — series with fewer than 3 issues is skipped to avoid flipping
/// a tiny series on a single mis-tagged file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_skips_tiny_series() {
    let app = TestApp::spawn().await;
    let _ = register_admin(&app).await;
    let (lib_id, _) = seed(&app, "auto-tiny", "ltr", None).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let series = fetch_series_by_slug(&db, "auto-tiny").await;
    // 2 issues, both manga — still under the 3-issue minimum.
    seed_manga_issues(&db, lib_id, series.id, 2, Some("YesAndRightToLeft"), "m3c").await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series.id)
        .await
        .unwrap();

    let refreshed = fetch_series_by_slug(&db, "auto-tiny").await;
    assert!(refreshed.reading_direction.is_none());
}

/// M3 — admin-set value is sticky. The heuristic must never overwrite
/// a pinned value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_does_not_overwrite_admin_set_value() {
    let app = TestApp::spawn().await;
    let _ = register_admin(&app).await;
    // Series already pinned to "ltr" by an admin.
    let (lib_id, _) = seed(&app, "auto-sticky", "ltr", Some("ltr")).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let series = fetch_series_by_slug(&db, "auto-sticky").await;
    // Plant 4 manga issues — heuristic WOULD fire on a NULL series.
    seed_manga_issues(&db, lib_id, series.id, 4, Some("YesAndRightToLeft"), "m3d").await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series.id)
        .await
        .unwrap();

    let refreshed = fetch_series_by_slug(&db, "auto-sticky").await;
    assert_eq!(
        refreshed.reading_direction.as_deref(),
        Some("ltr"),
        "admin-set value must not be overwritten by the heuristic",
    );
}

/// M3 — `manga = "Yes"` (without "AndRightToLeft") still counts. Some
/// taggers emit `Yes` for left-to-right manga (rare but valid); the
/// heuristic groups both forms because a series tagged "Yes" is still
/// declaring its genre, and the user's per-account default or library
/// default can take it from there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_counts_plain_yes_as_manga() {
    let app = TestApp::spawn().await;
    let _ = register_admin(&app).await;
    let (lib_id, _) = seed(&app, "auto-yes", "ltr", None).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let series = fetch_series_by_slug(&db, "auto-yes").await;
    seed_manga_issues(&db, lib_id, series.id, 4, Some("Yes"), "m3e").await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series.id)
        .await
        .unwrap();

    let refreshed = fetch_series_by_slug(&db, "auto-yes").await;
    assert_eq!(refreshed.reading_direction.as_deref(), Some("rtl"));
}
