//! `GET /me/reading-log` — event-union feed.
//!
//! Covers the four event kinds, cursor pagination, range + kind +
//! series + library filters, ACL filtering, and the
//! session-threshold drop rule (active_ms < 60s OR pages == 0).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access, marker,
    progress_record::ActiveModel as ProgressAM,
    reading_session,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn get_json(app: &TestApp, uri: &str, auth: &Authed) -> (StatusCode, serde_json::Value) {
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
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Seed one library + one series + `count` issues. Returns (library_id,
/// series_id, issue_ids[]).
async fn seed_library_with_issues(
    app: &TestApp,
    label: &str,
    count: usize,
) -> (Uuid, Uuid, Vec<String>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("rl-{label}")),
        root_path: Set(format!("/tmp/rl-{lib_id}")),
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
    }
    .insert(&db)
    .await
    .unwrap();

    let sid = Uuid::now_v7();
    let series_name = format!("{label}-series");
    SeriesAM {
        id: Set(sid),
        library_id: Set(lib_id),
        name: Set(series_name.clone()),
        normalized_name: Set(normalize_name(&series_name)),
        year: Set(Some(2020)),
        volume: Set(Some(1)),
        publisher: Set(Some("TestPub".into())),
        imprint: Set(None),
        status: Set("ended".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(format!("{label}-2020")),
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
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let mut issue_ids: Vec<String> = Vec::with_capacity(count);
    for n in 0..count {
        let id = format!("{:0>62}{:02x}", sid.simple(), n as u8);
        IssueAM {
            id: Set(id.clone()),
            library_id: Set(lib_id),
            series_id: Set(sid),
            slug: Set(format!("{label}-{n}")),
            file_path: Set(format!("/tmp/rl/{label}-{n}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(id.clone()),
            title: Set(None),
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
            writer: Set(Some("J. Writer".into())),
            penciller: Set(Some("P. Penciller".into())),
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
            comicinfo_count: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
        issue_ids.push(id);
    }
    (lib_id, sid, issue_ids)
}

async fn mark_finished_at(
    app: &TestApp,
    user: Uuid,
    issue_id: &str,
    at: chrono::DateTime<chrono::FixedOffset>,
) {
    let db = Database::connect(&app.db_url).await.unwrap();
    ProgressAM {
        user_id: Set(user),
        issue_id: Set(issue_id.into()),
        last_page: Set(19),
        percent: Set(1.0),
        finished: Set(true),
        finished_at: Set(Some(at)),
        updated_at: Set(at),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_session(
    app: &TestApp,
    user: Uuid,
    series_id: Uuid,
    issue_id: &str,
    started_at: chrono::DateTime<chrono::FixedOffset>,
    ended_at: chrono::DateTime<chrono::FixedOffset>,
    active_ms: i64,
    distinct_pages_read: i32,
) {
    let db = Database::connect(&app.db_url).await.unwrap();
    reading_session::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user),
        issue_id: Set(issue_id.into()),
        series_id: Set(series_id),
        client_session_id: Set(Uuid::new_v4().to_string()),
        started_at: Set(started_at),
        ended_at: Set(Some(ended_at)),
        last_heartbeat_at: Set(ended_at),
        active_ms: Set(active_ms),
        distinct_pages_read: Set(distinct_pages_read),
        page_turns: Set(distinct_pages_read),
        start_page: Set(0),
        end_page: Set(distinct_pages_read.saturating_sub(1).max(0)),
        furthest_page: Set(distinct_pages_read.saturating_sub(1).max(0)),
        device: Set(Some("desktop".into())),
        view_mode: Set(Some("single".into())),
        client_meta: Set(serde_json::json!({})),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn insert_marker(
    app: &TestApp,
    user: Uuid,
    series_id: Uuid,
    issue_id: &str,
    kind: &str,
    body: Option<&str>,
    at: chrono::DateTime<chrono::FixedOffset>,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    marker::ActiveModel {
        id: Set(id),
        user_id: Set(user),
        series_id: Set(series_id),
        issue_id: Set(issue_id.into()),
        page_index: Set(0),
        kind: Set(kind.into()),
        is_favorite: Set(false),
        tags: Set(Vec::new()),
        region: Set(None),
        selection: Set(None),
        body: Set(body.map(str::to_owned)),
        color: Set(None),
        created_at: Set(at),
        updated_at: Set(at),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn grant_library(app: &TestApp, user: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user),
        library_id: Set(library_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

// ─────────── Tests ───────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_log_returns_empty_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "empty@rl.test").await;
    let (status, body) = get_json(&app, "/api/me/reading-log", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().unwrap().len(), 0);
    assert!(body["next_cursor"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_finished_event_surfaces_in_reverse_chrono() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ifin@rl.test").await;
    let (_lib, _sid, ids) = seed_library_with_issues(&app, "ifin", 3).await;
    // First user is admin (first-user-becomes-admin Folio convention) →
    // no library_user_access grant needed.
    let base = Utc::now().fixed_offset();
    mark_finished_at(&app, auth.user_id, &ids[0], base - Duration::hours(2)).await;
    mark_finished_at(&app, auth.user_id, &ids[1], base - Duration::hours(1)).await;
    mark_finished_at(&app, auth.user_id, &ids[2], base).await;

    let (status, body) = get_json(&app, "/api/me/reading-log", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 4); // 3 issue_finished + 1 series_finished
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    // First event is the series_finished at MAX(finished_at) == base,
    // followed by the three issue_finished in DESC order.
    assert_eq!(kinds[0], "series_finished");
    assert_eq!(kinds[1], "issue_finished");
    assert_eq!(events[1]["issue"]["id"].as_str().unwrap(), ids[2]);
    assert_eq!(events[2]["issue"]["id"].as_str().unwrap(), ids[1]);
    assert_eq!(events[3]["issue"]["id"].as_str().unwrap(), ids[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_paginates_full_history_without_duplicates() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cursor@rl.test").await;
    let (_lib, _sid, ids) = seed_library_with_issues(&app, "cursor", 8).await;
    let base = Utc::now().fixed_offset();
    for (i, id) in ids.iter().enumerate() {
        mark_finished_at(&app, auth.user_id, id, base - Duration::hours(i as i64)).await;
    }
    // 8 issue_finished + 1 series_finished == 9 events. limit=3 should
    // walk in 3 pages with a 4th terminating empty.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let url = match &cursor {
            Some(c) => format!("/api/me/reading-log?limit=3&cursor={c}"),
            None => "/api/me/reading-log?limit=3".to_owned(),
        };
        let (status, body) = get_json(&app, &url, &auth).await;
        assert_eq!(status, StatusCode::OK);
        for e in body["events"].as_array().unwrap() {
            seen.push(e["id"].as_str().unwrap().to_owned());
        }
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    // No duplicates.
    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "cursor walk produced duplicate event ids: {seen:?}"
    );
    assert_eq!(seen.len(), 9, "expected 9 events total, got {seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kind_filter_narrows_to_single_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kind@rl.test").await;
    let (_lib, sid, ids) = seed_library_with_issues(&app, "kind", 3).await;
    let base = Utc::now().fixed_offset();
    mark_finished_at(&app, auth.user_id, &ids[0], base - Duration::hours(2)).await;
    insert_marker(
        &app,
        auth.user_id,
        sid,
        &ids[1],
        "bookmark",
        None,
        base - Duration::hours(1),
    )
    .await;
    insert_session(
        &app,
        auth.user_id,
        sid,
        &ids[2],
        base - Duration::minutes(30),
        base,
        120_000, // 2 min
        5,
    )
    .await;

    let (_, body) = get_json(&app, "/api/me/reading-log?kind=marker_created", &auth).await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"].as_str().unwrap(), "marker_created");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_below_threshold_is_dropped() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "thresh@rl.test").await;
    let (_lib, sid, ids) = seed_library_with_issues(&app, "thresh", 3).await;
    let base = Utc::now().fixed_offset();
    // Three sessions: one under 60s, one with 0 pages, one healthy.
    insert_session(
        &app,
        auth.user_id,
        sid,
        &ids[0],
        base - Duration::seconds(30),
        base,
        30_000, // 30s — dropped
        5,
    )
    .await;
    insert_session(
        &app,
        auth.user_id,
        sid,
        &ids[1],
        base - Duration::minutes(5),
        base,
        300_000,
        0, // 0 pages — dropped
    )
    .await;
    insert_session(
        &app,
        auth.user_id,
        sid,
        &ids[2],
        base - Duration::minutes(10),
        base - Duration::minutes(5),
        300_000,
        8, // healthy
    )
    .await;

    let (_, body) = get_json(&app, "/api/me/reading-log?kind=session_completed", &auth).await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(
        events.len(),
        1,
        "only the healthy session should surface, got: {body:#?}"
    );
    assert_eq!(events[0]["issue"]["id"].as_str().unwrap(), ids[2]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_to_range_filters_events() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "range@rl.test").await;
    let (_lib, _sid, ids) = seed_library_with_issues(&app, "range", 3).await;
    let base = Utc::now().fixed_offset();
    mark_finished_at(&app, auth.user_id, &ids[0], base - Duration::days(10)).await;
    mark_finished_at(&app, auth.user_id, &ids[1], base - Duration::days(5)).await;
    mark_finished_at(&app, auth.user_id, &ids[2], base - Duration::days(1)).await;

    // Window [-7d, -3d): expect only ids[1].
    let from = (base - Duration::days(7)).to_rfc3339();
    let to = (base - Duration::days(3)).to_rfc3339();
    let url = format!(
        "/api/me/reading-log?kind=issue_finished&from={}&to={}",
        urlencoding::encode(&from),
        urlencoding::encode(&to),
    );
    let (_, body) = get_json(&app, &url, &auth).await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["issue"]["id"].as_str().unwrap(), ids[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_finished_fires_only_when_all_active_issues_are_finished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ser@rl.test").await;
    let (_lib, _sid, ids) = seed_library_with_issues(&app, "ser", 3).await;
    let base = Utc::now().fixed_offset();
    // Finish 2 of 3 → no series_finished yet.
    mark_finished_at(&app, auth.user_id, &ids[0], base - Duration::hours(2)).await;
    mark_finished_at(&app, auth.user_id, &ids[1], base - Duration::hours(1)).await;
    let (_, body) = get_json(&app, "/api/me/reading-log?kind=series_finished", &auth).await;
    assert_eq!(body["events"].as_array().unwrap().len(), 0);

    // Finish the third → series_finished surfaces with occurred_at == base.
    mark_finished_at(&app, auth.user_id, &ids[2], base).await;
    let (_, body) = get_json(&app, "/api/me/reading-log?kind=series_finished", &auth).await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"].as_str().unwrap(), "series_finished");
    // occurred_at should equal the latest finish; we tolerate
    // microsecond round-trips by comparing only to second precision.
    let occurred = events[0]["occurred_at"].as_str().unwrap();
    assert!(
        occurred.starts_with(&base.to_rfc3339()[..19]),
        "expected occurred_at ~= {}, got {occurred}",
        base.to_rfc3339(),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_acl_hides_events_for_inaccessible_libraries() {
    let app = TestApp::spawn().await;
    // First-registered → admin (sees everything).
    let _admin = register(&app, "admin@rl.test").await;
    // Second user → role=user, no library grants by default.
    let user2 = register(&app, "user@rl.test").await;
    let (lib_a, sid_a, ids_a) = seed_library_with_issues(&app, "acl-a", 1).await;
    let (_lib_b, sid_b, ids_b) = seed_library_with_issues(&app, "acl-b", 1).await;
    let base = Utc::now().fixed_offset();
    insert_marker(
        &app,
        user2.user_id,
        sid_a,
        &ids_a[0],
        "bookmark",
        None,
        base,
    )
    .await;
    // Marker on library B — gets filtered out for user2 (no grant).
    insert_marker(
        &app,
        user2.user_id,
        sid_b,
        &ids_b[0],
        "bookmark",
        None,
        base + Duration::seconds(1),
    )
    .await;
    grant_library(&app, user2.user_id, lib_a).await;

    let (_, body) = get_json(&app, "/api/me/reading-log?kind=marker_created", &user2).await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(
        events.len(),
        1,
        "user without grant on library B should see only the lib-A marker, got: {body:#?}"
    );
    assert_eq!(events[0]["issue"]["id"].as_str().unwrap(), ids_a[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_cursor_returns_400() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "badc@rl.test").await;
    let (status, body) =
        get_json(&app, "/api/me/reading-log?cursor=not-a-real-cursor", &auth).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_kind_filter_returns_empty_not_error() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "unk@rl.test").await;
    let (status, body) = get_json(&app, "/api/me/reading-log?kind=sneeze", &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().unwrap().len(), 0);
}
