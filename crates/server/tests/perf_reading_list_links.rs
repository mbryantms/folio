//! Query-count guard for `GET /me/cbl-lists/{id}/links` (linked reading
//! lists, PR 2a): the view costs a fixed number of statements however many
//! events the list has — every event's progress comes from one grouped
//! read, not a query per event.
//!
//! Same counting layer as `perf_duplicates_health.rs`; one test per file
//! because the subscriber is process-wide.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{CblListSeed, IssueSeed, LibrarySeed, SeriesSeed, seed_cbl_entry};
use entity::reading_list_link::{self, AnchorStatus, LinkKind, LinkPlacement};
use sea_orm::{ActiveModelTrait, Set};
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
    user_id: Uuid,
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
                    r#"{"email":"perf-links@example.com","password":"correctly-horse-battery"}"#,
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed { session, user_id }
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

async fn link(db: &sea_orm::DatabaseConnection, parent: Uuid, child: Uuid, after: i32) {
    let now = Utc::now().fixed_offset();
    reading_list_link::ActiveModel {
        id: Set(Uuid::now_v7()),
        parent_list_id: Set(parent),
        child_list_id: Set(child),
        kind: Set(LinkKind::Event),
        anchor_cv_issue_id: Set(None),
        anchor_series: Set(Some("Seed".into())),
        anchor_number: Set(Some(after.to_string())),
        anchor_volume: Set(None),
        anchor_year: Set(None),
        anchor_position_hint: Set(Some(after)),
        anchor_status: Set(AnchorStatus::Placed),
        placement: Set(LinkPlacement::Manual),
        sort_within_anchor: Set(0),
        created_by: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn links_view_costs_the_same_with_one_event_or_five() {
    let rec = install_recorder();
    let app = TestApp::spawn_with_query_logging().await;
    let auth = register_admin(&app).await;
    let db = app.state().db.clone();

    let dir = tempfile::tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Perf Links").insert(&db).await;
    let mut issues = Vec::new();
    for n in 0..12u8 {
        let path = dir.path().join(format!("Perf Links {n:03}.cbz"));
        let payload = vec![n, 0xEE];
        issues.push(
            IssueSeed::new(lib_id, series_id, &path, &payload, f64::from(n))
                .insert(&db)
                .await,
        );
    }
    let master = CblListSeed::new(auth.user_id, "Master").insert(&db).await;
    for (i, id) in issues[..6].iter().enumerate() {
        seed_cbl_entry(&db, master, i as i32, Some(id)).await;
    }
    let mut events = Vec::new();
    for e in 0..5 {
        let ev = CblListSeed::new(auth.user_id, "Event").insert(&db).await;
        seed_cbl_entry(&db, ev, 0, Some(&issues[6 + e])).await;
        events.push(ev);
    }
    let path = format!("/api/me/cbl-lists/{master}/links");

    link(&db, master, events[0], 1).await;
    let _ = get(&app, &auth, &path).await; // warm per-process caches
    let w = Window::open(rec);
    let one = get(&app, &auth, &path).await;
    let (with_one, _) = w.close();
    assert_eq!(one["events"].as_array().unwrap().len(), 1);

    for (i, ev) in events.iter().enumerate().skip(1) {
        link(&db, master, *ev, i as i32).await;
    }
    let w = Window::open(rec);
    let five = get(&app, &auth, &path).await;
    let (with_five, texts) = w.close();
    assert_eq!(five["events"].as_array().unwrap().len(), 5);
    assert_eq!(five["events"][4]["in_library"], 1);
    assert_eq!(
        with_one, with_five,
        "the links view must not query per event; statements: {texts:#?}"
    );
}
