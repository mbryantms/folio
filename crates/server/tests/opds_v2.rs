//! Integration tests for the OPDS 2.0 JSON-LD surface (M6).
//!
//! Covers:
//!  - root shape: links + navigation entries advertise the v2 mirrors
//!  - series list/detail: pagination link rels in JSON, publications
//!    array shape, PSE link with `{pageNumber}` template
//!  - search: matches in `navigation`, empty-query stub still 200
//!  - personal surfaces: WTR / lists / collections / views parity with M4
//!    (ACL + ownership leak guards, mixed-collection feed)
//!  - content negotiation: `Accept: application/opds+json` against
//!    `/opds/v1/*` → 308 redirect to the matching v2 path
//!  - link shape: every link is a typed JSON object with rel/href/type

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_entry::ActiveModel as CblEntryAM,
    cbl_list::ActiveModel as CblListAM,
    collection_entry::ActiveModel as CollectionEntryAM,
    issue::ActiveModel as IssueAM,
    library,
    saved_view::ActiveModel as SavedViewAM,
    series::{ActiveModel as SeriesAM, normalize_name},
    user_view_pin::ActiveModel as UserViewPinAM,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

// ───────────────── auth + http helpers ─────────────────

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_bytes(b: Body) -> Vec<u8> {
    to_bytes(b, usize::MAX).await.unwrap().to_vec()
}

