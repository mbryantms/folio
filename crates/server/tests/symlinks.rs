//! Symbolic links below a library root are never followed: not by the
//! scanner (each is reported as a `SymlinkSkipped` health finding) and not
//! when an issue's file is served (`library::no_follow`). The library root
//! itself may be a link. Real files and real links in temp directories, a
//! real database.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, library_health_issue, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set,
    Statement,
};
use server::library::scanner;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use uuid::Uuid;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// CBZ with one stored real-signature PNG page, unique by `marker`.
fn write_cbz(path: &Path, series_name: &str, marker: u32) -> Vec<u8> {
    let mut page = PNG.to_vec();
    page.extend_from_slice(&marker.to_le_bytes());
    page.extend(std::iter::repeat_n(0u8, 64));
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&page).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(
        format!(
            r#"<?xml version="1.0"?><ComicInfo><Series>{series_name}</Series><Number>{marker}</Number></ComicInfo>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zw.finish().unwrap();
    page
}

async fn open_symlink_findings(app: &TestApp, lib_id: Uuid) -> Vec<String> {
    let mut paths: Vec<String> = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("SymlinkSkipped"))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.payload["data"]["path"].as_str().unwrap().to_owned())
        .collect();
    paths.sort();
    paths
}

async fn live_series_folders(app: &TestApp, lib_id: Uuid) -> Vec<String> {
    let mut v: Vec<String> = series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib_id))
        .filter(series::Column::RemovedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|s| s.folder_path)
        .collect();
    v.sort();
    v
}

async fn live_issue_paths(app: &TestApp, lib_id: Uuid) -> Vec<String> {
    let mut v: Vec<String> = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.file_path)
        .collect();
    v.sort();
    v
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Links to folders at the top level (pointing out of the root and into
/// it), a link inside a publisher folder and a file link in a series
/// folder: none is indexed, each is a finding, the real folders beside them
/// scan normally.
#[tokio::test]
async fn scanner_skips_and_reports_every_link() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let outside = tmp.path().join("outside").join("Elsewhere (2020)");
    std::fs::create_dir_all(&outside).unwrap();
    write_cbz(&outside.join("Elsewhere 001.cbz"), "Elsewhere", 1);

    let real = root.join("Real (2019)");
    std::fs::create_dir_all(&real).unwrap();
    write_cbz(&real.join("Real 001.cbz"), "Real", 2);
    let daredevil = root.join("Marvel").join("Daredevil (2019)");
    std::fs::create_dir_all(&daredevil).unwrap();
    write_cbz(&daredevil.join("Daredevil 001.cbz"), "Daredevil", 3);

    let link_out = root.join("Linked Out");
    symlink(&outside, &link_out).unwrap();
    let link_in = root.join("Linked In");
    symlink(&real, &link_in).unwrap();
    let link_pub = root.join("Marvel").join("Linked Series");
    symlink(&outside, &link_pub).unwrap();
    let link_file = real.join("Real 002.cbz");
    symlink(outside.join("Elsewhere 001.cbz"), &link_file).unwrap();

    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    assert_eq!(
        live_series_folders(&app, lib_id).await,
        vec![s(&daredevil), s(&real)]
    );
    assert_eq!(
        live_issue_paths(&app, lib_id).await,
        vec![
            s(&daredevil.join("Daredevil 001.cbz")),
            s(&real.join("Real 001.cbz"))
        ]
    );
    let mut expected = vec![s(&link_in), s(&link_out), s(&link_pub), s(&link_file)];
    expected.sort();
    assert_eq!(open_symlink_findings(&app, lib_id).await, expected);
}

