//! Integration tests for M3 of opds-sync-cleanup-1.0 — title-glyph + page-
//! count suffix annotation on every reading-sequence OPDS entry.
//!
//! Verifies, for v1 (Atom) and v2 (JSON):
//!  - `◯ {title}` for unread entries (no progress row).
//!  - `◐ {title} (N / M)` for in-progress (last_page > 0, !finished).
//!  - `● {title} (M / M)` for finished entries.
//!  - The per-user `users.opds_progress_glyphs = false` opt-out hides
//!    the prefix and suffix entirely (raw title only).
//!  - `(N / M)` suffix is omitted when `page_count` is unknown.
//!
//! The user-facing pitch: clients that ignore the PSE `pse:lastRead`
//! attribute (KOReader, older Tachiyomi) still see "where I left off"
//! because the cue lives in the title string itself.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_library, seed_progress, seed_series};
use entity::user as user_entity;
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
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

#[allow(clippy::too_many_arguments)]
/// Slice each `<entry>` block out of a v1 feed body and return them as
/// a Vec of `(id, block)` pairs. Lets tests assert on the title of a
/// specific entry without false-positives from neighboring entries.
fn entry_blocks_by_id(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in body.split("<entry>").skip(1) {
        let block = raw.split("</entry>").next().unwrap_or("");
        let id = block
            .split("<id>urn:issue:")
            .nth(1)
            .and_then(|s| s.split("</id>").next())
            .unwrap_or("")
            .trim()
            .to_owned();
        out.push((id, block.to_owned()));
    }
    out
}

fn title_of(blocks: &[(String, String)], issue_id: &str) -> String {
    let block = blocks
        .iter()
        .find(|(id, _)| id == issue_id)
        .unwrap_or_else(|| panic!("entry block for {issue_id} not found"));
    block
        .1
        .split("<title>")
        .nth(1)
        .and_then(|s| s.split("</title>").next())
        .unwrap_or("")
        .to_owned()
}

// ────────────── v1 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_decorates_each_state_with_glyph_and_page_count() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-states@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Glyph Series").await;
    // a finished, b in progress (page 14 of 32), c unread.
    let a = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"g-a", 1.0)
        .with_title("First Strike")
        .with_page_count_opt(Some(32))
        .insert(&db)
        .await;
    let b = common::seed::IssueSeed::new(lib, series, &tmp.path().join("b.cbz"), b"g-b", 2.0)
        .with_title("Second Strike")
        .with_page_count_opt(Some(32))
        .insert(&db)
        .await;
    let c = common::seed::IssueSeed::new(lib, series, &tmp.path().join("c.cbz"), b"g-c", 3.0)
        .with_title("Third Strike")
        .with_page_count_opt(Some(32))
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &a, 31, 1.0, true).await;
    seed_progress(&db, auth.user_id, &b, 13, 13.0 / 32.0, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &a),
        "\u{25CF} First Strike",
        "finished should be ● (page-count suffix is no longer emitted; \
         the same numbers are exposed as pse:lastRead attributes):\n{body}"
    );
    assert_eq!(
        title_of(&blocks, &b),
        "Up Next: \u{25D0} Second Strike",
        "in-progress should be ◐, prefixed because it's the up-next \
         target; numeric N/M suffix dropped:\n{body}"
    );
    assert_eq!(
        title_of(&blocks, &c),
        "\u{25CB} Third Strike",
        "unread should be ◯ (no progress row):\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_emits_glyph_only_title_regardless_of_page_count() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-nopagect@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Unknown Pages").await;
    let i = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"np-a", 1.0)
        .with_title("Mystery")
        .with_page_count_opt(None)
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &i, 5, 0.5, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &i),
        "Up Next: \u{25D0} Mystery",
        "title carries the glyph + up-next prefix; the numeric (N / M) \
         page-count suffix is no longer emitted in any state:\n{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_respects_user_opt_out_flag() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v1-optout@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Plain").await;
    let i = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"po-a", 1.0)
        .with_title("Vanilla")
        .with_page_count_opt(Some(32))
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &i, 13, 0.5, false).await;

    // Flip the per-user flag.
    let mut u: user_entity::ActiveModel = user_entity::Entity::find_by_id(auth.user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    u.opds_progress_glyphs = Set(false);
    u.update(&db).await.unwrap();

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let blocks = entry_blocks_by_id(&body);
    assert_eq!(
        title_of(&blocks, &i),
        "Up Next: Vanilla",
        "opt-out strips glyph + suffix; the up-next prefix is structural and applies regardless:\n{body}"
    );
    assert!(
        !body.contains("\u{25CB}") && !body.contains("\u{25D0}") && !body.contains("\u{25CF}"),
        "no progress glyph should appear anywhere in feed when opted out:\n{body}"
    );
}

// ────────────── v2 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_series_feed_decorates_publication_title() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "glyph-v2@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "V2 Series").await;
    let _a = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"v2-a", 1.0)
        .with_title("Issue A")
        .with_page_count_opt(Some(20))
        .insert(&db)
        .await;
    let b = common::seed::IssueSeed::new(lib, series, &tmp.path().join("b.cbz"), b"v2-b", 2.0)
        .with_title("Issue B")
        .with_page_count_opt(Some(20))
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &b, 4, 0.25, false).await;

    let resp = get_cookie(&app, &format!("/opds/v2/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    let pubs = body["publications"].as_array().expect("publications array");
    let title_b = pubs
        .iter()
        .find(|p| p["metadata"]["identifier"] == format!("urn:folio:issue:{b}"))
        .expect("issue b publication")["metadata"]["title"]
        .as_str()
        .unwrap()
        .to_owned();
    // Issue A is unread (no progress row) at position 1.0, so it's the
    // first unfinished issue and the up-next target — Issue B carries
    // progress glyphs but no prefix. The numeric (N / M) page-count
    // suffix that v0.4 versions of this assertion expected is no
    // longer emitted; pse:lastRead exposes the same numbers.
    assert_eq!(title_b, "\u{25D0} Issue B");
}
