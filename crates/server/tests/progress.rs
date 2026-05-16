//! `POST /progress` — per-issue progress upsert.
//!
//! Focus: `finished` stickiness. Once an issue is marked finished
//! (either explicitly via "Mark as read" or implicitly by the reader
//! reaching the last page), per-page progress writes from a later
//! reader session — e.g. opening the issue at a bookmark midway
//! through — MUST NOT clear that flag. Only an explicit
//! `finished: false` payload should unfinish the issue.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn post_progress(
    app: &TestApp,
    auth: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/progress")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_issue(app: &TestApp) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Lib".into()),
        root_path: Set(format!("/tmp/{lib_id}")),
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
        name: Set("Test Series".into()),
        normalized_name: Set(normalize_name("Test Series")),
        year: Set(Some(2020)),
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
        slug: Set(format!("ts-{series_id}")),
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

    let issue_id = format!("{:0>64}", series_id.simple());
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set("issue-1".into()),
        file_path: Set("/tmp/issue-1.cbz".into()),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
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
    issue_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_page_write_preserves_finished_when_finished_omitted() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    let issue_id = seed_issue(&app).await;

    // Mark the issue as finished (explicit, like the reader's
    // last-page auto-finish or "Mark as read").
    let (status, json) = post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_id,
            "page": 19,
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["finished"].as_bool(), Some(true));

    // Now the user jumps mid-issue via a bookmark deep-link. The
    // reader's debounced per-page write omits `finished`, sending only
    // `{issue_id, page}`. The server should preserve the previous
    // `finished: true` flag.
    let (status, json) = post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_id,
            "page": 12,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(
        json["finished"].as_bool(),
        Some(true),
        "finished must remain true when omitted from the write payload"
    );
    assert_eq!(json["page"].as_i64(), Some(12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_finished_false_does_unfinish() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    let issue_id = seed_issue(&app).await;

    // Mark finished.
    post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_id,
            "page": 19,
            "finished": true,
        }),
    )
    .await;

    // Explicit "Mark as unread" — server honors the explicit flag.
    let (status, json) = post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_id,
            "page": 0,
            "finished": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["finished"].as_bool(), Some(false));
    assert_eq!(json["page"].as_i64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_write_with_no_finished_defaults_to_false() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    let issue_id = seed_issue(&app).await;

    // No prior progress row. Reader's mid-issue write (no finished) on
    // a fresh issue should insert with finished=false.
    let (status, json) = post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_id,
            "page": 5,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["finished"].as_bool(), Some(false));
    assert_eq!(json["page"].as_i64(), Some(5));
}
