//! `GET /creators` browse index — audit A11.
//!
//! Seeds creators across `series_credits` and `issue_credits`, then
//! verifies the cursor-paginated browse list:
//! - aggregates roles + a distinct credit_count per person
//! - returns creators alphabetically
//! - walks every page without silently truncating (keyset cursor)
//! - reports `total` only on the first page
//! - ACL-gates by visible library (a restricted user can't see a
//!   creator who only appears in a library they lack access to)

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    issue_credit::ActiveModel as IssueCreditAM,
    library, library_user_access,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_credit::ActiveModel as SeriesCreditAM,
    user::Entity as UserEntity,
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

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn grant_access(app: &TestApp, user_id: Uuid, lib_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib_id),
        user_id: Set(user_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
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
    let req = builder.body(Body::empty()).unwrap();
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
        auto_convert_cbr_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    lib_id
}

async fn seed_series(app: &TestApp, lib_id: Uuid, name: &str) -> Uuid {
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
        text_language: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    series_id
}

async fn seed_issue(app: &TestApp, lib_id: Uuid, series_id: Uuid, idx: u8) -> String {
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
        title: Set(None),
        sort_number: Set(Some(idx as f64)),
        number_raw: Set(Some(idx.to_string())),
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
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    issue_id
}

async fn add_series_credit(app: &TestApp, series_id: Uuid, role: &str, person: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    SeriesCreditAM {
        series_id: Set(series_id),
        role: Set(role.into()),
        person: Set(person.into()),
        person_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn add_issue_credit(app: &TestApp, issue_id: String, role: &str, person: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    IssueCreditAM {
        issue_id: Set(issue_id),
        role: Set(role.into()),
        person: Set(person.into()),
        person_id: Set(None),
        ordinal: Set(0),
    }
    .insert(&db)
    .await
    .unwrap();
}

/// Pull the `person` strings out of a `CursorPage<CreatorListItem>` body.
fn people(json: &serde_json::Value) -> Vec<String> {
    json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["person"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_alphabetically_with_role_rollup() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "main").await;
    let s1 = seed_series(&app, lib, "Saga").await;
    let s2 = seed_series(&app, lib, "Paper Girls").await;
    // Vaughan: writer on two series + editor on an issue → 3 distinct refs.
    add_series_credit(&app, s1, "writer", "Brian K. Vaughan").await;
    add_series_credit(&app, s2, "writer", "Brian K. Vaughan").await;
    let i1 = seed_issue(&app, lib, s1, 1).await;
    add_issue_credit(&app, i1, "editor", "Brian K. Vaughan").await;
    // Two more creators so the alphabetical ordering is observable.
    add_series_credit(&app, s1, "artist", "Fiona Staples").await;
    add_series_credit(&app, s2, "artist", "Cliff Chiang").await;

    let (status, json) = http(&app, Method::GET, "/api/creators", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");

    // Alphabetical by name.
    assert_eq!(
        people(&json),
        vec!["Brian K. Vaughan", "Cliff Chiang", "Fiona Staples"],
        "creators come back name-ascending"
    );
    // `total` rides the first page.
    assert_eq!(json["total"].as_u64(), Some(3));
    assert!(json["next_cursor"].is_null(), "single page → no cursor");

    // Vaughan's row: writer + editor rolled up, credit_count 3.
    let vaughan = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["person"] == "Brian K. Vaughan")
        .unwrap();
    let roles: Vec<String> = vaughan["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(roles.contains(&"writer".to_owned()));
    assert!(roles.contains(&"editor".to_owned()));
    assert_eq!(vaughan["credit_count"].as_i64(), Some(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_walks_every_page_without_truncating() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "main").await;
    let s = seed_series(&app, lib, "Anthology").await;
    // Five creators; a limit-2 walk must surface all five over 3 pages.
    let names = ["Anna", "Bea", "Cara", "Dana", "Eve"];
    for n in names {
        add_series_credit(&app, s, "writer", n).await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut url = "/api/creators?limit=2".to_string();
    let mut total_first: Option<u64> = None;
    let mut pages = 0;
    loop {
        let (status, json) = http(&app, Method::GET, &url, Some(&auth)).await;
        assert_eq!(status, StatusCode::OK, "json: {json:#?}");
        if pages == 0 {
            total_first = json["total"].as_u64();
        } else {
            assert!(
                json["total"].is_null(),
                "total must only ride the first page: {json:#?}"
            );
        }
        seen.extend(people(&json));
        pages += 1;
        match json["next_cursor"].as_str() {
            Some(c) => url = format!("/api/creators?limit=2&cursor={c}"),
            None => break,
        }
        assert!(pages < 10, "cursor walk failed to terminate");
    }

    assert_eq!(total_first, Some(5), "total reported on first page");
    assert_eq!(
        seen,
        vec!["Anna", "Bea", "Cara", "Dana", "Eve"],
        "every creator surfaced exactly once, in order, across pages"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acl_gates_by_visible_library() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;

    let lib_a = seed_library(&app, "a").await;
    let lib_b = seed_library(&app, "b").await;
    let sa = seed_series(&app, lib_a, "Visible").await;
    let sb = seed_series(&app, lib_b, "Hidden").await;
    add_series_credit(&app, sa, "writer", "Allowed Author").await;
    add_series_credit(&app, sb, "writer", "Hidden Author").await;

    // A reader granted only library A.
    let reader = register(&app, "reader@example.com").await;
    grant_access(&app, reader.user_id, lib_a).await;

    let (status, json) = http(&app, Method::GET, "/api/creators", Some(&reader)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let names = people(&json);
    assert!(
        names.contains(&"Allowed Author".to_owned()),
        "creator in granted library is visible: {names:?}"
    );
    assert!(
        !names.contains(&"Hidden Author".to_owned()),
        "creator only in ungranted library is hidden: {names:?}"
    );
    assert_eq!(json["total"].as_u64(), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_user_with_no_grants_sees_nothing() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;
    let lib = seed_library(&app, "main").await;
    let s = seed_series(&app, lib, "Saga").await;
    add_series_credit(&app, s, "writer", "Brian K. Vaughan").await;

    let reader = register(&app, "reader@example.com").await;
    let (status, json) = http(&app, Method::GET, "/api/creators", Some(&reader)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
    assert_eq!(json["total"].as_u64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_cursor_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (status, _json) = http(
        &app,
        Method::GET,
        "/api/creators?cursor=not-a-real-cursor",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_with_buckets_by_name() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "main").await;
    let s = seed_series(&app, lib, "Anthology").await;
    add_series_credit(&app, s, "writer", "Alan Moore").await;
    add_series_credit(&app, s, "writer", "Brian K. Vaughan").await;
    add_series_credit(&app, s, "writer", "Stan Lee").await;
    // A non-letter-leading credit → the "#" bucket.
    add_series_credit(&app, s, "artist", "30 Coins Studio").await;

    // Letter bucket, case-insensitive.
    let (status, json) = http(
        &app,
        Method::GET,
        "/api/creators?starts_with=a",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    assert_eq!(people(&json), vec!["Alan Moore"], "a → Alan Moore");

    let (_, json) = http(
        &app,
        Method::GET,
        "/api/creators?starts_with=S",
        Some(&auth),
    )
    .await;
    assert_eq!(people(&json), vec!["Stan Lee"], "uppercase S matches");

    // "#" → the digit-leading name (URL-encoded).
    let (_, json) = http(
        &app,
        Method::GET,
        "/api/creators?starts_with=%23",
        Some(&auth),
    )
    .await;
    assert_eq!(people(&json), vec!["30 Coins Studio"], "# → digit-leading");

    // `total` honors the bucket on the first page.
    let (_, json) = http(
        &app,
        Method::GET,
        "/api/creators?starts_with=a",
        Some(&auth),
    )
    .await;
    assert_eq!(json["total"].as_u64(), Some(1), "total respects the bucket");

    // Invalid bucket → 422.
    let (status, _) = http(
        &app,
        Method::GET,
        "/api/creators?starts_with=ab",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
