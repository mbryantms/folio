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
        declared_age_rating: Set(None),
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
        hide_unrated: Set(false),
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
        ..Default::default()
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

// ───── entity surfaces under the issue-level cap ─────
//
// `entity_world` tags the Gap fixture and runs the scanner's junction and
// rollup passes, so the `series_*` rollups carry what the hidden #2 adds:
//
// - hidden-only: writer "Hidden Hand", character "Secret Agent", team
//   "Hidden Squad", location "Hidden Lair", genre "Hidden Genre", tag
//   "Hidden Tag", arc "Hidden Arc";
// - shared: character "Shared Sidekick" (#1 and #2) and creator "Cap
//   Writer" (writer everywhere, colorist only on #2).
//
// "Twin Run" (`Teen`) repeats the shape: #1 visible with the Sidekick, #2
// above the cap with the hidden-only names, so the similar-series rail
// pairs it with Gap Run on both visible and hidden entities.

#[allow(clippy::too_many_arguments)]
async fn tag_issue(
    db: &sea_orm::DatabaseConnection,
    id: &str,
    writer: &str,
    colorist: Option<&str>,
    characters: Option<&str>,
    teams: Option<&str>,
    locations: Option<&str>,
    genre: Option<&str>,
    tags: Option<&str>,
    story_arc: Option<&str>,
) {
    entity::issue::ActiveModel {
        id: Unchanged(id.to_owned()),
        writer: Set(Some(writer.into())),
        colorist: Set(colorist.map(str::to_owned)),
        characters: Set(characters.map(str::to_owned)),
        teams: Set(teams.map(str::to_owned)),
        locations: Set(locations.map(str::to_owned)),
        genre: Set(genre.map(str::to_owned)),
        tags: Set(tags.map(str::to_owned)),
        story_arc: Set(story_arc.map(str::to_owned)),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
    server::library::scanner::metadata_rollup::replace_issue_metadata_from_row(db, id)
        .await
        .unwrap();
}

struct EntityWorld {
    w: World,
    gap: Gap,
    twin_one: String,
}

async fn entity_world() -> EntityWorld {
    let (w, gap) = gap_world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let hidden = (
        Some("Secret Agent, Shared Sidekick"),
        Some("Hidden Squad"),
        Some("Hidden Lair"),
        Some("Hidden Genre"),
        Some("Hidden Tag"),
        Some("Hidden Arc"),
    );
    tag_issue(
        &db,
        &gap.one,
        "Cap Writer",
        None,
        Some("Shared Sidekick"),
        None,
        None,
        Some("Shared Genre"),
        None,
        None,
    )
    .await;
    tag_issue(
        &db,
        &gap.two,
        "Cap Writer, Hidden Hand",
        Some("Cap Writer"),
        hidden.0,
        hidden.1,
        hidden.2,
        hidden.3,
        hidden.4,
        hidden.5,
    )
    .await;
    tag_issue(
        &db,
        &gap.three,
        "Cap Writer",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    let twin = seed_series(&db, w.fx.lib_id, "Twin Run", "twin-run", Some("Teen")).await;
    let twin_one = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        twin,
        1,
        "twin-1",
        "Twin One",
        None,
        None,
    )
    .await;
    let twin_two = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        twin,
        2,
        "twin-2",
        "Twin Two",
        Some("Mature 17+"),
        None,
    )
    .await;
    tag_issue(
        &db,
        &twin_one,
        "Cap Writer",
        None,
        Some("Shared Sidekick"),
        None,
        None,
        Some("Shared Genre"),
        None,
        None,
    )
    .await;
    tag_issue(
        &db,
        &twin_two,
        "Cap Writer, Hidden Hand",
        None,
        hidden.0,
        hidden.1,
        hidden.2,
        hidden.3,
        hidden.4,
        hidden.5,
    )
    .await;
    for series_id in [gap.series_id, twin] {
        // The seeded rating is a person's, so the rollup keeps it.
        pin_series_rating(&db, series_id, "user").await;
        server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series_id)
            .await
            .unwrap();
    }
    EntityWorld { w, gap, twin_one }
}

/// `items[*][key]` of a list page.
fn item_strings(body: &serde_json::Value, key: &str) -> Vec<String> {
    names(body, key)
}

/// The slug the admin's browse index gives the entity named `name`.
async fn admin_slug(w: &World, list: &str, name_key: &str, name: &str) -> String {
    let (st, body) = get_json(&w.app, &format!("{list}?limit=100"), &w.admin).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i[name_key] == name)
        .and_then(|i| i["slug"].as_str())
        .unwrap_or_else(|| panic!("{name} missing from {list}: {body}"))
        .to_owned()
}

#[tokio::test]
async fn entity_lists_and_search_leave_out_a_hidden_only_entity() {
    let e = entity_world().await;
    let w = &e.w;
    let cases: [(&str, &str, &str); 7] = [
        ("/api/characters", "name", "Secret Agent"),
        ("/api/characters?q=secret", "name", "Secret Agent"),
        ("/api/teams", "name", "Hidden Squad"),
        ("/api/teams?q=hidden", "name", "Hidden Squad"),
        ("/api/locations", "name", "Hidden Lair"),
        ("/api/creators", "person", "Hidden Hand"),
        ("/api/people?q=hidden", "person", "Hidden Hand"),
    ];
    for (path, key, hidden) in cases {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        assert!(
            !item_strings(&body, key).contains(&hidden.to_owned()),
            "{path} names {hidden} for the capped reader: {body}"
        );
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            assert!(
                item_strings(&body, key).contains(&hidden.to_owned()),
                "{path} lost {hidden}: {body}"
            );
        }
    }
    // The shared character stays listed, counted over visible issues only.
    let count = |body: &serde_json::Value| {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "Shared Sidekick")
            .map(|i| (i["series_count"].as_i64(), i["issue_count"].as_i64()))
    };
    let (_, body) = get_json(&w.app, "/api/characters", &w.capped).await;
    assert_eq!(count(&body), Some((Some(2), Some(2))), "{body}");
    let (_, body) = get_json(&w.app, "/api/characters", &w.uncapped).await;
    assert_eq!(count(&body), Some((Some(2), Some(4))), "{body}");
    // The `total` of the capped first page agrees with its rows.
    let (_, body) = get_json(&w.app, "/api/teams", &w.capped).await;
    assert_eq!(body["total"], 0, "{body}");
}

#[tokio::test]
async fn hidden_only_entity_pages_answer_like_an_unknown_slug() {
    let e = entity_world().await;
    let w = &e.w;
    let cases = [
        ("/api/characters", "name", "Secret Agent", "/insights"),
        ("/api/teams", "name", "Hidden Squad", "/series"),
        ("/api/locations", "name", "Hidden Lair", "/issues"),
        ("/api/creators", "person", "Hidden Hand", ""),
    ];
    for (list, key, name, sub) in cases {
        let slug = admin_slug(w, list, key, name).await;
        let (unknown_st, unknown) = get_json(
            &w.app,
            &format!("{list}/no-such-entity-anywhere{sub}"),
            &w.capped,
        )
        .await;
        assert_eq!(unknown_st, StatusCode::NOT_FOUND, "{unknown}");
        for path in [format!("{list}/{slug}"), format!("{list}/{slug}{sub}")] {
            let (st, body) = get_json(&w.app, &path, &w.capped).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}: {body}");
            assert_eq!(body, unknown, "{path}");
            for user in [&w.uncapped, &w.admin] {
                let (st, body) = get_json(&w.app, &path, user).await;
                assert_eq!(st, StatusCode::OK, "{path}: {body}");
            }
        }
    }
}

#[tokio::test]
async fn shared_creator_lists_only_roles_on_visible_issues() {
    let e = entity_world().await;
    let w = &e.w;
    let slug = admin_slug(w, "/api/creators", "person", "Cap Writer").await;
    let roles = |body: &serde_json::Value| -> Vec<String> {
        body["roles"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect()
    };
    let path = format!("/api/creators/{slug}");
    let (st, body) = get_json(&w.app, &path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(roles(&body), vec!["writer"], "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, &path, user).await;
        assert_eq!(roles(&body), vec!["writer", "colorist"], "{body}");
    }
    // Same on the browse index and in people search.
    for (list, key) in [
        ("/api/creators", "person"),
        ("/api/people?q=cap%20writer", "person"),
    ] {
        let (_, body) = get_json(&w.app, list, &w.capped).await;
        let item = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i[key] == "Cap Writer")
            .cloned()
            .unwrap_or_else(|| panic!("{list}: {body}"));
        assert_eq!(
            item["roles"],
            serde_json::json!(["writer"]),
            "{list}: {item}"
        );
    }
}

#[tokio::test]
async fn shared_character_page_counts_and_grids_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    let slug = admin_slug(w, "/api/characters", "name", "Shared Sidekick").await;
    let base = format!("/api/characters/{slug}");
    let (st, body) = get_json(&w.app, &base, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        (body["series_count"].as_i64(), body["issue_count"].as_i64()),
        (Some(2), Some(2))
    );
    let (_, body) = get_json(&w.app, &base, &w.uncapped).await;
    assert_eq!(body["issue_count"], 4, "{body}");

    let (_, body) = get_json(&w.app, &format!("{base}/issues"), &w.capped).await;
    let ids: Vec<String> = item_strings(&body, "id");
    assert_eq!(ids.len(), 2, "{body}");
    assert!(!ids.contains(&e.gap.two), "{body}");
    assert!(ids.contains(&e.twin_one), "{body}");

    // Insights name nothing that only the hidden issues carry.
    let (st, body) = get_json(&w.app, &format!("{base}/insights"), &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let text = body.to_string();
    for hidden in [
        "Secret Agent",
        "Hidden Hand",
        "Hidden Squad",
        "Hidden Lair",
        "Hidden Arc",
    ] {
        assert!(!text.contains(hidden), "insights name {hidden}: {body}");
    }
    assert_eq!(body["issues"], 2, "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/insights"), &w.uncapped).await;
    assert!(body.to_string().contains("Secret Agent"), "{body}");
}

