//! WP-2.7: `library_user_access.age_rating_max` is enforced on every
//! read surface.
//!
//! Fixture: one library with three series — rated `Teen`, rated
//! `Mature 17+`, and unrated (whose second issue carries its own
//! `Mature 17+` rating). User A holds a `Teen` cap, user B an uncapped
//! grant, and the first-registered account is the admin.
//!
//! Decision D6: unrated content is SHOWN to capped users — only rows
//! whose rating ranks above the cap are hidden.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set, Unchanged};
use std::io::{Cursor, Write};
use tower::ServiceExt;
use uuid::Uuid;

// ───── scaffolding (mirrors next_up.rs; small enough not to share) ─────

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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let session = extract("__Host-comic_session=");
    let csrf = extract("__Host-comic_csrf=");
    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::{ColumnTrait, QueryFilter};
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row by email");
    Authed {
        session,
        csrf,
        user_id: user_row.id,
    }
}

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::user::ActiveModel {
        id: Unchanged(user_id),
        role: Set(entity::user::UserRole::User),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
}

async fn raw(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    let cookie = format!(
        "__Host-comic_session={}; __Host-comic_csrf={}",
        user.session, user.csrf
    );
    req = req.header(header::COOKIE, cookie);
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let body_bytes = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body_bytes).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn get_json(app: &TestApp, path: &str, user: &Authed) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = raw(app, Method::GET, path, user, None).await;
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, body)
}

/// A real two-page CBZ (PNG signatures pass the content sniff) so page
/// bytes / thumbnails can actually be served for the uncapped user.
fn write_cbz(path: &std::path::Path) {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for name in ["p1.png", "p2.png"] {
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                4,
                4,
                image::Rgb([90, 90, 90]),
            ));
            let mut pbuf = Cursor::new(Vec::new());
            img.write_to(&mut pbuf, image::ImageFormat::Png).unwrap();
            zw.start_file(name, opts).unwrap();
            zw.write_all(&pbuf.into_inner()).unwrap();
        }
        zw.finish().unwrap();
    }
    std::fs::write(path, buf.into_inner()).unwrap();
}

struct Fixture {
    lib_id: Uuid,
    teen_series: Uuid,
    mature_series: Uuid,
    unrated_series: Uuid,
    /// (id, slug) pairs.
    teen_1: String,
    teen_2: String,
    mature_1: String,
    mature_2: String,
    unrated_1: String,
    unrated_mature_2: String,
}

async fn seed_series(
    db: &sea_orm::DatabaseConnection,
    lib_id: Uuid,
    name: &str,
    slug: &str,
    rating: Option<&str>,
) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(Some("Cap Comics".into())),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(rating.map(str::to_owned)),
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
        slug: Set(slug.into()),
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
    .insert(db)
    .await
    .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