/// The root may itself be a link (a mount path): it scans normally and the
/// stored paths keep the configured spelling.
#[tokio::test]
async fn root_that_is_a_link_scans_normally() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let real_root = tmp.path().join("real-root");
    let series_dir = real_root.join("Saga (2012)");
    std::fs::create_dir_all(&series_dir).unwrap();
    write_cbz(&series_dir.join("Saga 001.cbz"), "Saga", 1);
    let alias = tmp.path().join("alias-root");
    symlink(&real_root, &alias).unwrap();

    let lib_id = LibrarySeed::new(&alias).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    assert_eq!(
        live_issue_paths(&app, lib_id).await,
        vec![s(&alias.join("Saga (2012)").join("Saga 001.cbz"))]
    );
    assert!(open_symlink_findings(&app, lib_id).await.is_empty());
}

/// A series that was indexed, then replaced by a link to the same content
/// elsewhere, is soft-removed with the finding recorded; putting the real
/// folder back restores it and resolves the finding.
#[tokio::test]
async fn series_replaced_by_a_link_is_soft_removed_then_restored() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let saga = root.join("Saga (2012)");
    std::fs::create_dir_all(&saga).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), "Saga", 1);
    // A second series keeps the root non-empty and the scan meaningful.
    let paper = root.join("Paper Girls (2015)");
    std::fs::create_dir_all(&paper).unwrap();
    write_cbz(&paper.join("Paper Girls 001.cbz"), "Paper Girls", 2);

    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(live_issue_paths(&app, lib_id).await.len(), 2);

    let moved = tmp.path().join("moved-away");
    std::fs::rename(&saga, &moved).unwrap();
    symlink(&moved, &saga).unwrap();
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    assert_eq!(live_series_folders(&app, lib_id).await, vec![s(&paper)]);
    let saga_row = series::Entity::find()
        .filter(series::Column::FolderPath.eq(s(&saga)))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(saga_row.removed_at.is_some(), "series soft-removed");
    let saga_issue = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(saga_row.id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(saga_issue.removed_at.is_some(), "issue soft-removed");
    assert_eq!(open_symlink_findings(&app, lib_id).await, vec![s(&saga)]);

    std::fs::remove_file(&saga).unwrap();
    std::fs::rename(&moved, &saga).unwrap();
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(
        live_series_folders(&app, lib_id).await,
        vec![s(&paper), s(&saga)]
    );
    assert_eq!(live_issue_paths(&app, lib_id).await.len(), 2);
    assert!(open_symlink_findings(&app, lib_id).await.is_empty());
}

// ───────────────────────── serving ─────────────────────────

struct User {
    session: String,
    csrf: String,
    id: Uuid,
}

async fn register(app: &TestApp, email: &str) -> User {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookie = |name: &str| {
        let prefix = format!("{name}=");
        resp.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| {
                c.split(';')
                    .next()
                    .and_then(|kv| kv.strip_prefix(prefix.as_str()))
                    .map(str::to_owned)
            })
            .expect("cookie")
    };
    let session = cookie("__Host-comic_session");
    let csrf = cookie("__Host-comic_csrf");
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    User { session, csrf, id }
}

async fn set_role_admin(app: &TestApp, user_id: Uuid) {
    let db = &app.state().db;
    let u = entity::user::Entity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = u.into();
    am.role = Set(entity::user::UserRole::Admin);
    am.update(db).await.unwrap();
}

async fn grant(app: &TestApp, lib_id: Uuid, user_id: Uuid) {
    app.state()
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO library_user_access (library_id, user_id) VALUES ($1, $2)",
            [lib_id.into(), user_id.into()],
        ))
        .await
        .unwrap();
}

