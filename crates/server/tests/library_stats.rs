//! `GET /admin/libraries/stats` + `GET /admin/libraries/{slug}/stats`
//! (audit 2026-11, §5 item 16): per-library counts and disk usage.
//!
//! One `#[tokio::test]` on purpose: the query-count guard uses the same
//! process-wide `sqlx::query` counting layer as `perf_regressions.rs` /
//! `perf_duplicates_health.rs`, and a second test in this binary would add
//! its statements to the counter.
//!
//! Covers: the figures are right per library (removed and unreadable
//! issues stay out of the active count and the archive bytes, removed
//! series out of the series count), backup figures come from the stored
//! tally and never from a walk of the root, a library with nothing in it
//! reports zeros, non-admins get 403, and the statement count is fixed
//! whatever the number of libraries.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use server::library::backup_stats::{self, Tally};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tower::ServiceExt;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

// ───────── statement counter (same layer as perf_duplicates_health.rs) ─────────

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

// ───────── HTTP helpers ─────────

async fn register(app: &TestApp, email: &str) -> String {
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
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.split(';')
                .next()?
                .strip_prefix("__Host-comic_session=")
                .map(str::to_owned)
        })
        .expect("session cookie")
}

async fn get(app: &TestApp, session: &str, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn exec(db: &sea_orm::DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

fn row_for(rows: &serde_json::Value, id: Uuid) -> &serde_json::Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["library_id"] == id.to_string())
        .unwrap_or_else(|| panic!("no stats row for {id}: {rows}"))
}

/// Admin check + library rows + series / issues / backups = 5. Lowering
/// this is fine; raising it needs a reason.
const MAX_QUERIES: u64 = 5;

