//! Integration tests for opds-richer-feeds 1.1.
//!
//! M1: synthetic "Up Next" entries get an `Up Next: ` title prefix.
//! M2: Continue Reading entries emit `pse:last_read` on the acquisition
//!     link (not just the PSE stream link) so Panels-class clients can
//!     resume at the right page on first download.
//! M3: root nav is restructured around user pages — Continue reading +
//!     On Deck stay top-level, each `user_page` becomes its own folder,
//!     and dropped entries (All series / Recently added / Read history /
//!     New this month / Want to Read / My pages) no longer surface.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_progress, seed_series};
use sea_orm::{ActiveValue::Set, Database, DatabaseConnection, EntityTrait};
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

async fn body_text(b: Body) -> String {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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

/// Carve the `<entry>` block whose `<id>` line names `issue_id` out
/// of a v1 feed body. Returns the text between the matching `<entry>`
/// and the next `</entry>`.
fn entry_block_by_issue<'a>(body: &'a str, issue_id: &str) -> &'a str {
    let needle = format!("urn:issue:{issue_id}");
    let id_pos = body.find(&needle).expect("issue id present in feed");
    // Walk backward to the opening `<entry>`.
    let start = body[..id_pos].rfind("<entry>").unwrap();
    let end = body[start..].find("</entry>").unwrap() + start + "</entry>".len();
    &body[start..end]
}

async fn seed_custom_page(
    db: &DatabaseConnection,
    user_id: Uuid,
    name: &str,
    slug: &str,
    position: i32,
) {
    use entity::user_page;
    user_page::Entity::insert(user_page::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        name: Set(name.into()),
        slug: Set(slug.into()),
        is_system: Set(false),
        position: Set(position),
        description: Set(None),
        ..Default::default()
    })
    .exec(db)
    .await
    .unwrap();
}

