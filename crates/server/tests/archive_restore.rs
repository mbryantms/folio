//! ARC-2: restore from `.bak` keeps the archive it replaces, and the
//! backups listing finds a converted archive's pre-conversion backups and
//! reports when each backup was made.
//!
//! Every file lives in a temp dir.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, Set};
use server::archive_rewrite::{self, RewriteError, RewriteStep};
use server::library::no_follow::LibraryRoot;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::tempdir;
use tower::ServiceExt;

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
                    r#"{"email":"restore@example.com","password":"correctly-horse-battery"}"#,
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

async fn call(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: String,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

/// A writeback library keeping `slots` backups, with one issue at
/// `file_name` holding `payload`. Restore never parses the archive, so the
/// bytes need not be a real archive.
async fn seed(
    app: &TestApp,
    dir: &Path,
    slots: i32,
    file_name: &str,
    payload: &[u8],
) -> (String, PathBuf) {
    let db = &app.state().db;
    let lib = LibrarySeed::new(dir)
        .with_sidecar_writeback()
        .insert(db)
        .await;
    entity::library::ActiveModel {
        id: Set(lib),
        archive_backup_retain_count: Set(slots),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
    let series = SeriesSeed::new(lib, "Restore Series").insert(db).await;
    let path = dir.join(file_name);
    let id = IssueSeed::new(lib, series, &path, payload, 1.0)
        .insert(db)
        .await;
    (id, path)
}

fn slot(path: &Path, n: i32) -> PathBuf {
    archive_rewrite::backup_slot(path, n)
}

// ───────── (2) restore keeps the replaced archive ─────────

/// Restore swaps the archive with its `.bak`: the backup comes back and
/// the file it replaces becomes the newest backup, older slots unchanged,
/// whatever the slot count (0, 1, 3). Restoring again undoes it. Pre-fix
/// the backup was renamed over the archive and the replaced file was gone.
#[tokio::test]
async fn restore_keeps_the_replaced_archive_as_the_newest_backup() {
    for slots in [0, 1, 3] {
        let app = TestApp::spawn().await;
        let auth = register_admin(&app).await;
        let dir = tempdir().unwrap();
        let (id, path) = seed(&app, dir.path(), slots, "issue.cbz", b"current").await;
        std::fs::write(slot(&path, 0), b"backup-0").unwrap();
        if slots == 3 {
            std::fs::write(slot(&path, 1), b"backup-1").unwrap();
            std::fs::write(slot(&path, 2), b"backup-2").unwrap();
        }

        let (status, body) = call(
            &app,
            &auth,
            Method::POST,
            format!("/api/issues/{id}/archive/restore"),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{slots} slots: {body}");
        assert_eq!(std::fs::read(&path).unwrap(), b"backup-0", "{slots} slots");
        assert_eq!(
            std::fs::read(slot(&path, 0)).unwrap(),
            b"current",
            "the replaced archive is the newest backup ({slots} slots)"
        );
        assert_eq!(
            body["kept_as"].as_str().map(PathBuf::from),
            Some(slot(&path, 0)),
            "{body}"
        );
        if slots == 3 {
            assert_eq!(std::fs::read(slot(&path, 1)).unwrap(), b"backup-1");
            assert_eq!(std::fs::read(slot(&path, 2)).unwrap(), b"backup-2");
        }
        assert!(!slot(&path, 3).exists(), "no slot is added ({slots} slots)");
        let others: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(others.is_empty(), "no leftover: {others:?}");

        // A wrong click is undone by restoring again.
        let (status, _) = call(
            &app,
            &auth,
            Method::POST,
            format!("/api/issues/{id}/archive/restore"),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(std::fs::read(&path).unwrap(), b"current");
        assert_eq!(std::fs::read(slot(&path, 0)).unwrap(), b"backup-0");
    }
}

/// The swap where `RENAME_EXCHANGE` is not available takes three steps.
/// Control: it ends like the exchange.
#[test]
fn three_step_restore_ends_like_the_exchange() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("issue.cbz");
    std::fs::write(&path, b"current").unwrap();
    std::fs::write(slot(&path, 0), b"backup-0").unwrap();
    let root = LibraryRoot::resolve(dir.path());
    let out = archive_rewrite::restore_latest_backup_with_faults(&root, &path, false, |_| Ok(()))
        .unwrap();
    assert_eq!(out.kept, Some(slot(&path, 0)));
    assert_eq!(std::fs::read(&path).unwrap(), b"backup-0");
    assert_eq!(std::fs::read(slot(&path, 0)).unwrap(), b"current");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

/// A failure half-way never leaves the archive missing: before the backup
/// is renamed over it, nothing changed (the second name is removed again);
/// after, the restored file is at the name and the replaced one is kept
/// under the second name the error names.
#[test]
fn a_restore_that_fails_half_way_leaves_the_archive_in_place() {
    let crash = |at: RewriteStep| {
        move |step: RewriteStep| {
            if step == at {
                Err(RewriteError::Io(std::io::Error::other("injected")))
            } else {
                Ok(())
            }
        }
    };

    // Fails once the replaced archive has its second name.
    let dir = tempdir().unwrap();
    let path = dir.path().join("issue.cbz");
    std::fs::write(&path, b"current").unwrap();
    std::fs::write(slot(&path, 0), b"backup-0").unwrap();
    let root = LibraryRoot::resolve(dir.path());
    archive_rewrite::restore_latest_backup_with_faults(
        &root,
        &path,
        false,
        crash(RewriteStep::BackedUp),
    )
    .unwrap_err();
    assert_eq!(std::fs::read(&path).unwrap(), b"current", "nothing changed");
    assert_eq!(std::fs::read(slot(&path, 0)).unwrap(), b"backup-0");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        2,
        "the second name was removed again"
    );

    // Fails after the backup took the archive's name.
    let dir = tempdir().unwrap();
    let path = dir.path().join("issue.cbz");
    std::fs::write(&path, b"current").unwrap();
    std::fs::write(slot(&path, 0), b"backup-0").unwrap();
    let root = LibraryRoot::resolve(dir.path());
    // Past the middle step the restore has happened: an outcome, not an
    // error, naming where the replaced archive is kept.
    let out = archive_rewrite::restore_latest_backup_with_faults(
        &root,
        &path,
        false,
        crash(RewriteStep::Swapped),
    )
    .expect("the restore happened");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"backup-0",
        "the archive is there"
    );
    let held: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p != &path)
        .collect();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(std::fs::read(&held[0]).unwrap(), b"current", "kept");
    let name = held[0].file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(out.kept.as_deref(), Some(held[0].as_path()));
    let warning = out.warning.expect("a warning");
    assert!(warning.contains(&name), "the warning names it: {warning}");
    assert!(
        !archive_rewrite::is_staging_name(&name),
        "startup cleanup leaves it"
    );
}

/// The same step-3 failure through the endpoint's restore path: the
/// answer is a success that names the hold file and carries the warning,
/// the audit row is written, and both files are where the answer says.
#[tokio::test]
async fn a_restore_whose_last_step_fails_is_reported_as_done_with_a_warning() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let app = TestApp::spawn().await;
    let _auth = register_admin(&app).await;
    let admin = entity::user::Entity::find()
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let dir = tempdir().unwrap();
    let (id, path) = seed(&app, dir.path(), 1, "issue.cbz", b"current").await;
    std::fs::write(slot(&path, 0), b"backup-0").unwrap();
    let row = entity::issue::Entity::find_by_id(id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();

    let resp = server::api::archive_edit::restore_issue(
        &app.state(),
        row,
        admin.id,
        None,
        None,
        None,
        |root, target| {
            archive_rewrite::restore_latest_backup_with_faults(root, target, false, |step| {
                if step == RewriteStep::Swapped {
                    Err(RewriteError::Io(std::io::Error::other(
                        "injected rename failure",
                    )))
                } else {
                    Ok(())
                }
            })
        },
    )
    .await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["status"], "restored", "{body}");
    let kept = PathBuf::from(body["kept_as"].as_str().expect("kept_as"));
    assert_ne!(kept, slot(&path, 0), "{body}");
    let warning = body["warning"].as_str().expect("a warning");
    let kept_name = kept.file_name().unwrap().to_string_lossy().into_owned();
    assert!(warning.contains(&kept_name), "{warning}");
    assert!(warning.contains("injected rename failure"), "{warning}");
    assert_eq!(std::fs::read(&path).unwrap(), b"backup-0");
    assert_eq!(std::fs::read(&kept).unwrap(), b"current");

    let audit = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.issue.archive_restore"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].payload["kept_as"], body["kept_as"]);
    assert_eq!(audit[0].payload["warning"], body["warning"]);
}

