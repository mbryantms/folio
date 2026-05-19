//! Integration tests for M2 of opds-sync-cleanup-1.0 — default up-next-first
//! reorder across every reading-sequence OPDS feed, with per-entity opt-out.
//!
//! Verifies, for each of series / CBL / collection / WTR:
//!  - DEFAULT: when up-next ≠ first canonical entry, up-next moves to position 0.
//!  - OPT-OUT: when the owning row's `preserve_canonical_order` (series, CBL,
//!    collection saved-view) — or `users.opds_wtr_reorder = false` for WTR —
//!    is set, the feed emits issues in canonical order.
//!  - The `?resume=1` synthetic-entry path is gone (no `folio:resume:` /
//!    `▶ Resume` artifacts ever appear in a feed body).

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
    progress_record::ActiveModel as ProgressAM,
    saved_view::ActiveModel as SavedViewAM,
    series::{ActiveModel as SeriesAM, normalize_name},
    user as user_entity,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use tower::ServiceExt;
use uuid::Uuid;

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

async fn body_text(b: Body) -> String {
    String::from_utf8(body_bytes(b).await).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
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
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get_cookie(app: &TestApp, uri: &str, auth: &Authed) -> Response<Body> {
    app.router
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
        .unwrap()
}

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

async fn seed_series(
    db: &DatabaseConnection,
    lib_id: Uuid,
    name: &str,
    preserve_canonical_order: bool,
) -> Uuid {
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
        reading_direction: Set(None),
        preserve_canonical_order: Set(preserve_canonical_order),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_issue(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
    sort_number: f64,
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
        title: Set(Some(format!("Issue {sort_number}"))),
        sort_number: Set(Some(sort_number)),
        number_raw: Set(Some(format!("{sort_number}"))),
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

async fn seed_progress_finished(db: &DatabaseConnection, user_id: Uuid, issue_id: &str) {
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(19),
        percent: Set(1.0),
        finished: Set(true),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_cbl_list(
    db: &DatabaseConnection,
    owner: Uuid,
    name: &str,
    preserve_canonical_order: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    CblListAM {
        id: Set(id),
        owner_user_id: Set(Some(owner)),
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
        preserve_canonical_order: Set(preserve_canonical_order),
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
    matched_issue_id: Option<&str>,
) {
    let now = Utc::now().fixed_offset();
    let status = if matched_issue_id.is_some() {
        "matched"
    } else {
        "missing"
    };
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
        matched_issue_id: Set(matched_issue_id.map(str::to_owned)),
        match_status: Set(status.into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(None),
        user_resolved_at: Set(None),
        matched_at: Set(matched_issue_id.map(|_| now)),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_collection(
    db: &DatabaseConnection,
    owner: Uuid,
    name: &str,
    preserve_canonical_order: bool,
) -> Uuid {
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
        preserve_canonical_order: Set(preserve_canonical_order),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_collection_entry_issue(
    db: &DatabaseConnection,
    view_id: Uuid,
    position: i32,
    issue_id: &str,
) {
    let now = Utc::now().fixed_offset();
    CollectionEntryAM {
        id: Set(Uuid::now_v7()),
        saved_view_id: Set(view_id),
        position: Set(position),
        entry_kind: Set("issue".into()),
        series_id: Set(None),
        issue_id: Set(Some(issue_id.into())),
        added_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Return the indices of `urn:issue:<id>` markers in feed order. Useful for
/// asserting the relative position of issues without coupling to nearby XML.
fn issue_positions(body: &str, ids: &[&str]) -> Vec<usize> {
    ids.iter()
        .map(|id| {
            body.find(&format!("urn:issue:{id}"))
                .unwrap_or_else(|| panic!("issue {id} not found in body:\n{body}"))
        })
        .collect()
}

fn assert_no_synthetic_resume(body: &str) {
    assert!(
        !body.contains("folio:resume:"),
        "feed must not contain synthetic resume marker:\n{body}"
    );
    assert!(
        !body.contains("\u{25B6} Resume"),
        "feed must not contain ▶ Resume synthetic title:\n{body}"
    );
}

// ────────────── series ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_default_reorder_moves_up_next_to_position_zero() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Reorder Me", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"r-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"r-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"r-c", 3.0).await;
    // a + b are finished → up-next = c. c should now lead the feed.
    seed_progress_finished(&db, auth.user_id, &a).await;
    seed_progress_finished(&db, auth.user_id, &b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&c, &a, &b]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "up-next (c) must precede a + b: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_preserve_canonical_order_opt_out_keeps_natural_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Year One", true).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"y-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"y-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"y-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;
    seed_progress_finished(&db, auth.user_id, &b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── CBL ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_default_reorder_moves_up_next_to_position_zero() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Crossover", false).await;
    let i1 = seed_issue(&db, lib, series, &tmp.path().join("1.cbz"), b"cb1", 1.0).await;
    let i2 = seed_issue(&db, lib, series, &tmp.path().join("2.cbz"), b"cb2", 2.0).await;
    let i3 = seed_issue(&db, lib, series, &tmp.path().join("3.cbz"), b"cb3", 3.0).await;
    let i4 = seed_issue(&db, lib, series, &tmp.path().join("4.cbz"), b"cb4", 4.0).await;
    // The user's actual scenario: first 3 finished, #4 is up-next.
    seed_progress_finished(&db, auth.user_id, &i1).await;
    seed_progress_finished(&db, auth.user_id, &i2).await;
    seed_progress_finished(&db, auth.user_id, &i3).await;

    let list = seed_cbl_list(&db, auth.user_id, "Storyline", false).await;
    seed_cbl_entry(&db, list, 0, Some(&i1)).await;
    seed_cbl_entry(&db, list, 1, Some(&i2)).await;
    seed_cbl_entry(&db, list, 2, Some(&i3)).await;
    seed_cbl_entry(&db, list, 3, Some(&i4)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&i4, &i1, &i2, &i3]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2] && pos[0] < pos[3],
        "i4 (up-next) must lead all read entries: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_preserve_canonical_order_opt_out_keeps_list_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Curated", false).await;
    let i1 = seed_issue(&db, lib, series, &tmp.path().join("1.cbz"), b"cp1", 1.0).await;
    let i2 = seed_issue(&db, lib, series, &tmp.path().join("2.cbz"), b"cp2", 2.0).await;
    let i3 = seed_issue(&db, lib, series, &tmp.path().join("3.cbz"), b"cp3", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &i1).await;

    let list = seed_cbl_list(&db, auth.user_id, "Year One Order", true).await;
    seed_cbl_entry(&db, list, 0, Some(&i1)).await;
    seed_cbl_entry(&db, list, 1, Some(&i2)).await;
    seed_cbl_entry(&db, list, 2, Some(&i3)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&i1, &i2, &i3]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "CBL canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── collection ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_default_reorder_moves_up_next_issue_first() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "coll-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Side Stories", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"co-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"co-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"co-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let view = seed_collection(&db, auth.user_id, "My Picks", false).await;
    seed_collection_entry_issue(&db, view, 0, &a).await;
    seed_collection_entry_issue(&db, view, 1, &b).await;
    seed_collection_entry_issue(&db, view, 2, &c).await;

    let resp = get_cookie(&app, &format!("/opds/v1/collections/{view}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&b, &a, &c]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "up-next (b) must lead a + c: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_preserve_canonical_order_opt_out_keeps_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "coll-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Curated Coll", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"cop-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"cop-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"cop-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let view = seed_collection(&db, auth.user_id, "Canonical", true).await;
    seed_collection_entry_issue(&db, view, 0, &a).await;
    seed_collection_entry_issue(&db, view, 1, &b).await;
    seed_collection_entry_issue(&db, view, 2, &c).await;

    let resp = get_cookie(&app, &format!("/opds/v1/collections/{view}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "collection canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── WTR ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_default_reorders_up_next_first() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wtr-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "WTR Series", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"w-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"w-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"w-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    // First /wtr hit seeds the WTR collection. Then add three issue rows
    // in canonical order; the second call should reorder.
    let _seed = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let wtr = entity::saved_view::Entity::find()
        .filter(entity::saved_view::Column::UserId.eq(auth.user_id))
        .filter(entity::saved_view::Column::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .expect("WTR row should exist after first /wtr hit");
    seed_collection_entry_issue(&db, wtr.id, 0, &a).await;
    seed_collection_entry_issue(&db, wtr.id, 1, &b).await;
    seed_collection_entry_issue(&db, wtr.id, 2, &c).await;

    let resp = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&b, &a, &c]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "WTR default reorder must move b first: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_user_opt_out_preserves_drag_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wtr-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "WTR Curated", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"wp-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"wp-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"wp-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let _seed = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let wtr = entity::saved_view::Entity::find()
        .filter(entity::saved_view::Column::UserId.eq(auth.user_id))
        .filter(entity::saved_view::Column::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    seed_collection_entry_issue(&db, wtr.id, 0, &a).await;
    seed_collection_entry_issue(&db, wtr.id, 1, &b).await;
    seed_collection_entry_issue(&db, wtr.id, 2, &c).await;

    // Flip the per-user opt-out flag.
    let mut u: user_entity::ActiveModel = user_entity::Entity::find_by_id(auth.user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    u.opds_wtr_reorder = Set(false);
    u.update(&db).await.unwrap();

    let resp = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "WTR opt-out preserves drag order: {pos:?}\n{body}"
    );
}

// ────────────── `?resume=1` is gone ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_query_param_is_ignored_after_cleanup() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "no-synth@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "No Synth", false).await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"ns-a", 1.0).await;
    let _b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"ns-b", 2.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}?resume=1"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
}
