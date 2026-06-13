//! Library Scanner v1 — Milestone 5 health issue catalog (spec §10).
//!
//! Validates:
//!   - layout violations (file at root, empty folder) are persisted as rows
//!   - re-scanning the same problem upserts the existing row (no duplicates)
//!   - fixing the underlying problem auto-resolves the row on the next scan
//!   - missing ComicInfo emits a row only when `report_missing_comicinfo=true`
//!   - the `GET .../health-issues` endpoint hides resolved + dismissed by
//!     default and surfaces them when asked
//!   - `POST .../dismiss` flips dismissed_at and the issue stops appearing

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{library::ActiveModel as LibraryAM, library_health_issue::Entity as HealthEntity};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

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
                    r#"{"email":"hi@example.com","password":"correctly-horse-battery"}"#,
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

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path, report_missing: bool) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Health Lib".into()),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(report_missing),
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
    id
}

async fn list_health(app: &TestApp, auth: &Authed, lib_id: Uuid, query: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/libraries/{lib_id}/health-issues?{query}"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

#[tokio::test]
async fn layout_violations_persist_and_auto_resolve() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    // One legit series + a stray file at root + an empty folder.
    let foo = tmp.path().join("Series Foo (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Foo 001.cbz"), 1);
    write_cbz(&tmp.path().join("orphan.cbz"), 2);
    let empty = tmp.path().join("Empty Series");
    std::fs::create_dir_all(&empty).unwrap();

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();

    scanner::scan_library(&state, lib_id).await.unwrap();

    let body = list_health(&app, &auth, lib_id, "").await;
    let issues = body.as_array().unwrap();
    let kinds: std::collections::HashSet<_> = issues
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(kinds.contains("FileAtRoot"), "missing FileAtRoot: {body}");
    assert!(kinds.contains("EmptyFolder"), "missing EmptyFolder: {body}");

    // Re-scan with the orphan removed and the empty folder filled.
    std::fs::remove_file(tmp.path().join("orphan.cbz")).unwrap();
    write_cbz(&empty.join("Filler 001.cbz"), 3);

    scanner::scan_library(&state, lib_id).await.unwrap();
    let body2 = list_health(&app, &auth, lib_id, "").await;
    let issues2 = body2.as_array().unwrap();
    let kinds2: std::collections::HashSet<_> = issues2
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !kinds2.contains("FileAtRoot"),
        "FileAtRoot should auto-resolve after fixing the layout: {body2}",
    );
    assert!(
        !kinds2.contains("EmptyFolder"),
        "EmptyFolder should auto-resolve: {body2}",
    );

    // include_resolved=true brings them back.
    let body3 = list_health(&app, &auth, lib_id, "include_resolved=true").await;
    let issues3 = body3.as_array().unwrap();
    let resolved: Vec<&serde_json::Value> = issues3
        .iter()
        .filter(|v| v["resolved_at"].is_string())
        .collect();
    assert!(
        resolved.len() >= 2,
        "expected at least 2 resolved rows, got {body3}",
    );

    // No duplicates: re-running the same FileAtRoot scenario doesn't multiply.
    write_cbz(&tmp.path().join("orphan2.cbz"), 4);
    scanner::scan_library(&state, lib_id).await.unwrap();
    write_cbz(&tmp.path().join("orphan2.cbz"), 4); // identical content (same hash, same mtime path)
    scanner::scan_library(&state, lib_id).await.unwrap();
    let body4 = list_health(&app, &auth, lib_id, "").await;
    let count_orphan2: usize = body4
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| {
            v["kind"] == "FileAtRoot"
                && v["payload"]["data"]["path"]
                    .as_str()
                    .map(|p| p.ends_with("orphan2.cbz"))
                    .unwrap_or(false)
        })
        .count();
    assert_eq!(
        count_orphan2, 1,
        "FileAtRoot should upsert, not duplicate: {body4}"
    );
}

#[tokio::test]
async fn missing_comicinfo_only_when_requested() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Bar (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Bar 001.cbz"), 1); // No ComicInfo.xml inside.

    // Default library: report_missing_comicinfo=false.
    let silent_lib = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, silent_lib).await.unwrap();

    let body = list_health(&app, &auth, silent_lib, "").await;
    let kinds: std::collections::HashSet<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !kinds.contains("MissingComicInfo"),
        "should not emit MissingComicInfo when flag is false: {body}",
    );

    // Same fixtures, second library, report_missing_comicinfo=true.
    let tmp2 = tempfile::tempdir().unwrap();
    let foo2 = tmp2.path().join("Series Bar (2024)");
    std::fs::create_dir_all(&foo2).unwrap();
    write_cbz(&foo2.join("Bar 001.cbz"), 1);
    let loud_lib = create_library(&app, tmp2.path(), true).await;
    scanner::scan_library(&state, loud_lib).await.unwrap();
    let body2 = list_health(&app, &auth, loud_lib, "").await;
    let kinds2: std::collections::HashSet<_> = body2
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        kinds2.contains("MissingComicInfo"),
        "should emit MissingComicInfo when flag is true: {body2}",
    );
}