// ───────── (3) pre-conversion backups ─────────

/// A CBR converted to CBZ keeps its original as `X.cbr.bak`: the listing
/// for the `.cbz` issue finds it (marked `pre_conversion`) beside the
/// `.cbz`'s own backups. Unrelated `.bak` files are not listed. Restore
/// still takes only the issue's own `.bak`.
#[tokio::test]
async fn listing_finds_the_backups_of_the_archive_it_was_converted_from() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (id, path) = seed(&app, dir.path(), 1, "Thanos 001.cbz", b"converted").await;
    std::fs::write(dir.path().join("Thanos 001.cbr.bak"), b"rar original").unwrap();
    std::fs::write(dir.path().join("Thanos 001.CB7.bak.1"), b"7z original").unwrap();
    // Not this issue's: another stem, a non-backup name, a CBZ's backup of
    // another file.
    std::fs::write(dir.path().join("Thanos 002.cbr.bak"), b"other").unwrap();
    std::fs::write(dir.path().join("Thanos 001.cbr.bak.txt"), b"notes").unwrap();
    std::fs::write(dir.path().join("Thanos 001.cbt.bak"), b"not a conversion").unwrap();

    let (status, body) = call(
        &app,
        &auth,
        Method::GET,
        format!("/api/issues/{id}/archive/backups"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    let listed: Vec<(String, i64, bool)> = rows
        .iter()
        .map(|r| {
            (
                Path::new(r["path"].as_str().unwrap())
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                r["slot"].as_i64().unwrap(),
                r["pre_conversion"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("Thanos 001.cbr.bak".to_owned(), 0, true),
            ("Thanos 001.CB7.bak.1".to_owned(), 1, true),
        ]
    );

    // Its own backup is listed first, as before.
    std::fs::write(slot(&path, 0), b"own backup").unwrap();
    let (_, body) = call(
        &app,
        &auth,
        Method::GET,
        format!("/api/issues/{id}/archive/backups"),
    )
    .await;
    let first = &body.as_array().unwrap()[0];
    assert_eq!(first["pre_conversion"], false);
    assert_eq!(first["slot"], 0);
}

// ───────── (6) when a backup was made ─────────

/// `made_at` is when the backup was made, not the original's old
/// modification time: for a hard-linked backup (made by a real rewrite of
/// an archive last modified 400 days ago) and for a copy whose mtime was
/// carried over from the original.
#[tokio::test]
async fn listing_reports_when_each_backup_was_made() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (id, path) = seed(&app, dir.path(), 3, "issue.cbz", b"v1").await;
    let old = SystemTime::now() - Duration::from_secs(400 * 86_400);
    filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(old)).unwrap();

    let before = SystemTime::now() - Duration::from_secs(2);
    // Hard link: the rewrite links the original into `.bak`.
    let root = LibraryRoot::resolve(dir.path());
    let out = archive_rewrite::rewrite_atomic(&root, &path, 3, |io| {
        use std::io::Write;
        io.staging_file()?.write_all(b"v2")?;
        Ok(())
    })
    .unwrap();
    let linked = out.backup.unwrap();
    // A copy that kept the original's modification time.
    let copy = slot(&path, 2);
    std::fs::write(&copy, b"v0").unwrap();
    filetime::set_file_mtime(&copy, filetime::FileTime::from_system_time(old)).unwrap();

    let (status, body) = call(
        &app,
        &auth,
        Method::GET,
        format!("/api/issues/{id}/archive/backups"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2, "{body}");
    let parse = |v: &serde_json::Value| -> SystemTime {
        chrono::DateTime::parse_from_rfc3339(v.as_str().expect("a date"))
            .unwrap()
            .into()
    };
    for (row, file) in rows.iter().zip([&linked, &copy]) {
        assert_eq!(Path::new(row["path"].as_str().unwrap()), file.as_path());
        assert!(
            parse(&row["made_at"]) >= before,
            "made now: {row} ({})",
            file.display()
        );
        assert!(
            parse(&row["modified_at"]) < before - Duration::from_secs(86_400),
            "modified_at keeps its old meaning: {row}"
        );
    }
}

// ───────── (3) restoring a pre-conversion backup: rebuilt as a CBZ ─────────

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    assert!(p.is_file(), "fixtures/{name} is committed");
    p
}

/// The pages of the archive at `path`, in reading order, as bytes.
fn page_bytes(path: &Path) -> Vec<Vec<u8>> {
    let mut a = archive::open_path_unchecked(path, archive::ArchiveLimits::default()).unwrap();
    let names: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
    names
        .iter()
        .map(|n| a.read_entry_bytes(n).unwrap())
        .collect()
}

/// Every file in `dir` with its bytes, sorted by name.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::symlink_metadata(e.path())
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|_| std::fs::read(e.path()).unwrap())
                    .unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

