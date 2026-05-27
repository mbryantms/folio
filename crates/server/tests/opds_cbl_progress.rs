//! Integration tests for M6 of opds-sync-1.0 — bidirectional CBL
//! progress surfaces.
//!
//! Verifies:
//!  - v1 CBL acquisition feed emits a feed-root `<pse:lastReadDate>`
//!    matching the most-recent progress event on any matched issue.
//!  - v2 `/opds/v2/lists` navigation entries advertise accurate
//!    `numberOfRead` / `numberOfFinished` / `numberOfItems` counts.
//!  - CBL position ordering is preserved on the per-entry `rel="next"`
//!    even when finished/in-progress states are mixed across the list.

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
    issue::ActiveModel as IssueAM,
    library,
    progress_record::ActiveModel as ProgressAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
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
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
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
        preserve_canonical_order: Set(false),
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
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    hash
}

async fn seed_progress(
    db: &DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    finished: bool,
    when: chrono::DateTime<chrono::FixedOffset>,
) {
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(if finished { 1.0 } else { 0.5 }),
        finished: Set(finished),
        finished_at: Set(if finished { Some(when) } else { None }),
        updated_at: Set(when),
        device: Set(None),
        is_backfill: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_cbl_list(db: &DatabaseConnection, owner: Uuid, name: &str) -> Uuid {
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
        preserve_canonical_order: Set(false),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_cbl_feed_emits_feed_level_last_read_date_of_newest_event() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-lrd@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Reading").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"m6-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"m6-b",
        2.0,
    )
    .await;

    let list_id = seed_cbl_list(&db, auth.user_id, "M6 Crossover").await;
    seed_cbl_entry(&db, list_id, 0, Some(&a)).await;
    seed_cbl_entry(&db, list_id, 1, Some(&b)).await;

    let t_a = chrono::DateTime::parse_from_rfc3339("2026-05-01T10:00:00+00:00").unwrap();
    let t_b = chrono::DateTime::parse_from_rfc3339("2026-05-17T14:30:00+00:00").unwrap();
    seed_progress(&db, auth.user_id, &a, 19, true, t_a).await;
    seed_progress(&db, auth.user_id, &b, 7, false, t_b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // The feed-root pse:lastReadDate must reflect the MOST RECENT
    // event (b at 14:30), not the earliest (a at 10:00).
    let expected = "<pse:lastReadDate>2026-05-17T14:30:00+00:00</pse:lastReadDate>";
    assert!(
        body.contains(expected),
        "feed-root lastReadDate matches newest event: looking for {expected}\nin body:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_cbl_lists_nav_carries_per_list_progress_counts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-counts@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Counts").await;
    // Three matched issues + one unmatched (missing) entry.
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"m6c-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"m6c-b",
        2.0,
    )
    .await;
    let c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"m6c-c",
        3.0,
    )
    .await;

    let list_id = seed_cbl_list(&db, auth.user_id, "Counts CBL").await;
    seed_cbl_entry(&db, list_id, 0, Some(&a)).await;
    seed_cbl_entry(&db, list_id, 1, Some(&b)).await;
    seed_cbl_entry(&db, list_id, 2, Some(&c)).await;
    // Unmatched entry — counts only matched issues, so this is dropped.
    seed_cbl_entry(&db, list_id, 3, None).await;

    // a finished, b in-progress, c untouched.
    let when = Utc::now().fixed_offset();
    seed_progress(&db, auth.user_id, &a, 19, true, when).await;
    seed_progress(&db, auth.user_id, &b, 5, false, when).await;

    let resp = get_cookie(&app, "/opds/v2/lists", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let nav = body["navigation"].as_array().expect("navigation array");
    let entry = nav
        .iter()
        .find(|n| {
            n["metadata"]["identifier"]
                .as_str()
                .map(|s| s.contains(&list_id.to_string()))
                .unwrap_or(false)
        })
        .expect("nav entry for our CBL");
    let meta = &entry["metadata"];
    assert_eq!(meta["numberOfItems"], 3, "3 matched issues");
    assert_eq!(
        meta["numberOfRead"], 2,
        "started count = a (finished) + b (page>0)"
    );
    assert_eq!(meta["numberOfFinished"], 1, "only a is finished");
    assert!(
        meta["lastReadDate"].is_string(),
        "lastReadDate present when progress exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_cbl_feed_per_entry_rel_next_honors_cbl_position_with_mixed_states() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-pos-mixed@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    // Cross-series CBL — proves rel=next uses CBL position, not
    // series sort_number, even when individual issues have varied
    // progress states.
    let s1 = seed_series(&db, lib_id, "Alpha").await;
    let s2 = seed_series(&db, lib_id, "Beta").await;
    let s3 = seed_series(&db, lib_id, "Gamma").await;
    let a = seed_issue(&db, lib_id, s1, &tmp.path().join("a.cbz"), b"m6m-a", 1.0).await;
    let b = seed_issue(&db, lib_id, s2, &tmp.path().join("b.cbz"), b"m6m-b", 1.0).await;
    let c = seed_issue(&db, lib_id, s3, &tmp.path().join("c.cbz"), b"m6m-c", 1.0).await;

    let list_id = seed_cbl_list(&db, auth.user_id, "Mixed-State CBL").await;
    seed_cbl_entry(&db, list_id, 0, Some(&a)).await;
    seed_cbl_entry(&db, list_id, 1, Some(&b)).await;
    seed_cbl_entry(&db, list_id, 2, Some(&c)).await;
    // opds-sync-cleanup M2 default-reorders the up-next issue to entry
    // index 0, which would scramble rel=next in this test. The
    // invariant under test is "rel=next follows positional order
    // through mixed progress states", so opt the list out of the
    // reorder.
    let mut am: entity::cbl_list::ActiveModel = entity::cbl_list::Entity::find_by_id(list_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.preserve_canonical_order = Set(true);
    am.update(&db).await.unwrap();
    // a finished; b in-progress; c untouched. Reading-sequence rel=next
    // is purely positional — should still emit a→b and b→c regardless
    // of finished states. M2 wires these via the sequential_nav flag.
    let when = Utc::now().fixed_offset();
    seed_progress(&db, auth.user_id, &a, 19, true, when).await;
    seed_progress(&db, auth.user_id, &b, 5, false, when).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;

    // Find a's entry block and check its rel=next points to b.
    let a_marker = format!("urn:issue:{a}");
    let a_idx = body.find(&a_marker).expect("a entry present");
    let a_block_end = body[a_idx..]
        .find("</entry>")
        .map(|i| a_idx + i)
        .expect("a entry closes");
    let a_block = &body[a_idx..a_block_end];
    assert!(
        a_block.contains(&format!(
            r#"<link rel="next" href="/opds/v1/issues/{b}/file""#
        )),
        "a→b rel=next: {a_block}"
    );

    // b's entry should rel=next to c.
    let b_marker = format!("urn:issue:{b}");
    let b_idx = body.find(&b_marker).expect("b entry present");
    let b_block_end = body[b_idx..]
        .find("</entry>")
        .map(|i| b_idx + i)
        .expect("b entry closes");
    let b_block = &body[b_idx..b_block_end];
    assert!(
        b_block.contains(&format!(
            r#"<link rel="next" href="/opds/v1/issues/{c}/file""#
        )),
        "b→c rel=next: {b_block}"
    );
}

/// v0.5: every entry in a CBL acquisition feed has its 1-indexed
/// position prefixed to its title (`5. <title>`). The prefix lets
/// clients that render the feed as a flat list show the reader
/// their place in the list at a glance. The position is the
/// canonical CBL position regardless of any default up-next reorder
/// — i.e. when the second entry is hoisted to the front of the
/// rendered feed because it's the resume target, its prefix is
/// still "2." rather than "1.".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_cbl_feed_prefixes_entry_titles_with_canonical_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-pos@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Positioned").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"pos-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"pos-b",
        2.0,
    )
    .await;
    let c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"pos-c",
        3.0,
    )
    .await;

    let list_id = seed_cbl_list(&db, auth.user_id, "Position Test").await;
    seed_cbl_entry(&db, list_id, 0, Some(&a)).await;
    seed_cbl_entry(&db, list_id, 1, Some(&b)).await;
    seed_cbl_entry(&db, list_id, 2, Some(&c)).await;

    // Mark `a` finished and `b` in-progress so `b` is the resolved
    // up-next target. With default reorder on, `b` gets hoisted to
    // the front of the rendered feed — but its title prefix should
    // still be "2." (its canonical CBL position), not "1.".
    let t_a = chrono::DateTime::parse_from_rfc3339("2026-05-01T10:00:00+00:00").unwrap();
    let t_b = chrono::DateTime::parse_from_rfc3339("2026-05-02T10:00:00+00:00").unwrap();
    seed_progress(&db, auth.user_id, &a, 19, true, t_a).await;
    seed_progress(&db, auth.user_id, &b, 5, false, t_b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;

    let extract_title = |urn_id: &str| -> String {
        let marker = format!("urn:issue:{urn_id}");
        let idx = body
            .find(&marker)
            .unwrap_or_else(|| panic!("entry for {urn_id} not present:\n{body}"));
        let block_end = body[idx..]
            .find("</entry>")
            .map(|i| idx + i)
            .unwrap_or_else(|| panic!("entry for {urn_id} unclosed:\n{body}"));
        let block = &body[idx..block_end];
        let title_open = block
            .find("<title>")
            .unwrap_or_else(|| panic!("entry for {urn_id} has no <title>:\n{block}"));
        let title_close = block[title_open..]
            .find("</title>")
            .map(|i| title_open + i)
            .unwrap_or_else(|| panic!("entry for {urn_id} has no </title>:\n{block}"));
        block[title_open + "<title>".len()..title_close].to_owned()
    };

    // The up-next target (b) carries position 2 alongside the
    // Up Next prefix. Position sits first because it's the
    // structural identity of the row.
    assert_eq!(
        extract_title(&b),
        "2. Up Next: \u{25D0} Issue 2",
        "b is at CBL position 2 and is the up-next target:\n{body}"
    );
    assert_eq!(
        extract_title(&a),
        "1. \u{25CF} Issue 1",
        "a is at CBL position 1 and finished:\n{body}"
    );
    assert_eq!(
        extract_title(&c),
        "3. \u{25CB} Issue 3",
        "c is at CBL position 3 and unread:\n{body}"
    );
}
