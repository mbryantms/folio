//! WP-4.3 — `/me/issues/{issue_id}/page-overrides`: per-user manual
//! spread controls for the double-page reader (force spread / force
//! single / shift pairing). Covers the round trip, cross-device
//! persistence (a second session reads the same row), per-user
//! isolation, validation, reset, and library-ACL + age-rating
//! visibility.

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
    library_user_access::ActiveModel as LibraryAccessAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    // Non-JSON bodies (e.g. axum's default `QueryRejection` plain-text
    // response) become `Null` so tests that only assert on status
    // don't crash here.
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
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

/// Seed a library + series + active issue with page_count=20. Returns
/// (library_id, series_id, issue_id).
async fn seed_issue(app: &TestApp, slug: &str) -> (Uuid, Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let series_id = Uuid::now_v7();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    let now = Utc::now().fixed_offset();

    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {slug}")),
        root_path: Set(format!("/tmp/{slug}-{lib_id}")),
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
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(slug.into()),
        normalized_name: Set(normalize_name(slug)),
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
        slug: Set(slug.into()),
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

    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("{slug}-1")),
        file_path: Set(format!("/tmp/{slug}.cbz")),
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
        metroninfo_present: Set(None),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
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

/// Grant a non-admin user explicit access to `library_id`. Required
/// because the markers ACL falls back to library_user_access when the
/// caller isn't an admin.
async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    LibraryAccessAM {
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

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

/// A second, independent session for an existing user — stands in for
/// "the same user on another device".
async fn login(app: &TestApp, email: &str, user_id: Uuid) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
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
        user_id,
    }
}

async fn set_issue_age_rating(app: &TestApp, issue_id: &str, rating: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: IssueAM = row.into();
    am.age_rating = Set(Some(rating.to_owned()));
    am.update(&db).await.unwrap();
}

