//! Home-rail integration tests — Continue Reading + Rail Dismissals (M1).
//!
//! Covers:
//!   - Seed: system saved views (`continue_reading`, `on_deck`) are present
//!     and auto-pinned for a fresh user.
//!   - System view immutability: admin PATCH/DELETE are 403.
//!   - Continue Reading happy path: in-progress issue surfaces; finished
//!     issue does not; unread issue does not; removed issue does not.
//!   - Ordering: most-recently-updated first.
//!   - Library ACL: a user without access doesn't see the issue.
//!   - Dismiss lifecycle: dismissing hides the card; subsequent progress
//!     write past `dismissed_at` auto-restores it; explicit DELETE on the
//!     dismissal also restores it.
//!   - Validation: bad kind / missing target / nonexistent target.

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
    library, library_user_access, progress_record, saved_view,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set, Unchanged};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

// ───── Test scaffolding (mirrors the pattern in saved_views.rs) ─────

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

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("user".into());
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Insert one library and one active issue under one series. Returns the
/// (library_id, series_id, issue_id) tuple so the test can write progress
/// + ACL grants against them.
async fn seed_one_issue(app: &TestApp, slug_prefix: &str) -> (Uuid, Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {slug_prefix}")),
        root_path: Set(format!("/tmp/{slug_prefix}-{lib_id}")),
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
        name: Set(format!("Series {slug_prefix}")),
        normalized_name: Set(normalize_name(&format!("Series {slug_prefix}"))),
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
        slug: Set(format!("{slug_prefix}-series")),
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

    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("{slug_prefix}-1")),
        file_path: Set(format!("/tmp/{slug_prefix}/1.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(Some("Issue One".into())),
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
        hash_algorithm: Set(0),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(Some(0)),
    }
    .insert(&db)
    .await
    .unwrap();

    (lib_id, series_id, issue_id)
}

/// Add another active issue under an existing series. `slug` is just for
/// the file path / issue slug — it doesn't need to match the series slug.
async fn seed_extra_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    sort_number: f64,
    slug: &str,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), sort_number as u8);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug.into()),
        file_path: Set(format!("/tmp/{slug}/{sort_number}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(Some(format!("Issue {sort_number}"))),
        sort_number: Set(Some(sort_number)),
        number_raw: Set(Some(format!("{sort_number}"))),
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
        hash_algorithm: Set(0),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(Some(0)),
    }
    .insert(&db)
    .await
    .unwrap();
    issue_id
}

