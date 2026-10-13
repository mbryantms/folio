//! Local metadata databases, M1 (`metadata::local_db` + the admin card
//! surface): opening an operator-supplied SQLite file read-only, the
//! structural probe behind *Test*, the four settings, and the audited
//! `POST /admin/metadata/providers/{id}/local/test` endpoint.
//!
//! Fixture files are built here with the same SQLite driver the
//! server reads them with, in the shapes the real dumps use (the
//! `cv_*` tables of the community ComicVine file; the `gcd_*` core
//! tables of the GCD dump).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ConnectionTrait, Database, Statement};
use server::metadata::local_db::{self, LocalDbError, LocalDbKind, LocalDbMode};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

async fn exec_sqlite(path: &Path, statements: &[&str]) {
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    for sql in statements {
        db.execute_raw(Statement::from_string(db.get_database_backend(), *sql))
            .await
            .unwrap();
    }
    db.close().await.unwrap();
}

/// A small "Local ComicVine" file: three volumes, five issues, a sync row.
async fn comicvine_fixture(dir: &Path) -> PathBuf {
    let p = dir.join("localcv.db");
    exec_sqlite(
        &p,
        &[
            "CREATE TABLE cv_publisher (id INTEGER PRIMARY KEY, name TEXT, image_url TEXT, site_detail_url TEXT)",
            "CREATE TABLE cv_person (id INTEGER PRIMARY KEY, name TEXT)",
            "CREATE TABLE cv_volume (id INTEGER PRIMARY KEY, name TEXT NOT NULL, aliases TEXT, start_year TEXT, publisher_id INTEGER, count_of_issues INTEGER, description TEXT, image_url TEXT, site_detail_url TEXT)",
            "CREATE TABLE cv_issue (id INTEGER PRIMARY KEY, volume_id INTEGER NOT NULL, name TEXT, issue_number TEXT, cover_date TEXT, store_date TEXT, description TEXT, image_url TEXT, site_detail_url TEXT, character_credits TEXT, person_credits TEXT, team_credits TEXT, location_credits TEXT, story_arc_credits TEXT, associated_images TEXT)",
            "CREATE TABLE cv_sync_metadata (endpoint TEXT PRIMARY KEY, last_sync_date TEXT NOT NULL, last_sync_timestamp TEXT NOT NULL, resume_state TEXT)",
            "INSERT INTO cv_publisher VALUES (10, 'DC Comics', NULL, NULL)",
            "INSERT INTO cv_volume VALUES (796, 'Batman', NULL, '1940', 10, 716, NULL, NULL, NULL), (42721, 'Batman', NULL, '2011', 10, 57, NULL, NULL, NULL), (1, 'Saga', NULL, '2012', 10, 72, NULL, NULL, NULL)",
            "INSERT INTO cv_issue (id, volume_id, name, issue_number, cover_date) VALUES (120789, 796, NULL, '27', '1945-02-28'), (2, 796, NULL, '28', '1945-04-30'), (3, 42721, NULL, '1', '2011-11-30'), (4, 1, NULL, '1', '2012-03-31'), (5, 1, NULL, '2', '2012-04-30')",
            "INSERT INTO cv_sync_metadata VALUES ('issues', '2026-07-16', '2026-07-16T22:04:24', NULL), ('volumes', '2026-07-15', '2026-07-15T22:03:20', NULL)",
        ],
    )
    .await;
    p
}

/// A small GCD file: the four core tables, a recent `modified` stamp.
async fn gcd_fixture(dir: &Path, modified: &str) -> PathBuf {
    let p = dir.join("gcd.db");
    exec_sqlite(
        &p,
        &[
            "CREATE TABLE gcd_publisher (id INTEGER PRIMARY KEY, name TEXT, country_id INTEGER, year_began INTEGER)",
            "CREATE TABLE gcd_series (id INTEGER PRIMARY KEY, name TEXT, sort_name TEXT, year_began INTEGER, year_ended INTEGER, publisher_id INTEGER, language_id INTEGER, issue_count INTEGER)",
            "CREATE TABLE gcd_issue (id INTEGER PRIMARY KEY, number TEXT, series_id INTEGER, key_date TEXT, on_sale_date TEXT, modified TEXT, variant_of_id INTEGER, deleted INTEGER)",
            "CREATE TABLE gcd_story (id INTEGER PRIMARY KEY, issue_id INTEGER, title TEXT, sequence_number INTEGER, type_id INTEGER, characters TEXT, genre TEXT, synopsis TEXT)",
            "INSERT INTO gcd_publisher VALUES (54, 'DC', 225, 1935)",
            "INSERT INTO gcd_series VALUES (2102, 'Detective Comics', 'Detective Comics', 1937, 2011, 54, 25, 881)",
            &format!("INSERT INTO gcd_issue VALUES (29202, '27', 2102, '1939-05-00', NULL, '{modified}', NULL, 0), (29203, '28', 2102, '1939-06-00', NULL, '2024-12-23 14:02:47', NULL, 0)"),
            "INSERT INTO gcd_story VALUES (1, 29202, 'The Case of the Chemical Syndicate', 1, 19, 'Batman [Bruce Wayne]; Commissioner Gordon', 'superhero', NULL)",
        ],
    )
    .await;
    p
}

