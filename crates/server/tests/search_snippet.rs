//! Search snippet (`ts_headline`) integration tests.
//!
//! Verifies the `snippet` field added by M1 of the search-improvements
//! plan returns `<mark>…</mark>`-wrapped excerpts of the matched
//! field on both `/series?q=` and `/issues/search`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM, library, series::ActiveModel as SeriesAM, series::normalize_name,
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

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
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

async fn seed_library(app: &TestApp, lib_name: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {lib_name}")),
        root_path: Set(format!("/tmp/{lib_name}-{lib_id}")),
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
    }
    .insert(&db)
    .await
    .unwrap();
    lib_id
}

async fn seed_series(app: &TestApp, lib_id: Uuid, name: &str, summary: Option<&str>) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(summary.map(str::to_owned)),
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
        series_group: Set(None),
        slug: Set(format!("{name}-{series_id}")),
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
    series_id
}

async fn seed_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    idx: u8,
    summary: Option<&str>,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), idx);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("issue-{idx}-{series_id}")),
        file_path: Set(format!("/tmp/{series_id}-{idx}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(Some(format!("Issue {idx}"))),
        sort_number: Set(Some(idx as f64)),
        number_raw: Set(Some(idx.to_string())),
        volume: Set(None),
        year: Set(Some(2020)),
        month: Set(None),
        day: Set(None),
        summary: Set(summary.map(str::to_owned)),
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
        hash_algorithm: Set(1),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
    }
    .insert(&db)
    .await
    .unwrap();
    issue_id
}

/// Series-search snippet: the response includes a `snippet` field with
/// `<mark>…</mark>` around the matched term inside the summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_search_returns_mark_highlighted_snippet() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "snip-series@example.com").await;

    let lib = seed_library(&app, "snip").await;
    seed_series(
        &app,
        lib,
        "Saga",
        Some("An epic space opera about lovers from warring planets and their interstellar fugitive family."),
    )
    .await;

    let (status, json) = http(&app, Method::GET, "/api/series?q=interstellar", &auth).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let items = json["items"].as_array().unwrap();
    assert!(!items.is_empty(), "expected at least one match");
    let row = items
        .iter()
        .find(|v| v["name"].as_str() == Some("Saga"))
        .expect("Saga in results");
    let snippet = row["snippet"]
        .as_str()
        .expect("snippet present on search-mode response");
    assert!(
        snippet.contains("<mark>interstellar</mark>"),
        "snippet should wrap the matched term: {snippet}"
    );
}

/// Issue-search snippet path. Mirrors the series test but exercises the
/// `/issues/search` endpoint with a match on the issue's `summary`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_search_returns_mark_highlighted_snippet() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "snip-issue@example.com").await;

    let lib = seed_library(&app, "snip-i").await;
    let series_id = seed_series(&app, lib, "Saga", None).await;
    seed_issue(
        &app,
        lib,
        series_id,
        1,
        Some("The lovers escape through a portal into the cosmic underbelly of the galaxy."),
    )
    .await;

    let (status, json) = http(&app, Method::GET, "/api/issues/search?q=portal", &auth).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let items = json["items"].as_array().unwrap();
    assert!(!items.is_empty(), "expected at least one match");
    let snippet = items[0]["snippet"]
        .as_str()
        .expect("snippet present on issue search");
    assert!(
        snippet.contains("<mark>portal</mark>"),
        "snippet should wrap the matched term: {snippet}"
    );
}

/// Snippet is omitted when the row's summary doesn't contain a
/// highlightable fragment (matched on name only, no summary text).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_search_omits_snippet_when_summary_does_not_match() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "snip-none@example.com").await;

    let lib = seed_library(&app, "snip-none").await;
    // No summary — the only match is on `name`, so `ts_headline` over
    // summary returns the empty string and the helper filters it out.
    seed_series(&app, lib, "Saga", None).await;

    let (status, json) = http(&app, Method::GET, "/api/series?q=Saga", &auth).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let row = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"].as_str() == Some("Saga"))
        .expect("Saga in results");
    assert!(
        row.get("snippet").map(|v| v.is_null()).unwrap_or(true),
        "no summary → no snippet field on the wire"
    );
}