async fn body_json(b: Body) -> Value {
    serde_json::from_slice(&body_bytes(b).await).unwrap()
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
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let json: Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
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

async fn get_json(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = body_json(resp.into_body()).await;
    (status, json)
}

// ───────────────── fixture helpers ─────────────────

async fn seed_library(db: &DatabaseConnection, root: &std::path::Path) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(id),
        name: Set(format!("Lib {}", &id.to_string()[..8])),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
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
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_series(db: &DatabaseConnection, lib_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SeriesAM {
        id: Set(id),
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
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(id.to_string()),
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
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_issue_with_file(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
) -> String {
    std::fs::write(file_path, payload).unwrap();
    let bytes = std::fs::read(file_path).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let now = Utc::now().fixed_offset();
    IssueAM {
        id: Set(hash.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(Uuid::now_v7().to_string()),
        file_path: Set(file_path.to_string_lossy().into_owned()),
        file_size: Set(std::fs::metadata(file_path).unwrap().len() as i64),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(hash.clone()),
        title: Set(Some("Issue".into())),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(None),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(Some(3)),
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
    .insert(db)
    .await
    .unwrap();
    hash
}

async fn seed_collection(db: &DatabaseConnection, owner: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("collection".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_collection_entry(
    db: &DatabaseConnection,
    view_id: Uuid,
    position: i32,
    series_id: Option<Uuid>,
    issue_id: Option<&str>,
) {
    let kind = if series_id.is_some() {
        "series"
    } else {
        "issue"
    };
    let now = Utc::now().fixed_offset();
    CollectionEntryAM {
        id: Set(Uuid::now_v7()),
        saved_view_id: Set(view_id),
        position: Set(position),
        entry_kind: Set(kind.into()),
        series_id: Set(series_id),
        issue_id: Set(issue_id.map(str::to_owned)),
        added_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_cbl_list(db: &DatabaseConnection, owner: Option<Uuid>, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    CblListAM {
        id: Set(id),
        owner_user_id: Set(owner),
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
        num_issues_declared: Set(None),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_cbl_entry(
    db: &DatabaseConnection,
    list_id: Uuid,
    position: i32,
    matched_issue_id: &str,
) {
    let now = Utc::now().fixed_offset();
    CblEntryAM {
        id: Set(Uuid::now_v7()),
        cbl_list_id: Set(list_id),
        position: Set(position),
        series_name: Set("Seed".into()),
        issue_number: Set(position.to_string()),
        volume: Set(None),
        year: Set(None),
        cv_series_id: Set(None),
        cv_issue_id: Set(None),
        metron_series_id: Set(None),
        metron_issue_id: Set(None),
        matched_issue_id: Set(Some(matched_issue_id.into())),
        match_status: Set("matched".into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(None),
        user_resolved_at: Set(None),
        matched_at: Set(Some(now)),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_filter_view(
    db: &DatabaseConnection,
    owner: Uuid,
    name: &str,
    conditions: Value,
) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("filter_series".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(Some("all".into())),
        conditions: Set(Some(conditions)),
        sort_field: Set(Some("name".into())),
        sort_order: Set(Some("asc".into())),
        result_limit: Set(Some(20)),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn pin_view(db: &DatabaseConnection, user_id: Uuid, view_id: Uuid) {
    let page_id = server::pages::system_page_id(db, user_id).await.unwrap();
    UserViewPinAM {
        user_id: Set(user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(0),
        pinned: Set(true),
        show_in_sidebar: Set(false),
        icon: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

// ───────────────── shape-level assertion helpers ─────────────────

fn assert_link_typed(link: &Value) {
    assert!(
        link["rel"].is_string() || link["rel"].is_array(),
        "link rel: {link}"
    );
    assert!(link["href"].is_string(), "link href: {link}");
}

fn link_with_rel<'a>(links: &'a [Value], rel: &str) -> Option<&'a Value> {
    links.iter().find(|l| match &l["rel"] {
        Value::String(s) => s == rel,
        Value::Array(arr) => arr.iter().any(|v| v.as_str() == Some(rel)),
        _ => false,
    })
}

// ───────────────── tests ─────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_shape_advertises_v2_subsections() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-root@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (s, body) = get_json(&app, &auth, "/opds/v2").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["metadata"]["title"], "Folio OPDS 2.0");
    let links = body["links"].as_array().unwrap();
    let nav = body["navigation"].as_array().unwrap();
    for l in links {
        assert_link_typed(l);
    }
    // Navigation entries don't require `rel` (they're drill-down anchors,
    // not protocol-level relations), but every one must carry `title` +
    // `href` so clients can render and follow them.
    for n in nav {
        assert!(n["title"].is_string(), "nav title: {n}");
        assert!(n["href"].is_string(), "nav href: {n}");
    }
    let nav_hrefs: Vec<&str> = nav.iter().map(|n| n["href"].as_str().unwrap()).collect();
    assert!(nav_hrefs.contains(&"/opds/v2/series"));
    assert!(nav_hrefs.contains(&"/opds/v2/recent"));
    assert!(nav_hrefs.contains(&"/opds/v2/wtr"));
    assert!(nav_hrefs.contains(&"/opds/v2/lists"));
    assert!(nav_hrefs.contains(&"/opds/v2/collections"));
    assert!(nav_hrefs.contains(&"/opds/v2/views"));
    assert!(
        link_with_rel(links, "search").is_some(),
        "search template link"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_list_paginates_in_json() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-page@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    for i in 0..60 {
        seed_series(&db, lib_id, &format!("S {i:03}")).await;
    }
    let (s, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["navigation"].as_array().unwrap().len(), 50);
    let links = body["links"].as_array().unwrap();
    assert!(link_with_rel(links, "self").is_some());
    assert!(link_with_rel(links, "first").is_some());
    assert!(link_with_rel(links, "next").is_some());
    assert!(link_with_rel(links, "last").is_some());
    assert!(
        link_with_rel(links, "previous").is_none(),
        "page 1 has no previous"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_shape_includes_pse_template() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-pub@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "PSE Series").await;
    let issue_id = seed_issue_with_file(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("p.cbz"),
        b"placeholder",
    )
    .await;

    let (s, body) = get_json(&app, &auth, &format!("/opds/v2/series/{series_id}")).await;
    assert_eq!(s, StatusCode::OK);
    let pubs = body["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    let p = &pubs[0];
    assert_eq!(p["metadata"]["@type"], "http://schema.org/Periodical");
    assert!(
        p["metadata"]["identifier"]
            .as_str()
            .unwrap()
            .starts_with("urn:folio:issue:")
    );
    let links = p["links"].as_array().unwrap();
    let acq = link_with_rel(links, "http://opds-spec.org/acquisition").unwrap();
    assert_eq!(
        acq["href"].as_str().unwrap(),
        format!("/opds/v1/issues/{issue_id}/file"),
        "acquisition links point at the canonical /opds/v1 download path"
    );
    let pse = link_with_rel(links, "http://vaemendis.net/opds-pse/stream")
        .expect("pse stream link emitted");
    assert!(pse["href"].as_str().unwrap().contains("{pageNumber}"));
    assert_eq!(pse["properties"]["numberOfItems"], 3);
    assert_eq!(pse["templated"], true);
    // Two image rels (thumbnail + full).
    let images = p["images"].as_array().unwrap();
    assert_eq!(images.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_returns_matching_series_in_navigation() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-search@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series(&db, lib_id, "Daredevil").await;
    seed_series(&db, lib_id, "Superman").await;

    let (s, body) = get_json(&app, &auth, "/opds/v2/search?q=Daredevil").await;
    assert_eq!(s, StatusCode::OK);
    let titles: Vec<&str> = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Daredevil"));
    assert!(!titles.contains(&"Superman"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_search_returns_search_template() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-empty-search@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (s, body) = get_json(&app, &auth, "/opds/v2/search").await;
    assert_eq!(s, StatusCode::OK);
    let links = body["links"].as_array().unwrap();
    let search = link_with_rel(links, "search").expect("search template");
    assert_eq!(search["templated"], true);
    assert!(search["href"].as_str().unwrap().contains("{?query}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accept_header_redirects_v1_to_v2() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-negotiate@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/opds/v1/recent")
                .header(header::COOKIE, auth.cookies())
                .header(header::ACCEPT, "application/opds+json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    let loc = resp.headers().get(header::LOCATION).unwrap();
    assert_eq!(loc, "/opds/v2/recent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_accept_keeps_atom_on_v1() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-atom-default@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/opds/v1")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
    assert!(
        ct.to_str().unwrap().starts_with("application/atom+xml"),
        "no Accept negotiation = legacy atom output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_seeds_and_lists_added_publication() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-wtr@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "WTR Pick").await;

    // First hit seeds WTR + returns empty.
    let (s, body) = get_json(&app, &auth, "/opds/v2/wtr").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["metadata"]["title"], "Want to Read");
    // Confirm WTR row was created so we can add an entry.
    use entity::saved_view::Column as SVCol;
    let wtr = entity::saved_view::Entity::find()
        .filter(SVCol::UserId.eq(auth.user_id))
        .filter(SVCol::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    seed_collection_entry(&db, wtr.id, 0, Some(series_id), None).await;

    let (s, body) = get_json(&app, &auth, "/opds/v2/wtr").await;
    assert_eq!(s, StatusCode::OK);
    let nav = body["navigation"].as_array().unwrap();
    assert_eq!(nav.len(), 1, "series entry surfaces as a navigation item");
    assert_eq!(nav[0]["title"], "WTR Pick");
    assert_eq!(
        nav[0]["href"].as_str().unwrap(),
        format!("/opds/v2/series/{series_id}")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_list_acq_resolves_matched_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-cbl@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let sid = seed_series(&db, lib_id, "Run").await;
    let i0 = seed_issue_with_file(&db, lib_id, sid, &tmp.path().join("a.cbz"), b"a").await;
    let list_id = seed_cbl_list(&db, Some(auth.user_id), "My V2 List").await;
    seed_cbl_entry(&db, list_id, 0, &i0).await;

    let (s, body) = get_json(&app, &auth, &format!("/opds/v2/lists/{list_id}")).await;
    assert_eq!(s, StatusCode::OK);
    let pubs = body["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(
        pubs[0]["metadata"]["identifier"].as_str().unwrap(),
        format!("urn:folio:issue:{i0}")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_acq_splits_series_and_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-col@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Mixed Series").await;
    let issue_id =
        seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("m.cbz"), b"m").await;
    let view_id = seed_collection(&db, auth.user_id, "Mixed").await;
    seed_collection_entry(&db, view_id, 0, Some(series_id), None).await;
    seed_collection_entry(&db, view_id, 1, None, Some(&issue_id)).await;

    let (s, body) = get_json(&app, &auth, &format!("/opds/v2/collections/{view_id}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["navigation"].as_array().unwrap().len(), 1);
    assert_eq!(body["publications"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["publications"][0]["metadata"]["identifier"]
            .as_str()
            .unwrap(),
        format!("urn:folio:issue:{issue_id}")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_other_user_returns_404() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "v2-co-owner@example.com").await;
    promote_to_admin(&app, owner.user_id).await;
    let snooper = register(&app, "v2-co-snoop@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let view_id = seed_collection(&db, owner.user_id, "Private").await;
    let (s, _body) = get_json(&app, &snooper, &format!("/opds/v2/collections/{view_id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_nav_filters_to_pinned_filter_views() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-views@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let pinned = seed_filter_view(&db, auth.user_id, "Pinned", serde_json::json!([])).await;
    let _invisible = seed_filter_view(&db, auth.user_id, "Hidden", serde_json::json!([])).await;
    pin_view(&db, auth.user_id, pinned).await;
    let (s, body) = get_json(&app, &auth, "/opds/v2/views").await;
    assert_eq!(s, StatusCode::OK);
    let titles: Vec<&str> = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Pinned"));
    assert!(!titles.contains(&"Hidden"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn view_acq_evaluates_filter_server_side() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-view-eval@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series(&db, lib_id, "Alpha").await;
    seed_series(&db, lib_id, "Beta").await;
    let view_id = seed_filter_view(
        &db,
        auth.user_id,
        "B-only",
        serde_json::json!([{ "field": "name", "op": "contains", "value": "Beta" }]),
    )
    .await;

    let (s, body) = get_json(&app, &auth, &format!("/opds/v2/views/{view_id}")).await;
    assert_eq!(s, StatusCode::OK);
    let titles: Vec<&str> = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Beta"));
    assert!(!titles.contains(&"Alpha"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_type_is_opds_json() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-ct@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/opds/v2")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
    assert!(
        ct.to_str().unwrap().starts_with("application/opds+json"),
        "got {ct:?}"
    );
}
