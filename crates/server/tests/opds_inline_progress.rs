//! Integration tests for M1 of opds-sync-1.0 — inline read-state on
//! OPDS feed entries.
//!
//! Verifies that every issue entry across the v1 and v2 feeds carries:
//!  - `pse:lastRead` + `pse:lastReadDate` (Atom) when a progress row exists
//!  - `metadata.position` (OPDS 2.0) when a progress row exists
//!  - no annotation at all when the user has no progress row
//!  - per-user fan-out across a multi-issue feed renders each entry's
//!    state independently
//!
//! The five tests below cover (1) progress-present in v1, (2) progress-absent
//! in v1, (3) finished issue, (4) multi-issue feed with mixed states, and
//! (5) v2 `metadata.position` shape.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_library, seed_progress, seed_series};
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

/// Extract the substring between the entry's `urn:issue:<id>` id line and
/// its closing `</entry>` tag. Used to scope assertions to a single
/// entry in multi-entry feed responses.
fn entry_block<'a>(body: &'a str, issue_id: &str) -> &'a str {
    let needle = format!("urn:issue:{issue_id}");
    let start = body.find(&needle).expect("entry present");
    // Walk back to `<entry>` boundary
    let entry_start = body[..start].rfind("<entry>").expect("entry open");
    let entry_end = body[entry_start..].find("</entry>").expect("entry close") + entry_start;
    &body[entry_start..entry_end]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_entry_carries_pse_last_read_when_progress_present() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-progress-present@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Saga").await;
    let issue_id = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"saga-01",
        1.0,
    )
    .with_page_count(32)
    .insert(&db)
    .await;
    seed_progress(&db, auth.user_id, &issue_id, 14, 0.4375, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    // OPDS-PSE spec: `last_read` / `last_read_date` are snake_case
    // ATTRIBUTES on the stream link, not free-standing child elements.
    // Verify the attribute shape that Panels / Chunky actually consume.
    assert!(
        block.contains(r#"pse:last_read="14""#),
        "last_read attribute present on stream link: {block}"
    );
    assert!(
        block.contains("pse:last_read_date=\""),
        "last_read_date attribute present on stream link: {block}"
    );
    // Regression guard: pse:count must survive on the same link.
    assert!(
        block.contains(r#"pse:count="32""#),
        "pse:count attribute preserved on stream link: {block}"
    );
    // Feed-root namespace declared.
    assert!(
        body.contains(r#"xmlns:pse="http://vaemendis.net/opds-pse/ns""#),
        "feed declares pse namespace"
    );
    // Negative-case guard: no free-standing child elements (the broken
    // shape Folio emitted in v0.3.29-31).
    assert!(
        !block.contains("<pse:lastRead>") && !block.contains("<pse:lastReadDate>"),
        "no free-standing child elements: {block}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_entry_omits_pse_last_read_when_progress_absent() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-progress-absent@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Unread").await;
    let issue_id = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"unread-01",
        1.0,
    )
    .with_page_count(24)
    .insert(&db)
    .await;
    // No progress seeded.

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    // Stream link still emitted (so `pse:count` carries page total),
    // but no last_read attributes when the caller has no progress row.
    assert!(
        block.contains(r#"pse:count="24""#),
        "stream link with count still emitted: {block}"
    );
    assert!(
        !block.contains("pse:last_read"),
        "no last_read attribute when no progress row: {block}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_finished_issue_emits_last_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-finished@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Done").await;
    let issue_id = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"done-01",
        1.0,
    )
    .with_page_count(20)
    .insert(&db)
    .await;
    // page=19 (last 0-based), finished=true, percent=1.0
    seed_progress(&db, auth.user_id, &issue_id, 19, 1.0, true).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    assert!(
        block.contains(r#"pse:last_read="19""#),
        "finished issue emits last_read=19 on the stream link: {block}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_multi_issue_feed_renders_mixed_states() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-multi@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Mixed").await;
    let a = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"mixed-a",
        1.0,
    )
    .with_page_count(30)
    .insert(&db)
    .await;
    let b = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"mixed-b",
        2.0,
    )
    .with_page_count(30)
    .insert(&db)
    .await;
    let c = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"mixed-c",
        3.0,
    )
    .with_page_count(30)
    .insert(&db)
    .await;
    // a: in progress (page 5); b: untouched; c: finished
    seed_progress(&db, auth.user_id, &a, 5, 0.1666, false).await;
    seed_progress(&db, auth.user_id, &c, 29, 1.0, true).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let block_a = entry_block(&body, &a);
    let block_b = entry_block(&body, &b);
    let block_c = entry_block(&body, &c);
    assert!(
        block_a.contains(r#"pse:last_read="5""#),
        "a in-progress: {block_a}"
    );
    assert!(
        !block_b.contains("pse:last_read"),
        "b untouched, no last_read attribute: {block_b}"
    );
    assert!(
        block_c.contains(r#"pse:last_read="29""#),
        "c finished: {block_c}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_publication_carries_metadata_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-position@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Readium").await;
    let issue_id = common::seed::IssueSeed::new(
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"readium-01",
        1.0,
    )
    .with_page_count(32)
    .insert(&db)
    .await;
    // last_page=14, percent=0.4375 → totalProgression=0.4375, position=15
    seed_progress(&db, auth.user_id, &issue_id, 14, 0.4375, false).await;

    let resp = get_cookie(&app, &format!("/opds/v2/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let publications = body["publications"].as_array().expect("publications array");
    let pub_ = publications
        .iter()
        .find(|p| {
            p["metadata"]["identifier"]
                .as_str()
                .map(|s| s.contains(&issue_id))
                .unwrap_or(false)
        })
        .expect("publication for seeded issue");
    let pos = &pub_["metadata"]["position"];
    assert_eq!(pos["position"], 15, "position = last_page + 1");
    assert!(
        (pos["totalProgression"].as_f64().unwrap() - 0.4375).abs() < 0.0001,
        "totalProgression matches stored percent: {pos:?}",
    );
    assert_eq!(pos["finished"], false);
    assert_eq!(pos["totalPages"], 32);
    assert!(pos["modified"].is_string(), "modified timestamp present");
}
