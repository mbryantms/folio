//! Integration tests for M2 of opds-sync-cleanup-1.0 — default up-next-first
//! reorder across every reading-sequence OPDS feed, with per-entity opt-out.
//!
//! Verifies, for each of series / CBL / collection / WTR:
//!  - DEFAULT: when up-next ≠ first canonical entry, up-next moves to position 0.
//!  - OPT-OUT: when the owning row's `preserve_canonical_order` (series, CBL,
//!    collection saved-view) — or `users.opds_wtr_reorder = false` for WTR —
//!    is set, the feed emits issues in canonical order.
//!  - The `?resume=1` synthetic-entry path is gone (no `folio:resume:` /
//!    `▶ Resume` artifacts ever appear in a feed body).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{
    seed_cbl_entry, seed_collection_entry_issue, seed_issue, seed_library, seed_progress_finished,
};
use entity::user as user_entity;
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
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

/// Return the indices of `urn:issue:<id>` markers in feed order. Useful for
/// asserting the relative position of issues without coupling to nearby XML.
fn issue_positions(body: &str, ids: &[&str]) -> Vec<usize> {
    ids.iter()
        .map(|id| {
            body.find(&format!("urn:issue:{id}"))
                .unwrap_or_else(|| panic!("issue {id} not found in body:\n{body}"))
        })
        .collect()
}

fn assert_no_synthetic_resume(body: &str) {
    assert!(
        !body.contains("folio:resume:"),
        "feed must not contain synthetic resume marker:\n{body}"
    );
    assert!(
        !body.contains("\u{25B6} Resume"),
        "feed must not contain ▶ Resume synthetic title:\n{body}"
    );
}

