//! Integration tests for the collection branch of
//! `GET /issues/{id}/next-up` / `prev-up` (`?collection=<saved_view_id>`)
//! and the collection page's `GET /me/collections/{id}/next-up`
//! (post-audit T4.1, decision D7).
//!
//! Covers:
//!   - collection order (issue entry, then a series entry expanded to its
//!     issues) drives next-up; finished entries are stepped over, like a
//!     CBL; the last entry falls through to series order / none.
//!   - a series entry expands main-run first, then the specials.
//!   - an issue the caller cannot open is skipped.
//!   - the current issue no longer in the collection → series fallback
//!     with `collection_param_was_stale: true`.
//!   - someone else's collection is ignored (no stale flag).
//!   - prev-up walks the collection backwards without a finished filter.
//!   - the collection next-up endpoint: start, continue, all finished.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set, Unchanged};
use tower::ServiceExt;
use uuid::Uuid;

// ───── Test scaffolding (same helpers as next_up.rs) ─────

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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let session = extract("__Host-comic_session=");
    let csrf = extract("__Host-comic_csrf=");
    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::{ColumnTrait, QueryFilter};
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row by email");
    let user_id = user_row.id;
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::user::ActiveModel {
        id: Unchanged(user_id),
        role: Set("user".into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    path: &str,
    user: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    if let Some(u) = user {
        let cookie = format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            u.session, u.csrf
        );
        req = req.header(header::COOKIE, cookie);
        if method != Method::GET {
            req = req.header("X-CSRF-Token", &u.csrf);
        }
    }
    let body_bytes = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body_bytes).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (status, body)
}

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
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 1u8);
    insert_issue(&db, lib_id, series_id, &issue_id, 1.0, slug_prefix, "1").await;
    (lib_id, series_id, issue_id)
}

async fn seed_extra_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    sort_number: f64,
    slug: &str,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), sort_number as u8);
    insert_issue(
        &db,
        lib_id,
        series_id,
        &issue_id,
        sort_number,
        slug,
        &format!("{sort_number}"),
    )
    .await;
    issue_id
}

