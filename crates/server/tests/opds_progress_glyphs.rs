//! Integration tests for M3 of opds-sync-cleanup-1.0 — title-glyph + page-
//! count suffix annotation on every reading-sequence OPDS entry.
//!
//! Verifies, for v1 (Atom) and v2 (JSON):
//!  - `◯ {title}` for unread entries (no progress row).
//!  - `◐ {title} (N / M)` for in-progress (last_page > 0, !finished).
//!  - `● {title} (M / M)` for finished entries.
//!  - The per-user `users.opds_progress_glyphs = false` opt-out hides
//!    the prefix and suffix entirely (raw title only).
//!  - `(N / M)` suffix is omitted when `page_count` is unknown.
//!
//! The user-facing pitch: clients that ignore the PSE `pse:last_read`
//! attribute (Komga, KOReader, older Tachiyomi) still see "where I left
//! off" because the cue lives in the title string itself.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    progress_record::ActiveModel as ProgressAM,
    series::{ActiveModel as SeriesAM, normalize_name},
    user as user_entity,
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
        reading_direction: Set(None),
        preserve_canonical_order: Set(true),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
async fn seed_issue(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
    sort_number: f64,
    title: &str,
    page_count: Option<i32>,
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
        title: Set(Some(title.into())),
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
        page_count: Set(page_count),
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

async fn seed_progress(
    db: &DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    percent: f64,
    finished: bool,
) {
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(percent),
        finished: Set(finished),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Slice each `<entry>` block out of a v1 feed body and return them as
/// a Vec of `(id, block)` pairs. Lets tests assert on the title of a
/// specific entry without false-positives from neighboring entries.
fn entry_blocks_by_id(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in body.split("<entry>").skip(1) {
        let block = raw.split("</entry>").next().unwrap_or("");
        let id = block
            .split("<id>urn:issue:")
            .nth(1)
            .and_then(|s| s.split("</id>").next())
            .unwrap_or("")
            .trim()
            .to_owned();
        out.push((id, block.to_owned()));
    }
    out
}

fn title_of(blocks: &[(String, String)], issue_id: &str) -> String {
    let block = blocks
        .iter()
        .find(|(id, _)| id == issue_id)
        .unwrap_or_else(|| panic!("entry block for {issue_id} not found"));
    block
        .1
        .split("<title>")
        .nth(1)
        .and_then(|s| s.split("</title>").next())
        .unwrap_or("")
        .to_owned()
}

// ────────────── v1 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_decorates_each_state_with_glyph_and_page_count() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-states@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Glyph Series").await;
    // a finished, b in progress (page 14 of 32), c unread.
    let a = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("a.cbz"),
        b"g-a",
        1.0,
        "First Strike",
        Some(32),
    )
    .await;
    let b = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("b.cbz"),
        b"g-b",
        2.0,
        "Second Strike",
        Some(32),
    )
    .await;
    let c = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("c.cbz"),
        b"g-c",
        3.0,
        "Third Strike",
        Some(32),
    )
    .await;
    seed_progress(&db, auth.user_id, &a, 31, 1.0, true).await;
    seed_progress(&db, auth.user_id, &b, 13, 13.0 / 32.0, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &a),
        "\u{25CF} First Strike (32 / 32)",
        "finished should be ● with (M / M):\n{body}"
    );
    assert_eq!(
        title_of(&blocks, &b),
        "\u{25D0} Second Strike (14 / 32)",
        "in-progress should be ◐ with (N+1 / M):\n{body}"
    );
    assert_eq!(
        title_of(&blocks, &c),
        "\u{25CB} Third Strike",
        "unread should be ◯ with no page suffix (no progress row):\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_omits_page_count_suffix_when_unknown() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-nopagect@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Unknown Pages").await;
    let i = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("a.cbz"),
        b"np-a",
        1.0,
        "Mystery",
        None,
    )
    .await;
    seed_progress(&db, auth.user_id, &i, 5, 0.5, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &i),
        "\u{25D0} Mystery",
        "unknown page_count should drop the (N / M) suffix:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_respects_user_opt_out_flag() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-optout@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Plain").await;
    let i = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("a.cbz"),
        b"po-a",
        1.0,
        "Vanilla",
        Some(32),
    )
    .await;
    seed_progress(&db, auth.user_id, &i, 13, 0.5, false).await;

    // Flip the per-user flag.
    let mut u: user_entity::ActiveModel = user_entity::Entity::find_by_id(auth.user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    u.opds_progress_glyphs = Set(false);
    u.update(&db).await.unwrap();

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &i),
        "Vanilla",
        "opt-out must strip glyph + suffix entirely:\n{body}"
    );
    assert!(
        !body.contains("\u{25CB}") && !body.contains("\u{25D0}") && !body.contains("\u{25CF}"),
        "no progress glyph should appear anywhere in feed when opted out:\n{body}"
    );
}

// ────────────── v2 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_series_feed_decorates_publication_title() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v2@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "V2 Series").await;
    let _a = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("a.cbz"),
        b"v2-a",
        1.0,
        "Issue A",
        Some(20),
    )
    .await;
    let b = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("b.cbz"),
        b"v2-b",
        2.0,
        "Issue B",
        Some(20),
    )
    .await;
    seed_progress(&db, auth.user_id, &b, 4, 0.25, false).await;

    let resp = get_cookie(&app, &format!("/opds/v2/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    let pubs = body["publications"].as_array().expect("publications array");
    let title_b = pubs
        .iter()
        .find(|p| p["metadata"]["identifier"] == format!("urn:folio:issue:{b}"))
        .expect("issue b publication")["metadata"]["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(title_b, "\u{25D0} Issue B (5 / 20)");
}
