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
    series_credit::ActiveModel as SeriesCreditAM,
    series_genre::ActiveModel as SeriesGenreAM,
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

// ─────────── M1 (opds-richer-feeds): series cover images ───────────

/// OPDS 2.0 series nav entries carry an `images[]` array pointing at
/// the cover issue's page-0 thumbnail + full image. Without these,
/// clients fall back to a folder icon — the visual regression the
/// opds-richer-feeds plan was started to fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_nav_carries_images_array() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2covers@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Series With Cover").await;
    let issue_id = seed_issue_with_file(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("v2-cover.cbz"),
        b"v2-cbz-stub",
    )
    .await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(status, StatusCode::OK);
    let nav = body
        .get("navigation")
        .and_then(|v| v.as_array())
        .expect("navigation array");
    let entry = nav
        .iter()
        .find(|n| {
            n.get("href")
                .and_then(|h| h.as_str())
                .is_some_and(|h| h.ends_with(&series_id.to_string()))
        })
        .expect("entry for seeded series");
    let images = entry
        .get("images")
        .and_then(|v| v.as_array())
        .expect("entry must carry images[]");
    assert_eq!(images.len(), 2, "thumbnail + full-size");
    let hrefs: Vec<&str> = images
        .iter()
        .filter_map(|i| i.get("href").and_then(|h| h.as_str()))
        .collect();
    assert!(
        hrefs.contains(&format!("/issues/{issue_id}/pages/0/thumb").as_str()),
        "missing thumbnail href in {hrefs:?}",
    );
    assert!(
        hrefs.contains(&format!("/issues/{issue_id}/pages/0").as_str()),
        "missing full-size href in {hrefs:?}",
    );
    let types: Vec<&str> = images
        .iter()
        .filter_map(|i| i.get("type").and_then(|h| h.as_str()))
        .collect();
    assert!(types.contains(&"image/webp"), "missing webp type");
    assert!(types.contains(&"image/jpeg"), "missing jpeg type");
}

/// Series with zero active issues degrades cleanly — entry rendered,
/// no `images[]` field, client picks its placeholder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_nav_omits_images_for_empty_series() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-empty@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Empty Series").await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(status, StatusCode::OK);
    let nav = body["navigation"].as_array().unwrap();
    let entry = nav
        .iter()
        .find(|n| {
            n.get("href")
                .and_then(|h| h.as_str())
                .is_some_and(|h| h.ends_with(&series_id.to_string()))
        })
        .expect("entry present");
    assert!(
        entry.get("images").is_none(),
        "empty series must not advertise images: {entry}"
    );
}

// ─────────── M2 (opds-richer-feeds): OPDS 2.0 metadata fields ───────────

async fn v2_set_series_meta(
    db: &DatabaseConnection,
    series_id: Uuid,
    publisher: Option<&str>,
    year: Option<i32>,
    language: Option<&str>,
) {
    use entity::series::Entity as SeriesEntity;
    let row = SeriesEntity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.publisher = Set(publisher.map(str::to_owned));
    am.year = Set(year);
    if let Some(l) = language {
        am.language_code = Set(l.to_owned());
    }
    am.update(db).await.unwrap();
}

async fn v2_add_writer(db: &DatabaseConnection, series_id: Uuid, person: &str) {
    SeriesCreditAM {
        series_id: Set(series_id),
        role: Set("writer".into()),
        person: Set(person.into()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn v2_add_genre(db: &DatabaseConnection, series_id: Uuid, genre: &str) {
    SeriesGenreAM {
        series_id: Set(series_id),
        genre: Set(genre.into()),
    }
    .insert(db)
    .await
    .unwrap();
}

/// OPDS 2.0 series nav entries carry `metadata.publisher.name`,
/// `metadata.published`, and `metadata.language` when those fields
/// are populated on the row. Mirrors the v1 `<dc:*>` block but uses
/// the readium-org webpub-manifest JSON shape that OPDS 2.0 extends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_nav_carries_publisher_published_language() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-pubmeta@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Watchmen").await;
    v2_set_series_meta(&db, series_id, Some("DC Comics"), Some(1986), Some("en")).await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(status, StatusCode::OK);
    let entry = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["href"]
                .as_str()
                .is_some_and(|h| h.ends_with(&series_id.to_string()))
        })
        .expect("entry present");
    let m = &entry["metadata"];
    assert_eq!(m["publisher"]["name"], "DC Comics");
    assert_eq!(m["published"], "1986");
    assert_eq!(m["language"], "en");
}

/// `metadata.author` is an array of contributor objects with a
/// `name` field; `metadata.subject` is an array of subject objects
/// with `name` + `scheme`. Sorted alphabetically inside the helper
/// so JSON output is stable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_nav_carries_author_and_subject_arrays() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-author-subj@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Y The Last Man").await;
    v2_add_writer(&db, series_id, "Pia Guerra").await;
    v2_add_writer(&db, series_id, "Brian K. Vaughan").await;
    v2_add_genre(&db, series_id, "Post-apocalyptic").await;
    v2_add_genre(&db, series_id, "Drama").await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(status, StatusCode::OK);
    let entry = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["href"]
                .as_str()
                .is_some_and(|h| h.ends_with(&series_id.to_string()))
        })
        .expect("entry present");
    let authors = entry["metadata"]["author"].as_array().unwrap();
    assert_eq!(authors.len(), 2);
    // Alphabetical: Brian < Pia.
    assert_eq!(authors[0]["name"], "Brian K. Vaughan");
    assert_eq!(authors[1]["name"], "Pia Guerra");
    let subjects = entry["metadata"]["subject"].as_array().unwrap();
    assert_eq!(subjects.len(), 2);
    assert_eq!(subjects[0]["name"], "Drama");
    assert_eq!(subjects[0]["scheme"], "urn:folio:genre");
    assert_eq!(subjects[1]["name"], "Post-apocalyptic");
}