async fn insert_issue(
    db: &sea_orm::DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    issue_id: &str,
    sort_number: f64,
    slug: &str,
    number_raw: &str,
) {
    let now = Utc::now().fixed_offset();
    IssueAM {
        id: Set(issue_id.into()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug.into()),
        file_path: Set(format!("/tmp/{slug}/{sort_number}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.into()),
        title: Set(Some(format!("Issue {sort_number}"))),
        sort_number: Set(Some(sort_number)),
        number_raw: Set(Some(number_raw.into())),
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
    .insert(db)
    .await
    .unwrap();
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

async fn finish_issue(app: &TestApp, user_id: Uuid, issue_id: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    progress_record::ActiveModel {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(19),
        percent: Set(1.0),
        finished: Set(true),
        finished_at: Set(Some(now)),
        updated_at: Set(now),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

// ───── Tests ─────

async fn tag_special(app: &TestApp, issue_id: &str, special_type: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::issue::ActiveModel {
        id: Unchanged(issue_id.into()),
        special_type: Set(Some(special_type.into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
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

async fn add_entry(app: &TestApp, auth: &Authed, cid: Uuid, kind: &str, ref_id: &str) {
    let (status, v) = http(
        app,
        Method::POST,
        &format!("/api/me/collections/{cid}/entries"),
        Some(auth),
        Some(serde_json::json!({ "entry_kind": kind, "ref_id": ref_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "add {kind}: {v:#?}");
}

/// A demoted user with a collection of `[issue b1, series A]` where
/// series A has issues a1, a2, a3 — reading order b1, a1, a2, a3.
struct Fixture {
    user: Authed,
    cid: Uuid,
    a: [String; 3],
    b1: String,
}

async fn fixture(app: &TestApp, tag: &str) -> Fixture {
    let user = register(app, &format!("{tag}@example.com")).await;
    demote_to_user(app, user.user_id).await;
    let (lib_a, series_a, a1) = seed_one_issue(app, &format!("{tag}-a")).await;
    let a2 = seed_extra_issue(app, lib_a, series_a, 2.0, &format!("{tag}-a2")).await;
    let a3 = seed_extra_issue(app, lib_a, series_a, 3.0, &format!("{tag}-a3")).await;
    let (lib_b, _series_b, b1) = seed_one_issue(app, &format!("{tag}-b")).await;
    grant_access(app, user.user_id, lib_a).await;
    grant_access(app, user.user_id, lib_b).await;
    let cid = create_collection(app, &user, &format!("Pile {tag}")).await;
    add_entry(app, &user, cid, "issue", &b1).await;
    add_entry(app, &user, cid, "series", &series_a.to_string()).await;
    Fixture {
        user,
        cid,
        a: [a1, a2, a3],
        b1,
    }
}

async fn next_up(app: &TestApp, user: &Authed, issue_id: &str, cid: Uuid) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::GET,
        &format!("/api/issues/{issue_id}/next-up?collection={cid}"),
        Some(user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body
}

async fn prev_up(app: &TestApp, user: &Authed, issue_id: &str, cid: Uuid) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::GET,
        &format!("/api/issues/{issue_id}/prev-up?collection={cid}"),
        Some(user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body
}

// ───── Tests ─────

#[tokio::test]
async fn collection_order_drives_next_up_and_steps_over_finished() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "nuc-order").await;

    // b1 (entry 0) → a1, the first issue of the series entry — not
    // "next in b1's series" (there is none) and not series-order a2.
    let body = next_up(&app, &f.user, &f.b1, f.cid).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(body["target"]["id"], f.a[0]);
    assert_eq!(body["collection_id"], f.cid.to_string());
    assert_eq!(body["collection_name"], "Pile nuc-order");
    assert_eq!(
        body["collection_position"], 2,
        "1-based position of the target"
    );
    assert_eq!(body["collection_total"], 4, "b1 + a1..a3");
    assert!(body.get("collection_param_was_stale").is_none());

    // Finished entries are stepped over (same rule as a CBL): with a2
    // finished, a1 → a3.
    finish_issue(&app, f.user.user_id, &f.a[1]).await;
    let body = next_up(&app, &f.user, &f.a[0], f.cid).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(body["target"]["id"], f.a[2]);
    assert_eq!(body["collection_position"], 4);

    // Last entry: nothing left in the collection and nothing after a3 in
    // its series → none (no stale flag: the issue IS in the collection).
    let body = next_up(&app, &f.user, &f.a[2], f.cid).await;
    assert_eq!(body["source"], "none");
    assert!(body.get("collection_param_was_stale").is_none());
}

#[tokio::test]
async fn collection_series_entry_expands_main_run_then_specials() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nuc-specials@example.com").await;
    demote_to_user(&app, user.user_id).await;
    let (lib, series, i1) = seed_one_issue(&app, "nuc-sp").await;
    let i2 = seed_extra_issue(&app, lib, series, 2.0, "nuc-sp-2").await;
    // An annual filed between #2 and #4 by sort number; it reads last.
    // (Test issue ids derive from the integer part of the sort number,
    // so the annual takes slot 3 and the next main-run issue slot 4.)
    let annual = seed_extra_issue(&app, lib, series, 3.0, "nuc-sp-annual").await;
    tag_special(&app, &annual, "annual").await;
    let i4 = seed_extra_issue(&app, lib, series, 4.0, "nuc-sp-4").await;
    grant_access(&app, user.user_id, lib).await;
    let cid = create_collection(&app, &user, "Specials").await;
    add_entry(&app, &user, cid, "series", &series.to_string()).await;

    let body = next_up(&app, &user, &i1, cid).await;
    assert_eq!(body["target"]["id"], i2);
    assert_eq!(body["collection_position"], 2);
    let body = next_up(&app, &user, &i2, cid).await;
    assert_eq!(
        body["target"]["id"], i4,
        "main run continues past the annual"
    );
    assert_eq!(body["collection_position"], 3);
    let body = next_up(&app, &user, &i4, cid).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(body["target"]["id"], annual, "specials follow the main run");
    assert_eq!(body["collection_position"], 4);
    assert_eq!(body["collection_total"], 4);
}

#[tokio::test]
async fn collection_skips_an_issue_the_caller_cannot_open() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nuc-acl@example.com").await;
    demote_to_user(&app, user.user_id).await;
    let (lib_a, _sa, a1) = seed_one_issue(&app, "nuc-acl-a").await;
    let (_lib_hidden, _sh, hidden) = seed_one_issue(&app, "nuc-acl-h").await;
    let (lib_c, _sc, c1) = seed_one_issue(&app, "nuc-acl-c").await;
    grant_access(&app, user.user_id, lib_a).await;
    grant_access(&app, user.user_id, lib_c).await;
    let cid = create_collection(&app, &user, "ACL").await;
    add_entry(&app, &user, cid, "issue", &a1).await;
    add_entry(&app, &user, cid, "issue", &hidden).await;
    add_entry(&app, &user, cid, "issue", &c1).await;

    let body = next_up(&app, &user, &a1, cid).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(
        body["target"]["id"], c1,
        "the invisible middle entry is skipped"
    );
    // Positions count every entry so they stay stable across users.
    assert_eq!(body["collection_position"], 3);
    assert_eq!(body["collection_total"], 3);
}

#[tokio::test]
async fn collection_stale_param_falls_back_to_series_with_flag() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "nuc-stale").await;
    // A second series the collection does not contain.
    let (lib_x, series_x, x1) = seed_one_issue(&app, "nuc-stale-x").await;
    let x2 = seed_extra_issue(&app, lib_x, series_x, 2.0, "nuc-stale-x2").await;
    grant_access(&app, f.user.user_id, lib_x).await;

    let body = next_up(&app, &f.user, &x1, f.cid).await;
    assert_eq!(body["source"], "series");
    assert_eq!(body["target"]["id"], x2);
    assert_eq!(body["collection_param_was_stale"], true);
    assert!(body.get("cbl_param_was_stale").is_none());
}

#[tokio::test]
async fn collection_of_another_user_is_ignored() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "nuc-other").await;
    let other = register(&app, "nuc-other-2@example.com").await;
    demote_to_user(&app, other.user_id).await;
    let (lib, series, m1) = seed_one_issue(&app, "nuc-other-m").await;
    let m2 = seed_extra_issue(&app, lib, series, 2.0, "nuc-other-m2").await;
    grant_access(&app, other.user_id, lib).await;

    // `other` sends the fixture user's collection id: not theirs, so the
    // branch is a plain miss — series order, no stale hint.
    let body = next_up(&app, &other, &m1, f.cid).await;
    assert_eq!(body["source"], "series");
    assert_eq!(body["target"]["id"], m2);
    assert!(body.get("collection_param_was_stale").is_none());
}

#[tokio::test]
async fn prev_up_follows_collection_order_without_finished_filter() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "nuc-prev").await;
    // b1 is finished; prev from a1 still backs up onto it.
    finish_issue(&app, f.user.user_id, &f.b1).await;

    let body = prev_up(&app, &f.user, &f.a[0], f.cid).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(body["target"]["id"], f.b1);
    assert_eq!(body["collection_position"], 1);

    let body = prev_up(&app, &f.user, &f.a[2], f.cid).await;
    assert_eq!(body["target"]["id"], f.a[1]);

    // First entry: nothing before it in the collection, and b1 is the
    // only issue of its series → none.
    let body = prev_up(&app, &f.user, &f.b1, f.cid).await;
    assert_eq!(body["source"], "none");
}

#[tokio::test]
async fn collection_next_up_endpoint_start_then_continue_then_done() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "nuc-page").await;
    let url = format!("/api/me/collections/{}/next-up", f.cid);

    let (status, body) = http(&app, Method::GET, &url, Some(&f.user), None).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["target"]["id"], f.b1, "nothing read yet → first entry");
    assert_eq!(body["position"], 1);
    assert_eq!(body["total"], 4);
    assert_eq!(body["started"], false);

    finish_issue(&app, f.user.user_id, &f.b1).await;
    let (_, body) = http(&app, Method::GET, &url, Some(&f.user), None).await;
    assert_eq!(body["target"]["id"], f.a[0]);
    assert_eq!(body["position"], 2);
    assert_eq!(body["started"], true);

    for id in &f.a {
        finish_issue(&app, f.user.user_id, id).await;
    }
    let (_, body) = http(&app, Method::GET, &url, Some(&f.user), None).await;
    assert!(
        body.get("target").is_none(),
        "everything finished: {body:#?}"
    );
    assert_eq!(body["total"], 4);
    assert_eq!(body["started"], true);

    // Someone else's collection is forbidden, a missing one is 404.
    let other = register(&app, "nuc-page-2@example.com").await;
    let (status, _) = http(&app, Method::GET, &url, Some(&other), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/collections/{}/next-up", Uuid::now_v7()),
        Some(&f.user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// `GET /me/collections/{id}/window` — the reading order from the
/// caller's first unfinished issue onward, progress overlaid, with a
/// position cursor for the offline "next N of this list" picker.
#[tokio::test]
async fn collection_window_anchors_on_first_unfinished_and_pages_by_position() {
    let app = TestApp::spawn().await;
    let f = fixture(&app, "win-page").await;
    let base = format!("/api/me/collections/{}/window", f.cid);

    // Nothing read: anchored on b1, every issue upcoming, in order.
    let (status, body) = http(&app, Method::GET, &base, Some(&f.user), None).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    let ids: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["issue"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![
            f.b1.as_str(),
            f.a[0].as_str(),
            f.a[1].as_str(),
            f.a[2].as_str()
        ]
    );
    assert_eq!(body["current_index"], 0);
    assert_eq!(body["total"], 4);
    assert_eq!(body["has_more_after"], false);
    assert_eq!(body["max_position"], 3);
    assert_eq!(body["items"][0]["position"], 0);
    assert_eq!(body["items"][0]["finished"], false);
    assert_eq!(
        body["items"][1]["issue"]["series_name"],
        "Series win-page-a"
    );

    // Finish b1 and a1: the anchor moves to a2; finished issues before
    // it are not in the window, finished issues after it would be
    // reported with `finished: true`.
    finish_issue(&app, f.user.user_id, &f.b1).await;
    finish_issue(&app, f.user.user_id, &f.a[0]).await;
    finish_issue(&app, f.user.user_id, &f.a[2]).await;
    let (_, body) = http(&app, Method::GET, &base, Some(&f.user), None).await;
    assert_eq!(body["items"][0]["issue"]["id"], f.a[1]);
    assert_eq!(body["items"][0]["position"], 2);
    assert_eq!(body["items"][1]["issue"]["id"], f.a[2]);
    assert_eq!(body["items"][1]["finished"], true);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    // `after` caps the page and reports more; `cursor` continues past a
    // position without re-anchoring (and carries no current_index).
    let (_, body) = http(
        &app,
        Method::GET,
        &format!("{base}?after=1"),
        Some(&f.user),
        None,
    )
    .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["has_more_after"], true);
    assert_eq!(body["max_position"], 2);
    let (_, body) = http(
        &app,
        Method::GET,
        &format!("{base}?after=10&cursor=2"),
        Some(&f.user),
        None,
    )
    .await;
    assert_eq!(body["items"][0]["issue"]["id"], f.a[2]);
    assert!(body.get("current_index").is_none(), "{body:#?}");
    assert_eq!(body["has_more_after"], false);

    // Everything finished: empty, no anchor.
    finish_issue(&app, f.user.user_id, &f.a[1]).await;
    let (_, body) = http(&app, Method::GET, &base, Some(&f.user), None).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
    assert!(body.get("current_index").is_none());
    assert_eq!(body["total"], 4);

    // Ownership: someone else's collection is forbidden.
    let other = register(&app, "win-page-2@example.com").await;
    let (status, _) = http(&app, Method::GET, &base, Some(&other), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
