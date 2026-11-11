//! Starter reading lists + first-run tour state (onboarding PR C).
//!
//! Coverage rows are seeded directly through `server::starter_lists::store`
//! so no test fetches from GitHub: every listed pick has a fresh row, so
//! the handler never starts a background run.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_list,
    issue::ActiveModel as IssueAM,
    library, library_user_access, saved_view,
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

/// Insert a library, one series, and one active issue belonging to it.
/// Returns the IDs so tests can ref them as entries.
async fn seed_series_with_issue(app: &TestApp, slug: &str) -> (Uuid, Uuid, String) {
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
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
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
        declared_age_rating: Set(None),
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
        state: Set(entity::issue::IssueState::Active),
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

async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(None),
        hide_unrated: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn seed_catalog_list(app: &TestApp, source: Uuid, path: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("catalog".into()),
        source_url: Set(None),
        catalog_source_id: Set(Some(source)),
        catalog_path: Set(Some(path.into())),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set("Imported".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(0)),
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
    list_id
}

/// Wrap a CBL list in a kind='cbl' saved view so the next-up resolver
/// can find it via the `?cbl=<saved_view_id>` query param.
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
        kind: Set(entity::saved_view::SavedViewKind::Cbl),
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
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

const DT: Uuid = server::starter_lists::DIESELTECH_SOURCE_ID;
const HAWKEYE: &str = "Marvel/Creator Runs/[Marvel] [2012-2015] Hawkeye by Fraction and Aja.cbl";

/// Give every shipped pick a fresh, empty coverage row.
async fn seed_all_coverage(app: &TestApp) {
    let db = Database::connect(&app.db_url).await.unwrap();
    for m in server::starter_lists::MANIFEST {
        server::starter_lists::store(&db, DT, m.path, 10, &[], None)
            .await
            .unwrap();
    }
}

fn find<'a>(body: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["catalog_path"] == path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn onboarding_flag_round_trips() {
    let app = TestApp::spawn().await;
    let user = register(&app, "tour@example.com").await;
    let (_, me) = http(&app, Method::GET, "/auth/me", Some(&user), None).await;
    assert!(
        me["onboarding_completed_at"].is_null(),
        "new accounts get the tour"
    );

    let (status, me) = http(
        &app,
        Method::PATCH,
        "/me/preferences",
        Some(&user),
        Some(serde_json::json!({ "onboarding_completed": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me:#?}");
    assert!(me["onboarding_completed_at"].is_string());

    let (_, me) = http(
        &app,
        Method::PATCH,
        "/me/preferences",
        Some(&user),
        Some(serde_json::json!({ "onboarding_completed": false })),
    )
    .await;
    assert!(me["onboarding_completed_at"].is_null(), "replay clears it");
}

/// The shipped shelf in manifest order; coverage counts only issues the
/// caller can read; an imported list links to the caller's view.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shelf_coverage_is_per_caller_and_links_imports() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "sl-admin@example.com").await;
    let reader = register(&app, "sl-reader@example.com").await;
    seed_all_coverage(&app).await;
    let (lib_a, _, issue_a) = seed_series_with_issue(&app, "sl-a").await;
    let (_lib_b, _, issue_b) = seed_series_with_issue(&app, "sl-b").await;
    grant_library(&app, reader.user_id, lib_a).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    server::starter_lists::store(&db, DT, HAWKEYE, 24, &[issue_a, issue_b], None)
        .await
        .unwrap();

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), server::starter_lists::MANIFEST.len());
    assert_eq!(items[0]["catalog_path"], HAWKEYE, "manifest order");
    let h = find(&body, HAWKEYE).unwrap();
    assert_eq!(h["total"], 24);
    assert_eq!(h["available"], 1, "library B is not granted");
    assert_eq!(h["coverage_status"], "ready");
    assert_eq!(h["origin"], "shipped");
    assert!(h["imported_view_id"].is_null());

    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(find(&body, HAWKEYE).unwrap()["available"], 2);

    // Import (list + view) shows up as a link for that user only.
    let list = seed_catalog_list(&app, DT, HAWKEYE).await;
    let view = seed_cbl_saved_view(&app, Some(reader.user_id), list, "Hawkeye").await;
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(
        find(&body, HAWKEYE).unwrap()["imported_view_id"],
        view.to_string()
    );
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&admin),
        None,
    )
    .await;
    assert!(find(&body, HAWKEYE).unwrap()["imported_view_id"].is_null());

    // A list an admin shared with everyone counts too, for every reader;
    // the reader's own import still wins.
    let all_star = "DC/Creator Runs/[DC] [2006-2008] All-Star Superman by Grant Morrison.cbl";
    let shared_list = seed_catalog_list(&app, DT, all_star).await;
    let shared = seed_cbl_saved_view(&app, None, shared_list, "All-Star Superman").await;
    for who in [&reader, &admin] {
        let (_, body) = http(&app, Method::GET, "/api/me/starter-lists", Some(who), None).await;
        assert_eq!(
            find(&body, all_star).unwrap()["imported_view_id"],
            shared.to_string()
        );
    }
    let shared_hawkeye = seed_catalog_list(&app, DT, HAWKEYE).await;
    seed_cbl_saved_view(&app, None, shared_hawkeye, "Shared Hawkeye").await;
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(
        find(&body, HAWKEYE).unwrap()["imported_view_id"],
        view.to_string()
    );
}