/// Status of the OPDS download, the page-bytes route and the PSE page
/// route for `user`, in that order.
async fn statuses(app: &TestApp, user: &User, issue_id: &str) -> [StatusCode; 3] {
    let get = |uri: String, cookie: bool| {
        let mut b = Request::builder().method(Method::GET).uri(uri);
        if cookie {
            b = b.header(
                header::COOKIE,
                format!("__Host-comic_session={}", user.session),
            );
        }
        b.body(Body::empty()).unwrap()
    };
    let key = app.state().secrets.url_signing_key.clone();
    let query = server::auth::url_signing::issue_query(issue_id, user.id, key.as_ref());
    let mut out = [StatusCode::OK; 3];
    for (i, (uri, cookie)) in [
        (format!("/opds/v1/issues/{issue_id}/file"), true),
        (format!("/issues/{issue_id}/pages/0"), true),
        (format!("/opds/pse/{issue_id}/0?{query}"), false),
    ]
    .into_iter()
    .enumerate()
    {
        let resp = app.router.clone().oneshot(get(uri, cookie)).await.unwrap();
        out[i] = resp.status();
    }
    out
}

const OK3: [StatusCode; 3] = [StatusCode::OK; 3];
const GONE3: [StatusCode; 3] = [StatusCode::NOT_FOUND; 3];

struct ServedLibrary {
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
    series_dir: PathBuf,
    archive: PathBuf,
    issue_id: String,
    admin: User,
    reader: User,
}

async fn served_library(app: &TestApp) -> ServedLibrary {
    let tmp_dir = tempfile::tempdir().unwrap();
    let tmp = tmp_dir.path().to_path_buf();
    let root = tmp.join("library");
    let series_dir = root.join("Saga (2012)");
    std::fs::create_dir_all(&series_dir).unwrap();
    let archive = series_dir.join("Saga 001.cbz");
    write_cbz(&archive, "Saga", 1);
    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let issue_id = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .id;
    let admin = register(app, "admin@example.com").await;
    set_role_admin(app, admin.id).await;
    let reader = register(app, "reader@example.com").await;
    grant(app, lib_id, reader.id).await;
    ServedLibrary {
        _tmp: tmp_dir,
        tmp,
        series_dir,
        archive,
        issue_id,
        admin,
        reader,
    }
}

/// After a scan, an issue's archive replaced by a link to another readable
/// archive, and then its series folder replaced by a link to a copy of
/// itself: every serving route answers not-found for a reader and an admin,
/// and serves again once the real file is back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serving_never_follows_a_link() {
    let app = TestApp::spawn().await;
    let l = served_library(&app).await;
    for u in [&l.admin, &l.reader] {
        assert_eq!(statuses(&app, u, &l.issue_id).await, OK3);
    }

    // The archive becomes a link to another file the server can read.
    let other = l.tmp.join("other.cbz");
    write_cbz(&other, "Other", 9);
    let kept = l.tmp.join("kept.cbz");
    std::fs::rename(&l.archive, &kept).unwrap();
    symlink(&other, &l.archive).unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(statuses(&app, u, &l.issue_id).await, GONE3);
    }
    std::fs::remove_file(&l.archive).unwrap();
    std::fs::rename(&kept, &l.archive).unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(statuses(&app, u, &l.issue_id).await, OK3);
    }

    // The series folder becomes a link to a copy of itself.
    let copy = l.tmp.join("copy");
    std::fs::create_dir(&copy).unwrap();
    std::fs::copy(&l.archive, copy.join("Saga 001.cbz")).unwrap();
    let original = l.tmp.join("original");
    std::fs::rename(&l.series_dir, &original).unwrap();
    symlink(&copy, &l.series_dir).unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(statuses(&app, u, &l.issue_id).await, GONE3);
    }
    std::fs::remove_file(&l.series_dir).unwrap();
    std::fs::rename(&original, &l.series_dir).unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(statuses(&app, u, &l.issue_id).await, OK3);
    }
}

