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
        role: Set("user".into()),
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
    }
    .insert(&db)
    .await
    .unwrap();

    async fn series(
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
    async fn issue(
        db: &sea_orm::DatabaseConnection,
        root: &std::path::Path,
        lib_id: Uuid,
        series_id: Uuid,
        n: u8,
        slug: &str,
        title: &str,
        rating: Option<&str>,
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
            state: Set("active".into()),
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
            special_type: Set(None),
            hash_algorithm: Set(0),
            metroninfo_present: Set(None),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(Some(0)),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
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

    let teen_series = series(&db, lib_id, "Teen Tales", "teen-tales", Some("Teen")).await;
    let mature_series = series(
        &db,
        lib_id,
        "Mature Matters",
        "mature-matters",
        Some("Mature 17+"),
    )
    .await;
    let unrated_series = series(&db, lib_id, "Unrated Stories", "unrated-stories", None).await;

    let teen_1 = issue(
        &db,
        &root,
        lib_id,
        teen_series,
        1,
        "teen-1",
        "Teen One",
        None,
    )
    .await;
    let teen_2 = issue(
        &db,
        &root,
        lib_id,
        teen_series,
        2,
        "teen-2",
        "Teen Two",
        None,
    )
    .await;
    let mature_1 = issue(
        &db,
        &root,
        lib_id,
        mature_series,
        1,
        "mature-1",
        "Mature One",
        None,
    )
    .await;
    let mature_2 = issue(
        &db,
        &root,
        lib_id,
        mature_series,
        2,
        "mature-2",
        "Mature Two",
        None,
    )
    .await;
    let unrated_1 = issue(
        &db,
        &root,
        lib_id,
        unrated_series,
        1,
        "unrated-1",
        "Unrated One",
        None,
    )
    .await;
    // Issue-level override: the series is unrated but this issue is not.
    let unrated_mature_2 = issue(
        &db,
        &root,
        lib_id,
        unrated_series,
        2,
        "unrated-2",
        "Unrated Mature Two",
        Some("mature 17+"),
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