/// Admins hide a shipped pick, add their own, and remove it again;
/// readers never see hidden picks; non-admins can't edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_edits_the_shelf() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "sle-admin@example.com").await;
    let reader = register(&app, "sle-reader@example.com").await;
    seed_all_coverage(&app).await;

    // Hide Hawkeye.
    let (status, body) = http(
        &app,
        Method::PUT,
        "/api/admin/starter-lists/visibility",
        Some(&admin),
        Some(serde_json::json!({
            "catalog_source_id": DT, "catalog_path": HAWKEYE, "hidden": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    let h = find(&body, HAWKEYE).unwrap();
    assert_eq!(h["hidden"], true, "admin listing keeps hidden picks");
    let override_id = h["override_id"].as_str().unwrap().to_owned();
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert!(find(&body, HAWKEYE).is_none(), "readers never see it");

    // Add a custom pick (seed its coverage so nothing is fetched).
    let custom = "Marvel/Creator Runs/[Marvel] [1983-1987] Thor by Walt Simonson.cbl";
    let db = Database::connect(&app.db_url).await.unwrap();
    server::starter_lists::store(&db, DT, custom, 40, &[], None)
        .await
        .unwrap();
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/admin/starter-lists",
        Some(&admin),
        Some(serde_json::json!({
            "catalog_source_id": DT, "catalog_path": custom,
            "reason": "House favourite."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    let c = find(&body, custom).unwrap();
    assert_eq!(c["origin"], "admin");
    assert_eq!(c["title"], "[Marvel] [1983-1987] Thor by Walt Simonson");
    assert_eq!(c["publisher"], "Marvel");
    let custom_id = c["override_id"].as_str().unwrap().to_owned();

    // Duplicates and shipped paths conflict.
    for path in [custom, HAWKEYE] {
        let (status, _) = http(
            &app,
            Method::POST,
            "/api/admin/starter-lists",
            Some(&admin),
            Some(serde_json::json!({
                "catalog_source_id": DT, "catalog_path": path, "reason": "x"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}");
    }

    // Readers see the addition; non-admins can't edit.
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert!(find(&body, custom).is_some());
    let (status, _) = http(
        &app,
        Method::GET,
        "/api/admin/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/admin/starter-lists/{custom_id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Deleting the hide row restores Hawkeye; deleting the addition drops it.
    for id in [&override_id, &custom_id] {
        let (status, _) = http(
            &app,
            Method::DELETE,
            &format!("/api/admin/starter-lists/{id}"),
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/starter-lists",
        Some(&reader),
        None,
    )
    .await;
    assert!(find(&body, HAWKEYE).is_some());
    assert!(find(&body, custom).is_none());
}
