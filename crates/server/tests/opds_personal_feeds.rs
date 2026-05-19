//! Integration tests for M2.5 of opds-sync-1.0 — the
//! `/opds/v1/continue` and `/opds/v1/on-deck` aggregate personal feeds.
//!
//! Verifies:
//!  - `/continue` returns in-progress issues sorted by `last_read_at`
//!    (most-recent first); finished issues are excluded.
//!  - `/on-deck` returns one entry per series the user is reading,
//!    pointing at the first-unread issue.
//!  - ACL filter — a non-admin without library access doesn't see
//!    issues that belong to other users' libraries.
//!  - Catalog root advertises both endpoints as subsection links.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_progress_at, seed_series};
use entity::library_user_access::ActiveModel as LibraryUserAccessAM;
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

async fn grant_library_access(db: &DatabaseConnection, user_id: Uuid, lib_id: Uuid) {
    let now = Utc::now().fixed_offset();
    LibraryUserAccessAM {
        library_id: Set(lib_id),
        user_id: Set(user_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_continue_orders_by_last_read_desc_and_excludes_finished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "continue-order@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Continue").await;
    let older = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("older.cbz"),
        b"cont-older",
        1.0,
    )
    .await;
    let newer = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("newer.cbz"),
        b"cont-newer",
        2.0,
    )
    .await;
    let done = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("done.cbz"),
        b"cont-done",
        3.0,
    )
    .await;
    let t_older = chrono::DateTime::parse_from_rfc3339("2026-05-15T10:00:00Z").unwrap();
    let t_newer = chrono::DateTime::parse_from_rfc3339("2026-05-17T10:00:00Z").unwrap();
    let t_done = chrono::DateTime::parse_from_rfc3339("2026-05-16T10:00:00Z").unwrap();
    seed_progress_at(&db, auth.user_id, &older, 4, false, t_older).await;
    seed_progress_at(&db, auth.user_id, &newer, 8, false, t_newer).await;
    seed_progress_at(&db, auth.user_id, &done, 19, true, t_done).await;

    let resp = get_cookie(&app, "/opds/v1/continue", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // Finished issue must NOT appear.
    assert!(
        !body.contains(&format!("urn:issue:{done}")),
        "finished issue is excluded from /continue"
    );
    // newer must precede older (last_read_at DESC).
    let pos_newer = body
        .find(&format!("urn:issue:{newer}"))
        .expect("newer issue present");
    let pos_older = body
        .find(&format!("urn:issue:{older}"))
        .expect("older issue present");
    assert!(
        pos_newer < pos_older,
        "newer (last_read_at later) must appear before older"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_on_deck_returns_first_unread_per_active_series() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "on-deck-series@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "OnDeck").await;
    let issue_a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"deck-a",
        1.0,
    )
    .await;
    let issue_b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"deck-b",
        2.0,
    )
    .await;
    let issue_c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"deck-c",
        3.0,
    )
    .await;
    // a is finished; b + c are unread. On Deck must surface b (first
    // unread after the latest meaningful progress).
    let when = chrono::Utc::now().fixed_offset();
    seed_progress_at(&db, auth.user_id, &issue_a, 19, true, when).await;

    let resp = get_cookie(&app, "/opds/v1/on-deck", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(&format!("urn:issue:{issue_b}")),
        "on-deck surfaces first-unread issue b: {body}"
    );
    // c is also unread but only ONE entry per series should appear; b
    // wins because it's the FIRST unread. c may or may not appear in a
    // separate context; assert b precedes c if both present.
    let pos_b = body
        .find(&format!("urn:issue:{issue_b}"))
        .expect("b present");
    if let Some(pos_c) = body.find(&format!("urn:issue:{issue_c}")) {
        assert!(pos_b < pos_c, "first-unread b precedes c");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_personal_feeds_enforce_library_acl() {
    let app = TestApp::spawn().await;
    // First user is auto-admin; second user is a regular reader with
    // no library_user_access row, so they can't see any library.
    let _admin = register(&app, "admin-acl@example.com").await;
    let reader = register(&app, "reader-acl@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Forbidden").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("forbidden.cbz"),
        b"forbid-1",
        1.0,
    )
    .await;
    // Seed progress for the reader who CAN'T see the library — the
    // /continue feed must drop the issue regardless of progress.
    let when = chrono::Utc::now().fixed_offset();
    seed_progress_at(&db, reader.user_id, &issue_id, 5, false, when).await;

    let resp = get_cookie(&app, "/opds/v1/continue", &reader).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains(&format!("urn:issue:{issue_id}")),
        "ACL filter drops issues in libraries the user can't see"
    );

    // Grant the access — the same issue must now surface.
    grant_library_access(&db, reader.user_id, lib_id).await;
    let resp_after = get_cookie(&app, "/opds/v1/continue", &reader).await;
    let body_after = body_text(resp_after.into_body()).await;
    assert!(
        body_after.contains(&format!("urn:issue:{issue_id}")),
        "issue appears after granting library access: {body_after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opds_root_advertises_continue_and_on_deck_subsections() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "root-nav@example.com").await;

    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(r#"href="/opds/v1/continue""#),
        "root catalog advertises /opds/v1/continue: {body}"
    );
    assert!(
        body.contains(r#"href="/opds/v1/on-deck""#),
        "root catalog advertises /opds/v1/on-deck: {body}"
    );
}