// ────────────── M1 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_series_feed_prefixes_up_next_entry_only() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m1-prefix@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Prefix Demo").await;
    // a finished, b unfinished (resume target), c unread.
    let a = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"pfx-a", 1.0)
        .with_title("First")
        .with_page_count(20)
        .insert(&db)
        .await;
    let b = common::seed::IssueSeed::new(lib, series, &tmp.path().join("b.cbz"), b"pfx-b", 2.0)
        .with_title("Second")
        .with_page_count(20)
        .insert(&db)
        .await;
    let c = common::seed::IssueSeed::new(lib, series, &tmp.path().join("c.cbz"), b"pfx-c", 3.0)
        .with_title("Third")
        .with_page_count(20)
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &a, 19, 1.0, true).await;
    seed_progress(&db, auth.user_id, &b, 5, 0.25, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;

    let block_a = entry_block_by_issue(&body, &a);
    let block_b = entry_block_by_issue(&body, &b);
    let block_c = entry_block_by_issue(&body, &c);
    assert!(
        !block_a.contains("Up Next:"),
        "finished issue must not be prefixed: {block_a}"
    );
    assert!(
        block_b.contains("<title>Up Next:"),
        "first-unfinished issue MUST be prefixed: {block_b}"
    );
    assert!(
        !block_c.contains("Up Next:"),
        "second-unread issue must not be prefixed: {block_c}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_series_feed_prefixes_up_next_publication_only() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m1-prefix-v2@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "V2 Prefix").await;
    let a = common::seed::IssueSeed::new(lib, series, &tmp.path().join("a.cbz"), b"v2pfx-a", 1.0)
        .with_title("First")
        .with_page_count(20)
        .insert(&db)
        .await;
    let b = common::seed::IssueSeed::new(lib, series, &tmp.path().join("b.cbz"), b"v2pfx-b", 2.0)
        .with_title("Second")
        .with_page_count(20)
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &a, 19, 1.0, true).await;
    seed_progress(&db, auth.user_id, &b, 5, 0.25, false).await;

    let resp = get_cookie(&app, &format!("/opds/v2/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    let pubs = body["publications"].as_array().expect("publications array");
    let title_a = pubs
        .iter()
        .find(|p| p["metadata"]["identifier"] == format!("urn:folio:issue:{a}"))
        .unwrap()["metadata"]["title"]
        .as_str()
        .unwrap();
    let title_b = pubs
        .iter()
        .find(|p| p["metadata"]["identifier"] == format!("urn:folio:issue:{b}"))
        .unwrap()["metadata"]["title"]
        .as_str()
        .unwrap();
    assert!(
        !title_a.contains("Up Next:"),
        "finished publication must not be prefixed: {title_a}"
    );
    assert!(
        title_b.starts_with("Up Next:"),
        "first-unfinished publication MUST be prefixed: {title_b}"
    );
}

// ────────────── M2 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_continue_reading_emits_pse_last_read_on_acquisition_link() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m2-continue@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Resume Me").await;
    let issue = common::seed::IssueSeed::new(lib, series, &tmp.path().join("r.cbz"), b"rm-1", 1.0)
        .with_title("Resume Me #1")
        .with_page_count(32)
        .insert(&db)
        .await;
    seed_progress(&db, auth.user_id, &issue, 14, 0.4375, false).await;

    let resp = get_cookie(&app, "/opds/v1/continue", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block_by_issue(&body, &issue);

    // The acquisition link must carry the progress hint so clients
    // that don't consume the PSE stream link (or download the full
    // archive instead of page-streaming) still see the resume target.
    // Both snake_case and camelCase spellings land — see
    // `pse_progress_attrs` doc-comment for rationale.
    let acq_line = block
        .lines()
        .find(|l| l.contains(r#"rel="http://opds-spec.org/acquisition""#))
        .expect("acquisition link present");
    assert!(
        acq_line.contains(r#"pse:last_read="15""#),
        "snake_case last_read on acquisition link (1-indexed): {acq_line}"
    );
    assert!(
        acq_line.contains(r#"pse:lastRead="15""#),
        "camelCase lastRead on acquisition link (Komga/Panels shape): {acq_line}"
    );
    assert!(
        acq_line.contains("pse:lastReadDate=\""),
        "camelCase lastReadDate on acquisition link: {acq_line}"
    );
    // Stream link emission preserved — regression guard.
    let stream_line = block
        .lines()
        .find(|l| l.contains(r#"rel="http://vaemendis.net/opds-pse/stream""#))
        .expect("stream link present");
    assert!(
        stream_line.contains(r#"pse:last_read="15""#),
        "snake_case last_read on stream link: {stream_line}"
    );
    assert!(
        stream_line.contains(r#"pse:lastRead="15""#),
        "camelCase lastRead on stream link: {stream_line}"
    );
}

/// Regression for the "Panels opens Continue Reading at the cover even
/// though the web UI shows progress" bug (reported 2026-05-19). Two
/// problems compounded:
///   1. Folio v1 emitted `pse:last_read` raw (0-indexed `last_page`),
///      so a user just past the cover (`last_page = 1`) got
///      `pse:last_read="1"`. Panels treats `1` as 1-indexed display
///      position → opens at page 1 = cover. Fix: emit `last_page + 1`.
///   2. Folio emitted snake_case attribute names. Komga / Kavita ship
///      camelCase and Panels follows that convention — strict parsers
///      didn't see our attribute at all. Fix: emit both spellings.
///
/// Pin both invariants here so this can't silently regress again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pse_progress_attrs_emit_1_indexed_and_both_spellings() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "panels-regress@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Just Past Cover").await;
    let issue = common::seed::IssueSeed::new(lib, series, &tmp.path().join("p.cbz"), b"pjc-1", 1.0)
        .with_title("Just Past Cover #1")
        .with_page_count(24)
        .insert(&db)
        .await;
    // The bug-reproducer: user one page past the cover. `last_page = 1`
    // (0-indexed; display page 2). Pre-fix Folio emitted "1" here,
    // which Panels treated as "open at display page 1 = cover".
    seed_progress(&db, auth.user_id, &issue, 1, 0.04, false).await;

    let resp = get_cookie(&app, "/opds/v1/continue", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block_by_issue(&body, &issue);

    // The literal "2" — last_page (1) + 1 — must appear with BOTH
    // attribute spellings on both the stream and acquisition links.
    let stream_line = block
        .lines()
        .find(|l| l.contains(r#"rel="http://vaemendis.net/opds-pse/stream""#))
        .expect("stream link present");
    let acq_line = block
        .lines()
        .find(|l| l.contains(r#"rel="http://opds-spec.org/acquisition""#))
        .expect("acquisition link present");
    for line in [stream_line, acq_line] {
        assert!(
            line.contains(r#"pse:last_read="2""#),
            "snake_case last_read = last_page + 1: {line}"
        );
        assert!(
            line.contains(r#"pse:lastRead="2""#),
            "camelCase lastRead = last_page + 1: {line}"
        );
        assert!(
            !line.contains(r#"pse:last_read="1""#),
            "must not emit raw last_page: {line}"
        );
        assert!(
            line.contains("pse:lastReadDate=\""),
            "camelCase lastReadDate companion: {line}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_acquisition_link_omits_pse_attrs_when_no_progress() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m2-no-progress@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Unread").await;
    let issue = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("u.cbz"),
        b"unread-1",
        1.0,
    )
    .await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block_by_issue(&body, &issue);
    let acq_line = block
        .lines()
        .find(|l| l.contains(r#"rel="http://opds-spec.org/acquisition""#))
        .expect("acquisition link present");
    assert!(
        !acq_line.contains("pse:last_read"),
        "acquisition link must NOT carry pse:last_read when no progress row: {acq_line}"
    );
}

// ────────────── M3 ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_root_lists_one_entry_per_user_page_in_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m3-pages@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    // First hit lazy-seeds the Home page (position 0); add two custom
    // pages so we can assert ordering across all three.
    let _ = get_cookie(&app, "/opds/v1", &auth).await;
    seed_custom_page(&db, auth.user_id, "Capes", "capes", 1).await;
    seed_custom_page(&db, auth.user_id, "Manga", "manga", 2).await;

    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let home_idx = body.find("/opds/v1/pages/home").expect("home entry");
    let capes_idx = body.find("/opds/v1/pages/capes").expect("capes entry");
    let manga_idx = body.find("/opds/v1/pages/manga").expect("manga entry");
    assert!(
        home_idx < capes_idx && capes_idx < manga_idx,
        "pages render in position order: home={home_idx}, capes={capes_idx}, manga={manga_idx}"
    );
    // Per-page titles surface as the entry titles.
    assert!(body.contains("<title>Home</title>"));
    assert!(body.contains("<title>Capes</title>"));
    assert!(body.contains("<title>Manga</title>"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_root_drops_redundant_top_level_entries() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m3-drops@example.com").await;
    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // Five entries dropped from root (handlers remain URL-addressable).
    assert!(!body.contains(r#"href="/opds/v1/series""#));
    assert!(!body.contains(r#"href="/opds/v1/recent""#));
    assert!(!body.contains(r#"href="/opds/v1/history""#));
    assert!(!body.contains(r#"href="/opds/v1/new-this-month""#));
    assert!(!body.contains(r#"href="/opds/v1/wtr""#));
    // "Browse" replaces "All series" as the canonical entry to the
    // full library — distinct, present.
    assert!(body.contains(r#"href="/opds/v1/browse""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_root_preserves_continue_and_on_deck_at_top_level() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m3-keepers@example.com").await;
    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    let body = body_text(resp.into_body()).await;
    let continue_idx = body.find("/opds/v1/continue").expect("continue entry");
    let on_deck_idx = body.find("/opds/v1/on-deck").expect("on-deck entry");
    // Both keepers appear before the first page entry.
    let home_idx = body.find("/opds/v1/pages/home").expect("home entry");
    assert!(
        continue_idx < home_idx && on_deck_idx < home_idx,
        "Continue + On Deck precede the page section: continue={continue_idx}, on-deck={on_deck_idx}, home={home_idx}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_root_navigation_mirrors_v1_restructure() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m3-v2@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _ = get_cookie(&app, "/opds/v2", &auth).await;
    seed_custom_page(&db, auth.user_id, "Capes", "capes", 1).await;

    let resp = get_cookie(&app, "/opds/v2", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    let nav = body["navigation"].as_array().expect("navigation array");
    let titles: Vec<&str> = nav.iter().filter_map(|n| n["title"].as_str()).collect();
    // Keepers + per-page + catch-alls + Browse. Specifically, NO
    // "All series" / "Recently added" / "Read history" / "New this
    // month" / "Want to Read" / "My pages" entries any more.
    assert!(titles.contains(&"Continue reading"), "{titles:?}");
    assert!(titles.contains(&"On Deck"), "{titles:?}");
    assert!(titles.contains(&"Home"), "{titles:?}");
    assert!(titles.contains(&"Capes"), "{titles:?}");
    assert!(titles.contains(&"Reading lists"), "{titles:?}");
    assert!(titles.contains(&"Collections"), "{titles:?}");
    assert!(titles.contains(&"Saved views"), "{titles:?}");
    assert!(titles.contains(&"Browse"), "{titles:?}");
    assert!(!titles.contains(&"All series"), "{titles:?}");
    assert!(!titles.contains(&"Recently added"), "{titles:?}");
    assert!(!titles.contains(&"Read history"), "{titles:?}");
    assert!(!titles.contains(&"New this month"), "{titles:?}");
    assert!(!titles.contains(&"Want to Read"), "{titles:?}");
    assert!(!titles.contains(&"My pages"), "{titles:?}");
}
