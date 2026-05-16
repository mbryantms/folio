//! Stats v2: integration coverage for the new admin observability endpoints
//! (`/admin/stats/users`, `/admin/stats/engagement`, `/admin/stats/content`,
//! `/admin/stats/quality`) plus the `exclude_from_aggregates` toggle.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access, reading_session,
    series::{ActiveModel as SeriesAM, normalize_name},
    user as user_entity,
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

async fn seed_library_with_issues(
    app: &TestApp,
    label: &str,
    issue_count: usize,
    publisher: Option<&str>,
) -> (Uuid, Uuid, Vec<String>) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {label}")),
        root_path: Set(format!("/tmp/{label}-{lib_id}")),
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
        name: Set(format!("Series {label}")),
        normalized_name: Set(normalize_name(&format!("Series {label}"))),
        year: Set(None),
        volume: Set(None),
        publisher: Set(publisher.map(str::to_owned)),
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

    let mut issue_ids = Vec::with_capacity(issue_count);
    for n in 0..issue_count {
        let issue_id = format!("{:0>64}", format!("{:x}{:x}", lib_id.as_u128(), n));
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            file_path: Set(format!("/tmp/{label}/issue-{n}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(Some(format!("Issue {n}"))),
            sort_number: Set(Some(n as f64 + 1.0)),
            number_raw: Set(Some((n + 1).to_string())),
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
            publisher: Set(publisher.map(str::to_owned)),
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
        issue_ids.push(issue_id);
    }
    (lib_id, series_id, issue_ids)
}

async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
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

#[allow(clippy::too_many_arguments)]
async fn insert_session(
    app: &TestApp,
    user_id: Uuid,
    issue_id: &str,
    series_id: Uuid,
    started: chrono::DateTime<chrono::FixedOffset>,
    active_ms: i64,
    pages: i32,
    end_page: i32,
    device: Option<&str>,
) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let ended = started + Duration::milliseconds(active_ms);
    reading_session::ActiveModel {
        id: Set(id),
        user_id: Set(user_id),
        issue_id: Set(issue_id.to_owned()),
        series_id: Set(series_id),
        client_session_id: Set(format!("seed-{id}")),
        started_at: Set(started),
        ended_at: Set(Some(ended)),
        last_heartbeat_at: Set(ended),
        active_ms: Set(active_ms),
        distinct_pages_read: Set(pages),
        page_turns: Set(pages + 1),
        start_page: Set(0),
        end_page: Set(end_page),
        furthest_page: Set(end_page),
        device: Set(device.map(str::to_owned)),
        view_mode: Set(Some("single".into())),
        client_meta: Set(serde_json::json!({})),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn set_exclude(app: &TestApp, user_id: Uuid, value: bool) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let row = user_entity::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: user_entity::ActiveModel = row.into();
    am.exclude_from_aggregates = Set(value);
    am.update(&db).await.unwrap();
}

#[tokio::test]
async fn users_list_returns_aggregates_for_each_user() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "demo", 1, Some("DC")).await;
    grant_library(&app, reader.user_id, lib).await;
    let now = Utc::now().fixed_offset();
    insert_session(
        &app,
        reader.user_id,
        &issues[0],
        series,
        now - Duration::hours(1),
        90_000,
        10,
        9,
        Some("mobile"),
    )
    .await;

    let (s, body) = get(&app, &admin, "/api/admin/stats/users").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    let users = body["users"].as_array().expect("users array");
    // 2 users registered.
    assert_eq!(users.len(), 2);
    // Reader is first (more activity); admin has 0 sessions.
    let reader_row = users
        .iter()
        .find(|u| u["user_id"] == reader.user_id.to_string())
        .unwrap();
    assert_eq!(reader_row["sessions_30d"], 1);
    assert_eq!(reader_row["active_ms_30d"], 90_000);
    assert_eq!(reader_row["top_series_name"], "Series demo");
    assert_eq!(reader_row["device_breakdown"][0]["device"], "mobile");
}

