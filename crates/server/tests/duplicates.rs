//! Duplicates page (roadmap WP-3.3, audit R15 / UX-9 / DI-21).
//!
//! Covers:
//!   - `number` groups: same `(series, sort_number, special_type)`; an
//!     Annual #1 never groups with the regular #1.
//!   - `hash` groups: exact content-hash copies in a library with
//!     `dedupe_by_content = false` (the flag is honoured by the scanner);
//!     the all-kinds listing suppresses the number group that is a subset.
//!   - `cover` groups: primary-cover pHash Hamming ≤ 8 within one series.
//!   - cross-library same file: ingested in both libraries, no
//!     `DuplicateContent` health row, no duplicate group.
//!   - decisions: keep hides a fully-reviewed group; remove soft-deletes and
//!     survives a rescan; clearing restores; all three are audited.
//!   - cursor pagination with `total` + `counts` on the first page only.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::{self, Entity as IssueEntity},
    library::ActiveModel as LibraryAM,
    series::Entity as SeriesEntity,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, Statement,
};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"dupes@example.com","password":"correctly-horse-battery"}"#,
                ))
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// CBZ with a unique PNG payload (`marker`) and optional ComicInfo.
fn write_cbz(path: &Path, marker: u32, comic_info: Option<&str>) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

fn comic_info(number: &str, format: Option<&str>) -> String {
    let fmt = format
        .map(|f| format!("<Format>{f}</Format>"))
        .unwrap_or_default();
    format!(r#"<?xml version="1.0"?><ComicInfo><Number>{number}</Number>{fmt}</ComicInfo>"#)
}

async fn create_library(app: &TestApp, root: &Path, dedupe_by_content: bool) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set(format!("Dupes Lib {id}")),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(dedupe_by_content),
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
        trust_fingerprint_on_first_import: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn list(app: &TestApp, auth: &Authed, lib: Uuid, qs: &str) -> serde_json::Value {
    let (status, body) = send(
        app,
        auth,
        Method::GET,
        &format!("/api/libraries/{lib}/duplicates{qs}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn group_sizes(body: &serde_json::Value) -> Vec<(String, usize)> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g["kind"].as_str().unwrap().to_owned(),
                g["issues"].as_array().unwrap().len(),
            )
        })
        .collect()
}

async fn issue_by_path(app: &TestApp, path: &Path) -> issue::Model {
    IssueEntity::find()
        .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("issue row")
}

async fn slugs(app: &TestApp, row: &issue::Model) -> (String, String) {
    let s = SeriesEntity::find_by_id(row.series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    (s.slug, row.slug.clone())
}

async fn audit_actions(app: &TestApp) -> Vec<String> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        action: String,
    }
    use sea_orm::FromQueryResult;
    Row::find_by_statement(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT action FROM audit_log WHERE action LIKE 'admin.issue.duplicate.%' ORDER BY created_at",
    ))
    .all(&app.state().db)
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.action)
    .collect()
}

#[tokio::test]
async fn number_groups_share_series_number_and_special_type() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Alpha (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    // Two different files both numbered #1 → one number group.
    write_cbz(
        &folder.join("Alpha 001.cbz"),
        1,
        Some(&comic_info("1", None)),
    );
    write_cbz(
        &folder.join("Alpha 001 (repack).cbz"),
        2,
        Some(&comic_info("1", None)),
    );
    // #2 is unique; Annual #1 differs by special_type → neither groups.
    write_cbz(
        &folder.join("Alpha 002.cbz"),
        3,
        Some(&comic_info("2", None)),
    );
    write_cbz(
        &folder.join("Alpha Annual 001.cbz"),
        4,
        Some(&comic_info("1", Some("Annual"))),
    );

    let lib = create_library(&app, tmp.path(), true).await;
    let stats = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(stats.files_added, 4, "{stats:?}");

    let body = list(&app, &auth, lib, "?kind=number").await;
    assert_eq!(group_sizes(&body), vec![("number".to_owned(), 2)], "{body}");
    assert_eq!(body["total"], 1);
    let g = &body["items"][0];
    let paths: Vec<&str> = g["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["file_path"].as_str().unwrap())
        .collect();
    assert!(paths.iter().all(|p| !p.contains("Annual")), "{paths:?}");
    assert!(g["issues"][0]["cover_url"].as_str().is_some());

    // No hash or cover groups in this library.
    assert_eq!(body["counts"]["hash"], 0);
    assert_eq!(body["counts"]["cover"], 0);
    assert_eq!(body["counts"]["number"], 1);
}

#[tokio::test]
async fn hash_groups_when_dedupe_by_content_is_off() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Beta (2021)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(
        &folder.join("Beta 001.cbz"),
        10,
        Some(&comic_info("1", None)),
    );
    std::fs::copy(
        folder.join("Beta 001.cbz"),
        folder.join("Beta 001 (copy).cbz"),
    )
    .unwrap();

    // dedupe_by_content = false: both copies ingest as their own issue.
    let lib = create_library(&app, tmp.path(), false).await;
    let stats = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(stats.files_added, 2, "{stats:?}");
    assert_eq!(stats.files_duplicate, 0, "{stats:?}");
    let a = issue_by_path(&app, &folder.join("Beta 001.cbz")).await;
    let b = issue_by_path(&app, &folder.join("Beta 001 (copy).cbz")).await;
    assert_eq!(a.content_hash, b.content_hash);
    assert_ne!(a.id, b.id, "second copy gets a path-derived id");

    let body = list(&app, &auth, lib, "?kind=hash").await;
    assert_eq!(group_sizes(&body), vec![("hash".to_owned(), 2)], "{body}");

    // Same pair is also a number group; in the all-kinds listing it is a
    // subset of the stronger hash group and is suppressed.
    let body = list(&app, &auth, lib, "").await;
    assert_eq!(group_sizes(&body), vec![("hash".to_owned(), 2)], "{body}");
    assert_eq!(body["total"], 1);
    assert_eq!(body["counts"]["hash"], 1);
    assert_eq!(body["counts"]["number"], 1);

    // Rescan is stable — the per-path lookup finds both rows again.
    let again = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(again.files_added, 0, "{again:?}");
}

