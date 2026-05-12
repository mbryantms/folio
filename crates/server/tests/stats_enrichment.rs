//! Stats v2: integration coverage for the enriched `/me/reading-stats`
//! payload — dow_hour grid, time_of_day buckets, pace_series, reread top
//! lists, top creators (via `series_credits`), and the completion view.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{
    audit_log,
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record, reading_session,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_credit,
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

/// Seed a library + series + one or more issues. Returns
/// `(library_id, series_id, Vec<issue_id>)`. Each issue gets `page_count=20`
/// and incrementing sort_number/title.
async fn seed_library_with_issues(
    app: &TestApp,
    label: &str,
    issue_count: usize,
    publisher: Option<&str>,
    imprint: Option<&str>,
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
        imprint: Set(imprint.map(str::to_owned)),
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
        // Deterministic 64-char hex id so the FK + page_bytes hashing works
        // without ever reading the file.
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

/// Insert a fully-finished session at the given `started_at`. End_page reaches
/// `page_count - 1` so completion logic treats it as finished. Returns the
/// inserted row's id.
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
) -> Uuid {
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
    id
}

async fn add_series_credit(app: &TestApp, series_id: Uuid, role: &str, person: &str) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    series_credit::ActiveModel {
        series_id: Set(series_id),
        role: Set(role.into()),
        person: Set(person.into()),
    }
    .insert(&db)
    .await
    .unwrap();
}

#[tokio::test]
async fn stats_returns_new_enrichment_fields() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;
    let (lib, series, issues) =
        seed_library_with_issues(&app, "alice", 3, Some("Marvel"), Some("Icon")).await;
    grant_library(&app, auth.user_id, lib).await;

    add_series_credit(&app, series, "writer", "Bendis").await;
    add_series_credit(&app, series, "penciller", "Maleev").await;

    // Spread 4 sessions across two days at different hours.
    let now = Utc::now().fixed_offset();
    let day_ago = now - Duration::days(1);
    let two_days_ago = now - Duration::days(2);
    insert_session(
        &app,
        auth.user_id,
        &issues[0],
        series,
        two_days_ago,
        60_000,
        10,
        9,
        Some("desktop"),
    )
    .await;
    insert_session(
        &app,
        auth.user_id,
        &issues[0],
        series,
        day_ago,
        90_000,
        15,
        14,
        Some("desktop"),
    )
    .await;
    // This one completes the issue (end_page = 19 = page_count - 1).
    insert_session(
        &app,
        auth.user_id,
        &issues[1],
        series,
        day_ago + Duration::hours(2),
        120_000,
        20,
        19,
        Some("mobile"),
    )
    .await;
    insert_session(
        &app,
        auth.user_id,
        &issues[2],
        series,
        now - Duration::hours(1),
        45_000,
        5,
        4,
        Some("desktop"),
    )
    .await;

    let (s, body) = get(&app, &auth, "/me/reading-stats?range=30d").await;
    assert_eq!(s, StatusCode::OK, "body={body}");

    // Totals + first/last read.
    assert_eq!(body["totals"]["sessions"], 4);
    assert!(body["first_read_at"].is_string());
    assert!(body["last_read_at"].is_string());

    // dow_hour: should have ≥3 cells covering distinct hours.
    let dow_hour = body["dow_hour"].as_array().expect("dow_hour array");
    assert!(!dow_hour.is_empty(), "dow_hour populated");
    for cell in dow_hour {
        assert!(cell["dow"].as_i64().unwrap() >= 0);
        assert!(cell["dow"].as_i64().unwrap() < 7);
        assert!(cell["hour"].as_i64().unwrap() >= 0);
        assert!(cell["hour"].as_i64().unwrap() < 24);
    }

    // time_of_day buckets sum to total sessions.
    let tod = &body["time_of_day"];
    let total: i64 = tod["morning"]["sessions"].as_i64().unwrap()
        + tod["afternoon"]["sessions"].as_i64().unwrap()
        + tod["evening"]["sessions"].as_i64().unwrap()
        + tod["night"]["sessions"].as_i64().unwrap();
    assert_eq!(total, 4, "time_of_day sums to sessions");

    // pace_series: only sessions w/ distinct_pages >= 3 → all 4 qualify.
    assert_eq!(body["pace_series"].as_array().unwrap().len(), 4);

    // Reread top issues: issue[0] read twice → top of list.
    let rti = body["reread_top_issues"].as_array().expect("rti array");
    assert!(!rti.is_empty());
    assert_eq!(rti[0]["reads"], 2);
    assert_eq!(rti[0]["issue_id"], issues[0]);

    // Reread top series: only one series in scope.
    let rts = body["reread_top_series"].as_array().expect("rts array");
    assert_eq!(rts.len(), 1);
    assert_eq!(rts[0]["reads"], 4);
    assert_eq!(rts[0]["distinct_issues"], 3);

    // Completion: issue[1] is the only completed one.
    assert_eq!(body["completion"]["started"], 3);
    assert_eq!(body["completion"]["completed"], 1);
    let rate = body["completion"]["rate"].as_f64().unwrap();
    assert!((rate - 1.0 / 3.0).abs() < 1e-6, "rate={rate}");

    // Top creators: 2 rows (writer + penciller).
    let creators = body["top_creators"].as_array().expect("creators array");
    assert_eq!(creators.len(), 2);
    let roles: Vec<&str> = creators
        .iter()
        .map(|r| r["role"].as_str().unwrap())
        .collect();
    assert!(roles.contains(&"writer"));
    assert!(roles.contains(&"penciller"));

    // Top imprints: Icon × 4 sessions.
    let imprints = body["top_imprints"].as_array().expect("imprints array");
    assert_eq!(imprints.len(), 1);
    assert_eq!(imprints[0]["name"], "Icon");
}