async fn download_status(app: &TestApp, user: &User, issue_id: &str) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/opds/v1/issues/{issue_id}/file"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", user.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A soft-removed issue, or an issue of a soft-removed series, is not
/// downloadable (the file is still on disk).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_removed_download_is_not_found() {
    let app = TestApp::spawn().await;
    let l = served_library(&app).await;
    let db = &app.state().db;
    for u in [&l.admin, &l.reader] {
        assert_eq!(download_status(&app, u, &l.issue_id).await, StatusCode::OK);
    }

    let now = chrono::Utc::now().fixed_offset();
    let row = issue::Entity::find_by_id(l.issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let series_id = row.series_id;
    let mut am: issue::ActiveModel = row.into();
    am.removed_at = Set(Some(now));
    let row = am.update(db).await.unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(
            download_status(&app, u, &l.issue_id).await,
            StatusCode::NOT_FOUND
        );
    }

    let mut am: issue::ActiveModel = row.into();
    am.removed_at = Set(None);
    am.update(db).await.unwrap();
    let s_row = series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut sam: series::ActiveModel = s_row.into();
    sam.removed_at = Set(Some(now));
    sam.update(db).await.unwrap();
    for u in [&l.admin, &l.reader] {
        assert_eq!(
            download_status(&app, u, &l.issue_id).await,
            StatusCode::NOT_FOUND
        );
    }
}

async fn get_status(app: &TestApp, user: &User, uri: String) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", user.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// The inline thumbnail, the OCR route and the `?w=` page variant refuse
/// an archive replaced by a link (each checks the path before the archive
/// reader opens it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumbnail_ocr_and_variant_refuse_a_link() {
    let app = TestApp::spawn().await;
    let l = served_library(&app).await;
    let other = l.tmp.join("other.cbz");
    write_cbz(&other, "Other", 9);
    std::fs::rename(&l.archive, l.tmp.join("kept.cbz")).unwrap();
    symlink(&other, &l.archive).unwrap();

    let id = &l.issue_id;
    assert_eq!(
        get_status(
            &app,
            &l.admin,
            format!("/issues/{id}/pages/0/thumb?variant=strip")
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_status(&app, &l.admin, format!("/issues/{id}/pages/0?w=100")).await,
        StatusCode::NOT_FOUND
    );
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/me/issues/{id}/ocr"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        l.admin.session, l.admin.csrf
                    ),
                )
                .header("X-CSRF-Token", &l.admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "page": 0,
                        "region": { "x": 0, "y": 0, "w": 1, "h": 1 },
                        "lang": "western"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A library whose root is a link serves an issue row stored under the
/// root's canonical spelling (written by older single-series rescans).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_root_serves_rows_stored_under_the_canonical_spelling() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let real_root = tmp.path().join("real-root");
    let series_dir = real_root.join("Saga (2012)");
    std::fs::create_dir_all(&series_dir).unwrap();
    write_cbz(&series_dir.join("Saga 001.cbz"), "Saga", 1);
    let alias = tmp.path().join("alias-root");
    symlink(&real_root, &alias).unwrap();
    let lib_id = LibrarySeed::new(&alias).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    let db = &app.state().db;
    let row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let canonical = std::fs::canonicalize(&real_root)
        .unwrap()
        .join("Saga (2012)")
        .join("Saga 001.cbz");
    assert_ne!(s(&canonical), row.file_path);
    let id = row.id.clone();
    let mut am: issue::ActiveModel = row.into();
    am.file_path = Set(s(&canonical));
    am.update(db).await.unwrap();

    let admin = register(&app, "admin@example.com").await;
    set_role_admin(&app, admin.id).await;
    assert_eq!(
        get_status(&app, &admin, format!("/issues/{id}/pages/0")).await,
        StatusCode::OK
    );
    assert_eq!(
        get_status(&app, &admin, format!("/opds/v1/issues/{id}/file")).await,
        StatusCode::OK
    );
}

