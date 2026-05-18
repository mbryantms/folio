//! Integration tests for M2.4 of opds-sync-1.0 — `?resume=1` opt-in
//! synthetic Resume entry.
//!
//! Verifies:
//!  - Synthetic entry sits at index 0 of a CBL feed when the user is
//!    mid-list (resolver lands on a non-first canonical entry).
//!  - Synthetic NOT emitted when the user hasn't started — up-next
//!    would point at the first entry and the synthetic would be a
//!    redundant duplicate.
//!  - Canonical entry ordering is preserved under the synthetic
//!    prepend (originals still follow CBL position).
//!  - Synthetic id matches the `folio:resume:<canonical_id>` shape so
//!    M2.3-aware clients can correlate and dedupe.

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
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
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

/// Setup helper: 3-issue CBL where `finished_count` issues at the front
/// are marked finished. Returns (list_id, issue_ids in CBL position order).
async fn setup_cbl_with_progress(
    app: &TestApp,
    user_id: Uuid,
    finished_count: usize,
    tmp: &std::path::Path,
) -> (Uuid, Vec<String>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = seed_library(&db, tmp).await;
    let series_id = seed_series(&db, lib_id, "Resume Series").await;

    let mut ids = Vec::new();
    for n in 0..3 {
        let path = tmp.join(format!("issue-{n}.cbz"));
        let payload = format!("resume-{n}").into_bytes();
        let id = seed_issue(&db, lib_id, series_id, &path, &payload, (n + 1) as f64).await;
        ids.push(id);
    }
    for id in ids.iter().take(finished_count) {
        seed_progress_finished(&db, user_id, id).await;
    }

    let list_id = seed_cbl_list(&db, user_id, "Resume Test").await;
    for (pos, id) in ids.iter().enumerate() {
        seed_cbl_entry(&db, list_id, pos as i32, Some(id)).await;
    }
    (list_id, ids)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_with_resume_prepends_synthetic_at_index_zero_when_mid_list() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "resume-mid@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    // Issues 0 and 1 finished; up-next = issue 2.
    let (list_id, ids) = setup_cbl_with_progress(&app, auth.user_id, 2, tmp.path()).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}?resume=1"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // First <entry> must be the synthetic resume entry.
    let first_entry_idx = body.find("<entry>").expect("at least one entry");
    let first_entry_end = body[first_entry_idx..]
        .find("</entry>")
        .expect("entry close")
        + first_entry_idx;
    let first_entry = &body[first_entry_idx..first_entry_end];
    let resume_target = &ids[2];
    assert!(
        first_entry.contains(&format!("folio:resume:{resume_target}")),
        "first entry is the synthetic for issue 2: {first_entry}"
    );
    assert!(
        first_entry.contains("term=\"folio:resume\""),
        "synthetic carries folio:resume category: {first_entry}"
    );
    assert!(
        first_entry.contains("\u{25B6} Resume"),
        "synthetic title prefixed with ▶ Resume: {first_entry}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_with_resume_has_no_synthetic_when_user_hasnt_started() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "resume-fresh@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    // No progress — up-next would land on issue 0, identical to the
    // first canonical entry; the synthetic would just duplicate.
    let (list_id, _ids) = setup_cbl_with_progress(&app, auth.user_id, 0, tmp.path()).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}?resume=1"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains("folio:resume:"),
        "fresh CBL does NOT prepend a synthetic — would be redundant: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_resume_preserves_canonical_entry_ordering() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "resume-order@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    // Issue 0 finished; up-next = issue 1.
    let (list_id, ids) = setup_cbl_with_progress(&app, auth.user_id, 1, tmp.path()).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}?resume=1"), &auth).await;
    let body = body_text(resp.into_body()).await;
    // After the synthetic, the canonical urn:issue: entries must still
    // appear in CBL position order. Find each one's position in the body
    // and assert monotonic.
    let pos_0 = body
        .find(&format!("urn:issue:{}", ids[0]))
        .expect("issue 0 present");
    let pos_1 = body
        .find(&format!("urn:issue:{}", ids[1]))
        .expect("issue 1 present");
    let pos_2 = body
        .find(&format!("urn:issue:{}", ids[2]))
        .expect("issue 2 present");
    assert!(pos_0 < pos_1, "canonical order 0→1 preserved");
    assert!(pos_1 < pos_2, "canonical order 1→2 preserved");
    // Synthetic's id contains `folio:resume:<id>`; its substring uses
    // `folio:resume:` so it must appear before urn:issue:ids[1] (the
    // resume target's canonical entry).
    let resume_target_id = &ids[1];
    let synth_pos = body
        .find(&format!("folio:resume:{resume_target_id}"))
        .expect("synthetic present");
    assert!(
        synth_pos < pos_1,
        "synthetic prepends BEFORE its canonical entry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_resume_synthetic_id_matches_folio_resume_shape() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "resume-id@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (list_id, ids) = setup_cbl_with_progress(&app, auth.user_id, 1, tmp.path()).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}?resume=1"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let resume_target = &ids[1];
    let expected_id_tag = format!("<id>folio:resume:{resume_target}</id>");
    assert!(
        body.contains(&expected_id_tag),
        "synthetic id uses folio:resume:<canonical_id> shape (got body: {body})"
    );
}
