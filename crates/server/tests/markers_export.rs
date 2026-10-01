//! Roadmap WP-5.1 — notes export (`GET /me/markers/export`) and the
//! marker permalink (`GET /markers/{id}`).
//!
//! The Markdown test is a snapshot: the rendered document is compared to
//! `tests/snapshots/notes_export.md` after the `Exported …` line is
//! normalised. Rerun with `UPDATE_SNAPSHOTS=1` to rewrite the file after
//! an intentional layout change.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset};
use common::TestApp;
use common::seed::{SeriesSeed, seed_issue, seed_library};
use entity::{library_user_access, marker};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
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
    let json: Value = serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get(app: &TestApp, uri: &str, auth: Option<&Authed>) -> Response<Body> {
    let mut req = Request::builder().method(Method::GET).uri(uri);
    if let Some(a) = auth {
        req = req.header(header::COOKIE, a.cookies());
    }
    app.router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn insert_marker(
    db: &DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
    series_id: Uuid,
    issue_id: &str,
    page_index: i32,
    kind: &str,
    body: Option<&str>,
    selection_text: Option<&str>,
    tags: &[&str],
    is_favorite: bool,
    created_at: DateTime<FixedOffset>,
) {
    let region = (kind == "highlight")
        .then(|| json!({"x": 10.0, "y": 20.0, "w": 30.0, "h": 15.0, "shape": "text"}));
    marker::ActiveModel {
        id: Set(id),
        user_id: Set(user_id),
        series_id: Set(series_id),
        issue_id: Set(issue_id.to_owned()),
        page_index: Set(page_index),
        kind: Set(kind.to_owned()),
        is_favorite: Set(is_favorite),
        tags: Set(tags.iter().map(|t| (*t).to_owned()).collect()),
        region: Set(region),
        selection: Set(selection_text.map(|t| json!({"text": t}))),
        body: Set(body.map(str::to_owned)),
        color: Set(None),
        created_at: Set(created_at),
        updated_at: Set(created_at),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

const M_SAGA1_P3_NOTE: &str = "00000000-0000-7000-8000-000000000001";
const M_SAGA1_P3_HL: &str = "00000000-0000-7000-8000-000000000002";
const M_SAGA1_P1_BM: &str = "00000000-0000-7000-8000-000000000003";
const M_SAGA2_P5_NOTE: &str = "00000000-0000-7000-8000-000000000004";
const M_ALPHA1_P2_FAV: &str = "00000000-0000-7000-8000-000000000005";
const M_OTHER_USER: &str = "00000000-0000-7000-8000-000000000006";
const M_GONE_P1_NOTE: &str = "00000000-0000-7000-8000-000000000007";

struct Seeded {
    saga_id: Uuid,
    saga1: String,
}

/// Two series, three issues, five markers for `owner` and one marker
/// for `other` that must never appear in the owner's export. Insert
/// order is deliberately scrambled against display order so the test
/// proves the export sorts.
async fn seed(db: &DatabaseConnection, root: &std::path::Path, owner: Uuid, other: Uuid) -> Seeded {
    let lib = seed_library(db, root).await;
    let saga_id = SeriesSeed::new(lib, "Saga").insert(db).await;
    let alpha_id = SeriesSeed::new(lib, "Alpha Flight").insert(db).await;
    let saga2 = seed_issue(db, lib, saga_id, &root.join("saga2.cbz"), b"saga-2", 2.0).await;
    let saga1 = seed_issue(db, lib, saga_id, &root.join("saga1.cbz"), b"saga-1", 1.0).await;
    let alpha1 = seed_issue(db, lib, alpha_id, &root.join("alpha1.cbz"), b"alpha-1", 1.0).await;

    let t0 = ts("2026-09-01T12:00:00+00:00");
    let at = |mins: i64| t0 + Duration::minutes(mins);
    let id = |s: &str| Uuid::parse_str(s).unwrap();

    insert_marker(
        db,
        id(M_SAGA2_P5_NOTE),
        owner,
        saga_id,
        &saga2,
        4,
        "note",
        Some("Second issue thought."),
        None,
        &[],
        false,
        at(0),
    )
    .await;
    insert_marker(
        db,
        id(M_SAGA1_P3_HL),
        owner,
        saga_id,
        &saga1,
        2,
        "highlight",
        None,
        Some("We are all\nborn of the stars."),
        &["quote"],
        true,
        at(2),
    )
    .await;
    insert_marker(
        db,
        id(M_SAGA1_P3_NOTE),
        owner,
        saga_id,
        &saga1,
        2,
        "note",
        Some("Alana's line lands.\n\n- callback to #1"),
        None,
        &["plot", "arc"],
        false,
        at(1),
    )
    .await;
    insert_marker(
        db,
        id(M_SAGA1_P1_BM),
        owner,
        saga_id,
        &saga1,
        0,
        "bookmark",
        None,
        None,
        &[],
        false,
        at(3),
    )
    .await;
    insert_marker(
        db,
        id(M_ALPHA1_P2_FAV),
        owner,
        alpha_id,
        &alpha1,
        1,
        "favorite",
        None,
        None,
        &[],
        false,
        at(4),
    )
    .await;
    insert_marker(
        db,
        id(M_OTHER_USER),
        other,
        saga_id,
        &saga1,
        0,
        "note",
        Some("someone else's note"),
        None,
        &[],
        false,
        at(5),
    )
    .await;

    // A marker on an issue since removed from the library: still
    // exported, but flagged unavailable with no Jump link.
    let gone_id = SeriesSeed::new(lib, "Gone Series").insert(db).await;
    let gone1 = seed_issue(db, lib, gone_id, &root.join("gone1.cbz"), b"gone-1", 1.0).await;
    insert_marker(
        db,
        id(M_GONE_P1_NOTE),
        owner,
        gone_id,
        &gone1,
        0,
        "note",
        Some("Note on a removed issue."),
        None,
        &[],
        false,
        at(6),
    )
    .await;
    let row = entity::issue::Entity::find_by_id(gone1.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.removed_at = Set(Some(t0));
    am.update(db).await.unwrap();

    Seeded { saga_id, saga1 }
}

fn normalise_exported_line(md: &str) -> String {
    md.lines()
        .map(|l| {
            if let Some(rest) = l.strip_prefix("Exported ") {
                let tail = rest.split_once(" · ").map(|(_, t)| t).unwrap_or("");
                format!("Exported <EXPORTED_AT> · {tail}")
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn markdown_export_matches_snapshot() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let other = register(&app, "other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    seed(&db, tmp.path(), owner.user_id, other.user_id).await;

    let resp = get(&app, "/api/me/markers/export?format=md", Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(ct.starts_with("text/markdown"), "content-type: {ct}");
    let cd = resp.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        cd.starts_with("attachment; filename=\"folio-notes-") && cd.ends_with(".md\""),
        "content-disposition: {cd}"
    );
    let md = String::from_utf8(body_bytes(resp.into_body()).await).unwrap();
    let actual = normalise_exported_line(&md);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("notes_export.md");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).expect("snapshot file (UPDATE_SNAPSHOTS=1)");
    assert_eq!(
        actual, expected,
        "notes export Markdown drifted from snapshot"
    );
    assert!(!md.contains("someone else's note"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_format_is_markdown() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let resp = get(&app, "/api/me/markers/export", Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(ct.starts_with("text/markdown"), "content-type: {ct}");
    let md = String::from_utf8(body_bytes(resp.into_body()).await).unwrap();
    assert!(md.starts_with("# Folio notes\n"));
    assert!(md.contains("· 0 markers"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_export_groups_series_issue_page() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let other = register(&app, "other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let seeded = seed(&db, tmp.path(), owner.user_id, other.user_id).await;

    let resp = get(&app, "/api/me/markers/export?format=json", Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cd.ends_with(".json\""), "content-disposition: {cd}");
    let doc: Value = serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();

    assert_eq!(doc["format"], "folio-notes-export");
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["total"], 6);
    let series = doc["series"].as_array().unwrap();
    let names: Vec<&str> = series
        .iter()
        .map(|s| s["series"]["series_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Alpha Flight", "Gone Series", "Saga"]);

    // Removed issue: exported, flagged unavailable, no jump_url.
    let gone = &series[1]["issues"][0]["pages"][0]["markers"][0];
    assert_eq!(gone["id"], M_GONE_P1_NOTE);
    assert_eq!(gone["available"], false);
    assert!(gone.get("jump_url").is_none(), "{gone:#}");
    assert_eq!(gone["body"], "Note on a removed issue.");

    let saga = &series[2];
    assert_eq!(saga["series"]["series_id"], seeded.saga_id.to_string());
    let issues = saga["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0]["issue"]["issue_id"], seeded.saga1);
    assert_eq!(issues[0]["issue"]["content_hash"], seeded.saga1);
    assert_eq!(issues[0]["issue"]["issue_number"], "1");
    assert_eq!(issues[0]["issue_title"], "Issue 1");

    let pages = issues[0]["pages"].as_array().unwrap();
    let page_indexes: Vec<i64> = pages
        .iter()
        .map(|p| p["page_index"].as_i64().unwrap())
        .collect();
    assert_eq!(page_indexes, [0, 2]);
    // Oldest first within a page.
    let p3: Vec<&str> = pages[1]["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(p3, [M_SAGA1_P3_NOTE, M_SAGA1_P3_HL]);

    // Marker carries the shared `ExportMarker` shape plus `jump_url`.
    let hl = &pages[1]["markers"][1];
    assert_eq!(hl["kind"], "highlight");
    assert_eq!(hl["is_favorite"], true);
    assert_eq!(hl["tags"], json!(["quote"]));
    assert_eq!(hl["selection"]["text"], "We are all\nborn of the stars.");
    assert_eq!(hl["issue"]["issue_id"], seeded.saga1);
    assert_eq!(hl["available"], true);
    assert_eq!(
        hl["jump_url"],
        format!("http://localhost:8080/markers/{M_SAGA1_P3_HL}")
    );

    let body = doc.to_string();
    assert!(!body.contains(M_OTHER_USER), "other user's marker leaked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_format_is_rejected_and_anonymous_is_401() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let resp = get(&app, "/api/me/markers/export?format=pdf", Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = get(&app, "/api/me/markers/export", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ───────── permalink ─────────

fn location(resp: &Response<Body>) -> String {
    resp.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permalink_redirects_owner_to_reader_page_in_peek_mode() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let other = register(&app, "other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let seeded = seed(&db, tmp.path(), owner.user_id, other.user_id).await;

    let series = entity::series::Entity::find_by_id(seeded.saga_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let issue = entity::issue::Entity::find_by_id(seeded.saga1.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();

    let resp = get(&app, &format!("/markers/{M_SAGA1_P3_HL}"), Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        location(&resp),
        format!("/read/{}/{}?page=2&peek=1", series.slug, issue.slug)
    );

    // Another user's marker is indistinguishable from a missing one.
    let resp = get(&app, &format!("/markers/{M_OTHER_USER}"), Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = get(&app, &format!("/markers/{}", Uuid::now_v7()), Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = get(&app, "/markers/not-a-uuid", Some(&owner)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permalink_without_session_bounces_through_sign_in() {
    let app = TestApp::spawn().await;
    let resp = get(&app, &format!("/markers/{M_SAGA1_P3_HL}"), None).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        location(&resp),
        format!("/sign-in?next=/markers/{M_SAGA1_P3_HL}")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permalink_404s_when_issue_is_no_longer_visible() {
    let app = TestApp::spawn().await;
    // First registered user is the admin; the second is a plain user.
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib, "Locked").insert(&db).await;
    let issue_id = seed_issue(
        &db,
        lib,
        series_id,
        &tmp.path().join("l.cbz"),
        b"locked",
        1.0,
    )
    .await;
    let now = chrono::Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user.user_id),
        library_id: Set(lib),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let marker_id = Uuid::now_v7();
    insert_marker(
        &db,
        marker_id,
        user.user_id,
        series_id,
        &issue_id,
        0,
        "bookmark",
        None,
        None,
        &[],
        false,
        now,
    )
    .await;

    let resp = get(&app, &format!("/markers/{marker_id}"), Some(&user)).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Grant revoked: the marker still exists but its issue is hidden.
    library_user_access::Entity::delete_by_id((lib, user.user_id))
        .exec(&db)
        .await
        .unwrap();
    let resp = get(&app, &format!("/markers/{marker_id}"), Some(&user)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_flags_markers_the_user_can_no_longer_see() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib, "Capped").insert(&db).await;
    let issue_id = seed_issue(
        &db,
        lib,
        series_id,
        &tmp.path().join("c.cbz"),
        b"capped",
        1.0,
    )
    .await;
    let now = chrono::Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user.user_id),
        library_id: Set(lib),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let marker_id = Uuid::now_v7();
    insert_marker(
        &db,
        marker_id,
        user.user_id,
        series_id,
        &issue_id,
        0,
        "bookmark",
        None,
        None,
        &[],
        false,
        now,
    )
    .await;

    let fetch = || async {
        let resp = get(&app, "/api/me/markers/export?format=json", Some(&user)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let doc: Value = serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        doc["series"][0]["issues"][0]["pages"][0]["markers"][0].clone()
    };
    assert_eq!(fetch().await["available"], true);

    // Age cap below the issue's rating hides it.
    let row = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.age_rating = Set(Some("Mature 17+".into()));
    am.update(&db).await.unwrap();
    let grant = library_user_access::Entity::find_by_id((lib, user.user_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut gam: library_user_access::ActiveModel = grant.into();
    gam.age_rating_max = Set(Some("Teen".into()));
    gam.update(&db).await.unwrap();
    let m = fetch().await;
    assert_eq!(m["available"], false);
    assert!(m.get("jump_url").is_none());

    // Revoked grant: still exported (it's the user's data), still flagged.
    library_user_access::Entity::delete_by_id((lib, user.user_id))
        .exec(&db)
        .await
        .unwrap();
    let m = fetch().await;
    assert_eq!(m["id"], marker_id.to_string());
    assert_eq!(m["available"], false);

    let resp = get(&app, "/api/me/markers/export?format=md", Some(&user)).await;
    let md = String::from_utf8(body_bytes(resp.into_body()).await).unwrap();
    assert!(md.contains("**Bookmark** · (no longer available)"), "{md}");
    assert!(!md.contains("/markers/"), "{md}");
}