#[tokio::test]
async fn filter_option_pickers_leave_out_hidden_only_values() {
    let e = entity_world().await;
    let w = &e.w;
    let cases = [
        ("/api/filter-options/genres", "Hidden Genre"),
        ("/api/filter-options/tags", "Hidden Tag"),
        ("/api/filter-options/credits", "Hidden Hand"),
        ("/api/filter-options/credits/writer", "Hidden Hand"),
    ];
    let values = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    for (path, hidden) in cases {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        assert!(
            !values(&body).contains(&hidden.to_owned()),
            "{path}: {body}"
        );
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            assert!(values(&body).contains(&hidden.to_owned()), "{path}: {body}");
        }
    }
    // A value a visible issue carries stays offered.
    let (_, body) = get_json(&w.app, "/api/filter-options/genres", &w.capped).await;
    assert!(values(&body).contains(&"Shared Genre".to_owned()), "{body}");
}

#[tokio::test]
async fn series_list_rollup_filters_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    for path in [
        "/api/series?writers=Hidden%20Hand",
        "/api/series?credits=Hidden%20Hand",
        "/api/series?genres=Hidden%20Genre",
        "/api/series?tags=Hidden%20Tag",
    ] {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        assert!(item_strings(&body, "slug").is_empty(), "{path}: {body}");
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            let mut got = item_strings(&body, "slug");
            got.sort();
            assert_eq!(got, vec!["gap-run", "twin-run"], "{path}: {body}");
        }
    }
    let (_, body) = get_json(&w.app, "/api/series?genres=Shared%20Genre", &w.capped).await;
    let mut got = item_strings(&body, "slug");
    got.sort();
    assert_eq!(got, vec!["gap-run", "twin-run"], "{body}");
}

#[tokio::test]
async fn series_appearances_leave_out_an_arc_only_on_the_hidden_issue() {
    let e = entity_world().await;
    let w = &e.w;
    let path = "/api/series/gap-run/appearances";
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["arcs"], serde_json::json!([]), "{body}");
    for user in [&w.uncapped, &w.admin] {
        let (_, body) = get_json(&w.app, path, user).await;
        assert_eq!(body["arcs"][0]["name"], "Hidden Arc", "{body}");
    }
}