#[tokio::test]
async fn dedupe_on_still_skips_same_library_copy() {
    // The flag's default behaviour is unchanged: a second copy in the
    // same library is skipped with a DuplicateContent health row.
    let tmp = tempfile::tempdir().unwrap();
    let app = TestApp::spawn().await;
    let folder = tmp.path().join("Gamma (2022)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Gamma 001.cbz"), 20, None);
    std::fs::copy(
        folder.join("Gamma 001.cbz"),
        folder.join("Gamma 001 (copy).cbz"),
    )
    .unwrap();
    let lib = create_library(&app, tmp.path(), true).await;
    let stats = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    assert_eq!(stats.files_duplicate, 1, "{stats:?}");
}

#[tokio::test]
async fn cross_library_same_file_is_not_a_duplicate() {
    use entity::library_health_issue::{Column as HCol, Entity as HealthEntity};
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp_a = tempfile::tempdir().unwrap();
    let tmp_b = tempfile::tempdir().unwrap();
    let folder_a = tmp_a.path().join("Delta (2023)");
    let folder_b = tmp_b.path().join("Delta (2023)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_cbz(&folder_a.join("Delta 001.cbz"), 30, None);
    std::fs::copy(
        folder_a.join("Delta 001.cbz"),
        folder_b.join("Delta 001.cbz"),
    )
    .unwrap();

    let lib_a = create_library(&app, tmp_a.path(), true).await;
    let lib_b = create_library(&app, tmp_b.path(), true).await;
    let sa = scanner::scan_library(&app.state(), lib_a).await.unwrap();
    let sb = scanner::scan_library(&app.state(), lib_b).await.unwrap();
    assert_eq!(sa.files_added, 1, "{sa:?}");
    assert_eq!(sb.files_added, 1, "library B ingests its own copy: {sb:?}");
    assert_eq!(sb.files_duplicate, 0, "{sb:?}");

    let a = issue_by_path(&app, &folder_a.join("Delta 001.cbz")).await;
    let b = issue_by_path(&app, &folder_b.join("Delta 001.cbz")).await;
    assert_eq!(a.library_id, lib_a);
    assert_eq!(b.library_id, lib_b);
    assert_eq!(a.content_hash, b.content_hash);
    assert_ne!(a.id, b.id);

    let dup_rows = HealthEntity::find()
        .filter(HCol::Kind.eq("DuplicateContent"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(dup_rows.is_empty(), "no DuplicateContent: {dup_rows:?}");

    for lib in [lib_a, lib_b] {
        let body = list(&app, &auth, lib, "").await;
        assert_eq!(body["total"], 0, "{body}");
    }

    // Rescans of both libraries stay stable (no churn, no duplicate).
    let sa2 = scanner::scan_library(&app.state(), lib_a).await.unwrap();
    let sb2 = scanner::scan_library(&app.state(), lib_b).await.unwrap();
    assert_eq!(sa2.files_duplicate + sb2.files_duplicate, 0);
    assert_eq!(sa2.files_added + sb2.files_added, 0);
}

async fn set_primary_phash(app: &TestApp, issue_id: &str, phash: i64) {
    app.state()
        .db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO issue_cover (issue_id, kind, ordinal, local_path, phash, is_active) \
             VALUES ($1, 'primary', 0, 'covers/x.webp', $2, true) \
             ON CONFLICT (issue_id, kind, ordinal) WHERE is_active \
             DO UPDATE SET phash = EXCLUDED.phash",
            [issue_id.into(), phash.into()],
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn cover_groups_by_phash_within_series() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let eps = tmp.path().join("Epsilon (2024)");
    let zeta = tmp.path().join("Zeta (2024)");
    std::fs::create_dir_all(&eps).unwrap();
    std::fs::create_dir_all(&zeta).unwrap();
    for (n, m) in [(1, 41), (2, 42), (3, 43)] {
        write_cbz(
            &eps.join(format!("Epsilon 00{n}.cbz")),
            m,
            Some(&comic_info(&n.to_string(), None)),
        );
    }
    write_cbz(&zeta.join("Zeta 001.cbz"), 44, Some(&comic_info("1", None)));

    let lib = create_library(&app, tmp.path(), true).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let e1 = issue_by_path(&app, &eps.join("Epsilon 001.cbz")).await;
    let e2 = issue_by_path(&app, &eps.join("Epsilon 002.cbz")).await;
    let e3 = issue_by_path(&app, &eps.join("Epsilon 003.cbz")).await;
    let z1 = issue_by_path(&app, &zeta.join("Zeta 001.cbz")).await;

    // e1 ↔ e2: distance 3 (grouped). e3: 40 bits away (not grouped).
    // z1: identical hash to e1 but a different series (not grouped).
    set_primary_phash(&app, &e1.id, 0).await;
    set_primary_phash(&app, &e2.id, 0b111).await;
    set_primary_phash(&app, &e3.id, 0x00FF_FFFF_FFFF).await;
    set_primary_phash(&app, &z1.id, 0).await;

    let body = list(&app, &auth, lib, "?kind=cover").await;
    assert_eq!(group_sizes(&body), vec![("cover".to_owned(), 2)], "{body}");
    let g = &body["items"][0];
    assert_eq!(g["max_cover_distance"], 3);
    let mut ids: Vec<&str> = g["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    ids.sort();
    let mut want = vec![e1.id.as_str(), e2.id.as_str()];
    want.sort();
    assert_eq!(ids, want);

    // Exactly at the threshold (8 bits) still groups; 9 does not.
    set_primary_phash(&app, &e3.id, 0xFF).await;
    let body = list(&app, &auth, lib, "?kind=cover").await;
    assert_eq!(group_sizes(&body), vec![("cover".to_owned(), 3)], "{body}");
    set_primary_phash(&app, &e3.id, 0x1FF << 8).await;
    let body = list(&app, &auth, lib, "?kind=cover").await;
    assert_eq!(group_sizes(&body), vec![("cover".to_owned(), 2)], "{body}");
}

#[tokio::test]
async fn keep_remove_and_clear_decisions() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Eta (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let p1 = folder.join("Eta 001.cbz");
    let p2 = folder.join("Eta 001 (repack).cbz");
    write_cbz(&p1, 51, Some(&comic_info("1", None)));
    write_cbz(&p2, 52, Some(&comic_info("1", None)));
    let lib = create_library(&app, tmp.path(), true).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let a = issue_by_path(&app, &p1).await;
    let b = issue_by_path(&app, &p2).await;
    let (sa, ia) = slugs(&app, &a).await;
    let (sb, ib) = slugs(&app, &b).await;
    let decide = |s: &str, i: &str| format!("/api/series/{s}/issues/{i}/duplicate-decision");

    // Keep one → still listed (the other is undecided), badge carried.
    let (st, _) = send(
        &app,
        &auth,
        Method::PUT,
        &decide(&sa, &ia),
        Some(serde_json::json!({"decision": "keep"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let body = list(&app, &auth, lib, "").await;
    assert_eq!(body["total"], 1);
    let kept: Vec<&serde_json::Value> = body["items"][0]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["decision"] == "keep")
        .collect();
    assert_eq!(kept.len(), 1);

    // Bad decision value → rejected, nothing written.
    let (st, _) = send(
        &app,
        &auth,
        Method::PUT,
        &decide(&sb, &ib),
        Some(serde_json::json!({"decision": "nuke"})),
    )
    .await;
    assert!(st.is_client_error(), "{st}");

    // Remove the other → soft-removed, group gone.
    let (st, _) = send(
        &app,
        &auth,
        Method::PUT,
        &decide(&sb, &ib),
        Some(serde_json::json!({"decision": "remove"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let b_now = issue_by_path(&app, &p2).await;
    assert!(b_now.removed_at.is_some());
    let body = list(&app, &auth, lib, "").await;
    assert_eq!(body["total"], 0, "{body}");

    // The file is still on disk: a full rescan must NOT restore it, and
    // neither must a per-series or single-issue rescan.
    scanner::scan_library(&app.state(), lib).await.unwrap();
    assert!(issue_by_path(&app, &p2).await.removed_at.is_some());
    let series_row = SeriesEntity::find_by_id(b_now.series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    scanner::scan_series_folder(
        &app.state(),
        lib,
        series_row.id,
        &folder,
        scanner::ScanKind::Series,
        None,
        true,
        None,
    )
    .await
    .unwrap();
    assert!(
        issue_by_path(&app, &p2).await.removed_at.is_some(),
        "duplicate soft-remove is sticky across rescans"
    );

    // Clear → restored, group back.
    let (st, _) = send(&app, &auth, Method::DELETE, &decide(&sb, &ib), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(issue_by_path(&app, &p2).await.removed_at.is_none());
    let body = list(&app, &auth, lib, "").await;
    assert_eq!(body["total"], 1);

    // Keep both → fully reviewed group drops off.
    let (st, _) = send(
        &app,
        &auth,
        Method::PUT,
        &decide(&sb, &ib),
        Some(serde_json::json!({"decision": "keep"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let body = list(&app, &auth, lib, "").await;
    assert_eq!(body["total"], 0, "{body}");

    assert_eq!(
        audit_actions(&app).await,
        vec![
            "admin.issue.duplicate.keep",
            "admin.issue.duplicate.remove",
            "admin.issue.duplicate.clear",
            "admin.issue.duplicate.keep",
        ]
    );
}

#[tokio::test]
async fn removed_tab_restore_clears_the_duplicate_pin() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Theta (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let p1 = folder.join("Theta 001.cbz");
    let p2 = folder.join("Theta 001 (repack).cbz");
    write_cbz(&p1, 61, Some(&comic_info("1", None)));
    write_cbz(&p2, 62, Some(&comic_info("1", None)));
    let lib = create_library(&app, tmp.path(), true).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let b = issue_by_path(&app, &p2).await;
    let (s, i) = slugs(&app, &b).await;
    let (st, _) = send(
        &app,
        &auth,
        Method::PUT,
        &format!("/api/series/{s}/issues/{i}/duplicate-decision"),
        Some(serde_json::json!({"decision": "remove"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(
        &app,
        &auth,
        Method::POST,
        &format!("/api/series/{s}/issues/{i}/restore"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let pins = entity::issue_duplicate_decision::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(pins.is_empty(), "restore drops the pin: {pins:?}");
}

#[tokio::test]
async fn duplicates_list_paginates_by_cursor() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Iota (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    for n in 1..=3u32 {
        write_cbz(
            &folder.join(format!("Iota 00{n}.cbz")),
            70 + n,
            Some(&comic_info(&n.to_string(), None)),
        );
        write_cbz(
            &folder.join(format!("Iota 00{n} (alt).cbz")),
            80 + n,
            Some(&comic_info(&n.to_string(), None)),
        );
    }
    let lib = create_library(&app, tmp.path(), true).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();

    let page1 = list(&app, &auth, lib, "?kind=number&limit=2").await;
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    assert_eq!(page1["total"], 3);
    assert!(page1["counts"].is_object());
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("next_cursor")
        .to_owned();

    let page2 = list(
        &app,
        &auth,
        lib,
        &format!("?kind=number&limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["next_cursor"].is_null());
    assert!(page2["total"].is_null(), "total is first-page only");
    assert!(page2["counts"].is_null(), "counts are first-page only");

    let mut keys: Vec<String> = page1["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["items"].as_array().unwrap())
        .map(|g| g["key"].as_str().unwrap().to_owned())
        .collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 3, "pages partition the group set");

    // Numeric order within the series: #1, #2 on page 1.
    let first_num = page1["items"][0]["issues"][0]["number_raw"].as_str();
    assert_eq!(first_num, Some("1"));

    // Garbage cursor → 400 with the canonical envelope.
    let (st, body) = send(
        &app,
        &auth,
        Method::GET,
        &format!("/api/libraries/{lib}/duplicates?cursor=nope"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.cursor");
}

#[tokio::test]
async fn duplicates_list_is_admin_only() {
    let app = TestApp::spawn().await;
    let _admin = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = create_library(&app, tmp.path(), true).await;
    // Unauthenticated → 401/403, never 200.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/libraries/{lib}/duplicates"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_client_error(), "{}", resp.status());
}