#[tokio::test]
async fn users_list_forbidden_for_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(&app, &user, "/api/admin/stats/users").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn engagement_returns_90_day_series_and_devices() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "demo", 1, None).await;
    grant_library(&app, reader.user_id, lib).await;
    let now = Utc::now().fixed_offset();
    insert_session(
        &app,
        reader.user_id,
        &issues[0],
        series,
        now - Duration::hours(2),
        90_000,
        10,
        9,
        Some("desktop"),
    )
    .await;
    insert_session(
        &app,
        reader.user_id,
        &issues[0],
        series,
        now - Duration::days(8),
        90_000,
        10,
        9,
        Some("mobile"),
    )
    .await;

    let (s, body) = get(&app, &admin, "/api/admin/stats/engagement").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    let series_arr = body["series"].as_array().expect("series array");
    assert_eq!(series_arr.len(), 90, "90 daily samples");
    let last = series_arr.last().unwrap();
    // Today: dau ≥ 1, wau ≥ 1, mau ≥ 1.
    assert!(last["dau"].as_i64().unwrap() >= 1);
    assert!(last["wau"].as_i64().unwrap() >= 1);
    assert!(last["mau"].as_i64().unwrap() >= 1);

    let devices = body["devices_30d"].as_array().expect("devices array");
    let names: Vec<&str> = devices
        .iter()
        .map(|d| d["device"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"desktop"));
}

#[tokio::test]
async fn engagement_respects_exclude_flag() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "demo", 1, None).await;
    grant_library(&app, reader.user_id, lib).await;
    insert_session(
        &app,
        reader.user_id,
        &issues[0],
        series,
        Utc::now().fixed_offset() - Duration::hours(1),
        60_000,
        10,
        9,
        Some("desktop"),
    )
    .await;

    // Baseline: reader is counted.
    let (_, before) = get(&app, &admin, "/api/admin/stats/engagement").await;
    let last_before = before["series"].as_array().unwrap().last().unwrap().clone();
    assert!(last_before["dau"].as_i64().unwrap() >= 1);

    // Opt out.
    set_exclude(&app, reader.user_id, true).await;
    let (_, after) = get(&app, &admin, "/api/admin/stats/engagement").await;
    let last_after = after["series"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last_after["dau"], 0);
}

#[tokio::test]
async fn content_endpoint_returns_dead_stock_abandoned_funnel() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    // Read library: someone reads it (counted in abandoned/funnel)
    let (lib_a, series_a, issues_a) =
        seed_library_with_issues(&app, "read", 2, Some("Image")).await;
    grant_library(&app, reader.user_id, lib_a).await;
    // Dead-stock library: no sessions.
    let (_lib_b, _series_b, _issues_b) =
        seed_library_with_issues(&app, "dead", 3, Some("Vault")).await;

    let now = Utc::now().fixed_offset();
    // 3 sessions on issue[0] — never finished (end_page = 4 vs. 19 needed).
    for n in 0..3 {
        insert_session(
            &app,
            reader.user_id,
            &issues_a[0],
            series_a,
            now - Duration::hours(n + 1),
            60_000,
            5,
            4,
            None,
        )
        .await;
    }

    let (s, body) = get(&app, &admin, "/api/admin/stats/content").await;
    assert_eq!(s, StatusCode::OK, "body={body}");

    let dead = body["dead_stock"].as_array().expect("dead_stock");
    let dead_names: Vec<&str> = dead.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(
        dead_names.contains(&"Series dead"),
        "Series dead in dead_stock; got {dead_names:?}"
    );
    assert!(
        !dead_names.contains(&"Series read"),
        "Series read NOT in dead_stock"
    );

    let aban = body["abandoned"].as_array().expect("abandoned");
    let aban_names: Vec<&str> = aban.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(
        aban_names.contains(&"Series read"),
        "Series read flagged abandoned"
    );

    let funnel = body["completion_funnel"].as_array().expect("funnel");
    assert_eq!(funnel.len(), 5);
    let buckets: Vec<&str> = funnel
        .iter()
        .map(|f| f["bucket"].as_str().unwrap())
        .collect();
    assert_eq!(buckets, vec!["0-25", "25-50", "50-75", "75-99", "100"]);
    let zero_to_25 = funnel.iter().find(|f| f["bucket"] == "0-25").unwrap();
    assert!(zero_to_25["issues"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn quality_endpoint_reports_long_sessions_and_metadata() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "demo", 1, None).await;
    grant_library(&app, reader.user_id, lib).await;
    // Suspicious: active_ms > 6h.
    insert_session(
        &app,
        reader.user_id,
        &issues[0],
        series,
        Utc::now().fixed_offset() - Duration::days(1),
        7 * 60 * 60 * 1000, // 7h
        100,
        19,
        None,
    )
    .await;

    let (s, body) = get(&app, &admin, "/api/admin/stats/quality").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    assert!(body["long_sessions"].as_i64().unwrap() >= 1);
    assert_eq!(body["orphan_sessions"], 0);
    assert_eq!(body["metadata"]["total_issues"], 1);
    // No writer was set in the seed → 1 missing.
    assert_eq!(body["metadata"]["missing_writer"], 1);
}