/// Empty metadata fields are omitted entirely — no `author`/`subject`
/// keys when there are zero credits/genres. Lets clients distinguish
/// "field missing" from "field is an empty array".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_nav_omits_empty_metadata_fields() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-omit-empty@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Bare Series").await;
    v2_set_series_meta(&db, series_id, None, None, Some("en")).await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/series?page=1").await;
    assert_eq!(status, StatusCode::OK);
    let entry = body["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["href"]
                .as_str()
                .is_some_and(|h| h.ends_with(&series_id.to_string()))
        })
        .expect("entry present");
    let m = entry["metadata"].as_object().unwrap();
    assert!(!m.contains_key("publisher"));
    assert!(!m.contains_key("published"));
    assert!(!m.contains_key("author"));
    assert!(!m.contains_key("subject"));
    // language is always set (column non-nullable, defaults to "en").
    assert_eq!(m.get("language").and_then(|v| v.as_str()), Some("en"));
}

// ─────────── M3 (opds-richer-feeds): user Pages → OPDS 2.0 ───────────

async fn seed_custom_page_v2(
    db: &DatabaseConnection,
    user_id: Uuid,
    name: &str,
    slug: &str,
    position: i32,
) -> Uuid {
    use entity::user_page::ActiveModel as UserPageAM;
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    UserPageAM {
        id: Set(id),
        user_id: Set(user_id),
        name: Set(name.into()),
        slug: Set(slug.into()),
        is_system: Set(false),
        position: Set(position),
        description: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn pin_view_to_page_v2(
    db: &DatabaseConnection,
    user_id: Uuid,
    page_id: Uuid,
    view_id: Uuid,
    position: i32,
    pinned: bool,
    sidebar: bool,
) {
    UserViewPinAM {
        user_id: Set(user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(position),
        pinned: Set(pinned),
        show_in_sidebar: Set(sidebar),
        icon: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

/// `/opds/v2/pages` returns the user's pages as a navigation array in
/// position order, each href routing into the JSON per-page feed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_pages_nav_lists_user_pages() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-pages-list@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _ = server::pages::system_page_id(&db, auth.user_id)
        .await
        .unwrap();
    seed_custom_page_v2(&db, auth.user_id, "Capes", "capes", 1).await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/pages").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["metadata"]["title"], "My pages");
    let nav = body["navigation"].as_array().unwrap();
    let titles: Vec<&str> = nav.iter().filter_map(|n| n["title"].as_str()).collect();
    assert!(titles.contains(&"Home"));
    assert!(titles.contains(&"Capes"));
    let capes = nav.iter().find(|n| n["title"] == "Capes").unwrap();
    assert_eq!(capes["href"], "/opds/v2/pages/capes");
}

/// `/opds/v2/pages/{slug}` expands to the page's pinned views in
/// pin-position order, each entry routing back into the existing
/// /opds/v2/views/{id} handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_page_acq_expands_pinned_views_in_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-page-pins@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let page_id = seed_custom_page_v2(&db, auth.user_id, "Picks", "picks", 1).await;
    let v_a = seed_filter_view(&db, auth.user_id, "First", serde_json::json!({"all":[]})).await;
    let v_b = seed_filter_view(&db, auth.user_id, "Second", serde_json::json!({"all":[]})).await;
    pin_view_to_page_v2(&db, auth.user_id, page_id, v_b, 1, true, false).await;
    pin_view_to_page_v2(&db, auth.user_id, page_id, v_a, 0, true, false).await;

    let (status, body) = get_json(&app, &auth, "/opds/v2/pages/picks").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["metadata"]["title"], "Picks");
    let nav = body["navigation"].as_array().unwrap();
    assert_eq!(nav.len(), 2);
    // pin-position 0 first.
    assert_eq!(nav[0]["title"], "First");
    assert_eq!(nav[0]["href"], format!("/opds/v2/views/{v_a}"));
    assert_eq!(nav[1]["title"], "Second");
    assert_eq!(nav[1]["href"], format!("/opds/v2/views/{v_b}"));
}

/// One user cannot read another user's page slug — must 404 (status
/// only; content isn't inspected because none should be returned).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_page_acq_returns_404_for_other_users_page() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "v2-pages-owner@example.com").await;
    promote_to_admin(&app, owner.user_id).await;
    let intruder = register(&app, "v2-pages-intruder@example.com").await;
    promote_to_admin(&app, intruder.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    seed_custom_page_v2(&db, owner.user_id, "Owned", "owned", 1).await;

    let (status, _) = get_json(&app, &intruder, "/opds/v2/pages/owned").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