#[tokio::test]
async fn similar_series_reasons_never_name_a_hidden_only_entity() {
    let e = entity_world().await;
    let w = &e.w;
    let reasons = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["series"]["slug"] == "twin-run")
            .map(|i| {
                i["because"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|r| r["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let path = "/api/series/gap-run/similar";
    let (_, full) = get_json(&w.app, path, &w.uncapped).await;
    let full = reasons(&full);
    assert!(full.contains(&"Secret Agent".to_owned()), "{full:?}");
    assert!(full.contains(&"Shared Sidekick".to_owned()), "{full:?}");
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let capped = reasons(&body);
    assert!(capped.contains(&"Shared Sidekick".to_owned()), "{body}");
    for hidden in [
        "Secret Agent",
        "Hidden Hand",
        "Hidden Squad",
        "Hidden Genre",
        "Hidden Tag",
    ] {
        assert!(!capped.contains(&hidden.to_owned()), "{hidden}: {body}");
    }
    let (_, body) = get_json(&w.app, path, &w.admin).await;
    assert_eq!(reasons(&body), full);
}

#[tokio::test]
async fn opds_author_facets_and_by_creator_feeds_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    for path in ["/opds/v1/series", "/opds/v2/series"] {
        let (st, bytes) = raw(&w.app, Method::GET, path, &w.capped, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Gap Run"), "{path}: {text}");
        assert!(text.contains("Cap Writer"), "{path}: {text}");
        assert!(!text.contains("Hidden Hand"), "{path}: {text}");
        assert!(!text.contains("Hidden Genre"), "{path}: {text}");
        let (_, bytes) = raw(&w.app, Method::GET, path, &w.uncapped, None).await;
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Hidden Hand"), "{path}: {text}");
    }
    for path in [
        "/opds/v1/by-creator/Hidden%20Hand",
        "/opds/v2/by-creator/Hidden%20Hand",
    ] {
        let (st, bytes) = raw(&w.app, Method::GET, path, &w.capped, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("Gap Run"), "{path}: {text}");
        assert!(!text.contains("Twin Run"), "{path}: {text}");
        let (_, bytes) = raw(&w.app, Method::GET, path, &w.uncapped, None).await;
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Gap Run"), "{path}: {text}");
    }
}

#[tokio::test]
async fn reading_stats_top_creators_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    for user in [&w.capped, &w.uncapped] {
        entity::reading_session::ActiveModel {
            id: Set(Uuid::now_v7()),
            user_id: Set(user.user_id),
            issue_id: Set(e.gap.one.clone()),
            series_id: Set(e.gap.series_id),
            client_session_id: Set(Uuid::new_v4().to_string()),
            started_at: Set(now - chrono::Duration::minutes(10)),
            ended_at: Set(Some(now)),
            last_heartbeat_at: Set(now),
            active_ms: Set(600_000),
            distinct_pages_read: Set(2),
            page_turns: Set(2),
            start_page: Set(0),
            end_page: Set(1),
            furthest_page: Set(1),
            device: Set(None),
            view_mode: Set(None),
            client_meta: Set(serde_json::json!({})),
            hidden_from_log: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    let people = |body: &serde_json::Value| -> Vec<String> {
        body["top_creators"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["person"].as_str().map(str::to_owned))
            .collect()
    };
    let (st, body) = get_json(&w.app, "/api/me/reading-stats", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let got = people(&body);
    assert!(got.contains(&"Cap Writer".to_owned()), "{body}");
    assert!(!got.contains(&"Hidden Hand".to_owned()), "{body}");
    let (_, body) = get_json(&w.app, "/api/me/reading-stats", &w.uncapped).await;
    assert!(people(&body).contains(&"Hidden Hand".to_owned()), "{body}");
}

async fn saved_view_multi(
    app: &TestApp,
    user: &Authed,
    field: &str,
    op: &str,
    value: serde_json::Value,
) -> Vec<String> {
    let (st, view) = post_json(
        app,
        "/api/me/saved-views",
        user,
        serde_json::json!({
            "kind": "filter_series",
            "name": format!("{field} {op} {value}"),
            "filter": {
                "match_mode": "all",
                "conditions": [
                    { "group_id": 0, "field": field, "op": op, "value": value }
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
    let mut got = names(&body, "slug");
    got.sort();
    got
}

#[tokio::test]
async fn saved_view_junction_filters_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    let both = vec!["gap-run".to_owned(), "twin-run".to_owned()];
    for (field, value) in [
        ("writer", "Hidden Hand"),
        ("characters", "Secret Agent"),
        ("teams", "Hidden Squad"),
        ("locations", "Hidden Lair"),
        ("genres", "Hidden Genre"),
        ("tags", "Hidden Tag"),
    ] {
        let v = serde_json::json!([value]);
        let got = saved_view_multi(&w.app, &w.capped, field, "includes_any", v.clone()).await;
        assert!(got.is_empty(), "{field}: {got:?}");
        let got = saved_view_multi(&w.app, &w.capped, field, "includes_all", v.clone()).await;
        assert!(got.is_empty(), "{field}: {got:?}");
        for user in [&w.uncapped, &w.admin] {
            let got = saved_view_multi(&w.app, user, field, "includes_any", v.clone()).await;
            assert_eq!(got, both, "{field}");
        }
    }
    // `is_empty` on teams: for the capped reader neither series has a
    // visible team.
    let got = saved_view_multi(
        &w.app,
        &w.capped,
        "teams",
        "is_empty",
        serde_json::Value::Null,
    )
    .await;
    assert!(got.contains(&"gap-run".to_owned()), "{got:?}");
    let got = saved_view_multi(
        &w.app,
        &w.uncapped,
        "teams",
        "is_empty",
        serde_json::Value::Null,
    )
    .await;
    assert!(!got.contains(&"gap-run".to_owned()), "{got:?}");
    // A value on a visible issue still selects for the capped reader.
    let got = saved_view_multi(
        &w.app,
        &w.capped,
        "characters",
        "includes_any",
        serde_json::json!(["Shared Sidekick"]),
    )
    .await;
    assert_eq!(got, both);
}

// ───── entity surfaces never name a series the caller cannot open ─────
//
// "Dark Saga" is rated `Mature 17+` (hidden from the `Teen` reader) but its
// #1 carries its own `Teen` rating. Its "Bob", "Dark Writer", "Dark Genre"
// and "Dark Tag" occur nowhere else; it also carries "Shared Sidekick".

async fn dark_world() -> EntityWorld {
    let e = entity_world().await;
    let db = Database::connect(&e.w.app.db_url).await.unwrap();
    let root = e.w.app._data_dir.path().join("cap-lib");
    let dark = seed_series(
        &db,
        e.w.fx.lib_id,
        "Dark Saga",
        "dark-saga",
        Some("Mature 17+"),
    )
    .await;
    let dark_one = seed_issue(
        &db,
        &root,
        e.w.fx.lib_id,
        dark,
        1,
        "dark-1",
        "Dark One",
        Some("Teen"),
        None,
    )
    .await;
    tag_issue(
        &db,
        &dark_one,
        "Cap Writer, Dark Writer",
        None,
        Some("Bob, Shared Sidekick"),
        None,
        None,
        Some("Dark Genre"),
        Some("Dark Tag"),
        None,
    )
    .await;
    pin_series_rating(&db, dark, "user").await;
    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, dark)
        .await
        .unwrap();
    e
}

#[tokio::test]
async fn an_issue_of_a_hidden_series_names_nothing_on_entity_surfaces() {
    let e = dark_world().await;
    let w = &e.w;
    let lists: [(&str, &str, &str); 8] = [
        ("/api/characters", "name", "Bob"),
        ("/api/characters?q=bob", "name", "Bob"),
        ("/api/creators", "person", "Dark Writer"),
        ("/api/people?q=dark", "person", "Dark Writer"),
        ("/api/filter-options/credits", "", "Dark Writer"),
        ("/api/filter-options/genres", "", "Dark Genre"),
        ("/api/filter-options/tags", "", "Dark Tag"),
        ("/api/filter-options/characters", "", "Bob"),
    ];
    let values = |body: &serde_json::Value, key: &str| -> Vec<String> {
        if key.is_empty() {
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        } else {
            item_strings(body, key)
        }
    };
    for (path, key, name) in lists {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        assert!(
            !values(&body, key).contains(&name.to_owned()),
            "{path}: {body}"
        );
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            assert!(
                values(&body, key).contains(&name.to_owned()),
                "{path}: {body}"
            );
        }
    }

    // Detail routes answer like an unknown slug.
    let bob = admin_slug(w, "/api/characters", "name", "Bob").await;
    let writer = admin_slug(w, "/api/creators", "person", "Dark Writer").await;
    for (base, slug, subs) in [
        (
            "/api/characters",
            &bob,
            vec!["", "/insights", "/series", "/issues"],
        ),
        ("/api/creators", &writer, vec![""]),
    ] {
        for sub in subs {
            let (_, unknown) =
                get_json(&w.app, &format!("{base}/no-such-entity{sub}"), &w.capped).await;
            let path = format!("{base}/{slug}{sub}");
            let (st, body) = get_json(&w.app, &path, &w.capped).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}: {body}");
            assert_eq!(body, unknown, "{path}");
            for user in [&w.uncapped, &w.admin] {
                let (st, _) = get_json(&w.app, &path, user).await;
                assert_eq!(st, StatusCode::OK, "{path}");
            }
        }
    }
    for path in [
        format!("/opds/v1/characters/{bob}"),
        format!("/opds/v2/characters/{bob}"),
    ] {
        let (st, _) = raw(&w.app, Method::GET, &path, &w.capped, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        let (st, _) = raw(&w.app, Method::GET, &path, &w.uncapped, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
    }

    // The shared character: grids, counts and insights leave Dark Saga out.
    let sidekick = admin_slug(w, "/api/characters", "name", "Shared Sidekick").await;
    let base = format!("/api/characters/{sidekick}");
    let (_, body) = get_json(&w.app, &base, &w.capped).await;
    assert_eq!(body["series_count"], 2, "{body}");
    let (_, body) = get_json(&w.app, &base, &w.uncapped).await;
    assert_eq!(body["series_count"], 3, "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/series"), &w.capped).await;
    let mut got = item_strings(&body, "slug");
    got.sort();
    assert_eq!(got, vec!["gap-run", "twin-run"], "{body}");
    assert_eq!(body["total"], 2, "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/issues"), &w.capped).await;
    assert!(!body.to_string().contains("Dark"), "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/insights"), &w.capped).await;
    assert!(!body.to_string().contains("Dark Saga"), "{body}");
    assert!(!body.to_string().contains("Dark Writer"), "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/insights"), &w.uncapped).await;
    assert!(body.to_string().contains("Dark Saga"), "{body}");

    // Saved-view junction filter (guard: the series root is hidden anyway).
    let got = saved_view_multi(
        &w.app,
        &w.capped,
        "writer",
        "includes_any",
        serde_json::json!(["Dark Writer"]),
    )
    .await;
    assert!(got.is_empty(), "{got:?}");
    let got = saved_view_multi(
        &w.app,
        &w.uncapped,
        "writer",
        "includes_any",
        serde_json::json!(["Dark Writer"]),
    )
    .await;
    assert_eq!(got, vec!["dark-saga"]);
}

#[tokio::test]
async fn series_appearances_leave_out_an_arc_of_a_removed_issue() {
    let e = entity_world().await;
    let w = &e.w;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    entity::issue::ActiveModel {
        id: Unchanged(e.gap.two.clone()),
        removed_at: Set(Some(Utc::now().fixed_offset())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    for user in [&w.uncapped, &w.admin] {
        let (st, body) = get_json(&w.app, "/api/series/gap-run/appearances", user).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["arcs"], serde_json::json!([]), "{body}");
    }
}

#[tokio::test]
async fn similar_series_drops_a_neighbour_known_only_through_a_hidden_issue() {
    let e = entity_world().await;
    let w = &e.w;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let shadow = seed_series(&db, w.fx.lib_id, "Shadow Run", "shadow-run", Some("Teen")).await;
    let one = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        shadow,
        1,
        "shadow-1",
        "Shadow One",
        None,
        None,
    )
    .await;
    let two = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        shadow,
        2,
        "shadow-2",
        "Shadow Two",
        Some("Mature 17+"),
        None,
    )
    .await;
    tag_issue(
        &db,
        &one,
        "Shadow Writer",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    tag_issue(
        &db,
        &two,
        "Shadow Writer",
        None,
        Some("Secret Agent"),
        Some("Hidden Squad"),
        None,
        None,
        None,
        None,
    )
    .await;
    pin_series_rating(&db, shadow, "user").await;
    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, shadow)
        .await
        .unwrap();
    let slugs = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["series"]["slug"].as_str().map(str::to_owned))
            .collect()
    };
    let path = "/api/series/gap-run/similar";
    let (_, body) = get_json(&w.app, path, &w.uncapped).await;
    assert!(slugs(&body).contains(&"shadow-run".to_owned()), "{body}");
    let (st, body) = get_json(&w.app, path, &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!slugs(&body).contains(&"shadow-run".to_owned()), "{body}");
    assert_eq!(
        body["total"].as_u64(),
        Some(slugs(&body).len() as u64),
        "{body}"
    );
    // Every listed neighbour has a reason, and its score is their sum.
    for item in body["items"].as_array().unwrap() {
        let because = item["because"].as_array().unwrap();
        assert!(!because.is_empty(), "{item}");
        let sum: f64 = because.iter().map(|r| r["weight"].as_f64().unwrap()).sum();
        assert!(
            (item["score"].as_f64().unwrap() - sum).abs() < 0.01,
            "{item}"
        );
    }
}

#[tokio::test]
async fn opds_entity_feeds_404_for_a_hidden_only_entity() {
    let e = entity_world().await;
    let w = &e.w;
    let character = admin_slug(w, "/api/characters", "name", "Secret Agent").await;
    let team = admin_slug(w, "/api/teams", "name", "Hidden Squad").await;
    let arc = admin_slug(w, "/api/arcs", "name", "Hidden Arc").await;
    for v in ["v1", "v2"] {
        for path in [
            format!("/opds/{v}/characters/{character}"),
            format!("/opds/{v}/teams/{team}"),
            format!("/opds/{v}/arcs/{arc}"),
        ] {
            let (st, _) = raw(&w.app, Method::GET, &path, &w.capped, None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
            for user in [&w.uncapped, &w.admin] {
                let (st, _) = raw(&w.app, Method::GET, &path, user, None).await;
                assert_eq!(st, StatusCode::OK, "{path}");
            }
        }
    }
}

#[tokio::test]
async fn credit_counts_cover_visible_issues_only() {
    let e = entity_world().await;
    let w = &e.w;
    // Cap Writer is credited on all 16 issues; `series_credits` rows exist
    // for the two rolled-up series (Gap Run, Twin Run). The capped reader
    // sees 9 of the issues and both rollup rows (each backed by a visible
    // issue).
    for path in ["/api/creators", "/api/people?q=cap%20writer"] {
        let count = |body: &serde_json::Value| {
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["person"] == "Cap Writer")
                .and_then(|i| i["credit_count"].as_i64())
        };
        let (_, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(count(&body), Some(11), "{path}: {body}");
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            assert_eq!(count(&body), Some(18), "{path}: {body}");
        }
    }
}

#[tokio::test]
async fn saved_view_excludes_and_is_not_empty_follow_the_cap() {
    let e = entity_world().await;
    let w = &e.w;
    let has = |got: &[String], slug: &str| got.iter().any(|s| s == slug);
    let v = serde_json::json!(["Secret Agent"]);
    let got = saved_view_multi(&w.app, &w.capped, "characters", "excludes", v.clone()).await;
    assert!(has(&got, "gap-run") && has(&got, "twin-run"), "{got:?}");
    let got = saved_view_multi(&w.app, &w.uncapped, "characters", "excludes", v).await;
    assert!(!has(&got, "gap-run") && !has(&got, "twin-run"), "{got:?}");
    let null = serde_json::Value::Null;
    let got = saved_view_multi(&w.app, &w.capped, "teams", "is_not_empty", null.clone()).await;
    assert!(got.is_empty(), "{got:?}");
    let got = saved_view_multi(&w.app, &w.uncapped, "teams", "is_not_empty", null).await;
    assert_eq!(got, vec!["gap-run", "twin-run"]);
}

/// A bare library row (no series), for the mixed-grant test.
async fn insert_library(db: &sea_orm::DatabaseConnection, root: &std::path::Path) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(id),
        name: Set("Second Lib".into()),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("en".into()),
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
    .insert(db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_cap_in_one_library_leaves_an_uncapped_library_whole() {
    let e = entity_world().await;
    let w = &e.w;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("lib-b");
    std::fs::create_dir_all(&root).unwrap();
    let lib_b = insert_library(&db, &root).await;
    let bee = seed_series(&db, lib_b, "Bee Run", "bee-run", None).await;
    let bee_one = seed_issue(
        &db,
        &root,
        lib_b,
        bee,
        1,
        "bee-1",
        "Bee One",
        Some("Mature 17+"),
        None,
    )
    .await;
    tag_issue(
        &db,
        &bee_one,
        "Bee Writer",
        None,
        Some("Bee Villain"),
        None,
        None,
        Some("Bee Genre"),
        None,
        None,
    )
    .await;
    server::library::scanner::metadata_rollup::rollup_series_metadata(&db, bee)
        .await
        .unwrap();
    // Library A keeps its Teen cap; library B is granted without one.
    grant(&w.app, w.capped.user_id, lib_b, None).await;

    let (_, body) = get_json(&w.app, "/api/characters", &w.capped).await;
    let names_seen = item_strings(&body, "name");
    assert!(names_seen.contains(&"Bee Villain".to_owned()), "{body}");
    assert!(!names_seen.contains(&"Secret Agent".to_owned()), "{body}");
    let bee_counts = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "Bee Villain")
        .map(|i| (i["series_count"].as_i64(), i["issue_count"].as_i64()));
    assert_eq!(bee_counts, Some((Some(1), Some(1))), "{body}");

    let writer = admin_slug(w, "/api/creators", "person", "Bee Writer").await;
    let (st, body) = get_json(&w.app, &format!("/api/creators/{writer}"), &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["roles"], serde_json::json!(["writer"]), "{body}");
    let hidden = admin_slug(w, "/api/creators", "person", "Hidden Hand").await;
    let (st, _) = get_json(&w.app, &format!("/api/creators/{hidden}"), &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (_, body) = get_json(&w.app, "/api/filter-options/credits", &w.capped).await;
    let values: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    assert!(values.contains(&"Bee Writer".to_owned()), "{body}");
    assert!(!values.contains(&"Hidden Hand".to_owned()), "{body}");
    let (_, body) = get_json(&w.app, "/api/filter-options/genres", &w.capped).await;
    assert!(body.to_string().contains("Bee Genre"), "{body}");
    assert!(!body.to_string().contains("Hidden Genre"), "{body}");
}

// ───── a series is rated by its highest-rated issue ─────
//
// `series.age_rating` is derived from the series' active issues (the
// highest rung on the ladder) unless a person or a provider set it. The
// fixtures above seed ratings directly; a series a test rolls up keeps its
// seeded rating only when it is pinned.

/// Record that `set_by` (`user`, `comicvine`, …) set the series' rating,
/// so the derivation leaves it alone.
async fn pin_series_rating(db: &sea_orm::DatabaseConnection, series_id: Uuid, set_by: &str) {
    entity::field_provenance::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series_id.to_string()),
        field: Set("age_rating".into()),
        set_by: Set(set_by.into()),
        set_at: Set(Utc::now().fixed_offset()),
        source_external_id: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn rollup(db: &sea_orm::DatabaseConnection, series_id: Uuid) {
    server::library::scanner::metadata_rollup::rollup_series_metadata(db, series_id)
        .await
        .unwrap();
}

async fn series_rating(db: &sea_orm::DatabaseConnection, series_id: Uuid) -> Option<String> {
    entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .age_rating
}

async fn reconcile(db: &sea_orm::DatabaseConnection, series_id: Uuid) {
    let mut stats = server::library::scanner::stats::ScanStats::default();
    server::library::reconcile::reconcile_series(db, series_id, &mut stats)
        .await
        .unwrap();
}

/// A series left soft-removed while its issues are live again (a partial
/// failure of a restore) carries a rating derived while none of them were
/// live. The scan that lifts the removal re-derives it first, so the series
/// does not come back unrated to a capped reader.
#[tokio::test]
async fn a_series_the_scan_brings_back_is_rated_from_its_live_issues() {
    let w = world().await;
    let fx = &w.fx;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    entity::issue::ActiveModel {
        id: Unchanged(fx.mature_1.clone()),
        age_rating: Set(Some("Mature 17+".into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    entity::series::ActiveModel {
        id: Unchanged(fx.mature_series),
        age_rating: Set(None),
        declared_age_rating: Set(None),
        removed_at: Set(Some(Utc::now().fixed_offset())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    let mut stats = server::library::scanner::stats::ScanStats::default();
    server::library::reconcile::reconcile_library(&db, fx.lib_id, &mut stats)
        .await
        .unwrap();

    let row = entity::series::Entity::find_by_id(fx.mature_series)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.removed_at.is_none(), "the scan lifts the removal");
    assert_eq!(row.age_rating.as_deref(), Some("Mature 17+"));
    let (st, _) = get_json(&w.app, "/api/series/mature-matters", &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert!(
        !names(&body, "name").contains(&"Mature Matters".to_owned()),
        "{body}"
    );
    let (st, _) = get_json(&w.app, "/api/series/mature-matters", &w.uncapped).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn series_rating_follows_its_highest_rated_active_issue() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    // Seeded with a stale scan-order value and no provenance row.
    let s = seed_series(
        &db,
        w.fx.lib_id,
        "Derived Run",
        "derived-run",
        Some("Everyone"),
    )
    .await;
    for (n, rating) in [(1u8, Some("teen")), (2, None), (3, Some("Sixteen and up"))] {
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            n,
            &format!("derived-{n}"),
            &format!("Derived {n}"),
            rating,
            None,
        )
        .await;
    }
    rollup(&db, s).await;
    // Canonical spelling; the off-ladder rating ranks as unrated.
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Teen"));

    // A higher-rated issue arrives: the series rises with it.
    let four = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        4,
        "derived-4",
        "Derived 4",
        Some(" mature 17+ "),
        None,
    )
    .await;
    rollup(&db, s).await;
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature 17+"));
    let (_, body) = get_json(&w.app, "/api/series?q=Derived", &w.capped).await;
    assert!(names(&body, "name").is_empty(), "{body}");

    // Its file goes: the soft-removal lowers the series again.
    let file = root.join("derived-4.cbz");
    std::fs::remove_file(&file).unwrap();
    reconcile(&db, s).await;
    let removed = entity::issue::Entity::find_by_id(four.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(removed.removed_at.is_some());
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Teen"));
    let (_, body) = get_json(&w.app, "/api/series?q=Derived", &w.capped).await;
    assert_eq!(names(&body, "name"), vec!["Derived Run"], "{body}");

    // The file comes back: the restore raises it again.
    write_cbz(&file);
    reconcile(&db, s).await;
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature 17+"));
}

#[tokio::test]
async fn series_rating_ignores_unrated_issues_and_respects_who_set_it() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let mk = |name: &'static str, slug: &'static str| (name, slug);
    let mut ids = Vec::new();
    for (name, slug) in [
        mk("Unknown Run", "unknown-run"),
        mk("User Run", "user-run"),
        mk("Provider Run", "provider-run"),
    ] {
        let s = seed_series(&db, w.fx.lib_id, name, slug, Some("Everyone")).await;
        ids.push(s);
    }
    let (unknown, user_set, provider_set) = (ids[0], ids[1], ids[2]);
    // Only unrated issues (NULL, `Unknown`, off-ladder text): unrated.
    for (n, rating) in [(1u8, None), (2, Some("Unknown")), (3, Some("Sixteen"))] {
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            unknown,
            n,
            &format!("unknown-run-{n}"),
            &format!("Unknown Run {n}"),
            rating,
            None,
        )
        .await;
    }
    // A person set one series' rating, a provider the other's; their
    // issues rank higher, and neither value moves.
    for (s, slug) in [(user_set, "user-run"), (provider_set, "provider-run")] {
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            1,
            &format!("{slug}-1"),
            &format!("{slug} one"),
            Some("X18+"),
            None,
        )
        .await;
    }
    pin_series_rating(&db, provider_set, "comicvine").await;
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/series/user-run",
        &w.admin,
        Some(serde_json::json!({ "age_rating": "Teen" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    for s in ids.iter().copied() {
        rollup(&db, s).await;
    }
    assert_eq!(series_rating(&db, unknown).await, None);
    assert_eq!(series_rating(&db, user_set).await.as_deref(), Some("Teen"));
    assert_eq!(
        series_rating(&db, provider_set).await.as_deref(),
        Some("Everyone")
    );
}

#[tokio::test]
async fn an_issue_rating_edit_rederives_the_series_rating() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    // No rollup on this path: the PATCH re-derives the series itself.
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/series/teen-tales/issues/teen-2",
        &w.admin,
        Some(serde_json::json!({ "age_rating": "Adults Only 18+" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        series_rating(&db, w.fx.teen_series).await.as_deref(),
        Some("Adults Only 18+")
    );
    let (st, _) = get_json(&w.app, "/api/series/teen-tales", &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Bulk metadata follows the same rule.
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/me/issues/bulk-metadata",
        &w.admin,
        Some(serde_json::json!({
            "issue_ids": [w.fx.teen_2],
            "patch": { "age_rating": "Everyone" },
            "mode": "replace",
        })),
    )
    .await;
    assert!(st.is_success(), "{st} {}", String::from_utf8_lossy(&body));
    assert_eq!(
        series_rating(&db, w.fx.teen_series).await.as_deref(),
        Some("Everyone")
    );
}

#[tokio::test]
async fn migration_ladder_and_recompute_agree_with_the_runtime_derivation() {
    use sea_orm::ConnectionTrait;
    use server::library::age_rating::{ALIASES, LADDER};
    assert_eq!(migration::AGE_RATING_LADDER, LADDER);
    assert_eq!(migration::AGE_RATING_ALIASES, ALIASES);

    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let mut rung_series = Vec::new();
    for (i, rung) in LADDER.iter().enumerate() {
        let slug = format!("rung-{i}");
        let s = seed_series(
            &db,
            w.fx.lib_id,
            &format!("Rung {i}"),
            &slug,
            Some("Unknown"),
        )
        .await;
        // An unranked issue and one on this rung, in odd case and spacing;
        // plus a removed issue on the top rung that must not count.
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            1,
            &format!("{slug}-1"),
            "r1",
            Some("Rating Pending"),
            None,
        )
        .await;
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            2,
            &format!("{slug}-2"),
            "r2",
            Some(&format!("  {}  ", rung.to_uppercase())),
            None,
        )
        .await;
        let gone = seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            3,
            &format!("{slug}-3"),
            "r3",
            Some("X18+"),
            None,
        )
        .await;
        entity::issue::ActiveModel {
            id: Unchanged(gone),
            removed_at: Set(Some(Utc::now().fixed_offset())),
            ..Default::default()
        }
        .update(&db)
        .await
        .unwrap();
        rung_series.push((s, *rung));
    }
    // A user-set rating that disagrees with its issues.
    let pinned = seed_series(&db, w.fx.lib_id, "Pinned", "pinned", Some("Everyone")).await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        pinned,
        1,
        "pinned-1",
        "p1",
        Some("X18+"),
        None,
    )
    .await;
    pin_series_rating(&db, pinned, "user").await;

    // The upgrade statement recomputes the wrong values and leaves the
    // user-set one.
    db.execute_unprepared(&migration::recompute_all_series_age_ratings_sql())
        .await
        .unwrap();
    for (s, rung) in &rung_series {
        assert_eq!(
            series_rating(&db, *s).await.as_deref(),
            Some(*rung),
            "{rung}"
        );
    }
    assert_eq!(
        series_rating(&db, pinned).await.as_deref(),
        Some("Everyone")
    );
    // The upgrade never lowers: an unpinned fixture series whose issues
    // carry no rating keeps its stored rating.
    assert_eq!(
        series_rating(&db, w.fx.teen_series).await.as_deref(),
        Some("Teen")
    );

    // The runtime derivation gives the same answer on every rung.
    for (s, _) in &rung_series {
        entity::series::ActiveModel {
            id: Unchanged(*s),
            age_rating: Set(Some("Unknown".into())),
            declared_age_rating: Set(None),
            ..Default::default()
        }
        .update(&db)
        .await
        .unwrap();
    }
    let mut ids: Vec<Uuid> = rung_series.iter().map(|(s, _)| *s).collect();
    ids.push(pinned);
    let changed = server::library::series_rating::recompute_series_age_ratings(&db, &ids)
        .await
        .unwrap();
    assert_eq!(changed, LADDER.len() as u64);
    for (s, rung) in &rung_series {
        assert_eq!(
            series_rating(&db, *s).await.as_deref(),
            Some(*rung),
            "{rung}"
        );
        // And the in-memory twin agrees.
        assert_eq!(
            server::library::age_rating::highest([
                Some("Rating Pending"),
                Some(&format!("  {}  ", rung.to_uppercase())),
            ]),
            Some(*rung)
        );
    }
    assert_eq!(
        series_rating(&db, pinned).await.as_deref(),
        Some("Everyone")
    );
}

// ───── an issue is visible only when its series is ─────
//
// "Dark Saga" is rated `Mature 17+` by a person (pinned), above the `Teen`
// reader's cap; both its issues carry their own `Teen` rating, which alone
// would pass.

struct DarkSeries {
    one: String,
    two: String,
}

async fn hidden_series_world() -> (World, DarkSeries) {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let dark = seed_series(
        &db,
        w.fx.lib_id,
        "Dark Saga",
        "dark-saga",
        Some("Mature 17+"),
    )
    .await;
    pin_series_rating(&db, dark, "user").await;
    let one = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        dark,
        1,
        "dark-1",
        "Dark One",
        Some("Teen"),
        None,
    )
    .await;
    let two = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        dark,
        2,
        "dark-2",
        "Dark Two",
        Some("Teen"),
        None,
    )
    .await;
    rollup(&db, dark).await;
    assert_eq!(
        series_rating(&db, dark).await.as_deref(),
        Some("Mature 17+")
    );
    (w, DarkSeries { one, two })
}

#[tokio::test]
async fn an_issue_of_a_hidden_series_answers_like_an_unknown_id() {
    let (w, dark) = hidden_series_world().await;
    let hidden = dark.one.clone();
    let unknown = unknown_issue_id();
    // The Komga-compatible routes answer only in that compat mode.
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/admin/settings",
        &w.admin,
        Some(serde_json::json!({ "compat.opds_panels_mode": "komga" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let get_routes = [
        "/api/issues/{}/covers",
        "/api/me/issues/{}/markers",
        "/api/me/issues/{}/page-overrides",
        "/api/issues/{}/next-up",
        "/api/issues/{}/prev-up",
        "/issues/{}/pages/0",
        "/issues/{}/pages/0/thumb",
        "/issues/{}",
        "/opds/v1/issues/{}/file",
        "/api/v1/books/{}",
    ];
    for t in get_routes {
        let h = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &hidden),
            &w.capped,
            None,
            &hidden,
        )
        .await;
        let u = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &unknown),
            &w.capped,
            None,
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{t}");
        assert!(h.0.is_client_error(), "{t}: {h:?}");
        for user in [&w.uncapped, &w.admin] {
            let (st, body) = raw(&w.app, Method::GET, &t.replace("{}", &hidden), user, None).await;
            assert!(
                st.is_success() || st.is_redirection(),
                "{t}: {st} {}",
                String::from_utf8_lossy(&body)
            );
        }
    }
    // The neighbour's next-up / prev-up never lands on it either: the
    // series walk of a hidden series is not the reader's to take.
    for t in ["/api/issues/{}/next-up", "/api/issues/{}/prev-up"] {
        let h = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &dark.two),
            &w.capped,
            None,
            &dark.two,
        )
        .await;
        let u = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &unknown),
            &w.capped,
            None,
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{t}");
    }

    // Writes keyed on the issue.
    type Body = fn(&str) -> serde_json::Value;
    let writes: [(Method, &str, Body); 4] = [
        (
            Method::POST,
            "/api/progress",
            |id| serde_json::json!({"issue_id": id, "page": 1, "finished": true}),
        ),
        (
            Method::POST,
            "/api/me/markers",
            |id| serde_json::json!({"issue_id": id, "page_index": 0, "kind": "bookmark"}),
        ),
        (
            Method::POST,
            "/api/me/issues/{}/ocr",
            |_| serde_json::json!({"page": 0, "region": {"x": 0, "y": 0, "w": 1, "h": 1}}),
        ),
        (
            Method::PATCH,
            "/api/v1/books/{}/read-progress",
            |_| serde_json::json!({"page": 1}),
        ),
    ];
    for (method, path, body) in writes {
        let h = masked(
            &w.app,
            method.clone(),
            &path.replace("{}", &hidden),
            &w.capped,
            Some(body(&hidden)),
            &hidden,
        )
        .await;
        let u = masked(
            &w.app,
            method.clone(),
            &path.replace("{}", &unknown),
            &w.capped,
            Some(body(&unknown)),
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{method} {path}");
        assert!(h.0.is_client_error(), "{method} {path}: {h:?}");
    }
    assert!(!has_progress_row(&w.app, w.capped.user_id, &hidden).await);
    let (st, body) = post_json(
        &w.app,
        "/api/progress",
        &w.uncapped,
        serde_json::json!({"issue_id": hidden, "page": 1}),
    )
    .await;
    assert!(st.is_success(), "{st} {body}");
}

