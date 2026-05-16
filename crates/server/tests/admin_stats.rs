//! M6c: integration coverage for the admin stats + server info endpoints.
//!
//! Boots a TestApp, seeds a small library + series + issue + user, exercises
//! the overview totals + the per-user reading-stats endpoint. Asserts that
//! the audit-log emits an `admin.user.activity.view` row when an admin
//! drills into another user's reading stats.

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
    library, library_user_access,
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

/// Seed a library + series + active issue. Returns ids.
async fn seed_one(app: &TestApp, name: &str) -> (Uuid, Uuid, String) {
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
        publisher: Set(None),
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
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(None),
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

/// Grant non-admin user access to a library so the upsert ACL passes.
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

#[tokio::test]
async fn overview_returns_admin_only_payload() {
    let app = TestApp::spawn().await;
    // First user → admin (bootstrap rule).
    let admin = register(&app, "admin@example.com").await;
    // Second user → regular.
    let _user = register(&app, "user@example.com").await;
    seed_one(&app, "demo").await;

    let (s, body) = get(&app, &admin, "/api/admin/stats/overview").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    assert!(body["totals"]["libraries"].as_i64().unwrap() >= 1);
    assert!(body["totals"]["series"].as_i64().unwrap() >= 1);
    assert!(body["totals"]["issues"].as_i64().unwrap() >= 1);
    assert!(body["totals"]["users"].as_i64().unwrap() >= 2);
    assert_eq!(body["scans_in_flight"], 0);
    assert_eq!(body["sessions_today"], 0);
    assert_eq!(body["active_readers_now"], 0);
    assert!(body["reads_per_day"].is_array());
}

#[tokio::test]
async fn overview_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(&app, &user, "/api/admin/stats/overview").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn user_reading_stats_audits_each_access() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (lib_id, _series_id, issue_id) = seed_one(&app, "demo").await;
    grant_library(&app, user.user_id, lib_id).await;

    // Have the regular user post a session so the stats endpoint has data.
    let started = (Utc::now() - Duration::seconds(120)).to_rfc3339();
    let body = serde_json::json!({
        "client_session_id": "u-1",
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 60_000,
        "distinct_pages_read": 5,
        "page_turns": 6,
        "start_page": 0,
        "end_page": 5,
    })
    .to_string();
    let (s, _) = post(&app, &user, "/api/me/reading-sessions", &body).await;
    assert!(s.is_success());

    let target = user.user_id.to_string();
    let (s, body) = get(
        &app,
        &admin,
        &format!("/api/admin/users/{target}/reading-stats?range=30d"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    assert_eq!(body["totals"]["sessions"], 1);
    assert_eq!(body["totals"]["active_ms"], 60_000);

    // Audit row landed.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.user.activity.view"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one audit row");
    assert_eq!(rows[0].actor_id, admin.user_id);
    assert_eq!(rows[0].target_id.as_deref(), Some(target.as_str()));
    assert_eq!(rows[0].payload["range"].as_str().unwrap(), "30d");
}

#[tokio::test]
async fn user_reading_stats_404_on_missing_user() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let phantom = Uuid::now_v7();
    let (s, _) = get(
        &app,
        &admin,
        &format!("/api/admin/users/{phantom}/reading-stats"),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // No audit row written for a 404.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.user.activity.view"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 0);
}

#[tokio::test]
async fn user_reading_stats_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(
        &app,
        &user,
        &format!("/api/admin/users/{}/reading-stats", user.user_id),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn server_info_reports_pings_and_uptime() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (s, body) = get(&app, &admin, "/api/admin/server/info").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    assert!(body["postgres_ok"].as_bool().unwrap());
    assert!(body["redis_ok"].as_bool().unwrap());
    assert!(body["scheduler_running"].as_bool().unwrap());
    assert!(body["uptime_secs"].as_i64().unwrap() >= 0);
    assert!(!body["version"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn server_info_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(&app, &user, "/api/admin/server/info").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
