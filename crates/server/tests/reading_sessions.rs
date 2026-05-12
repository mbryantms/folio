//! M6a: integration coverage for `/me/reading-sessions` (POST/GET) and
//! `/me/reading-stats`. Hits the real Postgres harness via testcontainers;
//! seeds a library + series + issue, exercises the heartbeat round-trip,
//! threshold gating, opt-out, clock-skew rejection, ACL, list pagination,
//! and the stats aggregation (totals + per-day + streak).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    reading_session::Entity as ReadingSessionEntity,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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

async fn post(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

async fn patch(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
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
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[derive(Default)]
struct SeedExtras<'a> {
    genre: Option<&'a str>,
    tags: Option<&'a str>,
    publisher: Option<&'a str>,
    series_publisher: Option<&'a str>,
    issue_title: Option<&'a str>,
    issue_number: Option<&'a str>,
}

/// Seed a library + series + one issue. Returns (library_id, series_id, issue_id).
async fn seed_one(app: &TestApp, name: &str) -> (Uuid, Uuid, String) {
    seed_with(app, name, SeedExtras::default()).await
}

async fn seed_with(app: &TestApp, name: &str, extras: SeedExtras<'_>) -> (Uuid, Uuid, String) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
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

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(format!("Series {name}")),
        normalized_name: Set(normalize_name(&format!("Series {name}"))),
        year: Set(None),
        volume: Set(None),
        publisher: Set(extras.series_publisher.map(str::to_owned)),
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

    let issue_id = format!("{:0>64}", format!("{:x}", lib_id.as_u128()));
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        file_path: Set(format!("/tmp/{name}/issue.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(extras.issue_title.map(str::to_owned)),
        sort_number: Set(Some(1.0)),
        number_raw: Set(extras.issue_number.map(str::to_owned)),
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
        tags: Set(extras.tags.map(str::to_owned)),
        genre: Set(extras.genre.map(str::to_owned)),
        writer: Set(None),
        penciller: Set(None),
        inker: Set(None),
        colorist: Set(None),
        letterer: Set(None),
        cover_artist: Set(None),
        editor: Set(None),
        translator: Set(None),
        publisher: Set(extras.publisher.map(str::to_owned)),
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

    (lib_id, series_id, issue_id)
}

fn upsert_body(client_session_id: &str, issue_id: &str, started_at: &str) -> String {
    serde_json::json!({
        "client_session_id": client_session_id,
        "issue_id": issue_id,
        "started_at": started_at,
        "active_ms": 60_000,
        "distinct_pages_read": 5,
        "page_turns": 6,
        "start_page": 0,
        "end_page": 5,
        "device": "desktop",
        "view_mode": "single",
    })
    .to_string()
}

#[tokio::test]
async fn upsert_creates_then_updates_idempotent() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;
    let (_lib, series_id, issue_id) = seed_one(&app, "alice").await;
    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();

    // First write — should be 201 with a fresh row.
    let (status, body) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c1", &issue_id, &started),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body={body}");
    let session_id_1 = body["id"].as_str().unwrap().to_owned();
    assert_eq!(body["issue_id"], issue_id);
    assert_eq!(body["series_id"], series_id.to_string());
    assert_eq!(body["distinct_pages_read"], 5);
    assert!(body["ended_at"].is_null());

    // Second write with the SAME client_session_id and bigger counters —
    // 200, same row, monotonic counters advance.
    let body2 = serde_json::json!({
        "client_session_id": "c1",
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 90_000,
        "distinct_pages_read": 9,
        "page_turns": 12,
        "start_page": 0,
        "end_page": 9,
    })
    .to_string();
    let (status, body) = post(&app, &auth, "/me/reading-sessions", &body2).await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["id"].as_str().unwrap(), session_id_1, "same row");
    assert_eq!(body["active_ms"], 90_000);
    assert_eq!(body["distinct_pages_read"], 9);
    assert_eq!(body["furthest_page"], 9);

    // Heartbeat that arrives with REGRESSED counters — server takes the max,
    // so the row stays at the higher values.
    let body3 = serde_json::json!({
        "client_session_id": "c1",
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 70_000,
        "distinct_pages_read": 4,
        "page_turns": 5,
        "start_page": 1,
        "end_page": 4,
    })
    .to_string();
    let (status, body) = post(&app, &auth, "/me/reading-sessions", &body3).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["active_ms"], 90_000, "max wins");
    assert_eq!(body["distinct_pages_read"], 9);
    assert_eq!(body["start_page"], 0, "min wins on start_page");
}