async fn restore_named(
    app: &TestApp,
    auth: &Authed,
    id: &str,
    backup: &str,
) -> (StatusCode, serde_json::Value) {
    let q: String = url_encode(backup);
    call(
        app,
        auth,
        Method::POST,
        format!("/api/issues/{id}/archive/restore?backup={q}"),
    )
    .await
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A marker on page 2 and reading progress for the issue.
async fn anchor(app: &TestApp, issue_id: &str) -> uuid::Uuid {
    use sea_orm::EntityTrait;
    let db = &app.state().db;
    let user = entity::user::Entity::find().one(db).await.unwrap().unwrap();
    let row = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now().fixed_offset();
    let marker_id = uuid::Uuid::now_v7();
    entity::marker::ActiveModel {
        id: Set(marker_id),
        user_id: Set(user.id),
        series_id: Set(row.series_id),
        issue_id: Set(issue_id.to_owned()),
        page_index: Set(2),
        kind: Set(entity::marker::MarkerKind::Bookmark),
        is_favorite: Set(false),
        tags: Set(vec![]),
        region: Set(None),
        selection: Set(None),
        body: Set(None),
        color: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    common::seed::seed_progress(db, user.id, issue_id, 1, 0.5, false).await;
    marker_id
}

/// Restoring `X.cbr.bak` / `X.cb7.bak` for the issue at `X.cbz` rebuilds the
/// original's pages as a CBZ at `X.cbz`; the replaced `X.cbz` becomes
/// `X.cbz.bak`; the pre-conversion backup is untouched; the row keeps its
/// `file_path`, progress and markers. The library has auto-convert off for
/// both formats (the seed's default), with one slot and with zero.
#[tokio::test]
async fn restoring_a_pre_conversion_backup_rebuilds_it_as_a_cbz() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    for (fixture_name, ext) in [
        ("synthetic-3page.cbr", "cbr"),
        ("synthetic-3page.cb7", "cb7"),
    ] {
        for slots in [0, 1] {
            let app = TestApp::spawn().await;
            let auth = register_admin(&app).await;
            let dir = tempdir().unwrap();
            // The current CBZ: whatever a later edit made of it.
            let current_cbz = {
                use std::io::Write;
                let mut buf = std::io::Cursor::new(Vec::new());
                let mut zw = zip::ZipWriter::new(&mut buf);
                let opts = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                zw.start_file("p1.png", opts).unwrap();
                zw.write_all(b"\x89PNG\r\n\x1a\nedited").unwrap();
                zw.finish().unwrap();
                buf.into_inner()
            };
            let (id, path) = seed(&app, dir.path(), slots, "Thanos001.cbz", &current_cbz).await;
            let lib = entity::library::Entity::find()
                .one(&app.state().db)
                .await
                .unwrap()
                .unwrap();
            assert!(!lib.auto_convert_cbr_on_scan && !lib.auto_convert_cb7_on_scan);
            let pre = dir.path().join(format!("Thanos001.{ext}.bak"));
            let original = std::fs::read(fixture(fixture_name)).unwrap();
            std::fs::write(&pre, &original).unwrap();
            let marker_id = anchor(&app, &id).await;

            let (status, body) =
                restore_named(&app, &auth, &id, &format!("Thanos001.{ext}.bak")).await;
            assert_eq!(
                status,
                StatusCode::ACCEPTED,
                "{fixture_name} {slots}: {body}"
            );
            assert_eq!(body["rebuilt_from"], format!("Thanos001.{ext}.bak"));
            let message = body["message"].as_str().unwrap();
            assert!(message.contains("rebuilt as a CBZ"), "{message}");
            assert!(
                message.contains("original bytes stay available"),
                "{message}"
            );
            assert_eq!(
                body["kept_as"].as_str().map(PathBuf::from),
                Some(slot(&path, 0))
            );
            // X.cbz holds the original's pages, in order.
            let pages = page_bytes(&path);
            assert_eq!(pages.len(), 3);
            assert_eq!(pages, page_bytes(&fixture(fixture_name)), "{fixture_name}");
            let mut z = std::fs::File::open(&path).unwrap();
            let mut magic = [0u8; 2];
            std::io::Read::read_exact(&mut z, &mut magic).unwrap();
            assert_eq!(&magic, b"PK", "a ZIP");
            assert_eq!(std::fs::read(slot(&path, 0)).unwrap(), current_cbz);
            assert_eq!(std::fs::read(&pre).unwrap(), original, "backup untouched");

            let row = entity::issue::Entity::find_by_id(id.clone())
                .one(&app.state().db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                Path::new(&row.file_path),
                path.as_path(),
                "file_path unchanged"
            );
            let m = entity::marker::Entity::find_by_id(marker_id)
                .one(&app.state().db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(m.issue_id, id);
            let progress = entity::progress_record::Entity::find()
                .filter(entity::progress_record::Column::IssueId.eq(id.clone()))
                .all(&app.state().db)
                .await
                .unwrap();
            assert_eq!(progress.len(), 1);
            let audit = entity::audit_log::Entity::find()
                .filter(entity::audit_log::Column::Action.eq("admin.issue.archive_restore"))
                .all(&app.state().db)
                .await
                .unwrap();
            assert_eq!(audit.len(), 1);
            assert_eq!(audit[0].payload["rebuilt_from"], body["rebuilt_from"]);
            assert_eq!(audit[0].payload["message"], body["message"]);
        }
    }
}

/// A corrupt pre-conversion backup is a refusal with the reader's reason,
/// and nothing in the folder changes.
#[tokio::test]
async fn a_corrupt_pre_conversion_backup_is_refused_and_nothing_changes() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (id, _path) = seed(&app, dir.path(), 1, "Thanos001.cbz", b"current").await;
    std::fs::write(
        dir.path().join("Thanos001.cbr.bak"),
        b"Rar!\x1a\x07\x01\x00this is not a rar archive at all",
    )
    .unwrap();
    std::fs::write(dir.path().join("Thanos001.cbz.bak"), b"own backup").unwrap();
    let before = snapshot(dir.path());

    let (status, body) = restore_named(&app, &auth, &id, "Thanos001.cbr.bak").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["code"], "archive.restore_conversion_failed",
        "{body}"
    );
    assert_eq!(snapshot(dir.path()), before, "nothing changed on disk");
}

