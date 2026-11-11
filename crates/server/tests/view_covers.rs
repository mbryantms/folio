//! `GET /me/view-covers` — cover URLs behind the cover-stack cards on the
//! Collections index and Settings → Views.

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
    library, library_user_access, saved_view,
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

async fn revoke_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    library_user_access::Entity::delete_by_id((library_id, user_id))
        .exec(&db)
        .await
        .unwrap();
}

async fn set_issue_removed(app: &TestApp, issue_id: &str, removed: bool) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.removed_at = Set(removed.then(|| Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();
}

async fn create_collection(app: &TestApp, auth: &Authed, name: &str) -> Uuid {
    let (status, v) = http(
        app,
        Method::POST,
        "/api/me/collections",
        Some(auth),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {v:#?}");
    Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
}

async fn add_entry(app: &TestApp, auth: &Authed, cid: Uuid, kind: &str, ref_id: &str) -> String {
    let (status, e) = http(
        app,
        Method::POST,
        &format!("/api/me/collections/{cid}/entries"),
        Some(auth),
        Some(serde_json::json!({ "entry_kind": kind, "ref_id": ref_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "add {kind}: {e:#?}");
    e["id"].as_str().unwrap().to_owned()
}

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
        preserve_canonical_order: Set(false),
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

fn covers_for(body: &serde_json::Value, view_id: Uuid) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["view_id"] == view_id.to_string())
        .map(|i| {
            i["cover_urls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn thumb(issue_id: &str) -> String {
    format!("/issues/{issue_id}/pages/0/thumb")
}

/// Collections: entries in position order, an issue is its own cover, a
/// series uses its series cover; a revoked library or a removed issue
/// drops out; another user's collection never appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_covers_follow_order_and_acl() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "vc-admin@example.com").await;
    let reader = register(&app, "vc-reader@example.com").await;
    let other = register(&app, "vc-other@example.com").await;
    let (lib_a, _sa, issue_a) = seed_series_with_issue(&app, "vc-a").await;
    let (lib_b, series_b, issue_b) = seed_series_with_issue(&app, "vc-b").await;
    grant_library(&app, reader.user_id, lib_a).await;
    grant_library(&app, reader.user_id, lib_b).await;

    let cid = create_collection(&app, &reader, "Stack").await;
    add_entry(&app, &reader, cid, "series", &series_b.to_string()).await;
    add_entry(&app, &reader, cid, "issue", &issue_a).await;

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/view-covers",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(
        covers_for(&body, cid),
        vec![thumb(&issue_b), thumb(&issue_a)]
    );

    // Another user sees none of the reader's covers.
    let (_, body) = http(&app, Method::GET, "/api/me/view-covers", Some(&other), None).await;
    assert!(covers_for(&body, cid).is_empty());

    // Revoked library: the series entry drops out.
    revoke_library(&app, reader.user_id, lib_b).await;
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-covers",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(covers_for(&body, cid), vec![thumb(&issue_a)]);

    // Removed issue: nothing readable left, so no item for the view.
    set_issue_removed(&app, &issue_a, true).await;
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-covers",
        Some(&reader),
        None,
    )
    .await;
    assert!(covers_for(&body, cid).is_empty());
}

/// Reading lists: first matched entries in position order, capped at 4,
/// shared lists included, only issues the caller can read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_covers_cap_order_and_acl() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "vcc-admin@example.com").await;
    let reader = register(&app, "vcc-reader@example.com").await;
    let mut libs = Vec::new();
    let mut issues = Vec::new();
    for i in 0..6 {
        let (lib, _s, issue) = seed_series_with_issue(&app, &format!("vcc-{i}")).await;
        libs.push(lib);
        issues.push(issue);
    }
    // Reader can read everything except library 1.
    for (i, lib) in libs.iter().enumerate() {
        if i != 1 {
            grant_library(&app, reader.user_id, *lib).await;
        }
    }
    // Positions deliberately out of insert order.
    let entries: Vec<(i32, &str)> = vec![
        (5, issues[5].as_str()),
        (0, issues[0].as_str()),
        (1, issues[1].as_str()),
        (2, issues[2].as_str()),
        (3, issues[3].as_str()),
        (4, issues[4].as_str()),
    ];
    let list = seed_cbl_list(&app, "Shared list", &entries).await;
    // Shared (owner NULL) view.
    let shared = seed_cbl_saved_view(&app, None, list, "Shared list").await;

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/view-covers",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(
        covers_for(&body, shared),
        vec![
            thumb(&issues[0]),
            thumb(&issues[2]),
            thumb(&issues[3]),
            thumb(&issues[4]),
        ],
        "position order, ungranted library skipped, capped at 4"
    );

    // Another user's private CBL view is not listed.
    let private = seed_cbl_saved_view(&app, Some(admin.user_id), list, "Admin private").await;
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-covers",
        Some(&reader),
        None,
    )
    .await;
    assert!(covers_for(&body, private).is_empty());
}