/// Seed an admin-owned CBL list with matched entries at sequential
/// positions. Each tuple in `entries` is `(position, matched_issue_id)`.
async fn seed_cbl_list(app: &TestApp, name: &str, entries: &[(i32, &str)]) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set(name.into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(entries.len() as i32)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    for (pos, issue_id) in entries {
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(*pos),
            series_name: Set("Series".into()),
            issue_number: Set(format!("{pos}")),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(Some((*issue_id).into())),
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
    list_id
}

async fn grant_access(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
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

/// Write a progress row directly (skipping the rate-limit + library-ACL
/// path in `/progress`). Useful for setting up "in the past" timestamps so
/// the auto-restore comparison has a clean before/after to assert on.
async fn write_progress(
    app: &TestApp,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    finished: bool,
    when: chrono::DateTime<chrono::FixedOffset>,
) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let existing = progress_record::Entity::find_by_id((user_id, issue_id.to_owned()))
        .one(&db)
        .await
        .unwrap();
    let am = progress_record::ActiveModel {
        user_id: if existing.is_some() {
            Unchanged(user_id)
        } else {
            Set(user_id)
        },
        issue_id: if existing.is_some() {
            Unchanged(issue_id.to_owned())
        } else {
            Set(issue_id.to_owned())
        },
        last_page: Set(last_page),
        // Match the percent the production handler would compute (page / 20).
        percent: Set((last_page as f64 / 20.0).clamp(0.0, 1.0)),
        finished: Set(finished),
        updated_at: Set(when),
        device: Set(None),
    };
    if existing.is_some() {
        am.update(&db).await.unwrap();
    } else {
        am.insert(&db).await.unwrap();
    }
}

// ───── Tests ─────

#[tokio::test]
async fn system_rails_seeded_and_immutable() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-seed@example.com").await;

    // The list should include both system rails, both pinned, in seed order.
    let (status, body) = http(
        &app,
        Method::GET,
        "/me/saved-views?pinned=true",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    let system_keys: Vec<&str> = items
        .iter()
        .filter(|v| v["kind"] == "system")
        .filter_map(|v| v["system_key"].as_str())
        .collect();
    assert!(system_keys.contains(&"continue_reading"));
    assert!(system_keys.contains(&"on_deck"));
    for v in items.iter().filter(|v| v["kind"] == "system") {
        assert_eq!(v["pinned"], true);
        assert_eq!(v["is_system"], true);
    }

    // First user is auto-admin per project default — leave that, since
    // PATCH+DELETE against system rails should 403 even for admins.
    let cr_id = items
        .iter()
        .find(|v| v["system_key"] == "continue_reading")
        .and_then(|v| v["id"].as_str())
        .unwrap()
        .to_owned();
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/admin/saved-views/{cr_id}"),
        Some(&user),
        Some(json!({"name": "renamed"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "system rails cannot be edited"
    );

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/admin/saved-views/{cr_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "system rails cannot be deleted"
    );
}

#[tokio::test]
async fn continue_reading_includes_only_in_progress_issues() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-cr@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, _series_id, issue_id) = seed_one_issue(&app, "cr").await;
    grant_access(&app, user.user_id, lib_id).await;

    // Empty rail initially.
    let (status, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);

    // Write in-progress: last_page=5, !finished.
    let t0 = Utc::now().fixed_offset();
    write_progress(&app, user.user_id, &issue_id, 5, false, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "in-progress issue should appear");
    assert_eq!(items[0]["issue"]["id"], issue_id);
    assert_eq!(items[0]["progress"]["last_page"], 5);
    assert!(items[0]["series_name"].is_string());

    // Mark finished — should drop out.
    write_progress(&app, user.user_id, &issue_id, 19, true, t0).await;
    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "finished issue should not appear"
    );

    // Re-open (back to in-progress) — should re-appear.
    write_progress(&app, user.user_id, &issue_id, 3, false, t0).await;
    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        1,
        "re-opened issue should re-appear"
    );
}

#[tokio::test]
async fn continue_reading_skips_invisible_libraries() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-acl@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (_lib_id, _series_id, issue_id) = seed_one_issue(&app, "acl").await;
    // No ACL grant. Even though we write progress (via direct DB), the rail
    // query should hide the issue.
    let t0 = Utc::now().fixed_offset();
    write_progress(&app, user.user_id, &issue_id, 4, false, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "issue in non-visible library should not appear",
    );
}

#[tokio::test]
async fn continue_reading_orders_by_most_recent_activity() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-order@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_a, _, issue_a) = seed_one_issue(&app, "old").await;
    let (lib_b, _, issue_b) = seed_one_issue(&app, "new").await;
    grant_access(&app, user.user_id, lib_a).await;
    grant_access(&app, user.user_id, lib_b).await;

    let old = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    let new = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue_a, 4, false, old).await;
    write_progress(&app, user.user_id, &issue_b, 2, false, new).await;

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0]["issue"]["id"], issue_b,
        "most-recent activity comes first"
    );
    assert_eq!(items[1]["issue"]["id"], issue_a);
}

#[tokio::test]
async fn dismissal_hides_and_auto_restores() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-dismiss@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, _, issue_id) = seed_one_issue(&app, "dismiss").await;
    grant_access(&app, user.user_id, lib_id).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue_id, 5, false, t0).await;

    // Visible to start.
    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    // Dismiss.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "issue", "target_id": issue_id})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "dismissed issue should disappear"
    );

    // Write fresh progress in the future — auto-restore.
    let t_new = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue_id, 7, false, t_new).await;

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        1,
        "new progress past dismissed_at should restore the card"
    );
}

