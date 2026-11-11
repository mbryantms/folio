//! ARC-2: the backups listing finds a converted archive's pre-conversion backups and
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
use server::archive_rewrite;
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
