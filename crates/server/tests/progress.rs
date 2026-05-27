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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
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
        reading_direction: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let issue_id = format!("{:0>64}", series_id.simple());
    // Per-issue uniqueness — file_path + slug both have UNIQUE
    // constraints, so tests that call `seed_issue` repeatedly
    // (multi-select bulk tests, especially) must vary them by id.
    let slug = format!("issue-{}", &issue_id[..8]);
    let file_path = format!("/tmp/{issue_id}.cbz");
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug),
        file_path: Set(file_path),
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

/// Multi-select bulk-mark M2:
/// `POST /me/progress/bulk` upserts progress rows for an array of
/// issue ids. Mirrors `upsert_series` but for arbitrary cross-series
/// selection. Bucket counts (updated / skipped / forbidden / not_found)
/// let the toast surface "N marked, M were already marked".
async fn post_bulk(
    app: &TestApp,
    auth: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/progress/bulk")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_read_updates_each_id_and_counts_skipped() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-mark@example.com").await;
    let issue_a = seed_issue(&app).await;
    let issue_b = seed_issue(&app).await;
    let issue_c = seed_issue(&app).await;

    // Pre-mark issue_a as read so the bulk call should `skipped += 1`.
    let (status, _) = post_progress(
        &app,
        &auth,
        serde_json::json!({
            "issue_id": issue_a,
            "page": 19,
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Bulk-mark all three read.
    let (status, json) = post_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": [&issue_a, &issue_b, &issue_c],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["updated"].as_u64(), Some(2));
    assert_eq!(json["skipped"].as_u64(), Some(1));
    assert_eq!(json["forbidden"].as_u64(), Some(0));
    assert_eq!(json["not_found"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_unread_resets_to_page_zero() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-unread@example.com").await;
    let issue_a = seed_issue(&app).await;

    // Pre-mark as read.
    let (_, _) = post_progress(
        &app,
        &auth,
        serde_json::json!({"issue_id": issue_a, "page": 19, "finished": true}),
    )
    .await;

    let (status, json) = post_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": [&issue_a],
            "finished": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["updated"].as_u64(), Some(1));

    // Verify via the progress sync-delta endpoint — the bulk-mark
    // call is the only mutation in scope, so the only row should
    // reflect the unread state.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/progress")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp.into_body()).await;
    let records = body["records"].as_array().expect("records array");
    let row = records
        .iter()
        .find(|r| r["issue_id"].as_str() == Some(issue_a.as_str()))
        .expect("progress row");
    assert_eq!(row["page"].as_i64(), Some(0));
    assert_eq!(row["finished"].as_bool(), Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_with_nonexistent_id_counts_not_found_silently() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-missing@example.com").await;
    let issue_a = seed_issue(&app).await;

    let (status, json) = post_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": [&issue_a, "this-id-does-not-exist"],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["updated"].as_u64(), Some(1));
    assert_eq!(json["not_found"].as_u64(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_empty_list_returns_zero_counts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-empty@example.com").await;
    let (status, json) = post_bulk(
        &app,
        &auth,
        serde_json::json!({"issue_ids": [], "finished": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["updated"].as_u64(), Some(0));
    assert_eq!(json["skipped"].as_u64(), Some(0));
    assert_eq!(json["forbidden"].as_u64(), Some(0));
    assert_eq!(json["not_found"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_over_cap_rejects() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-cap@example.com").await;
    let ids: Vec<String> = (0..501).map(|i| format!("id-{i}")).collect();
    let (status, _) = post_bulk(
        &app,
        &auth,
        serde_json::json!({"issue_ids": ids, "finished": true}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_dedupes_ids() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-dedupe@example.com").await;
    let issue_a = seed_issue(&app).await;

    // The same id submitted three times should mark once, count once.
    let (status, json) = post_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": [&issue_a, &issue_a, &issue_a],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["updated"].as_u64(), Some(1));
    assert_eq!(json["skipped"].as_u64(), Some(0));
}

/// M6 extension: series-bulk endpoint backs filter views, where each
/// card is a series rather than an issue. Each series id expands
/// server-side to its active issues, then walks through `upsert_for`.
async fn post_series_bulk(
    app: &TestApp,
    auth: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/progress/series-bulk")
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

/// Seeds a single series with `n_issues` active issues and returns the
/// series_id along with the issue ids. Sister to `seed_issue` which
/// makes one series per issue — series-bulk tests need to verify that
/// every issue under a single series gets marked.
async fn seed_series_with_issues(app: &TestApp, n_issues: usize) -> (Uuid, Vec<String>) {
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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
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
        reading_direction: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let mut issue_ids = Vec::with_capacity(n_issues);
    for i in 0..n_issues {
        // Per-issue uniqueness — file_path + slug + content_hash all
        // need to be distinct.
        let unique = Uuid::now_v7();
        let issue_id = format!("{:0>64}", unique.simple());
        let slug = format!("issue-{}-{}", series_id.simple(), i);
        let file_path = format!("/tmp/{}-{}.cbz", series_id, i);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(slug),
            file_path: Set(file_path),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some((i + 1) as f64)),
            number_raw: Set(Some(format!("{}", i + 1))),
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
        }
        .insert(&db)
        .await
        .unwrap();
        issue_ids.push(issue_id);
    }
    (series_id, issue_ids)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_marks_every_active_issue_read() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-read@example.com").await;
    let (series_id, _issues) = seed_series_with_issues(&app, 3).await;

    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({
            "series_ids": [series_id.to_string()],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["updated"].as_u64(), Some(3));
    assert_eq!(json["skipped"].as_u64(), Some(0));
    assert_eq!(json["forbidden_series"].as_u64(), Some(0));
    assert_eq!(json["not_found_series"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_counts_already_read_as_skipped() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-skip@example.com").await;
    let (series_id, issues) = seed_series_with_issues(&app, 2).await;

    // Pre-mark the first issue as read.
    let (status, _) = post_progress(
        &app,
        &auth,
        serde_json::json!({"issue_id": &issues[0], "page": 19, "finished": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({
            "series_ids": [series_id.to_string()],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["updated"].as_u64(), Some(1));
    assert_eq!(json["skipped"].as_u64(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_unread_resets_every_issue() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-unread@example.com").await;
    let (series_id, issues) = seed_series_with_issues(&app, 2).await;

    // Pre-mark both as read.
    for id in &issues {
        let (_, _) = post_progress(
            &app,
            &auth,
            serde_json::json!({"issue_id": id, "page": 19, "finished": true}),
        )
        .await;
    }

    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({
            "series_ids": [series_id.to_string()],
            "finished": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["updated"].as_u64(), Some(2));
    assert_eq!(json["skipped"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_empty_list_returns_zero_counts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-empty@example.com").await;
    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({"series_ids": [], "finished": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["updated"].as_u64(), Some(0));
    assert_eq!(json["skipped"].as_u64(), Some(0));
    assert_eq!(json["forbidden_series"].as_u64(), Some(0));
    assert_eq!(json["not_found_series"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_over_cap_rejects() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-cap@example.com").await;
    let ids: Vec<String> = (0..101).map(|_| Uuid::now_v7().to_string()).collect();
    let (status, _) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({"series_ids": ids, "finished": true}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_missing_series_counts_not_found() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-missing@example.com").await;
    let (series_id, _) = seed_series_with_issues(&app, 1).await;
    let nonexistent = Uuid::now_v7();

    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({
            "series_ids": [series_id.to_string(), nonexistent.to_string()],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(json["updated"].as_u64(), Some(1));
    assert_eq!(json["not_found_series"].as_u64(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_bulk_dedupes_series_ids() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-bulk-dedupe@example.com").await;
    let (series_id, _) = seed_series_with_issues(&app, 1).await;

    let (status, json) = post_series_bulk(
        &app,
        &auth,
        serde_json::json!({
            "series_ids": [
                series_id.to_string(),
                series_id.to_string(),
                series_id.to_string(),
            ],
            "finished": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 1 issue × dedup means a single update.
    assert_eq!(json["updated"].as_u64(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_backfill_flag_is_persisted() {
    // `POST /me/progress/bulk` accepts a `backfill` field. When the
    // user sets it true alongside `finished = true`, the resulting
    // progress_records carry `is_backfill = true` and are excluded
    // from time-bound activity surfaces (reading log feed, Just
    // Finished sort, etc).
    use entity::progress_record;
    use sea_orm::{Database, EntityTrait};
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-bf@example.com").await;
    let id = seed_issue(&app).await;

    let (status, _) = post_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": [id],
            "finished": true,
            "backfill": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::connect(&app.db_url).await.unwrap();
    use entity::progress_record::Column as Pc;
    use sea_orm::{ColumnTrait, QueryFilter};
    let row = progress_record::Entity::find()
        .filter(Pc::IssueId.eq(id.clone()))
        .one(&db)
        .await
        .unwrap()
        .expect("row exists after backfill bulk write");
    assert!(row.finished);
    assert!(row.is_backfill, "backfill flag must persist");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_mark_unread_clears_backfill_flag() {
    // Any unread write must clear `is_backfill`, regardless of what
    // the caller passes. The user just said "this isn't done"; the
    // catalog/sync origin is no longer load-bearing.
    use entity::progress_record;
    use sea_orm::{Database, EntityTrait};
    let app = TestApp::spawn().await;
    let auth = register(&app, "bulk-bf-clear@example.com").await;
    let id = seed_issue(&app).await;

    // First, bulk-mark as backfill (is_backfill = true).
    post_bulk(
        &app,
        &auth,
        serde_json::json!({"issue_ids": [id], "finished": true, "backfill": true}),
    )
    .await;

    // Then bulk-mark unread — even with backfill=true in the body,
    // is_backfill must clear because finished is now false.
    let (status, _) = post_bulk(
        &app,
        &auth,
        serde_json::json!({"issue_ids": [id], "finished": false, "backfill": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::connect(&app.db_url).await.unwrap();
    use entity::progress_record::Column as Pc;
    use sea_orm::{ColumnTrait, QueryFilter};
    let row = progress_record::Entity::find()
        .filter(Pc::IssueId.eq(id.clone()))
        .one(&db)
        .await
        .unwrap()
        .expect("row exists after unread write");
    assert!(!row.finished);
    assert!(
        !row.is_backfill,
        "unread write must clear is_backfill regardless of caller flag",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_issue_reader_write_always_clears_backfill() {
    // Per-issue progress writes from the reader (POST /me/progress)
    // are by definition active reading. They must always set
    // is_backfill = false, even when the user previously bulk-marked
    // the issue as backfill.
    use entity::progress_record;
    use sea_orm::{Database, EntityTrait};
    let app = TestApp::spawn().await;
    let auth = register(&app, "reader-clear@example.com").await;
    let id = seed_issue(&app).await;

    // Seed via bulk-mark backfill so the row starts is_backfill=true.
    post_bulk(
        &app,
        &auth,
        serde_json::json!({"issue_ids": [id], "finished": true, "backfill": true}),
    )
    .await;

    // Per-issue reader write — bumps a page without changing finished.
    let (status, _) =
        post_progress(&app, &auth, serde_json::json!({"issue_id": id, "page": 5})).await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::connect(&app.db_url).await.unwrap();
    use entity::progress_record::Column as Pc;
    use sea_orm::{ColumnTrait, QueryFilter};
    let row = progress_record::Entity::find()
        .filter(Pc::IssueId.eq(id.clone()))
        .one(&db)
        .await
        .unwrap()
        .expect("row exists");
    assert!(
        !row.is_backfill,
        "reader writes always clear the backfill flag",
    );
}
