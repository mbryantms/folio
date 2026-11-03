//! Start Here + default pins M2 — shared + featured reading lists.
//!
//! Covers:
//!   - A brand-new account gets the `start_here` rail pinned first; a user
//!     who already has pin rows never gets it pushed onto their page.
//!   - Admin share: an admin's own CBL view becomes a shared list every
//!     user can see; sharing someone else's / a non-CBL view is 403.
//!   - Shared lists are admin-managed: a regular user can read but not
//!     rename, re-match, refresh or delete them.
//!   - `PUT /admin/start-here` sets the featured order (422 for a private
//!     view, 403 for non-admins) and `GET /me/start-here` returns per-user
//!     counts + covers, hiding lists the caller can't open.

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
    library, library_user_access, progress_record,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, EntityTrait, Set, Unchanged};
use tower::ServiceExt;
use uuid::Uuid;

const START_HERE_ID: &str = "00000000-0000-0000-0000-000000000040";

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
    am.role = Set(entity::user::UserRole::User);
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
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
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
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
        series_json_present: Set(None),
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
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
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
        state: Set(entity::issue::IssueState::Active),
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
        hash_algorithm: Set(0),
        metroninfo_present: Set(None),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        comicinfo_count: Set(Some(0)),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        last_sidecar_rewrite_at: Set(None),
        metron_info_raw: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    (lib_id, series_id, issue_id)
}

/// A shared CBL list whose entries can be unmatched (`None`), with
/// `match_status = 'missing'` — for exercising lists whose head entry
/// has no local match.
async fn seed_cbl_list_mixed(app: &TestApp, name: &str, entries: &[(i32, Option<&str>)]) -> Uuid {
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
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    for (pos, issue_id) in entries {
        let matched = issue_id.is_some();
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
            matched_issue_id: Set(issue_id.map(Into::into)),
            match_status: Set(if matched { "matched" } else { "missing" }.into()),
            match_method: Set(matched.then(|| "test".into())),
            match_confidence: Set(matched.then_some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(matched.then_some(now)),
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
        finished_at: Set(if finished { Some(when) } else { None }),
        updated_at: Set(when),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    };
    if existing.is_some() {
        am.update(&db).await.unwrap();
    } else {
        am.insert(&db).await.unwrap();
    }
}

/// An admin-owned CBL list + saved view over `entries` (the shape a normal
/// import produces). Returns `(list_id, view_id)`.
async fn seed_owned_cbl_view(
    app: &TestApp,
    owner: Uuid,
    name: &str,
    entries: &[(i32, Option<&str>)],
) -> (Uuid, Uuid) {
    let list_id = seed_cbl_list_mixed(app, name, entries).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "UPDATE cbl_lists SET owner_user_id = '{owner}' WHERE id = '{list_id}'"
    ))
    .await
    .unwrap();
    let view_id = Uuid::now_v7();
    db.execute_unprepared(&format!(
        "INSERT INTO saved_views (id, user_id, kind, name, custom_tags, cbl_list_id, auto_pin) \
         VALUES ('{view_id}', '{owner}', 'cbl', '{name}', ARRAY[]::text[], '{list_id}', FALSE)"
    ))
    .await
    .unwrap();
    (list_id, view_id)
}

async fn pinned_ids(app: &TestApp, auth: &Authed) -> Vec<String> {
    let (status, json) = http(
        app,
        Method::GET,
        "/api/me/saved-views?pinned=true",
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json:#?}");
    let mut items: Vec<(i64, String)> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["pinned_position"].as_i64().unwrap_or(i64::MAX),
                v["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    items.sort();
    items.into_iter().map(|(_, id)| id).collect()
}

// ───── Tests ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_account_gets_start_here_first_existing_users_do_not() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let pins = pinned_ids(&app, &alice).await;
    assert_eq!(
        pins.first().map(String::as_str),
        Some(START_HERE_ID),
        "{pins:?}"
    );

    // A user who already has pin rows (everyone who existed before this
    // release) never gets it re-added, even after unpinning it entirely.
    let db = Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "DELETE FROM user_view_pins WHERE user_id = '{}' AND view_id = '{START_HERE_ID}'",
        alice.user_id
    ))
    .await
    .unwrap();
    let pins = pinned_ids(&app, &alice).await;
    assert!(!pins.iter().any(|p| p == START_HERE_ID), "{pins:?}");
    assert!(!pins.is_empty(), "the auto-pinned rails are still there");
}

