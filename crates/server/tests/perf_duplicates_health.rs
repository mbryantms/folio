//! Query-count guards for the two admin pages the 2026-11 audit found to
//! be O(library) per request (N-L3, N-L4):
//!
//! - `GET /libraries/{slug}/duplicates` — the first page fills the
//!   per-library memo (three whole-library statements + hydrate); page 2
//!   must be the memo plus the hydrate of one page: no window-function
//!   grouping, O(1) statements.
//! - `GET /libraries/{slug}/backup-storage` — the first request counts
//!   the `.bak` files and stores the tally; the second is a one-row read.
//!
//! Same counting layer as `perf_regressions.rs` (a process-wide
//! tracing subscriber over `sqlx::query` events), which is why this file
//! holds a single `#[tokio::test]`: a second test in the same process
//! would add its statements to the counter. The layer also keeps the
//! statement text, so the guard can say *which* SQL page 2 did not run.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tower::ServiceExt;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

struct Recorder {
    count: Arc<AtomicU64>,
    statements: Arc<Mutex<Vec<String>>>,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

fn install_recorder() -> &'static Recorder {
    RECORDER.get_or_init(|| {
        let count = Arc::new(AtomicU64::new(0));
        let statements = Arc::new(Mutex::new(Vec::new()));
        let layer = QueryLayer {
            count: count.clone(),
            statements: statements.clone(),
        };
        let filter = tracing_subscriber::EnvFilter::new("sqlx::query=info");
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .init();
        Recorder { count, statements }
    })
}

struct QueryLayer {
    count: Arc<AtomicU64>,
    statements: Arc<Mutex<Vec<String>>>,
}

struct StatementVisitor(String);

impl Visit for StatementVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "db.statement" || field.name() == "summary" {
            self.0.push_str(&format!("{value:?} "));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "db.statement" || field.name() == "summary" {
            self.0.push_str(value);
            self.0.push(' ');
        }
    }
}

impl<S> Layer<S> for QueryLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut v = StatementVisitor(String::new());
        event.record(&mut v);
        self.statements.lock().unwrap().push(v.0);
    }
}

/// Statements fired between `start` and now, as `(count, texts)`.
struct Window<'a> {
    rec: &'a Recorder,
    base_count: u64,
    base_len: usize,
}

impl<'a> Window<'a> {
    fn open(rec: &'a Recorder) -> Self {
        let base_len = rec.statements.lock().unwrap().len();
        Self {
            rec,
            base_count: rec.count.load(Ordering::Relaxed),
            base_len,
        }
    }
    fn close(self) -> (u64, Vec<String>) {
        let count = self.rec.count.load(Ordering::Relaxed) - self.base_count;
        let texts = self.rec.statements.lock().unwrap()[self.base_len..].to_vec();
        (count, texts)
    }
}

struct Authed {
    session: String,
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
                    r#"{"email":"perf-dup@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let session = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.split(';')
                .next()?
                .strip_prefix("__Host-comic_session=")
                .map(str::to_owned)
        })
        .expect("session cookie");
    Authed { session }
}