#[tokio::test]
async fn final_flush_below_threshold_is_discarded() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "bob").await;
    let started = (Utc::now() - Duration::seconds(60)).to_rfc3339();
    let ended = Utc::now().to_rfc3339();

    // Final-flush with active_ms below user's reading_min_active_ms (default 30s).
    let body = serde_json::json!({
        "client_session_id": "c-tiny",
        "issue_id": issue_id,
        "started_at": started,
        "ended_at": ended,
        "active_ms": 5_000,
        "distinct_pages_read": 1,
        "page_turns": 1,
        "start_page": 0,
        "end_page": 0,
    })
    .to_string();
    let (status, _) = post(&app, &auth, "/me/reading-sessions", &body).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // No row was persisted.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = ReadingSessionEntity::find()
        .filter(entity::reading_session::Column::UserId.eq(auth.user_id))
        .all(&db)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn opt_out_silently_discards() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "carol@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "carol").await;

    // Toggle activity tracking off via PATCH /me/preferences.
    let (s, _) = patch(
        &app,
        &auth,
        "/me/preferences",
        r#"{"activity_tracking_enabled": false}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    let (status, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c-optout", &issue_id, &started),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let n = ReadingSessionEntity::find().all(&db).await.unwrap().len();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn rejects_clock_skew_and_invalid_inputs() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dave@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "dave").await;

    let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let (s, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c-future", &issue_id, &future),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let too_old = (Utc::now() - Duration::days(60)).to_rfc3339();
    let (s, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c-old", &issue_id, &too_old),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Empty client_session_id.
    let started = (Utc::now() - Duration::seconds(60)).to_rfc3339();
    let (s, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("", &issue_id, &started),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Inverted page range.
    let body = serde_json::json!({
        "client_session_id": "c-bad",
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 60_000,
        "distinct_pages_read": 5,
        "page_turns": 6,
        "start_page": 10,
        "end_page": 5,
    })
    .to_string();
    let (s, _) = post(&app, &auth, "/me/reading-sessions", &body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_issue_returns_404() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ed@example.com").await;
    let started = (Utc::now() - Duration::seconds(60)).to_rfc3339();
    let bogus = format!("{:0>64}", "deadbeef");
    let (s, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c-bogus", &bogus, &started),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_enriches_sessions_with_issue_and_series_labels() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "lara@example.com").await;
    let (_lib, _series, issue_id) = seed_with(
        &app,
        "lara",
        SeedExtras {
            issue_title: Some("The Will"),
            issue_number: Some("1"),
            ..SeedExtras::default()
        },
    )
    .await;

    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    let (s, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("c-label", &issue_id, &started),
    )
    .await;
    assert!(s.is_success());

    let (s, body) = get(&app, &auth, "/me/reading-sessions").await;
    assert_eq!(s, StatusCode::OK);
    let row = &body["records"][0];
    assert_eq!(row["issue_title"].as_str().unwrap(), "The Will");
    assert_eq!(row["issue_number"].as_str().unwrap(), "1");
    assert!(
        row["series_name"]
            .as_str()
            .unwrap()
            .starts_with("Series lara")
    );
}

#[tokio::test]
async fn list_filters_by_series() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "fran@example.com").await;
    let (_lib_a, series_a, issue_a) = seed_one(&app, "lib-a").await;
    let (_lib_b, series_b, issue_b) = seed_one(&app, "lib-b").await;

    let started = (Utc::now() - Duration::seconds(60)).to_rfc3339();
    for (cs, iss) in [("a1", &issue_a), ("a2", &issue_a), ("b1", &issue_b)] {
        let (s, _) = post(
            &app,
            &auth,
            "/me/reading-sessions",
            &upsert_body(cs, iss, &started),
        )
        .await;
        assert!(s.is_success());
    }

    let (s, body) = get(&app, &auth, "/me/reading-sessions").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["records"].as_array().unwrap().len(), 3);

    let (s, body) = get(
        &app,
        &auth,
        &format!("/me/reading-sessions?series_id={series_a}"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["records"].as_array().unwrap().len(), 2);

    let (s, body) = get(
        &app,
        &auth,
        &format!("/me/reading-sessions?series_id={series_b}"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["records"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn list_paginates_via_cursor() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "gina@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "gina").await;
    // Stagger started_at so the cursor break-point is unambiguous.
    for i in 0..5 {
        let started = (Utc::now() - Duration::seconds(60 * (i + 1))).to_rfc3339();
        let (s, _) = post(
            &app,
            &auth,
            "/me/reading-sessions",
            &upsert_body(&format!("p{i}"), &issue_id, &started),
        )
        .await;
        assert!(s.is_success());
    }

    let (_, body) = get(&app, &auth, "/me/reading-sessions?limit=2").await;
    let records = body["records"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    let cursor = body["next_cursor"].as_str().unwrap().to_owned();

    let (_, body) = get(
        &app,
        &auth,
        &format!("/me/reading-sessions?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(body["records"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn stats_aggregates_totals_per_day_and_streak() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "hank@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "hank").await;
    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();

    let (_, _) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("h1", &issue_id, &started),
    )
    .await;
    let body2 = serde_json::json!({
        "client_session_id": "h2",
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 120_000,
        "distinct_pages_read": 12,
        "page_turns": 14,
        "start_page": 0,
        "end_page": 12,
    })
    .to_string();
    let (_, _) = post(&app, &auth, "/me/reading-sessions", &body2).await;

    let (s, body) = get(&app, &auth, "/me/reading-stats?range=30d").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["range"], "30d");
    assert_eq!(body["totals"]["sessions"], 2);
    assert_eq!(body["totals"]["active_ms"], 60_000 + 120_000);
    assert_eq!(body["totals"]["distinct_issues"], 1);
    assert_eq!(body["totals"]["days_active"], 1);
    // Streak: today active → current ≥ 1, longest ≥ 1.
    assert!(body["totals"]["current_streak"].as_i64().unwrap() >= 1);
    assert!(body["totals"]["longest_streak"].as_i64().unwrap() >= 1);
    let per_day = body["per_day"].as_array().unwrap();
    assert_eq!(per_day.len(), 1, "single day bucket");

    // Bad range → 400.
    let (s, _) = get(&app, &auth, "/me/reading-stats?range=bogus").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_top_n_rankings() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kit@example.com").await;
    // Issue A: action / horror, publisher Wonder
    let (_lib_a, _series_a, issue_a) = seed_with(
        &app,
        "kit-a",
        SeedExtras {
            genre: Some("Action,Horror"),
            tags: Some("zombies, nightmare"),
            publisher: Some("Wonder"),
            series_publisher: Some("Wonder"),
            ..SeedExtras::default()
        },
    )
    .await;
    // Issue B: action only, publisher Acme (different series → top_publishers picks both up)
    let (_lib_b, _series_b, issue_b) = seed_with(
        &app,
        "kit-b",
        SeedExtras {
            genre: Some("Action"),
            tags: Some("zombies"),
            publisher: None,
            series_publisher: Some("Acme"),
            ..SeedExtras::default()
        },
    )
    .await;

    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    // Issue A gets a heavier session — top entries should win on active_ms.
    let body_a = serde_json::json!({
        "client_session_id": "k-a",
        "issue_id": issue_a,
        "started_at": started,
        "active_ms": 600_000,
        "distinct_pages_read": 20,
        "page_turns": 22,
        "start_page": 0,
        "end_page": 19,
    })
    .to_string();
    let (s, _) = post(&app, &auth, "/me/reading-sessions", &body_a).await;
    assert!(s.is_success());
    let body_b = serde_json::json!({
        "client_session_id": "k-b",
        "issue_id": issue_b,
        "started_at": started,
        "active_ms": 60_000,
        "distinct_pages_read": 6,
        "page_turns": 6,
        "start_page": 0,
        "end_page": 5,
    })
    .to_string();
    let (s, _) = post(&app, &auth, "/me/reading-sessions", &body_b).await;
    assert!(s.is_success());

    let (s, body) = get(&app, &auth, "/me/reading-stats?range=30d").await;
    assert_eq!(s, StatusCode::OK);

    let top_series = body["top_series"].as_array().unwrap();
    assert_eq!(top_series.len(), 2);
    // Heavier session ordered first.
    assert_eq!(top_series[0]["active_ms"], 600_000);
    assert_eq!(top_series[1]["active_ms"], 60_000);

    let genres = body["top_genres"].as_array().unwrap();
    let action = genres.iter().find(|g| g["name"] == "Action").unwrap();
    assert_eq!(action["sessions"], 2, "Action appears in both issues");
    assert_eq!(action["active_ms"], 660_000);
    let horror = genres.iter().find(|g| g["name"] == "Horror").unwrap();
    assert_eq!(horror["sessions"], 1);

    let tags = body["top_tags"].as_array().unwrap();
    let zombies = tags.iter().find(|t| t["name"] == "zombies").unwrap();
    assert_eq!(zombies["sessions"], 2);

    let publishers = body["top_publishers"].as_array().unwrap();
    let wonder = publishers.iter().find(|p| p["name"] == "Wonder").unwrap();
    let acme = publishers.iter().find(|p| p["name"] == "Acme").unwrap();
    assert_eq!(wonder["active_ms"], 600_000);
    assert_eq!(acme["active_ms"], 60_000);

    // Issue-scoped stats: every top-N is empty (the issue's own genres
    // and tags are already on its Genres tab; one series + one publisher
    // are implied).
    let (_, body_issue) = get(
        &app,
        &auth,
        &format!("/me/reading-stats?range=30d&issue_id={issue_a}"),
    )
    .await;
    assert_eq!(body_issue["top_series"].as_array().unwrap().len(), 0);
    assert_eq!(body_issue["top_publishers"].as_array().unwrap().len(), 0);
    assert_eq!(body_issue["top_genres"].as_array().unwrap().len(), 0);
    assert_eq!(body_issue["top_tags"].as_array().unwrap().len(), 0);

    // Series-scoped stats: top_series + top_publishers are tautological
    // (the series itself / its publisher) so they're suppressed; genres
    // and tags still useful since they vary across issues.
    let series_a = body["top_series"][0]["series_id"].as_str().unwrap();
    let (_, body_series) = get(
        &app,
        &auth,
        &format!("/me/reading-stats?range=30d&series_id={series_a}"),
    )
    .await;
    assert_eq!(body_series["top_series"].as_array().unwrap().len(), 0);
    assert_eq!(body_series["top_publishers"].as_array().unwrap().len(), 0);
    assert!(
        !body_series["top_genres"].as_array().unwrap().is_empty(),
        "genres still emitted at series scope"
    );
}

#[tokio::test]
async fn dangling_session_sweeper_closes_stale_rows() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ivy@example.com").await;
    let (_lib, _series, issue_id) = seed_one(&app, "ivy").await;
    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();

    let (_, body) = post(
        &app,
        &auth,
        "/me/reading-sessions",
        &upsert_body("i1", &issue_id, &started),
    )
    .await;
    let session_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    assert!(body["ended_at"].is_null());

    // Backdate last_heartbeat_at past the sweeper's cutoff.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let row = ReadingSessionEntity::find_by_id(session_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::reading_session::ActiveModel = row.into();
    let stale = (Utc::now() - Duration::minutes(30)).fixed_offset();
    am.last_heartbeat_at = Set(stale);
    am.update(&db).await.unwrap();

    let n = server::jobs::close_dangling_sessions::run(&db)
        .await
        .unwrap();
    assert_eq!(n, 1);

    let row = ReadingSessionEntity::find_by_id(session_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let ended = row.ended_at.expect("sweeper set ended_at");
    // Postgres stores timestamptz at microsecond precision; chrono carries
    // nanoseconds. Compare the resulting microsecond timestamp.
    assert_eq!(
        ended.timestamp_micros(),
        stale.timestamp_micros(),
        "ended_at should match the backdated heartbeat"
    );
}

#[tokio::test]
async fn timezone_validation_rejects_unknown_zone() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "joel@example.com").await;
    let (s, body) = patch(
        &app,
        &auth,
        "/me/preferences",
        r#"{"timezone": "Mars/Olympus_Mons"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "body={body}");
}