#[tokio::test]
async fn probe_reads_counts_dump_date_and_optional_tables() {
    let dir = tempfile::tempdir().unwrap();
    let cv = comicvine_fixture(dir.path()).await;
    let p = local_db::probe(LocalDbKind::ComicVine, &cv).await.unwrap();
    assert_eq!(p.kind, LocalDbKind::ComicVine);
    assert_eq!(p.counts["volumes"], 3);
    assert_eq!(p.counts["issues"], 5);
    assert_eq!(p.counts["publishers"], 1);
    assert_eq!(p.counts["people"], 0);
    assert!(
        !p.counts.contains_key("covers"),
        "comic_covers absent → not counted"
    );
    assert_eq!(
        p.dump_date.as_deref(),
        Some("2026-07-16"),
        "newest sync row"
    );
    assert!(p.stale, "a July 2026 file is older than 60 days");
    assert!(p.missing_optional.contains(&"comic_covers".to_owned()));
    assert!(p.missing_optional.contains(&"volume_fts".to_owned()));
    assert!(p.warnings.iter().any(|w| w.contains("60 days")));
    assert!(p.size_bytes > 0);

    let today = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let gcd = gcd_fixture(dir.path(), &today).await;
    let g = local_db::probe(LocalDbKind::Gcd, &gcd).await.unwrap();
    assert_eq!(g.counts["series"], 1);
    assert_eq!(g.counts["issues"], 2);
    assert_eq!(g.counts["stories"], 1);
    assert_eq!(g.dump_date.as_deref(), Some(&today[..10]));
    assert!(!g.stale);
    assert!(g.warnings.iter().any(|w| w.contains("gcd_story_credit")));
}

#[tokio::test]
async fn probe_rejects_missing_non_sqlite_and_wrong_shape_files() {
    let dir = tempfile::tempdir().unwrap();
    let err = local_db::probe(LocalDbKind::Gcd, &dir.path().join("nope.db"))
        .await
        .unwrap_err();
    assert!(matches!(err, LocalDbError::Missing(_)), "{err}");

    let sql = dir.path().join("dump.sql");
    std::fs::write(
        &sql,
        b"-- MySQL dump 10.13\nCREATE TABLE gcd_issue (id int);\n",
    )
    .unwrap();
    let err = local_db::probe(LocalDbKind::Gcd, &sql).await.unwrap_err();
    assert!(matches!(err, LocalDbError::NotSqlite(_)), "{err}");

    // A real SQLite file of the other provider's shape.
    let cv = comicvine_fixture(dir.path()).await;
    let err = local_db::probe(LocalDbKind::Gcd, &cv).await.unwrap_err();
    match err {
        LocalDbError::MissingTables(t) => {
            assert_eq!(
                t,
                vec!["gcd_series", "gcd_issue", "gcd_story", "gcd_publisher"]
            );
        }
        other => panic!("expected MissingTables, got {other}"),
    }
}

#[tokio::test]
async fn open_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let cv = comicvine_fixture(dir.path()).await;
    let db = local_db::open(&cv).await.unwrap();
    let err = db
        .execute_raw(Statement::from_string(
            db.get_database_backend(),
            "DELETE FROM cv_issue",
        ))
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("readonly")
            || err.to_string().to_lowercase().contains("read-only"),
        "{err}"
    );
}

// ───────── admin surface ─────────

struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp, email: &str) -> Authed {
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn call(
    app: &TestApp,
    user: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                user.session, user.csrf
            ),
        )
        .header("X-CSRF-Token", &user.csrf)
        .header(header::CONTENT_TYPE, "application/json");
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::from("{}"),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, v)
}