#[tokio::test]
async fn dismissal_delete_explicitly_restores() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-restore@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, _, issue_id) = seed_one_issue(&app, "restore").await;
    grant_access(&app, user.user_id, lib_id).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue_id, 5, false, t0).await;

    http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "issue", "target_id": issue_id})),
    )
    .await;

    // DELETE on the dismissal restores immediately, even without new
    // progress activity.
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/me/rail-dismissals/issue/{issue_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = http(&app, Method::GET, "/me/continue-reading", Some(&user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    // Re-deleting the same dismissal returns 404 (nothing to remove).
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/me/rail-dismissals/issue/{issue_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dismissal_validation_rejects_bad_input() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-valid@example.com").await;
    demote_to_user(&app, user.user_id).await;

    // Bad kind.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "garbage", "target_id": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty target_id.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "issue", "target_id": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nonexistent issue → 404.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "issue", "target_id": "missing-issue-id"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Non-UUID series target → 400.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "series", "target_id": "not-a-uuid"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ───── On Deck ─────

#[tokio::test]
async fn on_deck_series_next_after_finishing_an_issue() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-series@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-series").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-series-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    // Finish issue 1. Issue 2 (unread) is now what's "on deck".
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (status, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "exactly one series_next card");
    assert_eq!(items[0]["kind"], "series_next");
    assert_eq!(items[0]["issue"]["id"], issue2_id);
}

#[tokio::test]
async fn on_deck_excludes_series_with_in_progress_issue() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-skip@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-skip").await;
    let _issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-skip-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    // Mid-issue progress on issue 1 means this series is in Continue
    // Reading, not On Deck.
    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 5, false, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "series with in-progress issue must not appear in On Deck"
    );
}

#[tokio::test]
async fn on_deck_cbl_next_picks_lowest_unfinished_position() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-cbl@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-cbl").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-cbl-2").await;
    let issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "od-cbl-3").await;
    grant_access(&app, user.user_id, lib_id).await;

    let list_id = seed_cbl_list(
        &app,
        "Trilogy",
        &[(0, &issue1_id), (1, &issue2_id), (2, &issue3_id)],
    )
    .await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    // Finish entry 0 — so entry 1 is next.
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let items = body["items"].as_array().unwrap();
    let cbl_card = items
        .iter()
        .find(|i| i["kind"] == "cbl_next")
        .expect("cbl_next card should exist");
    assert_eq!(cbl_card["cbl_list_id"], list_id.to_string());
    assert_eq!(cbl_card["cbl_list_name"], "Trilogy");
    assert_eq!(cbl_card["position"], 2, "1-based, so position 1 → 2");
    assert_eq!(cbl_card["issue"]["id"], issue2_id);
}

#[tokio::test]
async fn on_deck_cbl_wins_when_issue_overlaps_series_next() {
    // When the same issue is the next-unread in both a user's series and a
    // CBL that contains it, surface the CBL card only — the CBL frame
    // (list name + 1-based position) carries strictly more context than
    // the bare series card and the duplicate is the symptom this dedup
    // exists to prevent.
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-overlap@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-overlap").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-overlap-2").await;
    let _issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "od-overlap-3").await;
    grant_access(&app, user.user_id, lib_id).await;

    // CBL covers the same three issues as the series — so once issue 1 is
    // finished, both queries pick issue 2 as "next".
    let list_id = seed_cbl_list(
        &app,
        "Overlap",
        &[(0, &issue1_id), (1, &issue2_id), (2, &_issue3_id)],
    )
    .await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let items = body["items"].as_array().unwrap();

    // Exactly one card referencing issue2, and it must be the CBL framing.
    let cards_for_issue2: Vec<_> = items
        .iter()
        .filter(|i| i["issue"]["id"] == issue2_id)
        .collect();
    assert_eq!(
        cards_for_issue2.len(),
        1,
        "issue 2 must appear exactly once across On Deck cards, got {} ({:?})",
        cards_for_issue2.len(),
        items
    );
    assert_eq!(
        cards_for_issue2[0]["kind"], "cbl_next",
        "CBL wins on overlap"
    );
    assert_eq!(cards_for_issue2[0]["cbl_list_id"], list_id.to_string());

    // And no orphan SeriesNext for the same series should remain.
    let series_cards_for_series: Vec<_> = items
        .iter()
        .filter(|i| i["kind"] == "series_next" && i["issue"]["id"] == issue2_id)
        .collect();
    assert!(
        series_cards_for_series.is_empty(),
        "SeriesNext duplicate of the CBL pick must be filtered out"
    );
}

