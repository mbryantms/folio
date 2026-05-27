//! `GET /me/cbl-lists/{id}/window-paginated` — bidirectional rail
//! pagination over a CBL.
//!
//! Anchors on `position`, returns `min_position` / `max_position` +
//! `has_more_before` / `has_more_after` so a TanStack `useInfiniteQuery`
//! can walk both directions without disturbing the initial anchor band.
//! Tests seed 20 matched entries directly via sea-orm so they don't
//! depend on the CBL XML parser or matcher heuristics.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_entry, cbl_list,
    issue::ActiveModel as IssueAM,
    library, progress_record,
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

/// Insert a library + one series + `count` issues + a CBL list owned by
/// `user` with each issue placed at position 0..count. Returns the
/// list id and the issue ids in position order so tests can mark
/// arbitrary entries as finished.
async fn seed_cbl_with_matched_entries(
    app: &TestApp,
    user: Uuid,
    count: usize,
) -> (Uuid, Vec<String>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("cbl-window lib".into()),
        root_path: Set(format!("/tmp/cbl-window-{lib_id}")),
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
        name: Set("Window Test".into()),
        normalized_name: Set(normalize_name("Window Test")),
        year: Set(Some(2020)),
        volume: Set(Some(1)),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("ended".into()),
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
        slug: Set("window-test-2020".into()),
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
        let issue_id = format!("{:0>62}{:02x}", series_id.simple(), n as u8);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("window-test-{n}")),
            file_path: Set(format!("/tmp/cbl-window/Window #{n}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
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

    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(Some(user)),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set(String::new()),
        parsed_name: Set("Window Test".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(count as i32)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(Some(now)),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    for (i, issue_id) in issue_ids.iter().enumerate() {
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(i as i32),
            series_name: Set("Window Test".into()),
            issue_number: Set(i.to_string()),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(Some(issue_id.clone())),
            match_status: Set("matched".into()),
            match_method: Set(Some("test".into())),
            match_confidence: Set(Some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(Some(now)),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    (list_id, issue_ids)
}

/// Mark `positions` (0-indexed positions within the list) as finished
/// for `user`. Used to set up the anchor position deterministically.
async fn mark_finished(app: &TestApp, user: Uuid, issue_ids: &[String], positions: &[usize]) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    for &p in positions {
        progress_record::ActiveModel {
            user_id: Set(user),
            issue_id: Set(issue_ids[p].clone()),
            last_page: Set(20),
            percent: Set(1.0),
            finished: Set(true),
            finished_at: Set(Some(now)),
            updated_at: Set(now),
            device: Set(None),
            is_backfill: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_window_anchors_on_first_unfinished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@window.test").await;
    let (list_id, issue_ids) = seed_cbl_with_matched_entries(&app, auth.user_id, 20).await;
    // Mark the first three finished — anchor should land on position 3.
    mark_finished(&app, auth.user_id, &issue_ids, &[0, 1, 2]).await;

    let url = format!("/api/me/cbl-lists/{list_id}/window-paginated");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK, "body: {body:#?}");

    // Defaults: before=3, after=24. With only 20 matched and an anchor
    // at position 3, that's items at positions 0..=19 → all 20.
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 20);
    assert_eq!(items[0]["position"].as_i64(), Some(0));
    assert_eq!(items[19]["position"].as_i64(), Some(19));
    assert_eq!(body["current_index"].as_i64(), Some(3));
    assert_eq!(body["total_matched"].as_i64(), Some(20));
    assert_eq!(body["total_entries"].as_i64(), Some(20));
    assert_eq!(body["min_position"].as_i64(), Some(0));
    assert_eq!(body["max_position"].as_i64(), Some(19));
    assert_eq!(body["has_more_before"], false);
    assert_eq!(body["has_more_after"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_window_reports_has_more_flags_when_clipped() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@window.test").await;
    let (list_id, issue_ids) = seed_cbl_with_matched_entries(&app, auth.user_id, 60).await;
    // Anchor at position 20 — has read 0..=19.
    let read: Vec<usize> = (0..20).collect();
    mark_finished(&app, auth.user_id, &issue_ids, &read).await;

    let url = format!("/api/me/cbl-lists/{list_id}/window-paginated?before=3&after=8");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK);
    // Slice [17..=28] = 12 items, anchor offset = 3.
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 12);
    assert_eq!(items[0]["position"].as_i64(), Some(17));
    assert_eq!(items[11]["position"].as_i64(), Some(28));
    assert_eq!(body["current_index"].as_i64(), Some(3));
    assert_eq!(body["min_position"].as_i64(), Some(17));
    assert_eq!(body["max_position"].as_i64(), Some(28));
    assert_eq!(body["has_more_before"], true);
    assert_eq!(body["has_more_after"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_cursor_paginates_forward() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "carol@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 40).await;

    let url =
        format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=after&cursor=10&limit=5");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(items[0]["position"].as_i64(), Some(11));
    assert_eq!(items[4]["position"].as_i64(), Some(15));
    assert_eq!(body["min_position"].as_i64(), Some(11));
    assert_eq!(body["max_position"].as_i64(), Some(15));
    // Anchor-only fields elided on non-initial pages.
    assert!(body["current_index"].is_null());
    assert!(body["total_matched"].is_null());
    assert!(body["total_entries"].is_null());
    assert_eq!(body["has_more_before"], true);
    assert_eq!(body["has_more_after"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_cursor_paginates_backward_in_ascending_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dave@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 40).await;

    let url =
        format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=before&cursor=10&limit=4");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    // The 4 entries just before position 10 → positions 6..=9, returned
    // ascending so the client can prepend without sorting.
    assert_eq!(items[0]["position"].as_i64(), Some(6));
    assert_eq!(items[3]["position"].as_i64(), Some(9));
    assert_eq!(body["min_position"].as_i64(), Some(6));
    assert_eq!(body["max_position"].as_i64(), Some(9));
    assert_eq!(body["has_more_before"], true);
    assert_eq!(body["has_more_after"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_at_tail_returns_empty_with_no_more() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "eve@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 10).await;

    let url =
        format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=after&cursor=9&limit=10");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
    assert_eq!(body["has_more_after"], false);
    assert_eq!(body["has_more_before"], false);
    assert!(body["min_position"].is_null());
    assert!(body["max_position"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_at_head_returns_empty_with_no_more() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "frank@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 10).await;

    let url =
        format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=before&cursor=0&limit=10");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
    assert_eq!(body["has_more_before"], false);
    assert_eq!(body["has_more_after"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unowned_list_returns_403() {
    // `ensure_owner` returns 403 (not 404) so callers can distinguish
    // "list exists but isn't yours" from "no such list" — the rail
    // surfaces a sharing-friendly error message in the first case.
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@window.test").await;
    let other = register(&app, "other@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, owner.user_id, 5).await;

    let url = format!("/api/me/cbl-lists/{list_id}/window-paginated");
    let (status, _) = get_json(&app, &url, &other).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_direction_returns_400() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "gary@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 5).await;

    let url = format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=sideways");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "invalid_direction");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_without_cursor_returns_400() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "hank@window.test").await;
    let (list_id, _) = seed_cbl_with_matched_entries(&app, auth.user_id, 5).await;

    let url = format!("/api/me/cbl-lists/{list_id}/window-paginated?direction=after");
    let (status, body) = get_json(&app, &url, &auth).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "missing_cursor");
}