#[tokio::test]
async fn dismiss_hides_issue() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Baz (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Baz 001.cbz"), 1);
    write_cbz(&tmp.path().join("orphan.cbz"), 2);

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    // Find the FileAtRoot issue id directly via the entity, then call dismiss.
    let issues = HealthEntity::find().all(&state.db).await.unwrap();
    let target = issues
        .iter()
        .find(|i| i.kind == "FileAtRoot")
        .expect("FileAtRoot row");
    let issue_id = target.id;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/libraries/{lib_id}/health-issues/{issue_id}/dismiss"
                ))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Default list now hides the dismissed row.
    let body = list_health(&app, &auth, lib_id, "").await;
    let kinds: std::collections::HashSet<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(!kinds.contains("FileAtRoot"));

    // include_dismissed=true brings it back.
    let body2 = list_health(&app, &auth, lib_id, "include_dismissed=true").await;
    let dismissed = body2
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["dismissed_at"].is_string())
        .count();
    assert!(dismissed >= 1);
}

/// Write a CBZ that contains a 1 MiB entry of zero bytes alongside a
/// normal page. Deflate squashes the zeros to ~1 KiB → ratio ~1000:1,
/// far past the default 200x cap. The scanner's compression-ratio
/// soft defense drops the bomb entry from the page index; recovery-
/// visibility Tranche A emits a `SkippedArchiveEntries` health-issue.
fn write_cbz_with_bomb_entry(path: &Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let stored: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    // Real PNG header on the legit page so cover-thumb probing doesn't warn.
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&[0xAB, 0xCD, 0xEF, 0x12]);
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", stored).unwrap();
    zw.write_all(&png).unwrap();
    // 1 MiB of zeros → after deflate, ratio ≫ 200.
    let bomb = vec![0u8; 1024 * 1024];
    zw.start_file("page-002.png", deflated).unwrap();
    zw.write_all(&bomb).unwrap();
    zw.finish().unwrap();
}

/// Recovery-visibility Tranche A:
/// `SkippedArchiveEntries` health-issue fires when the archive's
/// compression-ratio soft defense drops one or more entries during
/// open. The archive still opens; the user sees fewer pages than the
/// archive contains; the admin Health tab gets a warning row that
/// names the count + reason.
#[tokio::test]
async fn skipped_archive_entries_emits_health_issue() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series Skip (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz_with_bomb_entry(&folder.join("Skip 001.cbz"));

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issues = HealthEntity::find().all(&state.db).await.unwrap();
    let skipped = issues
        .iter()
        .find(|i| i.kind == "SkippedArchiveEntries")
        .expect("SkippedArchiveEntries row should land in library_health_issues");
    let data = &skipped.payload["data"];
    assert_eq!(data["dropped"].as_u64().unwrap(), 1);
    // Total = legit page + bomb = 2 entries the archive crate saw.
    assert_eq!(data["total"].as_u64().unwrap(), 2);
    let reason = data["reason"].as_str().unwrap();
    assert!(
        reason.contains("compression ratio"),
        "reason should name the soft defense, got: {reason}",
    );
    assert_eq!(skipped.severity, "warning");

    // Re-scan: row should UPDATE in place, not duplicate.
    scanner::scan_library_with(&state, lib_id, true)
        .await
        .unwrap();
    let count = HealthEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == "SkippedArchiveEntries")
        .count();
    assert_eq!(
        count, 1,
        "re-scan must upsert by fingerprint, not duplicate"
    );
}