/// Regression: the Want to Read seed (collections / OPDS surfaces) can
/// write an unpinned pin row before the first home visit; that must not
/// make a brand-new account look like an existing one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_here_still_seeds_when_want_to_read_seeded_first() {
    let app = TestApp::spawn().await;
    let carol = register(&app, "carol@example.com").await;
    let (status, _) = http(&app, Method::GET, "/api/me/collections", Some(&carol), None).await;
    assert_eq!(status, StatusCode::OK);
    let pins = pinned_ids(&app, &carol).await;
    assert_eq!(
        pins.first().map(String::as_str),
        Some(START_HERE_ID),
        "{pins:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_shares_and_features_a_list_for_everyone() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let bob = register(&app, "bob@example.com").await;
    demote_to_user(&app, bob.user_id).await;

    let (lib_id, _series, issue_a) = seed_one_issue(&app, "sh").await;
    let (list_id, view_id) = seed_owned_cbl_view(
        &app,
        admin.user_id,
        "Intro to Marvel",
        &[(1, Some(issue_a.as_str())), (2, None)],
    )
    .await;

    // Not shared yet → featuring it is refused, and Bob can't see it.
    let put = |ids: Vec<String>| serde_json::json!({ "view_ids": ids });
    let (status, _) = http(
        &app,
        Method::PUT,
        "/api/admin/start-here",
        Some(&admin),
        Some(put(vec![view_id.to_string()])),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Only the owner can share, and only an admin.
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/admin/saved-views/{view_id}/share"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, json) = http(
        &app,
        Method::POST,
        &format!("/api/admin/saved-views/{view_id}/share"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json:#?}");

    // Shared: Bob can read it but not manage it.
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular users can't delete a shared list"
    );
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&bob),
        Some(serde_json::json!({ "refresh_schedule": null })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular users can't edit a shared list"
    );

    // Non-admins can't feature; admins can.
    let (status, _) = http(
        &app,
        Method::PUT,
        "/api/admin/start-here",
        Some(&bob),
        Some(put(vec![view_id.to_string()])),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, json) = http(
        &app,
        Method::PUT,
        "/api/admin/start-here",
        Some(&admin),
        Some(put(vec![view_id.to_string()])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json:#?}");
    assert_eq!(json["items"][0]["view_id"], view_id.to_string());

    // Bob has no access to the library yet → the list is hidden for him.
    let (_, json) = http(&app, Method::GET, "/api/me/start-here", Some(&bob), None).await;
    assert_eq!(json["items"].as_array().unwrap().len(), 0, "{json:#?}");

    grant_access(&app, bob.user_id, lib_id).await;
    write_progress(
        &app,
        bob.user_id,
        &issue_a,
        20,
        true,
        Utc::now().fixed_offset(),
    )
    .await;
    let (_, json) = http(&app, Method::GET, "/api/me/start-here", Some(&bob), None).await;
    let item = &json["items"][0];
    assert_eq!(item["name"], "Intro to Marvel");
    assert_eq!(item["total"], 2);
    assert_eq!(item["available"], 1);
    assert_eq!(item["read_count"], 1);
    assert_eq!(item["cover_issue_ids"], serde_json::json!([issue_a]));

    // Empty PUT unfeatures everything.
    let (status, json) = http(
        &app,
        Method::PUT,
        "/api/admin/start-here",
        Some(&admin),
        Some(put(vec![])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_rejects_a_view_the_admin_does_not_own() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let bob = register(&app, "bob@example.com").await;
    demote_to_user(&app, bob.user_id).await;
    let (_list, view_id) = seed_owned_cbl_view(&app, bob.user_id, "Bobs list", &[]).await;
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/admin/saved-views/{view_id}/share"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