#[tokio::test]
async fn on_deck_excludes_caught_up_cbls() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-done@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-done").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-done-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let _list_id = seed_cbl_list(&app, "Done", &[(0, &issue1_id), (1, &issue2_id)]).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    // Finish both matched entries → CBL is caught up.
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;
    write_progress(&app, user.user_id, &issue2_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let cbl_cards: Vec<_> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "cbl_next")
        .collect();
    assert_eq!(
        cbl_cards.len(),
        0,
        "fully-read CBL must not show up in On Deck"
    );
}

#[tokio::test]
async fn on_deck_dismissal_hides_series_and_auto_restores() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-dismiss@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-dismiss").await;
    // issue2 is required so `pick_next_in_series` has an unread pick once
    // issue1 is marked finished.
    let _issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-dismiss-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    // Use a past timestamp so the dismissal row (written at real-clock NOW)
    // is *after* the initial activity. Auto-restore only fires when new
    // progress lands past dismissed_at.
    let t_old = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t_old).await;

    // Card appears initially.
    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    // Dismiss the series.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/rail-dismissals",
        Some(&user),
        Some(json!({"target_kind": "series", "target_id": series_id.to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 0, "dismissed");

    // New activity in the future → auto-restore. Re-save issue1's
    // finished state with a fresher timestamp; only "meaningful" progress
    // (finished OR last_page > 0) counts toward the candidate CTE since
    // the mark-all-unread fix, so a zero-progress bump no longer works as
    // an activity signal.
    let t_new = chrono::DateTime::parse_from_rfc3339("2031-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t_new).await;
    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        1,
        "auto-restore on new activity"
    );
}

#[tokio::test]
async fn on_deck_excludes_fully_unread_series() {
    // After a user marks an entire series as unread, "mark all as unread"
    // writes (last_page = 0, finished = false) rows on every formerly-
    // touched issue. The series should drop off On Deck — there's no
    // genuine "next up after the issue I finished" signal anymore.
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-unread@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-unread").await;
    let _issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-unread-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    // Baseline: card present after finishing issue 1.
    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["kind"], "series_next");
    let _ = series_id;

    // Simulate "mark all unread" — zeroed progress row on the formerly
    // finished issue.
    let t1 = chrono::DateTime::parse_from_rfc3339("2030-02-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 0, false, t1).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        0,
        "fully-unread series must not appear in On Deck"
    );
}

