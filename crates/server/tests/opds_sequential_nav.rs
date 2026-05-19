//! Integration tests for M2 of opds-sync-1.0 — sequential `rel="next"` /
//! `rel="previous"` acquisition links on issue entries inside reading-
//! sequence feeds (per-series, CBL).
//!
//! Discovery feeds (Recent, Search, New this month) intentionally omit
//! these rels — they have no canonical reading order. The four tests
//! below cover (1) next-in-series, (2) previous-in-series, (3) the
//! last-entry-has-no-next boundary, and (4) next-in-CBL honoring list
//! position.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_cbl_entry, seed_cbl_list, seed_issue, seed_library, seed_series};
use sea_orm::Database;
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

fn entry_block<'a>(body: &'a str, issue_id: &str) -> &'a str {
    let needle = format!("urn:issue:{issue_id}");
    let start = body.find(&needle).expect("entry present");
    let entry_start = body[..start].rfind("<entry>").expect("entry open");
    let entry_end = body[entry_start..].find("</entry>").expect("entry close") + entry_start;
    &body[entry_start..entry_end]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_middle_entry_has_next_and_previous() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "next-mid@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Sequential").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"seq-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"seq-b",
        2.0,
    )
    .await;
    let c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"seq-c",
        3.0,
    )
    .await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let middle = entry_block(&body, &b);
    assert!(
        middle.contains(&format!(
            r#"<link rel="previous" href="/opds/v1/issues/{a}/file""#
        )),
        "middle entry has rel=previous to a: {middle}"
    );
    assert!(
        middle.contains(&format!(
            r#"<link rel="next" href="/opds/v1/issues/{c}/file""#
        )),
        "middle entry has rel=next to c: {middle}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_first_entry_only_has_next() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "next-first@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "First").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"first-a",
        1.0,
    )
    .await;
    let _b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"first-b",
        2.0,
    )
    .await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let first = entry_block(&body, &a);
    assert!(
        !first.contains(r#"<link rel="previous""#),
        "first entry has no previous link: {first}"
    );
    assert!(
        first.contains(r#"<link rel="next""#),
        "first entry has rel=next: {first}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_last_entry_has_no_next() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "no-next@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "End").await;
    let _a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"end-a",
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"end-b",
        2.0,
    )
    .await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let last = entry_block(&body, &b);
    assert!(
        last.contains(r#"<link rel="previous""#),
        "last entry has rel=previous: {last}"
    );
    assert!(
        !last.contains(r#"<link rel="next""#),
        "last entry has no rel=next (end of feed): {last}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_cbl_feed_emits_next_in_list_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-next@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    // Two separate series — verifies rel=next follows CBL position, NOT
    // sort_number within a single series.
    let s1 = seed_series(&db, lib_id, "Alpha").await;
    let s2 = seed_series(&db, lib_id, "Beta").await;
    let a = seed_issue(
        &db,
        lib_id,
        s1,
        &tmp.path().join("alpha.cbz"),
        b"cbl-a",
        1.0,
    )
    .await;
    let b = seed_issue(&db, lib_id, s2, &tmp.path().join("beta.cbz"), b"cbl-b", 1.0).await;

    let list_id = seed_cbl_list(&db, auth.user_id, "Crossover").await;
    seed_cbl_entry(&db, list_id, 0, Some(&a)).await;
    // Position 1 is an unmatched entry (missing issue) — the resolver
    // must skip it so a's rel=next points at b at position 2 directly.
    seed_cbl_entry(&db, list_id, 1, None).await;
    seed_cbl_entry(&db, list_id, 2, Some(&b)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let first = entry_block(&body, &a);
    assert!(
        first.contains(&format!(
            r#"<link rel="next" href="/opds/v1/issues/{b}/file""#
        )),
        "position-0 entry's rel=next points at next matched CBL issue (b at pos 2): {first}"
    );
    let last = entry_block(&body, &b);
    assert!(
        !last.contains(r#"<link rel="next""#),
        "position-2 entry has no rel=next (end of CBL): {last}"
    );
    assert!(
        last.contains(&format!(
            r#"<link rel="previous" href="/opds/v1/issues/{a}/file""#
        )),
        "position-2 entry has rel=previous to a (pos 0): {last}"
    );
}