async fn seed_issue(
    db: &sea_orm::DatabaseConnection,
    root: &std::path::Path,
    lib_id: Uuid,
    series_id: Uuid,
    n: u8,
    slug: &str,
    title: &str,
    rating: Option<&str>,
    special: Option<&str>,
) -> String {
    let now = Utc::now().fixed_offset();
    let id = format!("{:0>62}{:02x}", series_id.simple(), n);
    let file = root.join(format!("{slug}.cbz"));
    write_cbz(&file);
    IssueAM {
        id: Set(id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug.into()),
        file_path: Set(file.to_string_lossy().into_owned()),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set(entity::issue::IssueState::Active),
        content_hash: Set(id.clone()),
        title: Set(Some(title.into())),
        sort_number: Set(Some(n as f64)),
        number_raw: Set(Some(n.to_string())),
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
        age_rating: Set(rating.map(str::to_owned)),
        page_count: Set(Some(2)),
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
        writer: Set(Some("Cap Writer".into())),
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
        special_type: Set(special.map(str::to_owned)),
        hash_algorithm: Set(0),
        metroninfo_present: Set(None),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        comicinfo_count: Set(Some(0)),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        last_sidecar_rewrite_at: Set(None),
        metron_info_raw: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    // Junction credit row — what `/people` and `/creators` aggregate.
    entity::issue_credit::ActiveModel {
        issue_id: Set(id.clone()),
        role: Set("writer".into()),
        person: Set("Cap Writer".into()),
        person_id: Set(None),
        ordinal: Set(0),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_library(app: &TestApp) -> Fixture {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    let root = app._data_dir.path().join("cap-lib");
    std::fs::create_dir_all(&root).unwrap();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Cap Lib".into()),
        root_path: Set(root.to_string_lossy().into_owned()),
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
        show_in_sidebar_by_default: Set(true),
    }
    .insert(&db)
    .await
    .unwrap();

    let teen_series = seed_series(&db, lib_id, "Teen Tales", "teen-tales", Some("Teen")).await;
    let mature_series = seed_series(
        &db,
        lib_id,
        "Mature Matters",
        "mature-matters",
        Some("Mature 17+"),
    )
    .await;
    let unrated_series = seed_series(&db, lib_id, "Unrated Stories", "unrated-stories", None).await;

    let teen_1 = seed_issue(
        &db,
        &root,
        lib_id,
        teen_series,
        1,
        "teen-1",
        "Teen One",
        None,
        None,
    )
    .await;
    let teen_2 = seed_issue(
        &db,
        &root,
        lib_id,
        teen_series,
        2,
        "teen-2",
        "Teen Two",
        None,
        None,
    )
    .await;
    let mature_1 = seed_issue(
        &db,
        &root,
        lib_id,
        mature_series,
        1,
        "mature-1",
        "Mature One",
        None,
        None,
    )
    .await;
    let mature_2 = seed_issue(
        &db,
        &root,
        lib_id,
        mature_series,
        2,
        "mature-2",
        "Mature Two",
        None,
        None,
    )
    .await;
    let unrated_1 = seed_issue(
        &db,
        &root,
        lib_id,
        unrated_series,
        1,
        "unrated-1",
        "Unrated One",
        None,
        None,
    )
    .await;
    // Issue-level override: the series is unrated but this issue is not.
    let unrated_mature_2 = seed_issue(
        &db,
        &root,
        lib_id,
        unrated_series,
        2,
        "unrated-2",
        "Unrated Mature Two",
        Some("mature 17+"),
        None,
    )
    .await;

    Fixture {
        lib_id,
        teen_series,
        mature_series,
        unrated_series,
        teen_1,
        teen_2,
        mature_1,
        mature_2,
        unrated_1,
        unrated_mature_2,
    }
}

async fn grant(app: &TestApp, user_id: Uuid, library_id: Uuid, cap: Option<&str>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn progress(app: &TestApp, user_id: Uuid, issue_id: &str, finished: bool) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    progress_record::ActiveModel {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(1),
        percent: Set(if finished { 1.0 } else { 0.5 }),
        finished: Set(finished),
        finished_at: Set(finished.then_some(now)),
        updated_at: Set(now),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

struct World {
    app: TestApp,
    fx: Fixture,
    admin: Authed,
    capped: Authed,
    uncapped: Authed,
}

async fn world() -> World {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@cap.test").await;
    let capped = register(&app, "a@cap.test").await;
    demote_to_user(&app, capped.user_id).await;
    let uncapped = register(&app, "b@cap.test").await;
    demote_to_user(&app, uncapped.user_id).await;
    let fx = seed_library(&app).await;
    grant(&app, capped.user_id, fx.lib_id, Some("Teen")).await;
    grant(&app, uncapped.user_id, fx.lib_id, None).await;
    World {
        app,
        fx,
        admin,
        capped,
        uncapped,
    }
}

fn names(body: &serde_json::Value, key: &str) -> Vec<String> {
    body["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i[key].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// ───── tests ─────

#[tokio::test]
async fn series_list_hides_above_cap_and_shows_unrated() {
    let w = world().await;
    let (st, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    let mut got = names(&body, "name");
    got.sort();
    assert_eq!(got, vec!["Teen Tales", "Unrated Stories"]);
    assert_eq!(body["total"], 2);

    let (_, body) = get_json(&w.app, "/api/series", &w.uncapped).await;
    assert_eq!(names(&body, "name").len(), 3);
    let (_, body) = get_json(&w.app, "/api/series", &w.admin).await;
    assert_eq!(names(&body, "name").len(), 3);

    // Search arm of the series list.
    let (_, body) = get_json(&w.app, "/api/series?q=Mature", &w.capped).await;
    assert!(names(&body, "name").is_empty());
    let (_, body) = get_json(&w.app, "/api/series?q=Mature", &w.uncapped).await;
    assert_eq!(names(&body, "name"), vec!["Mature Matters"]);
}

#[tokio::test]
async fn issue_lists_hide_capped_issues_including_issue_level_override() {
    let w = world().await;
    let (st, body) = get_json(&w.app, "/api/issues?limit=50", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(got, vec!["Teen One", "Teen Two", "Unrated One"]);
    let (_, body) = get_json(&w.app, "/api/issues?limit=50", &w.uncapped).await;
    assert_eq!(names(&body, "title").len(), 6);

    // Per-series issue list: the unrated series' Mature issue is hidden.
    let (st, body) = get_json(&w.app, "/api/series/unrated-stories/issues", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(names(&body, "title"), vec!["Unrated One"]);
    let (_, body) = get_json(&w.app, "/api/series/unrated-stories/issues", &w.uncapped).await;
    assert_eq!(names(&body, "title").len(), 2);

    // Cross-library issue search.
    let (_, body) = get_json(&w.app, "/api/issues/search?q=Mature", &w.capped).await;
    assert!(names(&body, "title").is_empty(), "{body}");
    let (_, body) = get_json(&w.app, "/api/issues/search?q=Mature", &w.uncapped).await;
    assert_eq!(names(&body, "title").len(), 3);
}

#[tokio::test]
async fn detail_page_bytes_and_covers_404_for_capped_user() {
    let w = world().await;
    let fx = &w.fx;
    // Series detail.
    let (st, _) = get_json(&w.app, "/api/series/mature-matters", &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = get_json(&w.app, "/api/series/mature-matters", &w.uncapped).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get_json(&w.app, "/api/series/unrated-stories", &w.capped).await;
    assert_eq!(st, StatusCode::OK);

    // Issue detail (series-inherited rating + issue-level override).
    for path in [
        "/api/series/mature-matters/issues/mature-1",
        "/api/series/unrated-stories/issues/unrated-2",
    ] {
        let (st, _) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        let (st, _) = get_json(&w.app, path, &w.uncapped).await;
        assert_eq!(st, StatusCode::OK, "{path}");
    }
    let (st, _) = get_json(&w.app, "/api/series/teen-tales/issues/teen-1", &w.capped).await;
    assert_eq!(st, StatusCode::OK);

    // Page bytes + cover thumb: the direct URL must not leak.
    for id in [&fx.mature_1, &fx.unrated_mature_2] {
        for path in [
            format!("/issues/{id}/pages/0"),
            format!("/issues/{id}/pages/0/thumb"),
        ] {
            let (st, _) = raw(&w.app, Method::GET, &path, &w.capped, None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
            let (st, _) = raw(&w.app, Method::GET, &path, &w.uncapped, None).await;
            assert_eq!(st, StatusCode::OK, "{path}");
            let (st, _) = raw(&w.app, Method::GET, &path, &w.admin, None).await;
            assert_eq!(st, StatusCode::OK, "{path}");
        }
    }
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/issues/{}/pages/0", fx.teen_1),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/issues/{}/pages/0", fx.unrated_1),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn opds_feeds_are_filtered() {
    let w = world().await;
    let (st, bytes) = raw(&w.app, Method::GET, "/opds/v1/series", &w.capped, None).await;
    assert_eq!(st, StatusCode::OK);
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("Teen Tales"), "{xml}");
    assert!(xml.contains("Unrated Stories"), "{xml}");
    assert!(!xml.contains("Mature Matters"), "{xml}");

    let (_, bytes) = raw(&w.app, Method::GET, "/opds/v1/series", &w.uncapped, None).await;
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("Mature Matters"), "{xml}");

    // Per-series feed: the capped series 404s; the unrated series hides
    // its Mature issue.
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/opds/v1/series/{}", w.fx.mature_series),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, bytes) = raw(
        &w.app,
        Method::GET,
        &format!("/opds/v1/series/{}", w.fx.unrated_series),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("Unrated One"), "{xml}");
    assert!(!xml.contains("Unrated Mature Two"), "{xml}");

    // OPDS 2.0 mirrors 1.2.
    let (st, bytes) = raw(&w.app, Method::GET, "/opds/v2/series", &w.capped, None).await;
    assert_eq!(st, StatusCode::OK);
    let json = String::from_utf8(bytes).unwrap();
    assert!(json.contains("Teen Tales"), "{json}");
    assert!(!json.contains("Mature Matters"), "{json}");

    // Recent feed lists issues; the capped user never sees a Mature one.
    let (st, bytes) = raw(&w.app, Method::GET, "/opds/v1/recent", &w.capped, None).await;
    assert_eq!(st, StatusCode::OK);
    let xml = String::from_utf8(bytes).unwrap();
    assert!(xml.contains("Teen One"), "{xml}");
    assert!(!xml.contains("Mature"), "{xml}");

    // Download of a capped issue 404s.
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/opds/v1/issues/{}/file", w.fx.mature_1),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let _ = &w.fx.mature_2;
}

#[tokio::test]
async fn rails_and_next_up_skip_capped_issues() {
    let w = world().await;
    let fx = &w.fx;
    // A is mid-way through Mature #1 (continue reading) and finished
    // Teen #1 + Mature #2's predecessor — i.e. both series are "on deck".
    progress(&w.app, w.capped.user_id, &fx.mature_1, false).await;
    progress(&w.app, w.capped.user_id, &fx.teen_1, true).await;
    // B mirrors A so the uncapped rails prove the seed is visible.
    progress(&w.app, w.uncapped.user_id, &fx.mature_1, false).await;
    progress(&w.app, w.uncapped.user_id, &fx.teen_1, true).await;

    let (st, body) = get_json(&w.app, "/api/me/continue-reading", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
    let (_, body) = get_json(&w.app, "/api/me/continue-reading", &w.uncapped).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1, "{body}");

    let (st, body) = get_json(&w.app, "/api/me/on-deck", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    let titles: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["issue"]["title"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(titles, vec!["Teen Two"], "{body}");

    // Next-up: the capped current issue is a 404; a visible one resolves.
    let (st, _) = get_json(
        &w.app,
        &format!("/api/issues/{}/next-up", fx.mature_1),
        &w.capped,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, body) = get_json(
        &w.app,
        &format!("/api/issues/{}/next-up", fx.teen_1),
        &w.capped,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["target"]["title"], "Teen Two", "{body}");
    // Series walk skips the issue-level override.
    let (st, body) = get_json(
        &w.app,
        &format!("/api/issues/{}/next-up", fx.unrated_1),
        &w.capped,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(body["target"]["title"], "Unrated Mature Two", "{body}");
    let (_, body) = get_json(
        &w.app,
        &format!("/api/issues/{}/next-up", fx.unrated_1),
        &w.uncapped,
    )
    .await;
    assert_eq!(body["target"]["title"], "Unrated Mature Two", "{body}");
    let _ = (&fx.teen_2, &fx.teen_series);
}

#[tokio::test]
async fn creators_rails_are_filtered() {
    let w = world().await;
    // The writer credit exists on every issue; the count only includes
    // visible issues for the capped user (raw-SQL UNION arms).
    for path in ["/api/people?q=cap", "/api/creators"] {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        let capped_count = body["items"][0]["credit_count"].as_i64().unwrap_or(0);
        let (st, body) = get_json(&w.app, path, &w.uncapped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        let full_count = body["items"][0]["credit_count"].as_i64().unwrap_or(0);
        assert_eq!(full_count, 6, "{path}: {body}");
        assert_eq!(capped_count, 3, "{path}");
    }
}

#[tokio::test]
async fn recent_issues_rail_is_filtered() {
    let w = world().await;
    let (st, body) = get_json(&w.app, "/api/me/recent-issues", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(got, vec!["Teen One", "Teen Two", "Unrated One"], "{body}");
    let (_, body) = get_json(&w.app, "/api/me/recent-issues", &w.uncapped).await;
    assert_eq!(names(&body, "title").len(), 6, "{body}");
}

#[tokio::test]
async fn admin_grant_api_round_trips_the_cap_and_rejects_bad_values() {
    let w = world().await;
    let target = w.capped.user_id;
    let lib = w.fx.lib_id.to_string();
    let path = format!("/api/admin/users/{target}/library-access");

    // Canonical spelling is stored regardless of input case.
    let (st, bytes) = raw(
        &w.app,
        Method::POST,
        &path,
        &w.admin,
        Some(serde_json::json!({
            "library_ids": [lib],
            "age_rating_caps": { lib.clone(): "everyone 10+" },
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["library_access"][0]["library_id"], lib);
    assert_eq!(body["library_access"][0]["age_rating_max"], "Everyone 10+");
    assert!(body["library_access"][0].get("role").is_none());

    // The new cap is live: Teen is now above the cap too.
    let (_, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert_eq!(names(&body, "name"), vec!["Unrated Stories"]);

    // Omitting the map clears the cap.
    let (st, bytes) = raw(
        &w.app,
        Method::POST,
        &path,
        &w.admin,
        Some(serde_json::json!({ "library_ids": [lib] })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["library_access"][0]["age_rating_max"].is_null());
    let (_, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert_eq!(names(&body, "name").len(), 3);

    // Unknown rating → 422 with the field named.
    let (st, bytes) = raw(
        &w.app,
        Method::POST,
        &path,
        &w.admin,
        Some(serde_json::json!({
            "library_ids": [lib],
            "age_rating_caps": { lib.clone(): "Sixteen" },
        })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("AgeRating"),
        "{body}"
    );

    // Audit row carries the cap.
    let (_, body) = get_json(
        &w.app,
        "/api/admin/audit?action=admin.user.library_access.set",
        &w.admin,
    )
    .await;
    let entries = body["items"].as_array().cloned().unwrap_or_default();
    assert!(
        entries
            .iter()
            .any(|e| e["payload"]["age_rating_caps"][&lib] == "Everyone 10+"),
        "{body}"
    );
}

// ───── issue-level cap inside a visible series ─────
//
// "Gap Run" is rated `Teen` (visible to the `Teen`-capped reader) and holds
// three main-run issues; only #2 carries its own `Mature 17+` rating, so it
// is the one issue the capped reader must not learn about. It also carries
// the only "Hidden Hand" credit, the only "Secret Agent" character and the
// only 2031 cover year, so every rollup can be checked for it. "Gap
// Annuals" (unrated) repeats the shape in the `Annual` specials lane.

struct Gap {
    series_id: Uuid,
    one: String,
    two: String,
    three: String,
    /// "Lead Gap" (rated `Teen`): its #1 is the hidden one, so the
    /// heuristic cover of an uncapped card is the hidden issue.
    lead_hidden: String,
}

async fn seed_gap(app: &TestApp, lib_id: Uuid) -> Gap {
    let db = Database::connect(&app.db_url).await.unwrap();
    let root = app._data_dir.path().join("cap-lib");
    let series_id = seed_series(&db, lib_id, "Gap Run", "gap-run", Some("Teen")).await;
    let one = seed_issue(
        &db, &root, lib_id, series_id, 1, "gap-1", "Gap One", None, None,
    )
    .await;
    let two = seed_issue(
        &db,
        &root,
        lib_id,
        series_id,
        2,
        "gap-2",
        "Gap Two",
        Some("Mature 17+"),
        None,
    )
    .await;
    let three = seed_issue(
        &db,
        &root,
        lib_id,
        series_id,
        3,
        "gap-3",
        "Gap Three",
        None,
        None,
    )
    .await;
    entity::issue_credit::ActiveModel {
        issue_id: Set(two.clone()),
        role: Set("writer".into()),
        person: Set("Hidden Hand".into()),
        person_id: Set(None),
        ordinal: Set(1),
    }
    .insert(&db)
    .await
    .unwrap();
    entity::issue::ActiveModel {
        id: Unchanged(two.clone()),
        characters: Set(Some("Secret Agent".into())),
        year: Set(Some(2031)),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    let annuals = seed_series(&db, lib_id, "Gap Annuals", "gap-annuals", None).await;
    for (n, rating) in [(1u8, None), (2, Some("Mature 17+")), (3, None)] {
        seed_issue(
            &db,
            &root,
            lib_id,
            annuals,
            n,
            &format!("gap-annual-{n}"),
            &format!("Gap Annual {n}"),
            rating,
            Some("Annual"),
        )
        .await;
    }
    let lead = seed_series(&db, lib_id, "Lead Gap", "lead-gap", Some("Teen")).await;
    let lead_hidden = seed_issue(
        &db,
        &root,
        lib_id,
        lead,
        1,
        "lead-1",
        "Lead One",
        Some("Mature 17+"),
        None,
    )
    .await;
    seed_issue(
        &db, &root, lib_id, lead, 2, "lead-2", "Lead Two", None, None,
    )
    .await;
    Gap {
        series_id,
        one,
        two,
        three,
        lead_hidden,
    }
}

async fn gap_world() -> (World, Gap) {
    let w = world().await;
    let gap = seed_gap(&w.app, w.fx.lib_id).await;
    (w, gap)
}

async fn post_json(
    app: &TestApp,
    path: &str,
    user: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = raw(app, Method::POST, path, user, Some(body)).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

fn item_titles(items: &serde_json::Value) -> Vec<String> {
    items
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["title"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn has_progress_row(app: &TestApp, user_id: Uuid, issue_id: &str) -> bool {
    use sea_orm::{ColumnTrait, QueryFilter};
    let db = Database::connect(&app.db_url).await.unwrap();
    progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.eq(issue_id))
        .one(&db)
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn next_and_prev_skip_an_issue_above_the_cap() {
    let (w, _gap) = gap_world().await;
    let next = "/api/series/gap-run/issues/gap-1/next";
    let prev = "/api/series/gap-run/issues/gap-3/prev";

    let (st, body) = get_json(&w.app, next, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(item_titles(&body["items"]), vec!["Gap Three"], "{body}");
    let (st, body) = get_json(&w.app, prev, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["item"]["title"], "Gap One", "{body}");

    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, next, user).await;
        assert_eq!(
            item_titles(&body["items"]),
            vec!["Gap Two", "Gap Three"],
            "{body}"
        );
        let (_, body) = get_json(&w.app, prev, user).await;
        assert_eq!(body["item"]["title"], "Gap Two", "{body}");
    }

    // Specials lane: the same skip in both directions.
    let next = "/api/series/gap-annuals/issues/gap-annual-1/next";
    let prev = "/api/series/gap-annuals/issues/gap-annual-3/prev";
    let (_, body) = get_json(&w.app, next, &w.capped).await;
    assert_eq!(item_titles(&body["items"]), vec!["Gap Annual 3"], "{body}");
    let (_, body) = get_json(&w.app, prev, &w.capped).await;
    assert_eq!(body["item"]["title"], "Gap Annual 1", "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, next, user).await;
        assert_eq!(
            item_titles(&body["items"]),
            vec!["Gap Annual 2", "Gap Annual 3"],
            "{body}"
        );
        let (_, body) = get_json(&w.app, prev, user).await;
        assert_eq!(body["item"]["title"], "Gap Annual 2", "{body}");
    }
}

#[tokio::test]
async fn hidden_issue_routes_answer_like_an_unknown_id() {
    let (w, gap) = gap_world().await;
    let hidden = &gap.two;
    let unknown = format!("{:0>64}", "f");

    // Slug-addressed routes: (hidden path, unknown path).
    let slug_routes: Vec<(String, String)> = [
        "",
        "/metadata-overview",
        "/health-issues",
        "/appearances",
        "/next",
        "/prev",
        "/external-ids",
        "/metadata/candidates",
    ]
    .iter()
    .map(|suffix| {
        (
            format!("/api/series/gap-run/issues/gap-2{suffix}"),
            format!("/api/series/gap-run/issues/no-such-issue{suffix}"),
        )
    })
    .collect();
    // Id-addressed routes.
    let id_routes: Vec<(String, String)> = [
        "/api/issues/{}/covers",
        "/api/me/issues/{}/markers",
        "/api/me/issues/{}/page-overrides",
        "/api/issues/{}/next-up",
        "/api/issues/{}/prev-up",
        "/issues/{}/pages/0",
        "/issues/{}/pages/0/thumb",
        "/issues/{}",
    ]
    .iter()
    .map(|t| (t.replace("{}", hidden), t.replace("{}", &unknown)))
    .collect();

    for (hidden_path, unknown_path) in slug_routes.iter().chain(id_routes.iter()) {
        let (st_h, body_h) = raw(&w.app, Method::GET, hidden_path, &w.capped, None).await;
        let (st_u, body_u) = raw(&w.app, Method::GET, unknown_path, &w.capped, None).await;
        assert_eq!(st_h, StatusCode::NOT_FOUND, "{hidden_path}");
        assert_eq!(st_h, st_u, "{hidden_path} vs {unknown_path}");
        assert_eq!(
            String::from_utf8_lossy(&body_h),
            String::from_utf8_lossy(&body_u),
            "{hidden_path} body must match an unknown id's"
        );
        // Found for an uncapped reader and for the admin. The candidates
        // route has no run yet, which is its own, different 404.
        for user in [&w.uncapped, &w.admin] {
            let (st, body) = raw(&w.app, Method::GET, hidden_path, user, None).await;
            if hidden_path.ends_with("/metadata/candidates") {
                let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(
                    v["error"]["code"], "metadata.run_not_found",
                    "{hidden_path}"
                );
            } else {
                assert!(
                    st.is_success() || st.is_redirection(),
                    "{hidden_path}: {st} {}",
                    String::from_utf8_lossy(&body)
                );
            }
        }
    }

    // Writes keyed on the issue: a marker on it and an OCR request.
    for (path, body) in [
        (
            "/api/me/markers".to_owned(),
            serde_json::json!({"issue_id": "{}", "page_index": 0, "kind": "bookmark"}),
        ),
        (
            "/api/me/issues/{}/ocr".to_owned(),
            serde_json::json!({"page": 0, "region": {"x": 0, "y": 0, "w": 1, "h": 1}}),
        ),
    ] {
        let fill = |id: &str| {
            (
                path.replace("{}", id),
                serde_json::from_str::<serde_json::Value>(&body.to_string().replace("{}", id))
                    .unwrap(),
            )
        };
        let (hp, hb) = fill(hidden);
        let (up, ub) = fill(&unknown);
        let (st_h, body_h) = raw(&w.app, Method::POST, &hp, &w.capped, Some(hb)).await;
        let (st_u, body_u) = raw(&w.app, Method::POST, &up, &w.capped, Some(ub)).await;
        assert_eq!(st_h, StatusCode::NOT_FOUND, "{hp}");
        assert_eq!(st_h, st_u, "{hp}");
        assert_eq!(body_h, body_u, "{hp} body must match an unknown id's");
    }
    let (st, body) = post_json(
        &w.app,
        "/api/me/markers",
        &w.uncapped,
        serde_json::json!({"issue_id": hidden, "page_index": 0, "kind": "bookmark"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    // A visible sibling still answers for the capped reader.
    let (st, _) = get_json(
        &w.app,
        &format!("/api/issues/{}/covers", gap.one),
        &w.capped,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get_json(
        &w.app,
        "/api/series/gap-run/issues/gap-3/external-ids",
        &w.capped,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn series_detail_and_card_leave_out_the_hidden_issue() {
    let (w, gap) = gap_world().await;
    let detail = |body: &serde_json::Value| {
        (
            body["issue_count"].as_i64(),
            body["progress_summary"]["total"].as_i64(),
            body["total_page_count"].as_i64(),
            body["latest_year"].as_i64(),
        )
    };
    let (st, capped) = get_json(&w.app, "/api/series/gap-run", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{capped}");
    assert_eq!(
        detail(&capped),
        (Some(2), Some(2), Some(4), Some(2020)),
        "{capped}"
    );
    for user in [&w.uncapped, &w.admin] {
        let (_, full) = get_json(&w.app, "/api/series/gap-run", user).await;
        assert_eq!(
            detail(&full),
            (Some(3), Some(3), Some(6), Some(2031)),
            "{full}"
        );
        let writers = full["writers"].to_string();
        assert!(writers.contains("Hidden Hand"), "{full}");
        assert!(
            full["characters"].to_string().contains("Secret Agent"),
            "{full}"
        );
    }
    assert!(
        !capped["writers"].to_string().contains("Hidden Hand"),
        "{capped}"
    );
    assert!(
        !capped["characters"].to_string().contains("Secret Agent"),
        "{capped}"
    );
    assert!(!capped.to_string().contains(&gap.two), "{capped}");

    // The series card on the library grid agrees with the detail page and
    // with what the reader can list.
    let card = |body: &serde_json::Value| {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["slug"] == "gap-run")
            .map(|s| {
                (
                    s["issue_count"].as_i64(),
                    s["progress_summary"]["total"].as_i64(),
                )
            })
    };
    let (_, body) = get_json(&w.app, "/api/series?limit=50", &w.capped).await;
    assert_eq!(card(&body), Some((Some(2), Some(2))), "{body}");
    let (_, listed) = get_json(&w.app, "/api/series/gap-run/issues", &w.capped).await;
    assert_eq!(names(&listed, "title").len(), 2, "{listed}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, "/api/series?limit=50", user).await;
        assert_eq!(card(&body), Some((Some(3), Some(3))), "{body}");
    }

    // Collection report: the hidden issue is neither owned nor listed.
    let (st, body) = get_json(&w.app, "/api/series/gap-run/collection", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["issues"].as_array().map(Vec::len), Some(2), "{body}");
    assert!(!body.to_string().contains("Gap Two"), "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, "/api/series/gap-run/collection", user).await;
        assert_eq!(body["issues"].as_array().map(Vec::len), Some(3), "{body}");
    }
    let _ = gap.series_id;
}

#[tokio::test]
async fn series_metadata_tab_counts_only_visible_issues() {
    let (w, gap) = gap_world().await;
    let ids = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["id"].as_str().map(str::to_owned))
            .collect()
    };
    let path = "/api/series/gap-run/metadata/issues";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["series_total"], 2, "{body}");
    assert_eq!(body["total"], 2, "{body}");
    assert_eq!(ids(&body), vec![gap.one.clone(), gap.three.clone()]);
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        assert_eq!(body["series_total"], 3, "{body}");
        assert!(ids(&body).contains(&gap.two), "{body}");
    }

    let path = "/api/series/gap-run/metadata/overview";
    let figures = |body: &serde_json::Value| {
        (
            body["issue_ids"]["total"].as_i64(),
            body["files"]["archives"].as_i64(),
        )
    };
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(figures(&body), (Some(2), Some(2)), "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        assert_eq!(figures(&body), (Some(3), Some(3)), "{body}");
    }
}

#[tokio::test]
async fn series_wide_progress_never_touches_the_hidden_issue() {
    let (w, gap) = gap_world().await;
    let (st, body) = post_json(
        &w.app,
        "/api/series/gap-run/progress",
        &w.capped,
        serde_json::json!({"finished": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 2, "{body}");
    assert!(has_progress_row(&w.app, w.capped.user_id, &gap.one).await);
    assert!(has_progress_row(&w.app, w.capped.user_id, &gap.three).await);
    assert!(!has_progress_row(&w.app, w.capped.user_id, &gap.two).await);

    let (st, body) = post_json(
        &w.app,
        "/api/series/gap-run/progress-matching",
        &w.capped,
        serde_json::json!({"finished": false}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 2, "{body}");
    let (st, body) = post_json(
        &w.app,
        "/api/me/progress/series-bulk",
        &w.capped,
        serde_json::json!({"series_ids": [gap.series_id], "finished": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 2, "{body}");
    assert!(!has_progress_row(&w.app, w.capped.user_id, &gap.two).await);

    // The progress summary counts what the reader can see: all read.
    let (_, body) = get_json(&w.app, "/api/series/gap-run", &w.capped).await;
    assert_eq!(body["progress_summary"]["total"], 2, "{body}");
    assert_eq!(body["progress_summary"]["finished"], 2, "{body}");

    // Uncapped and admin readers mark all three.
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = post_json(
            &w.app,
            "/api/series/gap-run/progress",
            user,
            serde_json::json!({"finished": true}),
        )
        .await;
        assert_eq!(body["updated"], 3, "{body}");
        assert!(has_progress_row(&w.app, user.user_id, &gap.two).await);
    }
}

#[tokio::test]
async fn reading_log_leaves_out_a_read_of_the_hidden_issue() {
    let (w, gap) = gap_world().await;
    // Reads recorded as if before the cap applied: the capped reader
    // finished all three issues, so did the uncapped reader.
    for user in [&w.capped, &w.uncapped] {
        for id in [&gap.one, &gap.two, &gap.three] {
            progress(&w.app, user.user_id, id, true).await;
        }
    }
    let issue_ids = |body: &serde_json::Value| -> Vec<String> {
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["issue"]["id"].as_str().map(str::to_owned))
            .collect()
    };
    let (st, body) = get_json(&w.app, "/api/me/reading-log", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let got = issue_ids(&body);
    assert!(got.contains(&gap.one) && got.contains(&gap.three), "{body}");
    assert!(!got.contains(&gap.two), "{body}");
    assert!(!body.to_string().contains("Gap Two"), "{body}");
    // Every visible issue read → the series counts as finished for them.
    let series_finished = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "series_finished" && e["series"]["slug"] == "gap-run");
    assert!(series_finished, "{body}");

    let (_, body) = get_json(&w.app, "/api/me/reading-log", &w.uncapped).await;
    assert!(issue_ids(&body).contains(&gap.two), "{body}");

    // The CSV export runs the same pipeline.
    let (st, bytes) = raw(
        &w.app,
        Method::GET,
        "/api/me/reading-log/export",
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let csv = String::from_utf8(bytes).unwrap();
    assert!(csv.contains("Gap One") && !csv.contains("Gap Two"), "{csv}");
}

// ───── round two: coverage, OPDS covers, list filters, bulk progress ─────

/// `(status, body)` with every occurrence of `id` replaced by `X`, so the
/// answer for a hidden id can be compared with the one for an unknown id.
async fn masked(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
    id: &str,
) -> (StatusCode, String) {
    let (st, bytes) = raw(app, method, path, user, body).await;
    (st, String::from_utf8_lossy(&bytes).replace(id, "X"))
}

fn unknown_issue_id() -> String {
    format!("{:0>64}", "f")
}

#[tokio::test]
async fn provider_coverage_segments_leave_out_the_hidden_issue() {
    let (w, gap) = gap_world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::external_id::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(gap.series_id.to_string()),
        source: Set("comicvine".into()),
        external_id: Set("1000".into()),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    // Only the hidden #2 maps to another provider series.
    entity::series_provider_range::ActiveModel {
        id: Set(Uuid::now_v7()),
        series_id: Set(gap.series_id),
        source: Set("comicvine".into()),
        provider_series_id: Set("2000".into()),
        provider_series_url: Set(None),
        provider_series_name: Set(Some("Split".into())),
        range_low: Set(Some("2".into())),
        range_high: Set(Some("2".into())),
        declared_year: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let segments = |body: &serde_json::Value| -> Vec<(String, String, u64, String)> {
        body["providers"][0]["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["low"].as_str().unwrap().to_owned(),
                    s["high"].as_str().unwrap().to_owned(),
                    s["issue_count"].as_u64().unwrap(),
                    s["provider_series_id"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let path = "/api/series/gap-run/provider-coverage";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        segments(&body),
        vec![("1".into(), "3".into(), 2, "1000".into())],
        "{body}"
    );
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        assert_eq!(
            segments(&body),
            vec![
                ("1".into(), "1".into(), 1, "1000".into()),
                ("2".into(), "2".into(), 1, "2000".into()),
                ("3".into(), "3".into(), 1, "1000".into()),
            ],
            "{body}"
        );
    }
}

#[tokio::test]
async fn coverage_hints_local_view_leaves_out_the_hidden_issue() {
    use server::library::access::VisibleLibraries;
    use server::metadata::coverage::{load_local, load_local_for};
    let (w, gap) = gap_world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let numbers = |l: Vec<server::metadata::coverage::LocalCovIssue>| -> Vec<String> {
        l.into_iter().map(|i| i.canonical).collect()
    };
    let mut capped = VisibleLibraries::default();
    capped.allowed.insert(w.fx.lib_id);
    capped.caps.insert(w.fx.lib_id, "Teen".into());
    let got = numbers(load_local_for(&db, gap.series_id, &capped).await.unwrap());
    assert_eq!(got, vec!["1", "3"]);
    let mut uncapped = VisibleLibraries::default();
    uncapped.allowed.insert(w.fx.lib_id);
    for acl in [uncapped, VisibleLibraries::unrestricted()] {
        let got = numbers(load_local_for(&db, gap.series_id, &acl).await.unwrap());
        assert_eq!(got, vec!["1", "2", "3"]);
    }
    // The coverage job keeps the full view.
    let got = numbers(load_local(&db, gap.series_id).await.unwrap());
    assert_eq!(got, vec!["1", "2", "3"]);
}

#[tokio::test]
async fn opds_series_entries_never_name_the_hidden_cover_issue() {
    let (w, gap) = gap_world().await;
    let lead = gap.lead_hidden.clone();
    let lead_series = {
        use sea_orm::{ColumnTrait, QueryFilter};
        let db = Database::connect(&w.app.db_url).await.unwrap();
        entity::series::Entity::find()
            .filter(entity::series::Column::Slug.eq("lead-gap"))
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .id
    };
    for path in [
        "/opds/v1/series".to_owned(),
        "/opds/v2/series".to_owned(),
        format!("/opds/v1/series/{lead_series}"),
        format!("/opds/v2/series/{lead_series}"),
    ] {
        let (st, bytes) = raw(&w.app, Method::GET, &path, &w.capped, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Lead Gap"), "{path}: {text}");
        assert!(
            !text.contains(&lead),
            "{path} names the hidden issue: {text}"
        );
    }
    // The uncapped reader and the admin get the hidden #1 as the cover.
    for user in [&w.uncapped, &w.admin] {
        for path in ["/opds/v1/series", "/opds/v2/series"] {
            let (_, bytes) = raw(&w.app, Method::GET, path, user, None).await;
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains(&lead), "{path}: {text}");
        }
    }
}

#[tokio::test]
async fn series_list_filters_agree_with_the_capped_card() {
    let (w, _gap) = gap_world().await;
    let path = "/api/series?characters=Secret%20Agent";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        !names(&body, "slug").contains(&"gap-run".to_owned()),
        "{body}"
    );
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        assert_eq!(names(&body, "slug"), vec!["gap-run"], "{body}");
    }
}

#[tokio::test]
async fn relationship_card_leaves_out_a_hidden_first_issue() {
    let (w, gap) = gap_world().await;
    let (st, body) = post_json(
        &w.app,
        "/api/series/teen-tales/relationships",
        &w.admin,
        serde_json::json!({"target": "lead-gap", "kind": "sequel_of"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let card = |body: &serde_json::Value| -> serde_json::Value {
        body["relationships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["series"]["slug"] == "lead-gap")
            .map(|r| r["series"].clone())
            .unwrap_or_default()
    };
    let path = "/api/series/teen-tales/relationships";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let c = card(&body);
    assert_eq!(c["issue_count"], 1, "{body}");
    assert_eq!(c["progress_summary"]["total"], 1, "{body}");
    assert!(!body.to_string().contains(&gap.lead_hidden), "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        let c = card(&body);
        assert_eq!(c["issue_count"], 2, "{body}");
        assert!(
            c["cover_url"]
                .as_str()
                .unwrap_or_default()
                .contains(&gap.lead_hidden),
            "{body}"
        );
    }
}

#[tokio::test]
async fn bulk_progress_counts_a_hidden_issue_as_not_found() {
    let (w, gap) = gap_world().await;
    let unknown = unknown_issue_id();
    let (st, body) = post_json(
        &w.app,
        "/api/me/progress/bulk",
        &w.capped,
        serde_json::json!({"issue_ids": [&gap.one, &gap.two, &unknown], "finished": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        (&body["updated"], &body["not_found"], &body["forbidden"]),
        (
            &serde_json::json!(1),
            &serde_json::json!(2),
            &serde_json::json!(0)
        ),
        "{body}"
    );
    assert!(!has_progress_row(&w.app, w.capped.user_id, &gap.two).await);
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = post_json(
            &w.app,
            "/api/me/progress/bulk",
            user,
            serde_json::json!({"issue_ids": [&gap.one, &gap.two, &unknown], "finished": true}),
        )
        .await;
        assert_eq!(body["updated"], 2, "{body}");
        assert_eq!(body["not_found"], 1, "{body}");
        assert!(has_progress_row(&w.app, user.user_id, &gap.two).await);
    }
    // Series-level bulk: a series above the cap reads as unknown too.
    let (_, body) = post_json(
        &w.app,
        "/api/me/progress/series-bulk",
        &w.capped,
        serde_json::json!({"series_ids": [w.fx.mature_series, Uuid::now_v7()], "finished": true}),
    )
    .await;
    assert_eq!(body["not_found_series"], 2, "{body}");
    assert_eq!(body["forbidden_series"], 0, "{body}");
    assert_eq!(body["updated"], 0, "{body}");
}

#[tokio::test]
async fn writes_keyed_on_a_hidden_issue_answer_like_an_unknown_id() {
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
    let (w, gap) = gap_world().await;
    let hidden = gap.two.clone();
    let unknown = unknown_issue_id();
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let started = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let session = |id: &str| {
        serde_json::json!({
            "client_session_id": "s-1",
            "issue_id": id,
            "started_at": started,
            "active_ms": 60_000,
            "distinct_pages_read": 2,
            "page_turns": 2,
            "start_page": 0,
            "end_page": 1,
            "client_meta": {},
        })
    };
    let (st, created) = post_json(
        &w.app,
        "/api/me/collections",
        &w.capped,
        serde_json::json!({"name": "Pile"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let cid = created["id"].as_str().unwrap().to_owned();

    // (method, hidden path, unknown path, body builder)
    type Body = fn(&str) -> Option<serde_json::Value>;
    let cases: Vec<(Method, String, String, Body)> = vec![
        (
            Method::POST,
            "/api/progress".into(),
            "/api/progress".into(),
            |id| Some(serde_json::json!({"issue_id": id, "page": 1, "finished": true})),
        ),
        (
            Method::PUT,
            "/api/series/gap-run/issues/gap-2/rating".into(),
            "/api/series/gap-run/issues/no-such-issue/rating".into(),
            |_| Some(serde_json::json!({"rating": 4.0})),
        ),
        (
            Method::PUT,
            "/api/series/gap-run/issues/gap-2/note".into(),
            "/api/series/gap-run/issues/no-such-issue/note".into(),
            |_| Some(serde_json::json!({"body": "a note"})),
        ),
        (
            Method::POST,
            format!("/api/me/collections/{cid}/members/bulk-add"),
            format!("/api/me/collections/{cid}/members/bulk-add"),
            |id| Some(serde_json::json!({"members": [{"entry_kind": "issue", "ref_id": id}]})),
        ),
        (
            Method::POST,
            "/api/series/gap-run/issues/gap-2/metadata/search".into(),
            "/api/series/gap-run/issues/no-such-issue/metadata/search".into(),
            |_| None,
        ),
        (
            Method::POST,
            "/api/series/gap-run/issues/gap-2/metadata/lookup".into(),
            "/api/series/gap-run/issues/no-such-issue/metadata/lookup".into(),
            |_| Some(serde_json::json!({"source": "comicvine", "external_id": "1"})),
        ),
    ];
    for (method, hidden_path, unknown_path, body) in cases {
        let h = masked(
            &w.app,
            method.clone(),
            &hidden_path,
            &w.capped,
            body(&hidden),
            &hidden,
        )
        .await;
        let u = masked(
            &w.app,
            method.clone(),
            &unknown_path,
            &w.capped,
            body(&unknown),
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{method} {hidden_path}");
        if hidden_path.ends_with("/bulk-add") {
            // A batch endpoint: 200 with the id counted as not found.
            assert!(h.1.contains("\"not_found\":1"), "{hidden_path}: {h:?}");
        } else {
            assert!(h.0.is_client_error(), "{method} {hidden_path}: {h:?}");
        }
    }
    // Reading session start: same answer as an unknown id.
    let h = masked(
        &w.app,
        Method::POST,
        "/api/me/reading-sessions",
        &w.capped,
        Some(session(&hidden)),
        &hidden,
    )
    .await;
    let u = masked(
        &w.app,
        Method::POST,
        "/api/me/reading-sessions",
        &w.capped,
        Some(session(&unknown)),
        &unknown,
    )
    .await;
    assert_eq!(h, u);
    assert_eq!(h.0, StatusCode::NOT_FOUND);

    // Metadata diffs (GET with a run id): same answer as an unknown slug.
    let run = Uuid::now_v7();
    for suffix in [
        format!("metadata/proposed-diff?run_id={run}&ordinal=0"),
        format!("metadata/composite-diff?run_id={run}"),
    ] {
        let h = masked(
            &w.app,
            Method::GET,
            &format!("/api/series/gap-run/issues/gap-2/{suffix}"),
            &w.capped,
            None,
            &hidden,
        )
        .await;
        let u = masked(
            &w.app,
            Method::GET,
            &format!("/api/series/gap-run/issues/no-such-issue/{suffix}"),
            &w.capped,
            None,
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{suffix}");
        assert_eq!(h.0, StatusCode::NOT_FOUND, "{suffix}");
    }

    // Reading-list manual match onto the hidden issue.
    let list_id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    entity::cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(Some(w.capped.user_id)),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0; 32]),
        raw_xml: Set("<ReadingList/>".into()),
        parsed_name: Set("List".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(1)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    entity::cbl_entry::ActiveModel {
        id: Set(entry_id),
        cbl_list_id: Set(list_id),
        position: Set(0),
        series_name: Set("Gap Run".into()),
        issue_number: Set("2".into()),
        volume: Set(None),
        year: Set(None),
        cv_series_id: Set(None),
        cv_issue_id: Set(None),
        metron_series_id: Set(None),
        metron_issue_id: Set(None),
        matched_issue_id: Set(None),
        match_status: Set("missing".into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(None),
        user_resolved_at: Set(None),
        matched_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    let path = format!("/api/me/cbl-lists/{list_id}/entries/{entry_id}/match");
    let h = masked(
        &w.app,
        Method::POST,
        &path,
        &w.capped,
        Some(serde_json::json!({"issue_id": hidden})),
        &hidden,
    )
    .await;
    let u = masked(
        &w.app,
        Method::POST,
        &path,
        &w.capped,
        Some(serde_json::json!({"issue_id": unknown})),
        &unknown,
    )
    .await;
    assert_eq!(h, u);
    assert!(h.0.is_client_error(), "{h:?}");

    // Nothing was written for the hidden issue.
    assert!(!has_progress_row(&w.app, w.capped.user_id, &hidden).await);
    let entry = entity::cbl_entry::Entity::find_by_id(entry_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(entry.matched_issue_id.is_none());
    let n = entity::collection_entry::Entity::find()
        .filter(entity::collection_entry::Column::IssueId.eq(hidden.clone()))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let n = entity::reading_session::Entity::find()
        .filter(entity::reading_session::Column::IssueId.eq(hidden.clone()))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let n = entity::user_rating::Entity::find()
        .filter(entity::user_rating::Column::TargetId.eq(hidden.clone()))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(n, 0);

    // The uncapped reader and the admin can do the same writes.
    for user in [&w.uncapped, &w.admin] {
        let (st, body) = raw(
            &w.app,
            Method::PUT,
            "/api/series/gap-run/issues/gap-2/rating",
            user,
            Some(serde_json::json!({"rating": 4.0})),
        )
        .await;
        assert!(st.is_success(), "{st} {}", String::from_utf8_lossy(&body));
        let (st, body) = raw(
            &w.app,
            Method::POST,
            "/api/progress",
            user,
            Some(serde_json::json!({"issue_id": hidden, "page": 1})),
        )
        .await;
        assert!(st.is_success(), "{st} {}", String::from_utf8_lossy(&body));
    }
}

#[tokio::test]
async fn reading_log_keeps_a_read_of_a_later_removed_issue() {
    let (w, gap) = gap_world().await;
    for user in [&w.capped, &w.uncapped] {
        for id in [&gap.two, &gap.three] {
            progress(&w.app, user.user_id, id, true).await;
        }
    }
    // #3 is soft-removed after it was read: the log is history and keeps it.
    let db = Database::connect(&w.app.db_url).await.unwrap();
    entity::issue::ActiveModel {
        id: Unchanged(gap.three.clone()),
        removed_at: Set(Some(Utc::now().fixed_offset())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    let issue_ids = |body: &serde_json::Value| -> Vec<String> {
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["issue"]["id"].as_str().map(str::to_owned))
            .collect()
    };
    let (_, body) = get_json(&w.app, "/api/me/reading-log", &w.capped).await;
    let got = issue_ids(&body);
    assert!(got.contains(&gap.three), "{body}");
    assert!(!got.contains(&gap.two), "{body}");
    let (_, body) = get_json(&w.app, "/api/me/reading-log", &w.uncapped).await;
    let got = issue_ids(&body);
    assert!(got.contains(&gap.three) && got.contains(&gap.two), "{body}");
    let (st, bytes) = raw(
        &w.app,
        Method::GET,
        "/api/me/reading-log/export",
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let csv = String::from_utf8(bytes).unwrap();
    assert!(
        csv.contains("Gap Three") && !csv.contains("Gap Two"),
        "{csv}"
    );
}

// ───── capped-only filter SQL paths ─────

/// Create a `filter_series` saved view with one condition as `user` and
/// return the slugs of its results.
async fn saved_view_slugs(app: &TestApp, user: &Authed, field: &str, value: &str) -> Vec<String> {
    let (st, view) = post_json(
        app,
        "/api/me/saved-views",
        user,
        serde_json::json!({
            "kind": "filter_series",
            "name": format!("{field} {value}"),
            "filter": {
                "match_mode": "all",
                "conditions": [
                    { "group_id": 0, "field": field, "op": "is", "value": value }
                ]
            },
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 50,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{view}");
    let id = view["id"].as_str().unwrap();
    let (st, body) = get_json(app, &format!("/api/me/saved-views/{id}/results"), user).await;
    assert_eq!(st, StatusCode::OK, "{field}: {body}");
    names(&body, "slug")
}

#[tokio::test]
async fn completeness_filters_run_under_the_cap() {
    let (w, gap) = gap_world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    // #1 and #3 are accepted as metadata-complete, the hidden #2 is not: the
    // capped reader's Gap Run is `complete`, everyone else's `partial`.
    for id in [&gap.one, &gap.three] {
        entity::issue::ActiveModel {
            id: Unchanged(id.clone()),
            metadata_review_accepted_at: Set(Some(Utc::now().fixed_offset())),
            ..Default::default()
        }
        .update(&db)
        .await
        .unwrap();
    }
    // Three issues expected: complete for those who see three, not for the
    // capped reader who sees two.
    entity::series::ActiveModel {
        id: Unchanged(gap.series_id),
        total_issues: Set(Some(3)),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    let gap_run = "gap-run".to_owned();

    // (a) series list `?metadata_completeness=`.
    let path = "/api/series?metadata_completeness=complete&limit=50";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(names(&body, "slug").contains(&gap_run), "{body}");
    let (st, body) = get_json(&w.app, path, &w.uncapped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!names(&body, "slug").contains(&gap_run), "{body}");

    // (b) saved view on `metadata_completeness`.
    let got = saved_view_slugs(&w.app, &w.capped, "metadata_completeness", "complete").await;
    assert!(got.contains(&gap_run), "{got:?}");
    let got = saved_view_slugs(&w.app, &w.uncapped, "metadata_completeness", "complete").await;
    assert!(!got.contains(&gap_run), "{got:?}");

    // (c) saved view on `collection_completeness` (the active-issue count).
    let got = saved_view_slugs(&w.app, &w.capped, "collection_completeness", "complete").await;
    assert!(!got.contains(&gap_run), "{got:?}");
    let got = saved_view_slugs(&w.app, &w.uncapped, "collection_completeness", "complete").await;
    assert!(got.contains(&gap_run), "{got:?}");
}