#[tokio::test]
async fn library_stats_figures_access_and_query_count() {
    let rec = install_recorder();
    let app = TestApp::spawn_with_query_logging().await;
    let admin = register(&app, "stats-admin@example.com").await;
    let reader = register(&app, "stats-reader@example.com").await;
    let db = app.state().db.clone();

    // ── Library A: mixed series and issue states ─────────────────────
    let dir_a = tempfile::tempdir().unwrap();
    let lib_a = LibrarySeed::new(dir_a.path()).insert(&db).await;
    let live_series = SeriesSeed::new(lib_a, "Live Run").insert(&db).await;
    let gone_series = SeriesSeed::new(lib_a, "Gone Run").insert(&db).await;
    exec(
        &db,
        "UPDATE series SET removed_at = now() WHERE id = $1",
        vec![gone_series.into()],
    )
    .await;
    // Three active issues: 100 + 200 + 300 bytes.
    for (n, len) in [(1u8, 100usize), (2, 200), (3, 300)] {
        let path = dir_a.path().join(format!("Live Run {n:03}.cbz"));
        IssueSeed::new(lib_a, live_series, &path, &vec![n; len], f64::from(n))
            .insert(&db)
            .await;
    }
    // One removed (500 bytes) — out of the active count and the bytes.
    let removed_path = dir_a.path().join("Live Run 004.cbz");
    let removed_id = IssueSeed::new(lib_a, live_series, &removed_path, &[4u8; 500], 4.0)
        .insert(&db)
        .await;
    exec(
        &db,
        "UPDATE issues SET removed_at = now() WHERE id = $1",
        vec![removed_id.into()],
    )
    .await;
    // One malformed (50 bytes) — on disk but not readable: neither count.
    let bad_path = dir_a.path().join("Live Run 005.cbz");
    IssueSeed::new(lib_a, live_series, &bad_path, &[5u8; 50], 5.0)
        .with_state(entity::issue::IssueState::Malformed)
        .insert(&db)
        .await;
    // A backup on disk that no sweep has counted: the endpoint must not
    // walk the root to find it.
    std::fs::write(dir_a.path().join("Live Run 001.cbz.bak"), b"old bytes").unwrap();

    // ── Library B: one issue, a stored backup tally, a last scan ──────
    let dir_b = tempfile::tempdir().unwrap();
    let lib_b = LibrarySeed::new(dir_b.path()).insert(&db).await;
    let series_b = SeriesSeed::new(lib_b, "Other Run").insert(&db).await;
    let path_b = dir_b.path().join("Other Run 001.cbz");
    IssueSeed::new(lib_b, series_b, &path_b, &[9u8; 10], 1.0)
        .insert(&db)
        .await;
    let tally = Tally {
        file_count: 3,
        total_bytes: 4_096,
        oldest_modified: None,
        newest_modified: None,
    };
    backup_stats::store(&db, lib_b, &tally, chrono::Utc::now())
        .await
        .unwrap();
    exec(
        &db,
        "UPDATE libraries SET last_scan_at = now() WHERE id = $1",
        vec![lib_b.into()],
    )
    .await;

    // ── Library C: empty ─────────────────────────────────────────────
    let dir_c = tempfile::tempdir().unwrap();
    let lib_c = LibrarySeed::new(dir_c.path()).insert(&db).await;

    // ── all-libraries endpoint ───────────────────────────────────────
    let w = Window::open(rec);
    let (status, rows) = get(&app, &admin, "/api/admin/libraries/stats").await;
    let (all_count, all_sql) = w.close();
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows.as_array().unwrap().len(), 3, "{rows}");

    let a = row_for(&rows, lib_a);
    assert_eq!(a["slug"], lib_a.to_string());
    assert_eq!(a["series_count"], 1, "removed series excluded: {a}");
    assert_eq!(a["active_issue_count"], 3, "{a}");
    assert_eq!(a["removed_issue_count"], 1, "{a}");
    assert_eq!(a["archive_bytes"], 600, "active issues only: {a}");
    assert!(
        a["backup_bytes"].is_null() && a["backup_file_count"].is_null(),
        "uncounted library reports null, not a walk: {a}"
    );
    assert!(a["backup_counted_at"].is_null());
    assert_eq!(a["backup_stale"], false);
    assert!(a["last_scan_at"].is_null());

    let b = row_for(&rows, lib_b);
    assert_eq!(b["series_count"], 1);
    assert_eq!(b["active_issue_count"], 1);
    assert_eq!(b["removed_issue_count"], 0);
    assert_eq!(b["archive_bytes"], 10);
    assert_eq!(b["backup_bytes"], 4_096, "stored tally: {b}");
    assert_eq!(b["backup_file_count"], 3);
    assert!(b["backup_counted_at"].is_string());
    assert!(b["last_scan_at"].is_string());

    let c = row_for(&rows, lib_c);
    assert_eq!(c["series_count"], 0);
    assert_eq!(c["active_issue_count"], 0);
    assert_eq!(c["removed_issue_count"], 0);
    assert_eq!(c["archive_bytes"], 0);
    assert!(c["backup_bytes"].is_null());

    assert!(
        all_count > 0,
        "0 observed queries — sqlx statement logging is off on the app pool"
    );
    assert!(
        all_count <= MAX_QUERIES,
        "/admin/libraries/stats fired {all_count} queries (>{MAX_QUERIES}): {all_sql:#?}"
    );
    assert!(
        !all_sql.iter().any(|s| s
            .to_ascii_lowercase()
            .contains("insert into \"library_backup_stats\"")),
        "stats must not store a backup tally: {all_sql:#?}"
    );

    // The count does not grow with the number of libraries.
    // Issue ids are content hashes, so each extra file gets its own bytes.
    for i in 0..3u8 {
        let dir = tempfile::tempdir().unwrap();
        let lib = LibrarySeed::new(dir.path()).insert(&db).await;
        let s = SeriesSeed::new(lib, "Extra").insert(&db).await;
        let p = dir.path().join("Extra 001.cbz");
        IssueSeed::new(lib, s, &p, &[0xE0 + i; 8], 1.0)
            .insert(&db)
            .await;
    }
    let w = Window::open(rec);
    let (status, rows) = get(&app, &admin, "/api/admin/libraries/stats").await;
    let (more_count, more_sql) = w.close();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows.as_array().unwrap().len(), 6);
    assert_eq!(
        more_count, all_count,
        "six libraries must cost the same statements as three: {more_sql:#?}"
    );

    // ── one-library endpoint ─────────────────────────────────────────
    let w = Window::open(rec);
    let (status, one) = get(&app, &admin, &format!("/api/admin/libraries/{lib_a}/stats")).await;
    let (one_count, one_sql) = w.close();
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["active_issue_count"], 3);
    assert_eq!(one["removed_issue_count"], 1);
    assert_eq!(one["archive_bytes"], 600);
    assert_eq!(one["series_count"], 1);
    assert!(one["backup_bytes"].is_null());
    assert!(
        one_count > 0 && one_count <= MAX_QUERIES,
        "/admin/libraries/{{slug}}/stats fired {one_count} queries: {one_sql:#?}"
    );

    let (status, one_b) = get(&app, &admin, &format!("/api/admin/libraries/{lib_b}/stats")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one_b["backup_bytes"], 4_096);
    assert_eq!(one_b["backup_stale"], false);

    // A scan that changed the library flags the tally; the figures say so
    // and are still served from the row, not recounted.
    backup_stats::mark_stale(&db, lib_b).await;
    let (status, stale_b) = get(&app, &admin, &format!("/api/admin/libraries/{lib_b}/stats")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stale_b["backup_stale"], true, "{stale_b}");
    assert_eq!(stale_b["backup_bytes"], 4_096);
    assert_eq!(stale_b["backup_counted_at"], one_b["backup_counted_at"]);

    let (status, _) = get(&app, &admin, "/api/admin/libraries/no-such-library/stats").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // ── non-admin ────────────────────────────────────────────────────
    let (status, body) = get(&app, &reader, "/api/admin/libraries/stats").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]["code"].is_string(),
        "canonical envelope: {body}"
    );
    let (status, _) = get(
        &app,
        &reader,
        &format!("/api/admin/libraries/{lib_a}/stats"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
