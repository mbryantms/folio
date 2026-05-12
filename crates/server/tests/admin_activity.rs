//! M6d: integration coverage for `GET /admin/activity`.
//!
//! Seeds one entry per source kind (audit/scan/health/reading) and asserts
//! the combined feed surfaces all four, that filter chips drop the others,
//! and that pagination via the opaque cursor is stable.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    audit_log,
    issue::ActiveModel as IssueAM,
    library, library_health_issue,
    scan_run::ActiveModel as ScanRunAM,
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
    let json = if bytes.is_empty() {
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
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// Seed one library + series + active issue. Returns ids for downstream rows.
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

async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
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

async fn seed_audit(app: &TestApp, actor_id: Uuid) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    audit_log::ActiveModel {
        id: Set(id),
        actor_id: Set(actor_id),
        actor_type: Set("user".into()),
        action: Set("test.poke".into()),
        target_type: Set(None),
        target_id: Set(None),
        payload: Set(serde_json::json!({"reason": "fixture"})),
        ip: Set(None),
        user_agent: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn seed_scan_run(app: &TestApp, library_id: Uuid) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    ScanRunAM {
        id: Set(id),
        library_id: Set(library_id),
        state: Set("complete".into()),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn seed_health_issue(app: &TestApp, library_id: Uuid) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library_health_issue::ActiveModel {
        id: Set(id),
        library_id: Set(library_id),
        scan_id: Set(None),
        kind: Set("missing_comicinfo".into()),
        payload: Set(serde_json::json!({"path": "/tmp/foo.cbz"})),
        severity: Set("warning".into()),
        fingerprint: Set(format!("{}-fp", id)),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
        resolved_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn seed_reading_session(app: &TestApp, auth: &Authed, issue_id: &str, tag: &str) {
    let started = (Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
    let body = serde_json::json!({
        "client_session_id": tag,
        "issue_id": issue_id,
        "started_at": started,
        "active_ms": 60_000,
        "distinct_pages_read": 5,
        "page_turns": 6,
        "start_page": 0,
        "end_page": 5,
    })
    .to_string();
    let (s, _) = post(app, auth, "/me/reading-sessions", &body).await;
    assert!(s.is_success(), "seed_reading_session: {s}");
}

#[tokio::test]
async fn rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(&app, &user, "/admin/activity").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn feed_includes_all_four_kinds() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let (lib_id, _series_id, issue_id) = seed_one(&app, "demo").await;
    grant_library(&app, user.user_id, lib_id).await;

    seed_audit(&app, admin.user_id).await;
    seed_scan_run(&app, lib_id).await;
    seed_health_issue(&app, lib_id).await;
    seed_reading_session(&app, &user, &issue_id, "r1").await;

    let (s, body) = get(&app, &admin, "/admin/activity").await;
    assert_eq!(s, StatusCode::OK, "body={body}");
    let entries = body["entries"].as_array().unwrap();
    let kinds: std::collections::HashSet<&str> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains("audit"));
    assert!(kinds.contains("scan"));
    assert!(kinds.contains("health"));
    assert!(kinds.contains("reading"));

    // Reading entries are aggregated and never include user_id.
    for entry in entries {
        if entry["kind"] == "reading" {
            assert!(entry["payload"].get("user_id").is_none());
            assert!(entry["payload"]["sessions"].as_i64().unwrap() >= 1);
        }
    }
}

#[tokio::test]
async fn filter_chips_restrict_kinds() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let (lib_id, _series_id, issue_id) = seed_one(&app, "demo").await;
    grant_library(&app, user.user_id, lib_id).await;
    seed_audit(&app, admin.user_id).await;
    seed_scan_run(&app, lib_id).await;
    seed_health_issue(&app, lib_id).await;
    seed_reading_session(&app, &user, &issue_id, "r1").await;

    let (_, body) = get(&app, &admin, "/admin/activity?kinds=audit").await;
    let entries = body["entries"].as_array().unwrap();
    assert!(entries.iter().all(|e| e["kind"] == "audit"));

    let (_, body) = get(&app, &admin, "/admin/activity?kinds=health,scan").await;
    let entries = body["entries"].as_array().unwrap();
    let kinds: std::collections::HashSet<&str> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["scan", "health"]
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
    );
}

#[tokio::test]
async fn cursor_pagination_works() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (lib_id, _series_id, _issue_id) = seed_one(&app, "demo").await;
    // Seed five distinct audit rows; pagination shows them in DESC order.
    for _ in 0..5 {
        seed_audit(&app, admin.user_id).await;
    }
    seed_scan_run(&app, lib_id).await;

    let (_, body) = get(&app, &admin, "/admin/activity?kinds=audit&limit=3").await;
    let p1 = body["entries"].as_array().unwrap();
    assert_eq!(p1.len(), 3);
    let cursor = body["next_cursor"].as_str().unwrap().to_owned();

    let (_, body) = get(
        &app,
        &admin,
        &format!("/admin/activity?kinds=audit&limit=3&cursor={cursor}"),
    )
    .await;
    let p2 = body["entries"].as_array().unwrap();
    assert_eq!(p2.len(), 2);

    // Cursor entries don't overlap.
    let p1_ids: std::collections::HashSet<&str> = p1
        .iter()
        .map(|e| e["source_id"].as_str().unwrap())
        .collect();
    for entry in p2 {
        assert!(!p1_ids.contains(entry["source_id"].as_str().unwrap()));
    }
}

#[tokio::test]
async fn invalid_cursor_400s() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (s, _) = get(&app, &admin, "/admin/activity?cursor=garbage!!").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_kinds_returns_empty() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (_lib_id, _series_id, _issue_id) = seed_one(&app, "demo").await;
    seed_audit(&app, admin.user_id).await;

    let (s, body) = get(&app, &admin, "/admin/activity?kinds=nonsense").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
}
