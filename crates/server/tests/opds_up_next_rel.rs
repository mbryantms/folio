//! Integration tests for M2.3 of opds-sync-1.0 — feed-level
//! `rel="https://folio.local/rels/up-next"` on resume-context feeds.
//!
//! Verifies:
//!  - series feed: rel present, points at the first unfinished issue
//!  - CBL feed: rel honors CBL position (next-unfinished in list order,
//!    not series sort_number)
//!  - rel ABSENT when the user has finished every issue in the feed
//!  - rel ABSENT on discovery feeds (`/opds/v1/recent`) — those have
//!    no reading context.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{
    seed_cbl_entry, seed_cbl_list, seed_issue, seed_library, seed_progress_finished, seed_series,
};
use sea_orm::Database;
use tower::ServiceExt;
use uuid::Uuid;

const UP_NEXT_REL: &str = "https://folio.local/rels/up-next";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_emits_up_next_rel_pointing_at_first_unfinished_issue() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-up-next@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Resume Here").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"un-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"un-b",
        2.0,
    )
    .await;
    let _c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"un-c",
        3.0,
    )
    .await;
    // a is finished; b is unread → up-next must point at b.
    seed_progress_finished(&db, auth.user_id, &a).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let expected = format!(r#"<link rel="{UP_NEXT_REL}" href="/opds/v1/issues/{b}""#);
    assert!(
        body.contains(&expected),
        "expected up-next rel to point at b, got body:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_cbl_feed_emits_up_next_rel_honoring_list_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-up-next@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    // CBL position is independent of sort_number. Series A's issue
    // (sort=1.0) is at CBL position 0 but FINISHED; series B's issue
    // (sort=1.0) is at CBL position 1 and unread. Up-next must point
    // at series B's issue — proves list-position resolution, not the
    // first-unfinished-by-sort fallback.
    let sa = seed_series(&db, lib_id, "Alpha").await;
    let sb = seed_series(&db, lib_id, "Beta").await;
    let ia = seed_issue(&db, lib_id, sa, &tmp.path().join("a.cbz"), b"cbl-up-a", 1.0).await;
    let ib = seed_issue(&db, lib_id, sb, &tmp.path().join("b.cbz"), b"cbl-up-b", 1.0).await;
    seed_progress_finished(&db, auth.user_id, &ia).await;

    let list_id = seed_cbl_list(&db, auth.user_id, "Crossover").await;
    seed_cbl_entry(&db, list_id, 0, Some(&ia)).await;
    seed_cbl_entry(&db, list_id, 1, Some(&ib)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let expected = format!(r#"<link rel="{UP_NEXT_REL}" href="/opds/v1/issues/{ib}""#);
    assert!(
        body.contains(&expected),
        "expected CBL up-next to point at ib (pos 1), got body:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_omits_up_next_when_everything_is_finished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "all-done@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Caught Up").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"done-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"done-b",
        2.0,
    )
    .await;
    seed_progress_finished(&db, auth.user_id, &a).await;
    seed_progress_finished(&db, auth.user_id, &b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains(UP_NEXT_REL),
        "no up-next rel when user finished everything in the series:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_discovery_feed_does_not_emit_up_next_rel() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "discovery@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Recent").await;
    // Seed an unread issue so the recent feed has something to render.
    seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"rec-a",
        1.0,
    )
    .await;

    let resp = get_cookie(&app, "/opds/v1/recent", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains(UP_NEXT_REL),
        "/opds/v1/recent must NOT carry an up-next rel (no reading context):\n{body}"
    );
}