/// Write a CBZ with a poisoned Info-ZIP Unicode-Path extra (`0x7075`)
/// in the CDFH of the first entry. The publisher's tool computed the
/// CRC32 over the UTF-8 filename bytes; the zip crate computes it
/// over the raw CP437 `file_name` field; they never match, so opening
/// fails with `CRC32 checksum failed on Unicode extra field`. The
/// archive crate's recovery branch strips the bad extras and the
/// scanner emits a `RecoveredArchive` health-issue.
///
/// Pattern mirrors `cbz::tests::opens_cbz_with_corrupt_unicode_path_crc`
/// — kept inline here so the integration test stays self-contained.
fn write_cbz_with_poisoned_unicode_extra(path: &Path) {
    let plain_path = tempfile::NamedTempFile::new().unwrap();
    {
        let mut zw = zip::ZipWriter::new(plain_path.reopen().unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(std::iter::repeat_n(0u8, 64));
        zw.start_file("page.png", opts).unwrap();
        zw.write_all(&png).unwrap();
        zw.finish().unwrap();
    }
    let bytes = std::fs::read(plain_path.path()).unwrap();
    let eocd_off = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .expect("EOCD");
    let cd_size =
        u32::from_le_bytes(bytes[eocd_off + 12..eocd_off + 16].try_into().unwrap()) as usize;
    let cd_offset =
        u32::from_le_bytes(bytes[eocd_off + 16..eocd_off + 20].try_into().unwrap()) as usize;
    let fname_len =
        u16::from_le_bytes(bytes[cd_offset + 28..cd_offset + 30].try_into().unwrap()) as usize;
    let extra_len =
        u16::from_le_bytes(bytes[cd_offset + 30..cd_offset + 32].try_into().unwrap()) as usize;
    let poison: Vec<u8> = vec![
        0x75, 0x70, // tag = 0x7075 (LE)
        0x09, 0x00, // payload length = 9
        0x01, // version
        0xDE, 0xAD, 0xBE, 0xEF, // CRC32 that won't match the filename
        b'p', b'a', b'g', b'e',
    ];
    let inject_at = cd_offset + 46 + fname_len + extra_len;
    let mut poisoned = Vec::with_capacity(bytes.len() + poison.len());
    poisoned.extend_from_slice(&bytes[..inject_at]);
    poisoned.extend_from_slice(&poison);
    poisoned.extend_from_slice(&bytes[inject_at..]);
    let new_extra_len = (extra_len + poison.len()) as u16;
    poisoned[cd_offset + 30..cd_offset + 32].copy_from_slice(&new_extra_len.to_le_bytes());
    let new_eocd_off = eocd_off + poison.len();
    let new_cd_size = (cd_size + poison.len()) as u32;
    poisoned[new_eocd_off + 12..new_eocd_off + 16].copy_from_slice(&new_cd_size.to_le_bytes());
    std::fs::write(path, &poisoned).unwrap();
}

/// Recovery-visibility Tranche A:
/// `RecoveredArchive` health-issue fires when the archive crate's
/// `open_zip_with_recovery` had to repair the file's structure to
/// make it openable. The archive reads normally afterwards; the
/// health-issue is informational ("your library has files the
/// publisher's tooling botched; Folio papered over them").
#[tokio::test]
async fn recovered_archive_emits_health_issue() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series Recover (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz_with_poisoned_unicode_extra(&folder.join("Recover 001.cbz"));

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issues = HealthEntity::find().all(&state.db).await.unwrap();
    let recovered = issues
        .iter()
        .find(|i| i.kind == "RecoveredArchive")
        .expect("RecoveredArchive row should land in library_health_issues");
    let data = &recovered.payload["data"];
    assert_eq!(
        data["technique"].as_str().unwrap(),
        "unicode-path-crc-strip"
    );
    assert_eq!(recovered.severity, "info");
}

/// Write a CBZ whose page entry is GARBAGE BYTES that won't decode as
/// an image. The archive opens cleanly (header is fine, csize is
/// honest, entry name validates), but the `image` crate fails on
/// `load_from_memory`. Tranche C's deep-validate walks every page
/// through the decoder, so this file's run should produce an
/// `UnreadablePage` health-issue.
fn write_cbz_with_undecodable_page(path: &Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let stored: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    // Random non-image bytes. No PNG / JPEG magic, no recognized
    // header. The `image` crate's decoder family rejects on header.
    let junk: Vec<u8> = (0u8..=255u8).cycle().take(2048).collect();
    zw.start_file("page-001.png", stored).unwrap();
    zw.write_all(&junk).unwrap();
    zw.finish().unwrap();
}

/// Tranche C of recovery-visibility:
/// `library::deep_validate::run` walks every active issue, decodes
/// every page, and emits `UnreadablePage` for failures. The regular
/// scan ingests the corrupt-page file cleanly (LFH/CDFH intact); only
/// the deep run catches the decode failure.
#[tokio::test]
async fn deep_validate_emits_unreadable_page() {
    use server::library::deep_validate;

    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series Garbage (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz_with_undecodable_page(&folder.join("Garbage 001.cbz"));

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    // Regular scan ingests the file as a normal issue (no decode
    // probing).
    scanner::scan_library(&state, lib_id).await.unwrap();
    let pre = HealthEntity::find().all(&state.db).await.unwrap();
    assert!(
        !pre.iter().any(|i| i.kind == "UnreadablePage"),
        "regular scan must NOT decode pages",
    );

    // Deep-validate finds the decode failure.
    let stats = deep_validate::run(&state, lib_id).await.unwrap();
    assert!(stats.issues_probed >= 1);
    assert!(stats.pages_unreadable >= 1);

    let post = HealthEntity::find().all(&state.db).await.unwrap();
    let unreadable = post
        .iter()
        .find(|i| i.kind == "UnreadablePage")
        .expect("UnreadablePage row should land after deep-validate");
    let data = &unreadable.payload["data"];
    assert_eq!(data["page_index"].as_u64().unwrap(), 0);
    assert_eq!(unreadable.severity, "warning");

    // Re-run deep-validate: row should upsert by fingerprint, not
    // duplicate.
    let _ = deep_validate::run(&state, lib_id).await.unwrap();
    let count = HealthEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .filter(|i| i.kind == "UnreadablePage")
        .count();
    assert_eq!(count, 1, "re-validate must upsert by fingerprint");
}

/// Tranche B per-issue health-issue endpoint:
/// `GET /series/{series_slug}/issues/{issue_slug}/health-issues` returns
/// the open rows whose `payload.data.path` matches this issue's file
/// path. The endpoint powers the issue-detail badge + reader-open
/// toast.
#[tokio::test]
async fn per_issue_health_endpoint_returns_matching_rows() {
    use entity::{issue, series};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series PerIssue (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz_with_bomb_entry(&folder.join("PerIssue 001.cbz"));

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    // Look up the (series_slug, issue_slug) the scanner allocated.
    let issue_row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue ingested");
    let series_row = series::Entity::find_by_id(issue_row.series_id)
        .one(&state.db)
        .await
        .unwrap()
        .expect("series row");

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series/{}/issues/{}/health-issues",
                    series_row.slug, issue_row.slug,
                ))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let rows = body.as_array().expect("array");

    let kinds: std::collections::HashSet<_> = rows
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        kinds.contains("SkippedArchiveEntries"),
        "endpoint should surface the per-file row matched by path: {body}",
    );

    // A user with no library access (404 by design) — confirm the
    // endpoint isn't a side-channel for stranger-library data. We
    // achieve "no access" by querying with a different (unrelated)
    // user. Easiest: hit the endpoint without any cookie at all,
    // which should reject as 401.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series/{}/issues/{}/health-issues",
                    series_row.slug, issue_row.slug,
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

async fn admin_list_health(app: &TestApp, auth: &Authed, query: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/admin/health-issues?{query}"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

#[tokio::test]
async fn admin_cross_library_health_aggregates_and_filters() {
    // Two libraries, each with at least one health issue, then assert
    // the cross-library endpoint:
    //   - returns rows from BOTH libraries (the aggregate cuts the
    //     "click into 22 libraries" workflow)
    //   - enriches each row with the originating library's name + slug
    //   - filters by library_id when scoped to one
    //   - rejects invalid severity / library_id values with 422
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let tmp_a = tempfile::tempdir().unwrap();
    let foo_a = tmp_a.path().join("Series A (2020)");
    std::fs::create_dir_all(&foo_a).unwrap();
    write_cbz(&foo_a.join("A 001.cbz"), 1);
    write_cbz(&tmp_a.path().join("orphan-a.cbz"), 2); // FileAtRoot in lib A
    let lib_a = create_library(&app, tmp_a.path(), false).await;

    let tmp_b = tempfile::tempdir().unwrap();
    let foo_b = tmp_b.path().join("Series B (2020)");
    std::fs::create_dir_all(&foo_b).unwrap();
    write_cbz(&foo_b.join("B 001.cbz"), 3);
    write_cbz(&tmp_b.path().join("orphan-b.cbz"), 4); // FileAtRoot in lib B
    let lib_b = create_library(&app, tmp_b.path(), false).await;

    let state = app.state();
    scanner::scan_library(&state, lib_a).await.unwrap();
    scanner::scan_library(&state, lib_b).await.unwrap();

    // Default (no library filter): both libraries' rows surface.
    let body = admin_list_health(&app, &auth, "").await;
    let items = body["items"].as_array().expect("items");
    let library_ids: std::collections::HashSet<&str> = items
        .iter()
        .map(|v| v["library_id"].as_str().unwrap())
        .collect();
    assert!(
        library_ids.contains(lib_a.to_string().as_str()),
        "lib A rows missing from cross-library list: {body}",
    );
    assert!(
        library_ids.contains(lib_b.to_string().as_str()),
        "lib B rows missing from cross-library list: {body}",
    );

    // Enrichment: library_name + library_slug carried per row.
    for item in items {
        assert!(item["library_name"].as_str().is_some());
        assert!(item["library_slug"].as_str().is_some());
    }
    assert!(body["next_cursor"].is_null() || body["next_cursor"].is_string());

    // Scoped to lib_a: only lib_a rows.
    let scoped = admin_list_health(&app, &auth, &format!("library_id={lib_a}")).await;
    for item in scoped["items"].as_array().unwrap() {
        assert_eq!(item["library_id"], lib_a.to_string());
    }

    // Invalid severity → 422 (not a silent empty list).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/health-issues?severity=bogus")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Invalid library_id → 422.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/health-issues?library_id=not-a-uuid")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn admin_cross_library_health_cursor_paginates() {
    // Smaller libraries, force pagination with limit=2.
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let tmp = tempfile::tempdir().unwrap();
    // 3 stray files at root → 3 FileAtRoot rows
    for i in 1..=3u32 {
        write_cbz(&tmp.path().join(format!("orphan-{i}.cbz")), i);
    }
    // One real series so the library is valid
    let foo = tmp.path().join("Series Foo (2020)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Foo 001.cbz"), 100);

    let lib = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib).await.unwrap();

    let p1 = admin_list_health(&app, &auth, "limit=2").await;
    let p1_items = p1["items"].as_array().expect("items");
    assert_eq!(p1_items.len(), 2, "page 1 should hit the limit");
    let next = p1["next_cursor"]
        .as_str()
        .expect("more rows exist → cursor non-null");

    let p2 = admin_list_health(
        &app,
        &auth,
        &format!("limit=2&cursor={}", urlencoding::encode(next)),
    )
    .await;
    let p2_items = p2["items"].as_array().expect("items");
    assert!(!p2_items.is_empty(), "page 2 should have remaining rows");

    // No overlap between pages (cursor pagination is strict).
    let ids_1: std::collections::HashSet<&str> =
        p1_items.iter().map(|v| v["id"].as_str().unwrap()).collect();
    let ids_2: std::collections::HashSet<&str> =
        p2_items.iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(
        ids_1.is_disjoint(&ids_2),
        "page 1 and page 2 must not share ids",
    );
}