/// A request can only name a backup the listing offers for this issue: not
/// another issue's backup, not a path, not a link, not an older own slot.
#[tokio::test]
async fn restore_refuses_a_backup_the_listing_does_not_offer() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (id, _path) = seed(&app, dir.path(), 3, "Thanos001.cbz", b"current").await;
    let rar = std::fs::read(fixture("synthetic-3page.cbr")).unwrap();
    std::fs::write(dir.path().join("Other001.cbr.bak"), &rar).unwrap();
    std::fs::write(dir.path().join("Thanos001.cbz.bak.1"), b"older").unwrap();
    let elsewhere = tempdir().unwrap();
    std::fs::write(elsewhere.path().join("Thanos001.cbr.bak"), &rar).unwrap();
    std::os::unix::fs::symlink(
        elsewhere.path().join("Thanos001.cbr.bak"),
        dir.path().join("Thanos001.cbr.bak"),
    )
    .unwrap();
    let before = snapshot(dir.path());
    for named in [
        "Other001.cbr.bak".to_owned(),
        format!(
            "../{}/Thanos001.cbr.bak",
            elsewhere.path().file_name().unwrap().to_string_lossy()
        ),
        elsewhere
            .path()
            .join("Thanos001.cbr.bak")
            .to_string_lossy()
            .into_owned(),
        "Thanos001.cbr.bak".to_owned(),
        "Thanos001.cbz.bak.1".to_owned(),
    ] {
        let (status, body) = restore_named(&app, &auth, &id, &named).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{named}: {body}");
        assert_eq!(
            body["error"]["code"], "archive.backup_not_restorable",
            "{named}: {body}"
        );
    }
    assert_eq!(snapshot(dir.path()), before);
}