fn provider<'a>(list: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    list["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn settings_drive_the_card_and_the_test_endpoint_is_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let dir = tempfile::tempdir().unwrap();
    let cv = comicvine_fixture(dir.path()).await;

    // Unconfigured: both cards say so; Metron has no local slot.
    let (s, list) = call(
        &app,
        &admin,
        Method::GET,
        "/api/admin/metadata/providers",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert_eq!(
        provider(&list, "comicvine")["local"]["status"],
        "unconfigured"
    );
    assert_eq!(
        provider(&list, "comicvine")["local"]["label"],
        "Local ComicVine"
    );
    assert_eq!(provider(&list, "comicvine")["local"]["mode"], "off");
    assert_eq!(provider(&list, "gcd")["local"]["label"], "Local GCD");
    assert!(provider(&list, "metron").get("local").is_none());
    let (s, _) = call(
        &app,
        &admin,
        Method::POST,
        "/api/admin/metadata/providers/comicvine/local/test",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no path yet");
    let (s, _) = call(
        &app,
        &admin,
        Method::POST,
        "/api/admin/metadata/providers/metron/local/test",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "metron has no local file");

    // The registry types the mode as a string, so a bad value is
    // accepted by the settings endpoint and the overlay degrades it to
    // `off` with a warning. Set a real one for ComicVine, a bogus one for GCD.
    let (s, body) = call(
        &app,
        &admin,
        Method::PATCH,
        "/api/admin/settings",
        Some(serde_json::json!({
            "metadata.comicvine.local_path": cv.display().to_string(),
            "metadata.comicvine.local_mode": "prefer_local",
            "metadata.gcd.local_path": dir.path().join("missing.db").display().to_string(),
            "metadata.gcd.local_mode": "bogus",
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let (_, list) = call(
        &app,
        &admin,
        Method::GET,
        "/api/admin/metadata/providers",
        None,
    )
    .await;
    let cv_local = &provider(&list, "comicvine")["local"];
    assert_eq!(cv_local["status"], "ok", "{cv_local}");
    assert_eq!(cv_local["mode"], "prefer_local");
    assert_eq!(cv_local["probe"]["counts"]["issues"], 5);
    assert_eq!(cv_local["probe"]["dump_date"], "2026-07-16");
    assert_eq!(cv_local["probe"]["stale"], true);
    let gcd_local = &provider(&list, "gcd")["local"];
    assert_eq!(gcd_local["status"], "missing", "{gcd_local}");
    assert_eq!(gcd_local["mode"], "off", "an unknown mode degrades to off");
    assert!(gcd_local["error"].as_str().unwrap().contains("no file at"));

    // Test: users are refused, admins get the probe and an audit row;
    // the missing GCD file is a 422 with the reason.
    let (s, _) = call(
        &app,
        &user,
        Method::POST,
        "/api/admin/metadata/providers/comicvine/local/test",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, body) = call(
        &app,
        &admin,
        Method::POST,
        "/api/admin/metadata/providers/comicvine/local/test",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["local"]["probe"]["counts"]["volumes"], 3);
    let (s, body) = call(
        &app,
        &admin,
        Method::POST,
        "/api/admin/metadata/providers/gcd/local/test",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "metadata.local_db.invalid");

    let db = Database::connect(&app.db_url).await.unwrap();
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT target_id, payload->>'ok' AS ok FROM audit_log \
              WHERE action = 'admin.metadata.local_db.test' ORDER BY created_at",
        ))
        .await
        .unwrap();
    let seen: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.try_get::<String>("", "target_id").unwrap(),
                r.try_get::<String>("", "ok").unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            ("comicvine".to_owned(), "false".to_owned()),
            ("comicvine".to_owned(), "true".to_owned()),
            ("gcd".to_owned(), "false".to_owned()),
        ]
    );

    // The swapped-file case: replace the file, the cached probe is
    // re-done because the stamp changed.
    exec_sqlite(
        &cv,
        &["INSERT INTO cv_issue (id, volume_id, issue_number) VALUES (6, 1, '3')"],
    )
    .await;
    let (_, list) = call(
        &app,
        &admin,
        Method::GET,
        "/api/admin/metadata/providers",
        None,
    )
    .await;
    assert_eq!(
        provider(&list, "comicvine")["local"]["probe"]["counts"]["issues"],
        6
    );
}

#[test]
fn modes_are_the_three_documented_strings() {
    assert_eq!(
        LocalDbMode::ALL.map(LocalDbMode::as_str),
        ["off", "prefer_local", "local_only"]
    );
}

/// Probe the real files when the operator points at them
/// (`FOLIO_LOCAL_CV_DB=/path/localcv.db cargo test … -- --ignored`).
#[tokio::test]
#[ignore]
#[allow(clippy::print_stderr)]
async fn probe_real_files_from_env() {
    for (var, kind) in [
        ("FOLIO_LOCAL_CV_DB", LocalDbKind::ComicVine),
        ("FOLIO_LOCAL_GCD_DB", LocalDbKind::Gcd),
    ] {
        let Ok(path) = std::env::var(var) else {
            continue;
        };
        let p = local_db::probe(kind, Path::new(&path)).await.unwrap();
        eprintln!("{var}: {p:#?}");
        assert!(p.counts["issues"] > 1000);
    }
}
