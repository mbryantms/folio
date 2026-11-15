//! `GET /api/upcoming`, `/api/upcoming/summary`,
//! `/api/series/{slug}/next-release` and `/api/me/upcoming-this-week`
//! cost a fixed number of statements
//! (upcoming-releases 1.0, part D "Cost").
//!
//! One `#[tokio::test]` on purpose: the guard uses the same process-wide
//! `sqlx::query` counting layer as `library_stats.rs`, and a second test
//! in this binary would add its statements to the counter.
//!
//! The world is grown from 3 to 33 releases, each with its own linked
//! library series, watchlist row and pull-list line, and every endpoint
//! must cost exactly what it cost before. An upper bound is asserted too,
//! so a new per-request query has to be argued for.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{NaiveDate, Utc};
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use entity::{external_id, release_calendar_issue, release_calendar_series};
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{Value, json};
use server::upcoming::mylar::{self, MylarSnapshot};
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

// ───────── statement counter (same layer as library_stats.rs) ─────────

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

// ───────── helpers ─────────

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
        .find(|c| c.starts_with("__Host-comic_session="))
        .map(|c| c.split(';').next().unwrap().to_owned())
        .expect("session cookie")
}

async fn get(app: &TestApp, session: &str, path: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::COOKIE, session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every seeded release is out today: inside this week (the rail and the
/// summary count it) and not in the past (`next-release` lists it).
fn release_day() -> NaiveDate {
    Utc::now().date_naive()
}

/// One release `n`: a calendar row, a local series linked by ComicVine
/// id, and (returned) its watchlist and pull-list rows.
async fn seed_release(app: &TestApp, lib: Uuid, n: i64) -> (Value, Value, String) {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let cv = 90_000 + n;
    release_calendar_series::ActiveModel {
        metron_series_id: Set(n),
        name: Set(format!("Series {n}")),
        volume: Set(Some(1)),
        year_began: Set(Some(2024)),
        publisher: Set(Some(format!("Publisher {}", n % 4))),
        series_type: Set(None),
        cv_series_id: Set(Some(cv)),
        detail_fetched_at: Set(Some(now)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    release_calendar_issue::ActiveModel {
        metron_issue_id: Set(1000 + n),
        metron_series_id: Set(n),
        number: Set("5".to_owned()),
        store_date: Set(release_day()),
        cover_date: Set(None),
        image_url: Set(None),
        cover_hash: Set(None),
        metron_modified: Set(None),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    let series = SeriesSeed::new(lib, &format!("Series {n}"))
        .insert(db)
        .await;
    external_id::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series.to_string()),
        source: Set("comicvine".into()),
        external_id: Set(cv.to_string()),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    let slug = entity::series::Entity::find_by_id(series)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    (
        json!([cv, "Active", true]),
        json!({"ship_date": release_day().format("%Y-%m-%d").to_string(),
               "series": format!("Series {n}"), "issue": "5", "status": "Wanted",
               "cv_series_id": cv, "cv_issue_id": 500_000 + n, "series_year": 2024}),
        slug,
    )
}

async fn push(app: &TestApp, watch: &[Value], pull: &[Value]) {
    let snap: MylarSnapshot = serde_json::from_value(json!({
        "meta": {"schema_version": 1, "generated_at": "2026-11-15T03:36:49+00:00"},
        "pull_list": pull,
        "watchlist": watch,
    }))
    .unwrap();
    mylar::store(&app.state().db, &snap, Utc::now().fixed_offset(), None)
        .await
        .unwrap();
}

/// Session + grants + the loads described in `upcoming::view`. Lowering
/// these is fine; raising one needs a reason.
const MAX_LIST: u64 = 14;
const MAX_SUMMARY: u64 = 16;
const MAX_NEXT: u64 = 16;
const MAX_RAIL: u64 = 14;

use sea_orm::EntityTrait;

#[tokio::test]
async fn upcoming_endpoints_cost_a_fixed_number_of_statements() {
    let rec = install_recorder();
    let app = TestApp::spawn_with_query_logging().await;
    let admin = register(&app, "upcoming-admin@example.com").await;
    let mut cfg = (*app.state().cfg()).clone();
    cfg.upcoming_enabled = true;
    cfg.integrations_mylar_enabled = true;
    app.state().replace_cfg(cfg);

    let dir = tempfile::tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let (mut watch, mut pull, mut slug) = (Vec::new(), Vec::new(), String::new());
    for n in 1..=3 {
        let (w, p, s) = seed_release(&app, lib, n).await;
        watch.push(w);
        pull.push(p);
        slug = s;
    }
    push(&app, &watch, &pull).await;

    let list = "/api/upcoming?scope=all&limit=12";
    let summary = "/api/upcoming/summary";
    let next = format!("/api/series/{slug}/next-release");
    let rail = "/api/me/upcoming-this-week";
    let measure = |path: String| {
        let (app, admin) = (&app, &admin);
        async move {
            let w = Window::open(rec);
            let (status, body) = get(app, admin, &path).await;
            let (count, sql) = w.close();
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
            (count, sql, body)
        }
    };

    let (list_small, list_sql, body) = measure(list.to_owned()).await;
    assert_eq!(body["total"], 3);
    assert_eq!(body["items"][0]["entries"][0]["stage"], "wanted", "{body}");
    assert!(body["items"][0]["entries"][0]["library_series"].is_object());
    assert!(
        list_small <= MAX_LIST,
        "{list_small} statements: {list_sql:#?}"
    );
    let (summary_small, summary_sql, body) = measure(summary.to_owned()).await;
    assert_eq!(
        body["this_week"].as_u64().unwrap() + body["next_week"].as_u64().unwrap(),
        3
    );
    assert!(
        summary_small <= MAX_SUMMARY,
        "{summary_small} statements: {summary_sql:#?}"
    );
    let (next_small, next_sql, body) = measure(next.clone()).await;
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");
    assert!(
        next_small <= MAX_NEXT,
        "{next_small} statements: {next_sql:#?}"
    );

    let (rail_small, rail_sql, body) = measure(rail.to_owned()).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 3, "{body}");
    assert!(
        rail_small <= MAX_RAIL,
        "{rail_small} statements: {rail_sql:#?}"
    );

    // Ten times the releases, series, watch rows and pull lines.
    for n in 4..=33 {
        let (w, p, _) = seed_release(&app, lib, n).await;
        watch.push(w);
        pull.push(p);
    }
    push(&app, &watch, &pull).await;

    let (list_big, list_sql, body) = measure(list.to_owned()).await;
    assert_eq!(body["total"], 33);
    assert_eq!(
        list_big, list_small,
        "33 releases must cost what 3 did: {list_sql:#?}"
    );
    let (summary_big, summary_sql, _) = measure(summary.to_owned()).await;
    assert_eq!(summary_big, summary_small, "{summary_sql:#?}");
    let (next_big, next_sql, _) = measure(next).await;
    assert_eq!(next_big, next_small, "{next_sql:#?}");
    let (rail_big, rail_sql, body) = measure(rail.to_owned()).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 33, "{body}");
    assert_eq!(rail_big, rail_small, "{rail_sql:#?}");

    // The watchlist is asked about the ids on the page, never read whole.
    assert!(
        list_sql
            .iter()
            .filter(|s| s.contains("mylar_watch"))
            .all(|s| s.contains(" IN (") || s.contains("= ANY")),
        "{list_sql:#?}"
    );
}