/// The rebuild-restore takes the same three steps where `RENAME_EXCHANGE`
/// is unavailable, and moves an existing `.bak` up a slot instead of
/// overwriting it.
#[test]
fn rebuild_restore_without_exchange_ends_like_the_exchange() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("issue.cbz");
    std::fs::write(&path, b"current").unwrap();
    std::fs::write(slot(&path, 0), b"older").unwrap();
    let rar = std::fs::read(fixture("synthetic-3page.cbr")).unwrap();
    std::fs::write(dir.path().join("issue.cbr.bak"), &rar).unwrap();
    let root = LibraryRoot::resolve(dir.path());
    let out = archive_rewrite::restore_rebuilt(
        &root,
        &path,
        std::ffi::OsStr::new("issue.cbr.bak"),
        false,
        |io| {
            server::library::scanner::cbr_convert::rebuild_cbz_into(
                io,
                archive::ArchiveLimits::default(),
            )
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(out.kept, Some(slot(&path, 0)));
    assert!(out.warning.is_none());
    assert_eq!(
        page_bytes(&path),
        page_bytes(&fixture("synthetic-3page.cbr"))
    );
    assert_eq!(std::fs::read(slot(&path, 0)).unwrap(), b"current");
    assert_eq!(std::fs::read(slot(&path, 1)).unwrap(), b"older");
    assert_eq!(
        std::fs::read(dir.path().join("issue.cbr.bak")).unwrap(),
        rar
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
}

/// A failure before the swap leaves the folder exactly as it was: the
/// staged CBZ is removed, the archive and every backup untouched.
#[test]
fn a_rebuild_restore_that_fails_before_the_swap_changes_nothing() {
    for (step, exchange) in [
        (RewriteStep::Staged, true),
        (RewriteStep::Staged, false),
        (RewriteStep::BackedUp, false),
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("issue.cbz");
        std::fs::write(&path, b"current").unwrap();
        std::fs::write(
            dir.path().join("issue.cb7.bak"),
            std::fs::read(fixture("synthetic-3page.cb7")).unwrap(),
        )
        .unwrap();
        let before = snapshot(dir.path());
        let root = LibraryRoot::resolve(dir.path());
        let err = archive_rewrite::restore_rebuilt(
            &root,
            &path,
            std::ffi::OsStr::new("issue.cb7.bak"),
            exchange,
            |io| {
                server::library::scanner::cbr_convert::rebuild_cbz_into(
                    io,
                    archive::ArchiveLimits::default(),
                )
            },
            |s| {
                if s == step {
                    Err(RewriteError::Io(std::io::Error::other("injected")))
                } else {
                    Ok(())
                }
            },
        );
        assert!(err.is_err(), "{step:?}");
        assert_eq!(snapshot(dir.path()), before, "{step:?} exchange={exchange}");
    }
}