#[tokio::test]
async fn on_deck_cbl_carve_out_for_fully_unread_series() {
    // The single carve-out from the mark-all-unread filter: if the series'
    // first issue is the next-up in a CBL list the user has progress in,
    // the CBL card still surfaces (the series_next path is suppressed,
    // but the CBL path runs independently and shows the issue with its
    // list/position framing).
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-cbl-unread@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-cbl-unread").await;
    let _issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-cbl-unread-2").await;
    grant_access(&app, user.user_id, lib_id).await;
    let _ = series_id;

    let list_id = seed_cbl_list(&app, "CBL Carve", &[(0, &issue1_id), (1, &_issue2_id)]).await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    // Now "mark all unread" the series → both issues end up at zeroed
    // progress rows.
    let t1 = chrono::DateTime::parse_from_rfc3339("2030-02-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 0, false, t1).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let items = body["items"].as_array().unwrap();
    let cbl_cards: Vec<_> = items.iter().filter(|i| i["kind"] == "cbl_next").collect();
    let series_cards: Vec<_> = items
        .iter()
        .filter(|i| i["kind"] == "series_next")
        .collect();
    assert!(
        series_cards.is_empty(),
        "fully-unread series must not surface as series_next, got {series_cards:?}"
    );
    assert_eq!(
        cbl_cards.len(),
        1,
        "CBL carve-out: list still surfaces its next-unfinished entry"
    );
    assert_eq!(cbl_cards[0]["cbl_list_id"], list_id.to_string());
    assert_eq!(cbl_cards[0]["issue"]["id"], issue1_id);
    assert_eq!(cbl_cards[0]["position"], 1);
}

// ───── B-2: cbl_saved_view_id on CblNext ─────

/// Wrap a CBL list in a kind='cbl' saved view so the on-deck handler
/// can populate the `cbl_saved_view_id` field. Duplicated from the
/// next_up test helpers by design (the two test files cover different
/// handlers and the helper is tiny).
async fn seed_cbl_saved_view(
    app: &TestApp,
    owner_user_id: Option<Uuid>,
    cbl_list_id: Uuid,
    name: &str,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(owner_user_id),
        kind: Set("cbl".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(vec![]),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(Some(cbl_list_id)),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn on_deck_cbl_next_carries_saved_view_id_when_one_exists() {
    // B-2 fix: the home On Deck rail's CBL card must surface the
    // saved-view id so the web can thread `?cbl=` onto the reader URL
    // and keep the user's CBL context across page turns.
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-cbl-sv@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-cbl-sv").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-cbl-sv-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let list_id = seed_cbl_list(&app, "Wrapped", &[(0, &issue1_id), (1, &issue2_id)]).await;
    let sv_id = seed_cbl_saved_view(&app, None, list_id, "Wrapped view").await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let cbl_card = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "cbl_next")
        .expect("cbl_next card should exist");
    assert_eq!(cbl_card["cbl_list_id"], list_id.to_string());
    assert_eq!(
        cbl_card["cbl_saved_view_id"],
        sv_id.to_string(),
        "saved-view id missing — web can't thread `?cbl=` without it"
    );
}

#[tokio::test]
async fn on_deck_cbl_next_omits_saved_view_id_when_no_view_wraps_the_list() {
    // No saved view → field is absent (serde-skipped). The reader
    // still works, just without persistent CBL context.
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-cbl-no-sv@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-cbl-no-sv").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-cbl-no-sv-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let list_id = seed_cbl_list(&app, "Bare", &[(0, &issue1_id), (1, &issue2_id)]).await;
    let _ = list_id;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let cbl_card = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "cbl_next")
        .expect("cbl_next card should exist");
    assert!(
        cbl_card.get("cbl_saved_view_id").is_none() || cbl_card["cbl_saved_view_id"].is_null(),
        "expected cbl_saved_view_id absent / null; got {:?}",
        cbl_card.get("cbl_saved_view_id")
    );
}

#[tokio::test]
async fn on_deck_cbl_saved_view_tiebreak_prefers_user_owned() {
    // Tiebreak: if both a user-owned and a system-owned saved view
    // wrap the same CBL, the user-owned one wins.
    let app = TestApp::spawn().await;
    let user = register(&app, "rail-od-cbl-tiebreak@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "od-cbl-tb").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "od-cbl-tb-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    let list_id = seed_cbl_list(&app, "Shared", &[(0, &issue1_id), (1, &issue2_id)]).await;
    // System saved view inserted FIRST (lower id); user-owned second.
    // Without the tiebreak the system one would win on id order alone.
    let _sys_sv = seed_cbl_saved_view(&app, None, list_id, "System wrap").await;
    let user_sv = seed_cbl_saved_view(&app, Some(user.user_id), list_id, "My wrap").await;

    let t0 = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap();
    write_progress(&app, user.user_id, &issue1_id, 19, true, t0).await;

    let (_, body) = http(&app, Method::GET, "/me/on-deck", Some(&user), None).await;
    let cbl_card = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "cbl_next")
        .expect("cbl_next card should exist");
    assert_eq!(
        cbl_card["cbl_saved_view_id"],
        user_sv.to_string(),
        "user-owned saved view must win the tiebreak over system-owned"
    );
}