#[tokio::test]
async fn issue_lists_rails_and_feeds_leave_out_an_issue_of_a_hidden_series() {
    let (w, dark) = hidden_series_world().await;
    // The capped reader read #1 (and is mid-way through #2); so did the
    // uncapped reader, whose rails prove the seed shows up.
    for user in [&w.capped, &w.uncapped] {
        progress(&w.app, user.user_id, &dark.one, true).await;
        progress(&w.app, user.user_id, &dark.two, false).await;
    }
    let has_dark = |body: &serde_json::Value| body.to_string().contains("Dark ");
    let lists = [
        "/api/issues?limit=50",
        "/api/issues/search?q=Dark",
        "/api/me/recent-issues",
        "/api/me/continue-reading",
        "/api/me/on-deck",
        "/api/me/new-in-series",
    ];
    for path in lists {
        let (st, body) = get_json(&w.app, path, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        assert!(!has_dark(&body), "{path}: {body}");
    }
    for path in [
        "/api/issues?limit=50",
        "/api/issues/search?q=Dark",
        "/api/me/recent-issues",
    ] {
        for user in [&w.uncapped, &w.admin] {
            let (_, body) = get_json(&w.app, path, user).await;
            assert!(has_dark(&body), "{path}: {body}");
        }
    }

    // An issue-rooted saved view.
    let view = |user: &Authed| {
        let user = Authed {
            session: user.session.clone(),
            csrf: user.csrf.clone(),
            user_id: user.user_id,
        };
        let app = &w.app;
        async move {
            let (st, view) = post_json(
                app,
                "/api/me/saved-views",
                &user,
                serde_json::json!({
                    "kind": "filter_issues",
                    "name": "Teen issues",
                    "filter": {"match_mode": "all", "conditions": [
                        {"field": "title", "op": "contains", "value": "Dark"}
                    ]},
                    "sort_field": "name",
                    "sort_order": "asc",
                    "result_limit": 12,
                }),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{view}");
            let id = view["id"].as_str().unwrap().to_owned();
            let (st, body) = get_json(
                app,
                &format!("/api/me/saved-views/{id}/issue-results"),
                &user,
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{body}");
            body
        }
    };
    assert!(!has_dark(&view(&w.capped).await));
    assert!(has_dark(&view(&w.uncapped).await));

    // OPDS: recent issues and the reader's history feeds.
    for path in ["/opds/v1/recent", "/opds/v2/recent"] {
        let (st, bytes) = raw(&w.app, Method::GET, path, &w.capped, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        assert!(!String::from_utf8_lossy(&bytes).contains("Dark "), "{path}");
        let (_, bytes) = raw(&w.app, Method::GET, path, &w.uncapped, None).await;
        assert!(String::from_utf8_lossy(&bytes).contains("Dark "), "{path}");
    }
}

// ───── per-grant "hide unrated" ─────

/// Replace `user`'s grants through the admin endpoint.
async fn set_access(
    w: &World,
    user: &Authed,
    caps: serde_json::Value,
    hide_unrated: serde_json::Value,
) -> serde_json::Value {
    let lib = w.fx.lib_id.to_string();
    let (st, body) = post_json(
        &w.app,
        &format!("/api/admin/users/{}/library-access", user.user_id),
        &w.admin,
        serde_json::json!({
            "library_ids": [lib],
            "age_rating_caps": caps,
            "hide_unrated": hide_unrated,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn hide_unrated_round_trips_and_is_audited() {
    let w = world().await;
    let lib = w.fx.lib_id.to_string();
    let body = set_access(
        &w,
        &w.capped,
        serde_json::json!({ lib.clone(): "Teen" }),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    assert_eq!(body["library_access"][0]["hide_unrated"], true, "{body}");
    assert_eq!(
        body["library_access"][0]["age_rating_max"], "Teen",
        "{body}"
    );
    let (_, detail) = get_json(
        &w.app,
        &format!("/api/admin/users/{}", w.capped.user_id),
        &w.admin,
    )
    .await;
    assert_eq!(
        detail["library_access"][0]["hide_unrated"], true,
        "{detail}"
    );

    // Omitted: off again (the field is optional and defaults to false).
    let (st, body) = post_json(
        &w.app,
        &format!("/api/admin/users/{}/library-access", w.capped.user_id),
        &w.admin,
        serde_json::json!({ "library_ids": [lib], "age_rating_caps": { lib.clone(): "Teen" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["library_access"][0]["hide_unrated"], false, "{body}");

    let (_, audit) = get_json(
        &w.app,
        "/api/admin/audit?action=admin.user.library_access.set",
        &w.admin,
    )
    .await;
    let entries = audit["items"].as_array().cloned().unwrap_or_default();
    assert!(
        entries
            .iter()
            .any(|e| e["payload"]["hide_unrated"][&lib] == true),
        "{audit}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["payload"]["hide_unrated"][&lib] == false),
        "{audit}"
    );
}

#[tokio::test]
async fn hide_unrated_hides_an_unrated_series_and_its_issues() {
    let w = world().await;
    let fx = &w.fx;
    let lib = fx.lib_id.to_string();
    // Off: today's behaviour, unrated content is shown.
    let (_, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert!(names(&body, "name").contains(&"Unrated Stories".to_owned()));

    set_access(
        &w,
        &w.capped,
        serde_json::json!({ lib.clone(): "Teen" }),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;

    // Series list, search arm and the series page.
    let (_, body) = get_json(&w.app, "/api/series", &w.capped).await;
    assert_eq!(names(&body, "name"), vec!["Teen Tales"], "{body}");
    assert_eq!(body["total"], 1);
    let (_, body) = get_json(&w.app, "/api/series?q=Unrated", &w.capped).await;
    assert!(names(&body, "name").is_empty(), "{body}");
    let (st, _) = get_json(&w.app, "/api/series/unrated-stories", &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Issues of the unrated series, rated or not, are gone; the issues of
    // the rated series with no rating of their own follow it and stay.
    let (_, body) = get_json(&w.app, "/api/issues?limit=50", &w.capped).await;
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(got, vec!["Teen One", "Teen Two"], "{body}");
    let (_, body) = get_json(&w.app, "/api/issues/search?q=Unrated", &w.capped).await;
    assert!(names(&body, "title").is_empty(), "{body}");
    let (_, body) = get_json(&w.app, "/api/me/recent-issues", &w.capped).await;
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(got, vec!["Teen One", "Teen Two"], "{body}");
    let unknown = unknown_issue_id();
    for t in ["/issues/{}/pages/0", "/api/issues/{}/covers"] {
        let h = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &fx.unrated_1),
            &w.capped,
            None,
            &fx.unrated_1,
        )
        .await;
        let u = masked(
            &w.app,
            Method::GET,
            &t.replace("{}", &unknown),
            &w.capped,
            None,
            &unknown,
        )
        .await;
        assert_eq!(h, u, "{t}");
        assert_eq!(h.0, StatusCode::NOT_FOUND, "{t}");
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

    // Entity surfaces: the writer credit counts only the Teen issues.
    let (_, body) = get_json(&w.app, "/api/creators", &w.capped).await;
    assert_eq!(body["items"][0]["credit_count"], 2, "{body}");

    // OPDS.
    let (_, bytes) = raw(&w.app, Method::GET, "/opds/v1/series", &w.capped, None).await;
    let xml = String::from_utf8(bytes).unwrap();
    assert!(
        xml.contains("Teen Tales") && !xml.contains("Unrated Stories"),
        "{xml}"
    );
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/opds/v1/series/{}", fx.unrated_series),
        &w.capped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Admins are unaffected.
    let (_, body) = get_json(&w.app, "/api/series", &w.admin).await;
    assert_eq!(names(&body, "name").len(), 3);
}

#[tokio::test]
async fn hide_unrated_without_a_cap_hides_only_unrated_content() {
    let w = world().await;
    let lib = w.fx.lib_id.to_string();
    set_access(
        &w,
        &w.uncapped,
        serde_json::json!({}),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    let (_, body) = get_json(&w.app, "/api/series", &w.uncapped).await;
    let mut got = names(&body, "name");
    got.sort();
    assert_eq!(got, vec!["Mature Matters", "Teen Tales"], "{body}");
    let (_, body) = get_json(&w.app, "/api/issues?limit=50", &w.uncapped).await;
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(
        got,
        vec!["Mature One", "Mature Two", "Teen One", "Teen Two"],
        "{body}"
    );
    let (st, _) = raw(
        &w.app,
        Method::GET,
        &format!("/issues/{}/pages/0", w.fx.unrated_mature_2),
        &w.uncapped,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn hide_unrated_entity_and_rail_surfaces_skip_an_unrated_series() {
    let e = entity_world().await;
    let w = &e.w;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    // Twin Run loses its rating: with the option on, its cast is hidden.
    entity::series::ActiveModel {
        id: Unchanged(
            entity::series::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .into_iter()
                .find(|s| s.slug == "twin-run")
                .unwrap()
                .id,
        ),
        age_rating: Set(None),
        declared_age_rating: Set(None),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    let lib = w.fx.lib_id.to_string();
    let sidekick = admin_slug(w, "/api/characters", "name", "Shared Sidekick").await;
    let base = format!("/api/characters/{sidekick}");
    let (_, body) = get_json(&w.app, &format!("{base}/series"), &w.capped).await;
    let mut got = item_strings(&body, "slug");
    got.sort();
    assert_eq!(got, vec!["gap-run", "twin-run"], "{body}");
    // The reader is part-way through Twin One: Continue reading shows it.
    progress(&w.app, w.capped.user_id, &e.twin_one, false).await;
    let (st, body) = get_json(&w.app, "/api/me/continue-reading", &w.capped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("Twin One"), "{body}");
    let (_, body) = get_json(&w.app, "/api/me/recent-issues", &w.capped).await;
    assert!(body.to_string().contains("Twin One"), "{body}");
    // On deck needs a finished issue with a readable next one: the
    // uncapped reader finished Twin One, so Twin Two (rated above the
    // capped reader's cap) is on deck for them.
    progress(&w.app, w.uncapped.user_id, &e.twin_one, true).await;
    let (st, body) = get_json(&w.app, "/api/me/on-deck", &w.uncapped).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("Twin Two"), "{body}");

    set_access(
        w,
        &w.capped,
        serde_json::json!({ lib.clone(): "Teen" }),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    set_access(
        w,
        &w.uncapped,
        serde_json::json!({}),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    let (_, body) = get_json(&w.app, "/api/me/on-deck", &w.uncapped).await;
    assert!(!body.to_string().contains("Twin"), "{body}");
    let (_, body) = get_json(&w.app, &format!("{base}/series"), &w.capped).await;
    assert_eq!(item_strings(&body, "slug"), vec!["gap-run"], "{body}");
    let (_, body) = get_json(&w.app, &base, &w.capped).await;
    assert_eq!(body["series_count"], 1, "{body}");
    let (_, bytes) = raw(
        &w.app,
        Method::GET,
        &format!("/opds/v1/characters/{sidekick}"),
        &w.capped,
        None,
    )
    .await;
    assert!(!String::from_utf8_lossy(&bytes).contains("Twin"));
    // The rails leave the unrated series out too.
    for rail in [
        "/api/me/continue-reading",
        "/api/me/on-deck",
        "/api/me/recent-issues",
    ] {
        let (st, body) = get_json(&w.app, rail, &w.capped).await;
        assert_eq!(st, StatusCode::OK, "{rail}: {body}");
        assert!(!body.to_string().contains("Twin"), "{rail}: {body}");
    }
}

/// A reader whose grant hides unrated content but carries no cap: the
/// issue predicate then has no own-rating part, only the series rule
/// (`raw_issue_clause`'s hide-only arm). Every hand-written-SQL surface
/// must leave out what only the unrated series carries.
#[tokio::test]
async fn hide_only_reader_leaves_out_an_unrated_series_on_sql_surfaces() {
    let w = world().await;
    let fx = &w.fx;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    tag_issue(
        &db,
        &fx.teen_1,
        "Cap Writer",
        None,
        Some("Teen Hero"),
        None,
        None,
        Some("Teen Genre"),
        None,
        None,
    )
    .await;
    tag_issue(
        &db,
        &fx.unrated_1,
        "Cap Writer, Unrated Scribe",
        None,
        Some("Unrated Hero"),
        None,
        None,
        Some("Unrated Genre"),
        None,
        None,
    )
    .await;
    // Entity rows for the new names; the seeded ratings are a person's,
    // so the rollup keeps them (Unrated Stories stays unrated).
    for series_id in [fx.teen_series, fx.unrated_series] {
        pin_series_rating(&db, series_id, "user").await;
        server::library::scanner::metadata_rollup::rollup_series_metadata(&db, series_id)
            .await
            .unwrap();
    }
    assert_eq!(series_rating(&db, fx.unrated_series).await, None);
    // Part-way through one issue of each series; finished one of each of
    // Mature Matters and Unrated Stories.
    for id in [&fx.teen_1, &fx.unrated_1] {
        progress(&w.app, w.uncapped.user_id, id, false).await;
    }
    for id in [&fx.mature_1, &fx.unrated_mature_2] {
        progress(&w.app, w.uncapped.user_id, id, true).await;
    }
    let values = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };

    // Without the option the unrated series is there.
    let (_, body) = get_json(&w.app, "/api/characters", &w.uncapped).await;
    let mut got = item_strings(&body, "name");
    got.sort();
    assert_eq!(got, vec!["Teen Hero", "Unrated Hero"], "{body}");
    assert_eq!(body["total"], 2, "{body}");

    let lib = fx.lib_id.to_string();
    set_access(
        &w,
        &w.uncapped,
        serde_json::json!({}),
        serde_json::json!({ lib: true }),
    )
    .await;
    let u = &w.uncapped;

    let (_, body) = get_json(&w.app, "/api/characters", u).await;
    assert_eq!(item_strings(&body, "name"), vec!["Teen Hero"], "{body}");
    assert_eq!(body["total"], 1, "{body}");

    // Creators and people: the scribe is only on the unrated issue; the
    // shared writer counts the four issues of the two rated series plus
    // Teen Tales' series-level credit (its rollup ran above). Unrated
    // Stories' two issues and its series-level credit are left out.
    let (_, body) = get_json(&w.app, "/api/creators", u).await;
    assert_eq!(item_strings(&body, "person"), vec!["Cap Writer"], "{body}");
    assert_eq!(body["items"][0]["credit_count"], 5, "{body}");
    assert_eq!(body["total"], 1, "{body}");
    let (_, body) = get_json(&w.app, "/api/people?q=scribe", u).await;
    assert!(item_strings(&body, "person").is_empty(), "{body}");
    let (_, body) = get_json(&w.app, "/api/people?q=cap", u).await;
    assert_eq!(body["items"][0]["credit_count"], 5, "{body}");

    // Filter-option pickers.
    let (_, body) = get_json(&w.app, "/api/filter-options/genres", u).await;
    assert_eq!(values(&body), vec!["Teen Genre"], "{body}");
    for path in [
        "/api/filter-options/credits",
        "/api/filter-options/credits/writer",
    ] {
        let (st, body) = get_json(&w.app, path, u).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        let got = values(&body);
        assert!(
            got.contains(&"Cap Writer".to_owned()) && !got.contains(&"Unrated Scribe".to_owned()),
            "{path}: {body}"
        );
    }

    // Rails.
    let (_, body) = get_json(&w.app, "/api/me/continue-reading", u).await;
    let text = body.to_string();
    assert!(
        text.contains("Teen One") && !text.contains("Unrated"),
        "{body}"
    );
    let (_, body) = get_json(&w.app, "/api/me/recent-issues", u).await;
    let mut got = names(&body, "title");
    got.sort();
    assert_eq!(
        got,
        vec!["Mature One", "Mature Two", "Teen One", "Teen Two"],
        "{body}"
    );

    // Random picks reads series rows only (`raw_cap_clause`, no issue
    // predicate): the capped reader, who has read nothing, is offered the
    // unrated series until the option hides it.
    let (_, body) = get_json(&w.app, "/api/me/random-picks", &w.capped).await;
    let mut got = names(&body, "name");
    got.sort();
    assert_eq!(got, vec!["Teen Tales", "Unrated Stories"], "{body}");
    set_access(
        &w,
        &w.capped,
        serde_json::json!({ fx.lib_id.to_string(): "Teen" }),
        serde_json::json!({ fx.lib_id.to_string(): true }),
    )
    .await;
    let (_, body) = get_json(&w.app, "/api/me/random-picks", &w.capped).await;
    assert_eq!(names(&body, "name"), vec!["Teen Tales"], "{body}");

    // Reading log.
    let (st, body) = get_json(&w.app, "/api/me/reading-log", u).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let text = body.to_string();
    assert!(text.contains(&fx.mature_1), "{body}");
    assert!(
        !text.contains(&fx.unrated_mature_2) && !text.contains("Unrated"),
        "{body}"
    );
}

// ───── the series.json floor, the upgrade rule, Metron's words ─────

fn write_series_json(dir: &std::path::Path, rating: Option<&str>) {
    let mut meta = serde_json::json!({ "type": "comicSeries", "name": "Floor Run" });
    if let Some(r) = rating {
        meta["age_rating"] = r.into();
    }
    std::fs::write(
        dir.join("series.json"),
        serde_json::json!({ "metadata": meta }).to_string(),
    )
    .unwrap();
}

async fn only_series(db: &sea_orm::DatabaseConnection, lib: Uuid) -> entity::series::Model {
    use sea_orm::{ColumnTrait, QueryFilter};
    entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(lib))
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

async fn capped_opens_series(w: &World, slug: &str) -> bool {
    let (st, _) = get_json(&w.app, &format!("/api/series/{slug}"), &w.capped).await;
    st == StatusCode::OK
}

async fn run_upgrade(db: &sea_orm::DatabaseConnection) {
    use sea_orm::ConnectionTrait;
    db.execute_unprepared(&migration::provisional_declared_sql())
        .await
        .unwrap();
    db.execute_unprepared(&migration::recompute_all_series_age_ratings_sql())
        .await
        .unwrap();
}

async fn set_series_ratings(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
    rating: Option<&str>,
    declared: Option<&str>,
) {
    SeriesAM {
        id: Unchanged(id),
        age_rating: Set(rating.map(str::to_owned)),
        declared_age_rating: Set(declared.map(str::to_owned)),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_series_json_rating_is_a_floor_through_upgrade_rollup_and_rescan() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("floor-lib");
    let folder = root.join("Floor Run (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    // Unrated issues; the sidecar declares Mature 17+.
    write_cbz(&folder.join("Floor Run 001.cbz"));
    write_cbz(&folder.join("Floor Run 002.cbz"));
    write_series_json(&folder, Some("Mature 17+"));
    let lib = insert_library(&db, &root).await;
    grant(&w.app, w.capped.user_id, lib, Some("Teen")).await;
    let state = w.app.state();
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    let s = only_series(&db, lib).await;
    assert_eq!(s.declared_age_rating.as_deref(), Some("Mature 17+"));
    assert_eq!(s.age_rating.as_deref(), Some("Mature 17+"));
    assert!(!capped_opens_series(&w, &s.slug).await);

    // Before the upgrade the column did not exist: the stored rating was
    // the sidecar's copy and nothing declared it. The upgrade keeps it,
    // and a recompute before the next scan keeps it too.
    set_series_ratings(&db, s.id, Some("Mature 17+"), None).await;
    run_upgrade(&db).await;
    assert_eq!(
        series_rating(&db, s.id).await.as_deref(),
        Some("Mature 17+")
    );
    assert!(!capped_opens_series(&w, &s.slug).await);
    rollup(&db, s.id).await;
    assert_eq!(
        series_rating(&db, s.id).await.as_deref(),
        Some("Mature 17+")
    );
    assert!(!capped_opens_series(&w, &s.slug).await);

    // The next scan (the folder is unchanged) records the real floor.
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    let s = only_series(&db, lib).await;
    assert_eq!(s.declared_age_rating.as_deref(), Some("Mature 17+"));
    rollup(&db, s.id).await;
    assert_eq!(
        series_rating(&db, s.id).await.as_deref(),
        Some("Mature 17+")
    );
    assert!(!capped_opens_series(&w, &s.slug).await);

    // series.json goes: the next scan re-derives from the issues alone.
    std::fs::remove_file(folder.join("series.json")).unwrap();
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    let s = only_series(&db, lib).await;
    assert_eq!(s.declared_age_rating, None);
    assert_eq!(s.age_rating, None);
    assert!(capped_opens_series(&w, &s.slug).await);

    // A sidecar that exists but cannot be parsed leaves the floor alone.
    write_series_json(&folder, Some("Teen"));
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    assert_eq!(series_rating(&db, s.id).await.as_deref(), Some("Teen"));
    std::fs::write(folder.join("series.json"), b"{ not json").unwrap();
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    let s = only_series(&db, lib).await;
    assert_eq!(s.declared_age_rating.as_deref(), Some("Teen"));
    assert_eq!(s.age_rating.as_deref(), Some("Teen"));
}

#[tokio::test]
async fn an_issue_rated_above_the_declared_floor_wins() {
    use server::library::series_rating::apply_declared_age_rating;
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let s = seed_series(&db, w.fx.lib_id, "Above Floor", "above-floor", None).await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        1,
        "above-floor-1",
        "Above One",
        Some("Mature 17+"),
        None,
    )
    .await;
    // The writer re-derives when the declared value changes.
    assert!(
        apply_declared_age_rating(&db, s, Some(" Teen "))
            .await
            .unwrap()
    );
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature 17+"));
    // Unchanged declared value: no write, no recompute.
    assert!(
        !apply_declared_age_rating(&db, s, Some("Teen"))
            .await
            .unwrap()
    );
    // A floor above the issues raises the series; clearing it lowers it.
    assert!(
        apply_declared_age_rating(&db, s, Some("X18+"))
            .await
            .unwrap()
    );
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("X18+"));
    assert!(apply_declared_age_rating(&db, s, None).await.unwrap());
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature 17+"));
    // A person-set rating still wins over both.
    pin_series_rating(&db, s, "user").await;
    set_series_ratings(&db, s, Some("Everyone"), None).await;
    apply_declared_age_rating(&db, s, Some("X18+"))
        .await
        .unwrap();
    rollup(&db, s).await;
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Everyone"));
}

#[tokio::test]
async fn the_upgrade_never_lowers_a_rating_and_still_raises_and_fills() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let mut cases = Vec::new();
    // (stored, issue ratings, after upgrade, provisional floor)
    for (i, (stored, issues, want, floor)) in [
        (
            Some("Mature 17+"),
            vec![Some("Teen")],
            Some("Mature 17+"),
            Some("Mature 17+"),
        ),
        (
            Some("Everyone"),
            vec![Some("Teen"), None],
            Some("Teen"),
            None,
        ),
        (None, vec![Some("Mature")], Some("Mature 17+"), None),
        (Some("PG-13"), vec![Some("Teen")], Some("Teen"), None),
        (Some("PG-13"), vec![None], Some("PG-13"), None),
        (None, vec![None], None, None),
        (
            Some("Mature"),
            vec![Some("Teen")],
            Some("Mature 17+"),
            Some("Mature"),
        ),
        // Equal to the highest issue: the floor is recorded too.
        (
            Some("Teen"),
            vec![Some("teen"), None],
            Some("Teen"),
            Some("Teen"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let slug = format!("upgrade-{i}");
        let s = seed_series(&db, w.fx.lib_id, &format!("Upgrade {i}"), &slug, stored).await;
        for (n, r) in issues.iter().enumerate() {
            seed_issue(
                &db,
                &root,
                w.fx.lib_id,
                s,
                n as u8 + 1,
                &format!("{slug}-{n}"),
                "u",
                *r,
                None,
            )
            .await;
        }
        cases.push((s, want, floor));
    }
    run_upgrade(&db).await;
    for (s, want, floor) in &cases {
        let row = entity::series::Entity::find_by_id(*s)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.age_rating.as_deref(), *want, "{s}");
        assert_eq!(row.declared_age_rating.as_deref(), *floor, "{s}");
    }
    // The stored Mature 17+ outlives an issue removal before the next
    // scan; that scan (no series.json) drops the provisional floor and the
    // series follows its issues.
    let (held, ..) = cases[0];
    rollup(&db, held).await;
    assert_eq!(
        series_rating(&db, held).await.as_deref(),
        Some("Mature 17+")
    );
    server::library::series_rating::apply_declared_age_rating(&db, held, None)
        .await
        .unwrap();
    assert_eq!(series_rating(&db, held).await.as_deref(), Some("Teen"));
}

#[tokio::test]
async fn metron_words_rank_in_sql_like_their_rungs() {
    use server::library::age_rating::{ALIASES, LADDER, highest};
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let mut by_alias = Vec::new();
    for (i, (word, rank)) in ALIASES.iter().enumerate() {
        let slug = format!("alias-{i}");
        let s = seed_series(&db, w.fx.lib_id, &format!("Alias {i}"), &slug, None).await;
        seed_issue(
            &db,
            &root,
            w.fx.lib_id,
            s,
            1,
            &format!("{slug}-1"),
            "a",
            Some(&format!("\t{} ", word.to_uppercase())),
            None,
        )
        .await;
        by_alias.push((s, LADDER[*rank as usize]));
        assert_eq!(highest([Some(*word)]), Some(LADDER[*rank as usize]));
    }
    // Upgrade and runtime both store the rung's canonical spelling.
    run_upgrade(&db).await;
    for (s, rung) in &by_alias {
        assert_eq!(series_rating(&db, *s).await.as_deref(), Some(*rung));
        set_series_ratings(&db, *s, None, None).await;
        rollup(&db, *s).await;
        assert_eq!(series_rating(&db, *s).await.as_deref(), Some(*rung));
    }
    // Every alias is hidden from a Teen reader: the series, and the
    // issues by their own rating too.
    let (_, body) = get_json(&w.app, "/api/series?q=Alias", &w.capped).await;
    assert!(names(&body, "name").is_empty(), "{body}");
    let (_, body) = get_json(&w.app, "/api/series?q=Alias", &w.uncapped).await;
    assert_eq!(names(&body, "name").len(), ALIASES.len(), "{body}");
}

#[tokio::test]
async fn a_metron_worded_issue_is_held_to_the_cap() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    // A person rated the series Teen; one issue says Teen Plus (MA15+).
    let s = seed_series(&db, w.fx.lib_id, "Plus Run", "plus-run", Some("Teen")).await;
    pin_series_rating(&db, s, "user").await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        1,
        "plus-1",
        "Plus One",
        Some("Teen"),
        None,
    )
    .await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        2,
        "plus-2",
        "Plus Two",
        Some("Teen Plus"),
        None,
    )
    .await;
    assert!(capped_opens_series(&w, "plus-run").await);
    let (st, _) = get_json(&w.app, "/api/series/plus-run/issues/plus-1", &w.capped).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = get_json(&w.app, "/api/series/plus-run/issues/plus-2", &w.capped).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, body) = get_json(&w.app, "/api/issues/search?q=Plus", &w.capped).await;
    assert_eq!(names(&body, "title"), vec!["Plus One"], "{body}");
    let (st, _) = get_json(&w.app, "/api/series/plus-run/issues/plus-2", &w.uncapped).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn hide_unrated_counts_a_metron_word_as_a_rating() {
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let lib = w.fx.lib_id.to_string();
    // Stored as Metron's word, pinned so no derivation rewrites it.
    let s = seed_series(&db, w.fx.lib_id, "Worded Run", "worded-run", Some("Mature")).await;
    pin_series_rating(&db, s, "user").await;
    set_access(
        &w,
        &w.capped,
        serde_json::json!({ lib.clone(): "Mature 17+" }),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    assert!(capped_opens_series(&w, "worded-run").await);
    let (_, body) = get_json(&w.app, "/api/series?q=Worded", &w.capped).await;
    assert_eq!(names(&body, "name"), vec!["Worded Run"], "{body}");
    // The truly unrated fixture series stays hidden.
    assert!(!capped_opens_series(&w, "unrated-stories").await);
    // Under a Teen cap the alias ranks above it.
    set_access(
        &w,
        &w.capped,
        serde_json::json!({ lib.clone(): "Teen" }),
        serde_json::json!({ lib.clone(): true }),
    )
    .await;
    assert!(!capped_opens_series(&w, "worded-run").await);
}

#[tokio::test]
async fn clearing_a_series_rating_releases_the_pin_and_rederives() {
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    let s = seed_series(&db, w.fx.lib_id, "Held Run", "held-run", None).await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        1,
        "held-1",
        "Held One",
        Some("Teen"),
        None,
    )
    .await;
    rollup(&db, s).await;
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Teen"));
    let user_pins = || {
        entity::field_provenance::Entity::find()
            .filter(entity::field_provenance::Column::EntityType.eq("series"))
            .filter(entity::field_provenance::Column::EntityId.eq(s.to_string()))
            .filter(entity::field_provenance::Column::Field.eq("age_rating"))
            .filter(entity::field_provenance::Column::SetBy.eq("user"))
            .count(&db)
    };
    // An admin holds it at Mature 17+ (an alias is accepted)...
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/series/held-run",
        &w.admin,
        Some(serde_json::json!({ "age_rating": "Mature" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    rollup(&db, s).await;
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature"));
    assert_eq!(user_pins().await.unwrap(), 1);
    assert!(!capped_opens_series(&w, "held-run").await);
    // ...then clears it: the pin goes and the derived rating is back.
    let (st, body) = raw(
        &w.app,
        Method::PATCH,
        "/api/series/held-run",
        &w.admin,
        Some(serde_json::json!({ "age_rating": null })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let view: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(view["age_rating"], "Teen", "{view}");
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Teen"));
    assert_eq!(user_pins().await.unwrap(), 0);
    assert!(capped_opens_series(&w, "held-run").await);
}

#[tokio::test]
async fn age_rating_edits_must_be_recognised() {
    let w = world().await;
    let check = |body: Vec<u8>, field: &str| {
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let details = v["error"]["details"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            details
                .iter()
                .any(|d| d["field"] == field && d["message"] == "not a recognised age rating"),
            "{v}"
        );
    };
    let cases = [
        (
            "/api/series/teen-tales",
            serde_json::json!({ "age_rating": "PG-13" }),
            "age_rating",
        ),
        (
            "/api/series/teen-tales/issues/teen-2",
            serde_json::json!({ "age_rating": "Sixteen and up" }),
            "age_rating",
        ),
        (
            "/api/me/issues/bulk-metadata",
            serde_json::json!({
                "issue_ids": [w.fx.teen_2],
                "patch": { "age_rating": "R" },
                "mode": "replace",
            }),
            "patch.age_rating",
        ),
    ];
    for (path, body, field) in cases {
        let (st, resp) = raw(&w.app, Method::PATCH, path, &w.admin, Some(body)).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
        check(resp, field);
    }
    // The series PATCH refuses the "no rating" words: clear instead.
    for v in ["Unknown", " rating pending "] {
        let (st, resp) = raw(
            &w.app,
            Method::PATCH,
            "/api/series/teen-tales",
            &w.admin,
            Some(serde_json::json!({ "age_rating": v })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        let body: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(
            body["error"]["details"][0]["message"],
            server::library::age_rating::SERIES_UNRATED_MESSAGE,
            "{body}"
        );
    }
    // Recognised values (rung, alias, unrated word, blank) pass on issues.
    for v in ["Teen", "teen plus", "Unknown", ""] {
        let (st, body) = raw(
            &w.app,
            Method::PATCH,
            "/api/series/teen-tales/issues/teen-2",
            &w.admin,
            Some(serde_json::json!({ "age_rating": v })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v} {}", String::from_utf8_lossy(&body));
    }
}

#[tokio::test]
async fn an_issue_whose_series_rating_cannot_be_read_is_hidden() {
    use server::library::access;
    let w = world().await;
    let state = w.app.state();
    let mut row = entity::issue::Entity::find_by_id(w.fx.teen_2.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let capped = access::for_library_by_id(&state, w.capped.user_id, w.fx.lib_id).await;
    let open = access::for_library_by_id(&state, w.uncapped.user_id, w.fx.lib_id).await;
    assert!(access::issue_allowed(&state, &capped, &row).await);
    // Point the row at a series that does not exist: no rating to check.
    row.series_id = Uuid::now_v7();
    assert!(!access::issue_allowed(&state, &capped, &row).await);
    assert!(
        access::filter_issues(&state, &capped, vec![row.clone()])
            .await
            .is_empty()
    );
    // A grant that hides nothing does not look the series up.
    assert!(access::issue_allowed(&state, &open, &row).await);
    assert_eq!(
        access::filter_issues(&state, &open, vec![row]).await.len(),
        1
    );
}

#[tokio::test]
async fn a_recompute_queued_behind_a_writer_derives_from_its_committed_issues() {
    use sea_orm::{ConnectionTrait, Statement, TransactionTrait};
    let w = world().await;
    let db = Database::connect(&w.app.db_url).await.unwrap();
    let root = w.app._data_dir.path().join("cap-lib");
    // A stale stored value, one Teen issue and one unrated issue.
    let s = seed_series(&db, w.fx.lib_id, "Race Run", "race-run", Some("Everyone")).await;
    seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        1,
        "race-1",
        "Race One",
        Some("Teen"),
        None,
    )
    .await;
    let two = seed_issue(
        &db,
        &root,
        w.fx.lib_id,
        s,
        2,
        "race-2",
        "Race Two",
        None,
        None,
    )
    .await;

    // A writer (a scan committing a Mature issue and its rollup) holds
    // the series row lock with its work not yet committed.
    let writer_conn = Database::connect(&w.app.db_url).await.unwrap();
    let writer = writer_conn.begin().await.unwrap();
    let backend = writer.get_database_backend();
    writer
        .execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE issues SET age_rating = 'Mature 17+' WHERE id = $1",
            [two.clone().into()],
        ))
        .await
        .unwrap();
    writer
        .execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE series SET age_rating = 'Mature 17+' WHERE id = $1",
            [s.into()],
        ))
        .await
        .unwrap();

    // A recompute starts now (an issue edit elsewhere in the series) and
    // queues behind the writer's row lock.
    let url = w.app.db_url.clone();
    let recompute = tokio::spawn(async move {
        let c = Database::connect(&url).await.unwrap();
        server::library::series_rating::recompute_series_age_rating(&c, s)
            .await
            .unwrap()
    });
    let mut waited = false;
    for _ in 0..200 {
        let row = db
            .query_one_raw(Statement::from_string(
                backend,
                "SELECT count(*)::bigint AS n FROM pg_stat_activity \
                  WHERE datname = current_database() AND wait_event_type = 'Lock'",
            ))
            .await
            .unwrap()
            .unwrap();
        if row.try_get::<i64>("", "n").unwrap() > 0 {
            waited = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(waited, "the recompute never queued behind the writer");
    writer.commit().await.unwrap();
    recompute.await.unwrap();

    // It derives from the writer's committed issue, never writing back the
    // Teen it would have read before the lock.
    assert_eq!(series_rating(&db, s).await.as_deref(), Some("Mature 17+"));
}