#[tokio::test]
async fn completion_honors_progress_records_finished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "bob", 2, None, None).await;
    grant_library(&app, auth.user_id, lib).await;

    let now = Utc::now().fixed_offset();
    // First issue: short session that doesn't reach the end.
    insert_session(
        &app,
        auth.user_id,
        &issues[0],
        series,
        now - Duration::hours(3),
        60_000,
        5,
        4,
        None,
    )
    .await;
    // Second issue: short session but progress_records says finished.
    insert_session(
        &app,
        auth.user_id,
        &issues[1],
        series,
        now - Duration::hours(2),
        60_000,
        5,
        4,
        None,
    )
    .await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    progress_record::ActiveModel {
        user_id: Set(auth.user_id),
        issue_id: Set(issues[1].clone()),
        last_page: Set(4),
        percent: Set(1.0),
        finished: Set(true),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let (s, body) = get(&app, &auth, "/me/reading-stats?range=30d").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["completion"]["started"], 2);
    assert_eq!(body["completion"]["completed"], 1);
}

#[tokio::test]
async fn clear_history_deletes_sessions_and_audits() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "claire@example.com").await;
    let (lib, series, issues) = seed_library_with_issues(&app, "claire", 1, None, None).await;
    grant_library(&app, auth.user_id, lib).await;
    insert_session(
        &app,
        auth.user_id,
        &issues[0],
        series,
        Utc::now().fixed_offset() - Duration::hours(1),
        60_000,
        5,
        4,
        None,
    )
    .await;

    let (s, body) = post(&app, &auth, "/me/reading-sessions/clear", "{}").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    assert_eq!(body["deleted"], 1);

    // Subsequent stats call sees zeros.
    let (_, stats) = get(&app, &auth, "/me/reading-stats?range=30d").await;
    assert_eq!(stats["totals"]["sessions"], 0);

    // Audit row landed.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let audits = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("me.activity.history.clear"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].actor_id, auth.user_id);
    assert_eq!(audits[0].payload["deleted"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn range_1y_is_accepted() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "delta@example.com").await;
    let (s, _) = get(&app, &auth, "/me/reading-stats?range=1y").await;
    assert_eq!(s, StatusCode::OK);
}