// ────────────── series ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_default_reorder_moves_up_next_to_position_zero() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Reorder Me")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"r-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"r-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"r-c", 3.0).await;
    // a + b are finished → up-next = c. c should now lead the feed.
    seed_progress_finished(&db, auth.user_id, &a).await;
    seed_progress_finished(&db, auth.user_id, &b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&c, &a, &b]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "up-next (c) must precede a + b: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_preserve_canonical_order_opt_out_keeps_natural_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Year One")
        .with_preserve_canonical_order(true)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"y-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"y-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"y-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;
    seed_progress_finished(&db, auth.user_id, &b).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── CBL ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_default_reorder_moves_up_next_to_position_zero() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Crossover")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let i1 = seed_issue(&db, lib, series, &tmp.path().join("1.cbz"), b"cb1", 1.0).await;
    let i2 = seed_issue(&db, lib, series, &tmp.path().join("2.cbz"), b"cb2", 2.0).await;
    let i3 = seed_issue(&db, lib, series, &tmp.path().join("3.cbz"), b"cb3", 3.0).await;
    let i4 = seed_issue(&db, lib, series, &tmp.path().join("4.cbz"), b"cb4", 4.0).await;
    // The user's actual scenario: first 3 finished, #4 is up-next.
    seed_progress_finished(&db, auth.user_id, &i1).await;
    seed_progress_finished(&db, auth.user_id, &i2).await;
    seed_progress_finished(&db, auth.user_id, &i3).await;

    let list = common::seed::CblListSeed::new(auth.user_id, "Storyline")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    seed_cbl_entry(&db, list, 0, Some(&i1)).await;
    seed_cbl_entry(&db, list, 1, Some(&i2)).await;
    seed_cbl_entry(&db, list, 2, Some(&i3)).await;
    seed_cbl_entry(&db, list, 3, Some(&i4)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&i4, &i1, &i2, &i3]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2] && pos[0] < pos[3],
        "i4 (up-next) must lead all read entries: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_preserve_canonical_order_opt_out_keeps_list_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Curated")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let i1 = seed_issue(&db, lib, series, &tmp.path().join("1.cbz"), b"cp1", 1.0).await;
    let i2 = seed_issue(&db, lib, series, &tmp.path().join("2.cbz"), b"cp2", 2.0).await;
    let i3 = seed_issue(&db, lib, series, &tmp.path().join("3.cbz"), b"cp3", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &i1).await;

    let list = common::seed::CblListSeed::new(auth.user_id, "Year One Order")
        .with_preserve_canonical_order(true)
        .insert(&db)
        .await;
    seed_cbl_entry(&db, list, 0, Some(&i1)).await;
    seed_cbl_entry(&db, list, 1, Some(&i2)).await;
    seed_cbl_entry(&db, list, 2, Some(&i3)).await;

    let resp = get_cookie(&app, &format!("/opds/v1/lists/{list}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&i1, &i2, &i3]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "CBL canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── collection ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_default_reorder_moves_up_next_issue_first() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "coll-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Side Stories")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"co-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"co-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"co-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let view = common::seed::CollectionSeed::new(auth.user_id, "My Picks")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    seed_collection_entry_issue(&db, view, 0, &a).await;
    seed_collection_entry_issue(&db, view, 1, &b).await;
    seed_collection_entry_issue(&db, view, 2, &c).await;

    let resp = get_cookie(&app, &format!("/opds/v1/collections/{view}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&b, &a, &c]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "up-next (b) must lead a + c: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_preserve_canonical_order_opt_out_keeps_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "coll-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "Curated Coll")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"cop-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"cop-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"cop-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let view = common::seed::CollectionSeed::new(auth.user_id, "Canonical")
        .with_preserve_canonical_order(true)
        .insert(&db)
        .await;
    seed_collection_entry_issue(&db, view, 0, &a).await;
    seed_collection_entry_issue(&db, view, 1, &b).await;
    seed_collection_entry_issue(&db, view, 2, &c).await;

    let resp = get_cookie(&app, &format!("/opds/v1/collections/{view}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "collection canonical order must hold: {pos:?}\n{body}"
    );
}

// ────────────── WTR ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_default_reorders_up_next_first() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wtr-reorder-default@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "WTR Series")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"w-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"w-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"w-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    // First /wtr hit seeds the WTR collection. Then add three issue rows
    // in canonical order; the second call should reorder.
    let _seed = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let wtr = entity::saved_view::Entity::find()
        .filter(entity::saved_view::Column::UserId.eq(auth.user_id))
        .filter(entity::saved_view::Column::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .expect("WTR row should exist after first /wtr hit");
    seed_collection_entry_issue(&db, wtr.id, 0, &a).await;
    seed_collection_entry_issue(&db, wtr.id, 1, &b).await;
    seed_collection_entry_issue(&db, wtr.id, 2, &c).await;

    let resp = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&b, &a, &c]);
    assert!(
        pos[0] < pos[1] && pos[0] < pos[2],
        "WTR default reorder must move b first: {pos:?}\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_user_opt_out_preserves_drag_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wtr-preserve@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "WTR Curated")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"wp-a", 1.0).await;
    let b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"wp-b", 2.0).await;
    let c = seed_issue(&db, lib, series, &tmp.path().join("c.cbz"), b"wp-c", 3.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let _seed = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let wtr = entity::saved_view::Entity::find()
        .filter(entity::saved_view::Column::UserId.eq(auth.user_id))
        .filter(entity::saved_view::Column::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    seed_collection_entry_issue(&db, wtr.id, 0, &a).await;
    seed_collection_entry_issue(&db, wtr.id, 1, &b).await;
    seed_collection_entry_issue(&db, wtr.id, 2, &c).await;

    // Flip the per-user opt-out flag.
    let mut u: user_entity::ActiveModel = user_entity::Entity::find_by_id(auth.user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    u.opds_wtr_reorder = Set(false);
    u.update(&db).await.unwrap();

    let resp = get_cookie(&app, "/opds/v1/wtr", &auth).await;
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
    let pos = issue_positions(&body, &[&a, &b, &c]);
    assert!(
        pos[0] < pos[1] && pos[1] < pos[2],
        "WTR opt-out preserves drag order: {pos:?}\n{body}"
    );
}

// ────────────── `?resume=1` is gone ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_query_param_is_ignored_after_cleanup() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "no-synth@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = common::seed::SeriesSeed::new(lib, "No Synth")
        .with_preserve_canonical_order(false)
        .insert(&db)
        .await;
    let a = seed_issue(&db, lib, series, &tmp.path().join("a.cbz"), b"ns-a", 1.0).await;
    let _b = seed_issue(&db, lib, series, &tmp.path().join("b.cbz"), b"ns-b", 2.0).await;
    seed_progress_finished(&db, auth.user_id, &a).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}?resume=1"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_no_synthetic_resume(&body);
}
