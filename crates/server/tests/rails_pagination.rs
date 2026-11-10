//! Cursor pagination of the progress rails' "see all" walks:
//! `/me/continue-reading`, `/me/recent-history` and `/me/on-deck`
//! (`?cursor=&limit=`, `next_cursor`, `total` on the first page only).
//!
//! Covers:
//!   - each endpoint walks past the old 24-card cap with no repeat and no
//!     gap, in the documented total order (ties on the timestamp break on
//!     the issue id);
//!   - `total` rides the first page of a paged request only;
//!   - the home-rail preview (no params) still returns the short first
//!     page and, for On deck, no cursor;
//!   - a reader with an age cap never receives an issue above it on any
//!     page;
//!   - a malformed cursor is a 400.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use std::collections::HashSet;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
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
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn demote_to_user(db: &DatabaseConnection, user_id: Uuid) {
    let user = UserEntity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set(entity::user::UserRole::User);
    am.update(db).await.unwrap();
}

async fn get(app: &TestApp, uri: &str, auth: &Authed) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_library(db: &DatabaseConnection, tag: &str) -> Uuid {
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {tag}")),
        root_path: Set(format!("/tmp/{tag}-{lib_id}")),
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
    .insert(db)
    .await
    .unwrap();
    lib_id
}

async fn seed_series(db: &DatabaseConnection, lib_id: Uuid, slug: &str) -> Uuid {
    let series_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(format!("Series {slug}")),
        normalized_name: Set(normalize_name(&format!("Series {slug}"))),
        year: Set(Some(2020)),
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
        slug: Set(format!("{slug}-series")),
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
    series_id
}