/// "Scan issue" on an archive replaced by a link does not ingest the link's
/// target and leaves the row as it was. It opens no finding of its own: the
/// next library scan reports the link and reconciles the row.
#[tokio::test]
async fn issue_rescan_does_not_follow_a_link() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let saga = root.join("Saga (2012)");
    std::fs::create_dir_all(&saga).unwrap();
    let archive = saga.join("Saga 001.cbz");
    write_cbz(&archive, "Saga", 1);
    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let db = &app.state().db;
    let before = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();

    let other = tmp.path().join("other.cbz");
    write_cbz(&other, "Other", 9);
    std::fs::rename(&archive, tmp.path().join("kept.cbz")).unwrap();
    symlink(&other, &archive).unwrap();
    scanner::scan_issue_file(&app.state(), lib_id, &before.id, true, None)
        .await
        .unwrap();

    let after = issue::Entity::find_by_id(before.id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.content_hash, before.content_hash,
        "target not ingested"
    );
    assert_eq!(after.file_size, before.file_size);
    assert!(after.removed_at.is_none(), "row left for the full scan");
    assert!(
        open_symlink_findings(&app, lib_id).await.is_empty(),
        "the issue rescan must not pre-report the link"
    );

    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let reconciled = issue::Entity::find_by_id(before.id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(reconciled.removed_at.is_some(), "full scan removes the row");
    assert_eq!(open_symlink_findings(&app, lib_id).await, vec![s(&archive)]);
}

/// Set a path's own mtime (not a link target's) to 2000-01-01.
fn backdate(path: &Path) {
    use rustix::fs::{AtFlags, Timespec, Timestamps, utimensat};
    let old = Timespec {
        tv_sec: 946_684_800,
        tv_nsec: 0,
    };
    utimensat(
        rustix::fs::CWD,
        path,
        &Timestamps {
            last_access: old,
            last_modification: old,
        },
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
}

/// An unchanged series folder (mtime not past the last scan) in which an
/// archive became a link is processed on the first scan after the link
/// appears (row soft-removed, finding opened) and skipped again on the next.
#[tokio::test]
async fn unchanged_folder_with_a_new_link_is_processed_once() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let saga = root.join("Saga (2012)");
    std::fs::create_dir_all(&saga).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), "Saga", 1);
    let second = saga.join("Saga 002.cbz");
    write_cbz(&second, "Saga", 2);
    for p in [&saga, &saga.join("Saga 001.cbz"), &second] {
        backdate(p);
    }
    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(live_issue_paths(&app, lib_id).await.len(), 2);

    let elsewhere = tmp.path().join("elsewhere.cbz");
    std::fs::rename(&second, &elsewhere).unwrap();
    symlink(&elsewhere, &second).unwrap();
    backdate(&second);
    backdate(&saga);

    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.series_skipped_unchanged, 0, "new link: processed");
    assert_eq!(
        live_issue_paths(&app, lib_id).await,
        vec![s(&saga.join("Saga 001.cbz"))]
    );
    assert_eq!(open_symlink_findings(&app, lib_id).await, vec![s(&second)]);

    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.series_skipped_unchanged, 1, "reported link: skipped");
    assert_eq!(open_symlink_findings(&app, lib_id).await, vec![s(&second)]);
}

/// A single-series rescan of a folder that was replaced by a link refuses
/// the folder and soft-removes nothing.
#[tokio::test]
async fn series_rescan_refuses_a_linked_folder() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let saga = root.join("Saga (2012)");
    std::fs::create_dir_all(&saga).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), "Saga", 1);
    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let series_row = series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();

    // Points inside the root, at a copy: still a link, still refused.
    let copy = root.join(".copy");
    std::fs::create_dir(&copy).unwrap();
    std::fs::copy(saga.join("Saga 001.cbz"), copy.join("Saga 001.cbz")).unwrap();
    std::fs::rename(&saga, tmp.path().join("original")).unwrap();
    symlink(&copy, &saga).unwrap();

    let res = scanner::scan_series_folder(
        &app.state(),
        lib_id,
        series_row.id,
        &saga,
        scanner::ScanKind::Series,
        None,
        true,
        None,
    )
    .await;
    assert!(res.is_err(), "linked folder refused");
    assert_eq!(live_series_folders(&app, lib_id).await, vec![s(&saga)]);
    assert_eq!(live_issue_paths(&app, lib_id).await.len(), 1);
}