async fn cap_library(app: &TestApp, user_id: Uuid, library_id: Uuid, cap: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = entity::library_user_access::Entity::find_by_id((library_id, user_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: LibraryAccessAM = row.into();
    am.age_rating_max = Set(Some(cap.to_owned()));
    am.update(&db).await.unwrap();
}

async fn override_rows(app: &TestApp) -> usize {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::issue_page_override::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .len()
}

fn uri(issue_id: &str) -> String {
    format!("/api/me/issues/{issue_id}/page-overrides")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn defaults_then_round_trip_normalized_and_persisted_across_sessions() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "alpha").await;

    // No row yet → all-default view.
    let (s, body) = http(&app, Method::GET, &uri(&issue_id), Some(&alice), None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["issue_id"], issue_id);
    assert_eq!(body["shift_pairing"], false);
    assert_eq!(body["spread_pages"], serde_json::json!([]));
    assert_eq!(body["single_pages"], serde_json::json!([]));
    assert!(body["updated_at"].is_null());

    // Unsorted + duplicated input comes back sorted + unique.
    let (s, body) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(serde_json::json!({
            "shift_pairing": true,
            "spread_pages": [9, 4, 9],
            "single_pages": [12, 2],
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["shift_pairing"], true);
    assert_eq!(body["spread_pages"], serde_json::json!([4, 9]));
    assert_eq!(body["single_pages"], serde_json::json!([2, 12]));
    assert!(body["updated_at"].is_string());

    // "Another device": a fresh session for the same user sees the row.
    let alice_phone = login(&app, "alice@example.com", alice.user_id).await;
    let (s, body) = http(&app, Method::GET, &uri(&issue_id), Some(&alice_phone), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["shift_pairing"], true);
    assert_eq!(body["spread_pages"], serde_json::json!([4, 9]));
    assert_eq!(body["single_pages"], serde_json::json!([2, 12]));

    // PUT replaces the whole set (not a merge).
    let (s, body) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice_phone),
        Some(serde_json::json!({ "spread_pages": [5] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["shift_pairing"], false);
    assert_eq!(body["spread_pages"], serde_json::json!([5]));
    assert_eq!(body["single_pages"], serde_json::json!([]));
    let (_, body) = http(&app, Method::GET, &uri(&issue_id), Some(&alice), None).await;
    assert_eq!(body["spread_pages"], serde_json::json!([5]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_put_and_delete_reset_to_automatic() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "alpha").await;

    let set = serde_json::json!({ "shift_pairing": true });
    let (s, _) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(set.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(override_rows(&app).await, 1);

    // All-default body deletes the row instead of storing a no-op.
    let (s, body) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["updated_at"].is_null());
    assert_eq!(override_rows(&app).await, 0);

    let (s, _) = http(&app, Method::PUT, &uri(&issue_id), Some(&alice), Some(set)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = http(&app, Method::DELETE, &uri(&issue_id), Some(&alice), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(override_rows(&app).await, 0);
    // DELETE is idempotent.
    let (s, _) = http(&app, Method::DELETE, &uri(&issue_id), Some(&alice), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_rejects_overlap_out_of_range_and_negative() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "alpha").await; // 20 pages

    for bad in [
        serde_json::json!({ "spread_pages": [3], "single_pages": [3] }),
        serde_json::json!({ "spread_pages": [20] }),
        serde_json::json!({ "single_pages": [-1] }),
    ] {
        let (s, body) = http(
            &app,
            Method::PUT,
            &uri(&issue_id),
            Some(&alice),
            Some(bad.clone()),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad} → {body}");
        assert!(body["error"]["code"].is_string(), "{body}");
    }
    assert_eq!(override_rows(&app).await, 0);
    // Wrong JSON shape is rejected before the handler runs.
    let (s, _) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(serde_json::json!({ "spread_pages": "nope" })),
    )
    .await;
    assert!(s.is_client_error(), "{s}");
    // Unknown issue → 404.
    let (s, _) = http(
        &app,
        Method::GET,
        &uri("does-not-exist"),
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Unauthenticated → 401.
    let (s, _) = http(&app, Method::GET, &uri(&issue_id), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overrides_are_per_user() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let bob = register(&app, "bob@example.com").await;
    let (lib, _series, issue_id) = seed_issue(&app, "alpha").await;
    grant_library(&app, bob.user_id, lib).await;

    let (s, _) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(serde_json::json!({ "spread_pages": [7] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, body) = http(&app, Method::GET, &uri(&issue_id), Some(&bob), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["spread_pages"], serde_json::json!([]));

    // Bob's reset doesn't touch Alice's row.
    let (s, _) = http(&app, Method::DELETE, &uri(&issue_id), Some(&bob), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, body) = http(&app, Method::GET, &uri(&issue_id), Some(&alice), None).await;
    assert_eq!(body["spread_pages"], serde_json::json!([7]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invisible_issue_is_404_for_read_and_write() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;
    let bob = register(&app, "bob@example.com").await;
    let (lib, _series, issue_id) = seed_issue(&app, "alpha").await;
    let body = serde_json::json!({ "spread_pages": [1] });

    // No library grant → the issue doesn't exist as far as Bob can tell.
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        let b = (method == Method::PUT).then(|| body.clone());
        let (s, _) = http(&app, method.clone(), &uri(&issue_id), Some(&bob), b).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{method} without grant");
    }

    // Granted, but the issue's age rating is above Bob's cap.
    grant_library(&app, bob.user_id, lib).await;
    set_issue_age_rating(&app, &issue_id, "Adults Only 18+").await;
    cap_library(&app, bob.user_id, lib, "Everyone 10+").await;
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        let b = (method == Method::PUT).then(|| body.clone());
        let (s, _) = http(&app, method.clone(), &uri(&issue_id), Some(&bob), b).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{method} above age cap");
    }
    assert_eq!(
        override_rows(&app).await,
        0,
        "no row for an invisible issue"
    );

    // Admin still reads/writes it.
    let (s, _) = http(&app, Method::PUT, &uri(&issue_id), Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_cascade_with_the_issue() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "alpha").await;
    let (s, _) = http(
        &app,
        Method::PUT,
        &uri(&issue_id),
        Some(&alice),
        Some(serde_json::json!({ "shift_pairing": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(override_rows(&app).await, 1);
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::issue::Entity::delete_by_id(issue_id.clone())
        .exec(&db)
        .await
        .unwrap();
    assert_eq!(override_rows(&app).await, 0);
}