/// `POST .../undismiss` clears dismissed_at so the row reappears in
/// the default list, and writes its own audit action. D1 follow-up:
/// dismiss used to be silently irreversible.
#[tokio::test]
async fn undismiss_restores_issue_and_audits() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Undis (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Undis 001.cbz"), 1);
    write_cbz(&tmp.path().join("orphan.cbz"), 2);

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issues = HealthEntity::find().all(&state.db).await.unwrap();
    let target = issues
        .iter()
        .find(|i| i.kind == "FileAtRoot")
        .expect("FileAtRoot row");
    let issue_id = target.id;

    let post = |verb: &'static str| {
        let uri = format!("/api/libraries/{lib_id}/health-issues/{issue_id}/{verb}");
        let req = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    auth.session, auth.csrf
                ),
            )
            .header("X-CSRF-Token", &auth.csrf)
            .body(Body::empty())
            .unwrap();
        app.router.clone().oneshot(req)
    };

    let resp = post("dismiss").await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Hidden while dismissed.
    let body = list_health(&app, &auth, lib_id, "").await;
    assert!(
        !body
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["kind"] == "FileAtRoot"),
        "dismissed row must be hidden: {body}",
    );

    let resp = post("undismiss").await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Back in the default list with dismissed_at cleared.
    let body = list_health(&app, &auth, lib_id, "").await;
    let restored = body
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "FileAtRoot")
        .expect("undismissed row is visible again");
    assert!(restored["dismissed_at"].is_null());

    // Idempotent on a never/no-longer-dismissed row.
    let resp = post("undismiss").await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Both actions left audit rows.
    use sea_orm::{ColumnTrait, QueryFilter};
    let actions: Vec<String> = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::TargetId.eq(issue_id.to_string()))
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert!(
        actions
            .iter()
            .any(|a| a == "admin.library.health_issue.dismiss"),
        "dismiss audit row present: {actions:?}",
    );
    assert!(
        actions
            .iter()
            .any(|a| a == "admin.library.health_issue.undismiss"),
        "undismiss audit row present: {actions:?}",
    );
}