/// Insert issue `n` of a series; the id is derived from the series id and
/// `n`, so ids within one series sort by `n`.
async fn seed_issue(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    n: u8,
    age_rating: Option<&str>,
) -> String {
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), n);
    let slug = format!("{}-{n}", series_id.simple());
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug.clone()),
        file_path: Set(format!("/tmp/{slug}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set(entity::issue::IssueState::Active),
        content_hash: Set(issue_id.clone()),
        title: Set(Some(format!("Issue {n}"))),
        sort_number: Set(Some(f64::from(n))),
        number_raw: Set(Some(format!("{n}"))),
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
        age_rating: Set(age_rating.map(str::to_owned)),
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
    issue_id
}

async fn grant(db: &DatabaseConnection, user_id: Uuid, library_id: Uuid, cap: Option<&str>) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn write_progress(
    db: &DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    finished: bool,
    when: DateTime<FixedOffset>,
) {
    progress_record::ActiveModel {
        user_id: Set(user_id),
        issue_id: Set(issue_id.to_owned()),
        last_page: Set(if finished { 19 } else { 3 }),
        percent: Set(if finished { 1.0 } else { 0.15 }),
        finished: Set(finished),
        finished_at: Set(finished.then_some(when)),
        updated_at: Set(when),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

fn base_time() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap()
}

/// Walk every page of `path` at `limit`, asserting `total` rides the first
/// page only. Returns the issue ids in order and the first page's `total`.
async fn walk(app: &TestApp, auth: &Authed, path: &str, limit: u32) -> (Vec<String>, u64) {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut total = None;
    for page_no in 0.. {
        assert!(page_no < 100, "{path}: pagination does not terminate");
        let uri = match &cursor {
            Some(c) => format!("{path}?limit={limit}&cursor={c}"),
            None => format!("{path}?limit={limit}"),
        };
        let (status, body) = get(app, &uri, auth).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let items = body["items"].as_array().unwrap();
        assert!(items.len() <= limit as usize, "{uri}: page over limit");
        if page_no == 0 {
            total = Some(body["total"].as_u64().expect("total on the first page"));
        } else {
            assert!(
                body.get("total").is_none(),
                "{uri}: total only on the first page"
            );
        }
        ids.extend(
            items
                .iter()
                .map(|c| c["issue"]["id"].as_str().unwrap().to_owned()),
        );
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    (ids, total.unwrap())
}

fn assert_no_repeats(ids: &[String], what: &str) {
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{what}: a page repeated an item");
}

#[tokio::test]
async fn continue_reading_walks_every_page_past_the_old_cap() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = register(&app, "page-cr@example.com").await;
    let lib = seed_library(&db, "page-cr").await;
    let series = seed_series(&db, lib, "page-cr").await;
    // 30 in-progress issues. Issues 1..=4 share one timestamp (the tie
    // breaks on issue id, descending); the rest are one minute apart.
    let mut expected: Vec<(DateTime<FixedOffset>, String)> = Vec::new();
    for n in 1..=30u8 {
        let id = seed_issue(&db, lib, series, n, None).await;
        let when = base_time() + Duration::minutes(i64::from(n.max(4)));
        write_progress(&db, user.user_id, &id, false, when).await;
        expected.push((when, id));
    }
    expected.sort_by(|a, b| b.cmp(a));
    let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();

    let (ids, total) = walk(&app, &user, "/api/me/continue-reading", 7).await;
    assert_no_repeats(&ids, "continue-reading");
    assert_eq!(ids, expected, "every card once, newest first, id tie-break");
    assert_eq!(total, 30);

    // Home-rail preview: one request, the short first page, no total.
    let (status, body) = get(&app, "/api/me/continue-reading", &user).await;
    assert_eq!(status, StatusCode::OK);
    let preview: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["issue"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(preview.len(), 24);
    assert_eq!(
        preview,
        expected[..24]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert!(body.get("total").is_none(), "the preview carries no total");
    assert!(
        body["next_cursor"].is_string(),
        "more cards follow the preview"
    );

    let (status, _) = get(&app, "/api/me/continue-reading?cursor=not-a-cursor", &user).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn recent_history_walks_every_page_past_the_old_cap() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = register(&app, "page-rh@example.com").await;
    let lib = seed_library(&db, "page-rh").await;
    let series = seed_series(&db, lib, "page-rh").await;
    let mut expected = Vec::new();
    for n in 1..=31u8 {
        let id = seed_issue(&db, lib, series, n, None).await;
        let when = base_time() + Duration::minutes(i64::from(n));
        // Mostly finished, every fifth still in progress.
        write_progress(&db, user.user_id, &id, n % 5 != 0, when).await;
        expected.push(id);
    }
    expected.reverse();

    let (ids, total) = walk(&app, &user, "/api/me/recent-history", 10).await;
    assert_no_repeats(&ids, "recent-history");
    assert_eq!(ids, expected);
    assert_eq!(total, 31);

    let (_, body) = get(&app, "/api/me/recent-history", &user).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 24);
    assert!(body.get("total").is_none());
}

/// `n` series, each with #1 finished and #2 unread, so each contributes
/// one `series_next` card (#2). Returns the expected card ids, newest
/// activity first.
async fn seed_on_deck_series(
    db: &DatabaseConnection,
    user_id: Uuid,
    lib: Uuid,
    tag: &str,
    n: u8,
    rating_of_next: impl Fn(u8) -> Option<&'static str>,
) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for k in 0..n {
        let series = seed_series(db, lib, &format!("{tag}-{k}")).await;
        let first = seed_issue(db, lib, series, 1, None).await;
        let rating = rating_of_next(k);
        let next = seed_issue(db, lib, series, 2, rating).await;
        write_progress(
            db,
            user_id,
            &first,
            true,
            base_time() + Duration::minutes(i64::from(k)),
        )
        .await;
        out.push((next, rating.is_some()));
    }
    out.reverse();
    out
}

#[tokio::test]
async fn on_deck_walks_every_page_past_the_old_bound() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = register(&app, "page-od@example.com").await;
    let lib = seed_library(&db, "page-od").await;
    // 45 series: past the old 24-card cut AND the old 40-candidate bound.
    let expected: Vec<String> = seed_on_deck_series(&db, user.user_id, lib, "od", 45, |_| None)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    let (ids, total) = walk(&app, &user, "/api/me/on-deck", 10).await;
    assert_no_repeats(&ids, "on-deck");
    assert_eq!(ids, expected, "every card once, most recent activity first");
    assert_eq!(total, 45);
    let (_, first) = get(&app, "/api/me/on-deck?limit=10", &user).await;
    assert!(
        first.get("bounded_to").is_none(),
        "45 series is under the paged bound"
    );

    // Home-rail preview: unchanged — the 24 most recent of the rail's
    // 40 candidates, no cursor, no total.
    let (status, body) = get(&app, "/api/me/on-deck", &user).await;
    assert_eq!(status, StatusCode::OK);
    let preview: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["issue"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        preview,
        expected[..24]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert!(body["next_cursor"].is_null());
    assert!(body.get("total").is_none());

    let (status, _) = get(&app, "/api/me/on-deck?cursor=bad", &user).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn capped_reader_never_gets_a_hidden_issue_on_any_page() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = register(&app, "page-cap@example.com").await;
    demote_to_user(&db, user.user_id).await;
    let lib = seed_library(&db, "page-cap").await;
    grant(&db, user.user_id, lib, Some("Teen")).await;

    // Continue reading / Recently read: 30 in-progress issues, every third
    // rated above the cap.
    let series = seed_series(&db, lib, "cap-cr").await;
    let mut hidden = HashSet::new();
    let mut visible = HashSet::new();
    for n in 1..=30u8 {
        let rating = (n % 3 == 0).then_some("Mature 17+");
        let id = seed_issue(&db, lib, series, n, rating).await;
        write_progress(
            &db,
            user.user_id,
            &id,
            false,
            base_time() + Duration::hours(1) + Duration::minutes(i64::from(n)),
        )
        .await;
        if rating.is_some() {
            hidden.insert(id);
        } else {
            visible.insert(id);
        }
    }
    for path in ["/api/me/continue-reading", "/api/me/recent-history"] {
        let (ids, total) = walk(&app, &user, path, 4).await;
        assert_no_repeats(&ids, path);
        let got: HashSet<String> = ids.into_iter().collect();
        assert!(
            got.is_disjoint(&hidden),
            "{path}: an above-cap issue leaked"
        );
        assert_eq!(got, visible, "{path}: every visible card, once");
        assert_eq!(
            total as usize,
            visible.len(),
            "{path}: total counts visible cards"
        );
    }

    // On deck: 28 series whose next issue is above the cap for every
    // other series.
    let cards = seed_on_deck_series(&db, user.user_id, lib, "cap-od", 28, |k| {
        (k % 2 == 0).then_some("Mature 17+")
    })
    .await;
    let (ids, _) = walk(&app, &user, "/api/me/on-deck", 5).await;
    assert_no_repeats(&ids, "on-deck");
    let got: HashSet<String> = ids.into_iter().collect();
    for (id, is_hidden) in cards {
        if is_hidden {
            assert!(!got.contains(&id), "on-deck: an above-cap issue leaked");
        } else {
            assert!(got.contains(&id), "on-deck: a visible card is missing");
        }
    }
}