async fn get(app: &TestApp, auth: &Authed, path: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{path}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn is_grouping_sql(stmt: &str) -> bool {
    let s = stmt.to_ascii_lowercase();
    s.contains("over (partition by")
        || s.contains("bit_count(")
        || s.contains("issue_duplicate_decision where library_id")
}

#[tokio::test]
async fn duplicates_page_two_and_backup_card_are_o1_queries() {
    let rec = install_recorder();
    let app = TestApp::spawn_with_query_logging().await;
    let auth = register_admin(&app).await;
    let db = app.state().db.clone();

    // One library, one series, four duplicate pairs (#1..#4, two files
    // each) → four `number` groups, so `limit=1` gives four pages.
    let dir = tempfile::tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Perf Dupes").insert(&db).await;
    for n in 1..=4u8 {
        for copy in 0..2u8 {
            let path = dir.path().join(format!("Perf Dupes {n:03}-{copy}.cbz"));
            let payload = vec![n, copy, 0xAB, 0xCD];
            IssueSeed::new(lib_id, series_id, &path, &payload, f64::from(n))
                .insert(&db)
                .await;
        }
    }
    // Two `.bak` files for the storage card.
    std::fs::write(dir.path().join("Perf Dupes 001-0.cbz.bak"), b"old bytes").unwrap();
    std::fs::write(dir.path().join("Perf Dupes 002-0.cbz.bak.1"), b"older").unwrap();

    // ── Duplicates: page 1 fills the memo ────────────────────────────
    let w = Window::open(rec);
    let page1 = get(
        &app,
        &auth,
        &format!("/api/libraries/{lib_id}/duplicates?kind=number&limit=1"),
    )
    .await;
    let (page1_count, page1_sql) = w.close();
    assert_eq!(page1["total"], 4, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 1);
    assert!(
        page1_sql.iter().any(|s| is_grouping_sql(s)),
        "page 1 on a cold memo must group the library: {page1_sql:#?}"
    );
    assert!(
        app.state().duplicates.is_cached(lib_id),
        "page 1 stores the memo"
    );
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("next_cursor")
        .to_owned();

    // ── page 2: library lookup + hydrate, nothing else ───────────────
    let w = Window::open(rec);
    let page2 = get(
        &app,
        &auth,
        &format!("/api/libraries/{lib_id}/duplicates?kind=number&limit=1&cursor={cursor}"),
    )
    .await;
    let (page2_count, page2_sql) = w.close();
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["total"].is_null(), "total is first-page only");
    assert!(
        page2_count > 0,
        "page 2 fired 0 observed queries — sqlx statement logging is off on the app pool"
    );
    // admin lookup + find_by_slug + hydrate (issues chunk + series) = 4.
    // Lowering this is fine; raising it needs a reason.
    assert!(
        page2_count <= 4,
        "page 2 fired {page2_count} queries (>4): {page2_sql:#?}"
    );
    assert!(
        page2_count < page1_count,
        "page 2 ({page2_count}) must be cheaper than the memo fill ({page1_count})"
    );
    assert!(
        !page2_sql.iter().any(|s| is_grouping_sql(s)),
        "page 2 re-grouped the library: {page2_sql:#?}"
    );

    // Deeper pages stay O(1) and the pages partition the groups.
    let mut keys: Vec<String> = [&page1, &page2]
        .iter()
        .map(|p| p["items"][0]["key"].as_str().unwrap().to_owned())
        .collect();
    let mut cursor = page2["next_cursor"].as_str().expect("page 3").to_owned();
    loop {
        let w = Window::open(rec);
        let page = get(
            &app,
            &auth,
            &format!("/api/libraries/{lib_id}/duplicates?kind=number&limit=1&cursor={cursor}"),
        )
        .await;
        let (count, sql) = w.close();
        assert!(count <= 4, "deep page fired {count} queries: {sql:#?}");
        keys.push(page["items"][0]["key"].as_str().unwrap().to_owned());
        match page["next_cursor"].as_str() {
            Some(c) => cursor = c.to_owned(),
            None => break,
        }
    }
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 4, "four pages, four distinct groups");

    // ── backup-storage: first request counts + stores, then one row ──
    let w = Window::open(rec);
    let first = get(
        &app,
        &auth,
        &format!("/api/libraries/{lib_id}/backup-storage"),
    )
    .await;
    let (first_count, _) = w.close();
    assert_eq!(first["file_count"], 2, "{first}");
    assert_eq!(first["total_bytes"], 14);
    assert!(first["computed_at"].is_string());
    assert!(
        first_count >= 4,
        "miss = admin + library + load + store, got {first_count}"
    );

    // A new backup on disk is not seen until a recount: the card is the
    // stored row, and the request never touches the filesystem.
    std::fs::write(dir.path().join("Perf Dupes 003-0.cbz.bak"), b"newest").unwrap();
    let w = Window::open(rec);
    let second = get(
        &app,
        &auth,
        &format!("/api/libraries/{lib_id}/backup-storage"),
    )
    .await;
    let (second_count, second_sql) = w.close();
    assert_eq!(
        second["file_count"], 2,
        "stored tally, not a walk: {second}"
    );
    assert_eq!(second["computed_at"], first["computed_at"]);
    assert!(
        second_count > 0 && second_count <= 3,
        "warm card = admin + library lookup + one-row read, got {second_count}: {second_sql:#?}"
    );
    assert!(
        !second_sql.iter().any(|s| s
            .to_ascii_lowercase()
            .contains("insert into \"library_backup_stats\"")),
        "warm card must not re-store: {second_sql:#?}"
    );
}
