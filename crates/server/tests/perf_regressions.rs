//! Performance-regression guard (audit-remediation M5.5 + closing
//! follow-up, 2026-05-24).
//!
//! Seeds a small-but-realistic library — **10 series × 10 issues =
//! 100 issues, plus 5 CBLs** — registers a user, and hits the
//! ACL-walking endpoints flagged by the audit: `/me/on-deck`,
//! `/me/markers`, `/me/reading-log`, `/series`, `/admin/stats/users`.
//!
//! Two layers of assertion:
//!
//! 1. **Behavioural smoke** — each endpoint returns 200 with a
//!    well-formed body. Catches worst-case N+1 by 30s timeout.
//! 2. **Strict ≤N-queries** — a process-wide tracing-subscriber
//!    `Layer` counts every `sqlx::query` event (sqlx emits at INFO
//!    level for each statement). Each endpoint hit resets the
//!    counter, runs the request, and asserts the count is ≤ the
//!    documented baseline. Lowering a threshold is celebration —
//!    raising one needs a code-review justification.
//!
//! Caveats on the count-based assertion:
//!
//! - The subscriber is installed once per test process via a
//!   `OnceLock`. perf_regressions.rs has a single `#[tokio::test]`
//!   function so the global subscriber doesn't conflict with
//!   anything. Adding a second test in this file requires care.
//! - Sea-ORM defaults `sqlx_logging` to true at INFO; if a future
//!   refactor silences sqlx's per-query logging the counter goes
//!   to zero and the asserts here trivially pass — a separate
//!   "queries observed > 0" sanity check guards against that.
//! - Connection-pool setup queries (`SELECT 1` health pings) fire
//!   on the worker threads; we don't try to filter them out, the
//!   thresholds are calibrated against the observed real values.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_entry, cbl_list,
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, Set};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tower::ServiceExt;
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

/// Process-wide counter of sqlx::query events. Installed once via the
/// `QUERY_COUNTER` `OnceLock` on first test entry. Sea-ORM defers to
/// sqlx for actual query execution, and sqlx emits a tracing event
/// at INFO level on `target: "sqlx::query"` for every statement.
static QUERY_COUNTER: OnceLock<Arc<AtomicU64>> = OnceLock::new();

fn install_query_counter() -> Arc<AtomicU64> {
    QUERY_COUNTER
        .get_or_init(|| {
            let counter = Arc::new(AtomicU64::new(0));
            let layer = QueryCountingLayer {
                count: counter.clone(),
            };
            // EnvFilter at INFO lets sqlx::query through; setting it
            // explicitly so a developer's `RUST_LOG=warn` doesn't
            // silence the events the counter depends on.
            let filter = tracing_subscriber::EnvFilter::new("sqlx::query=info");
            tracing_subscriber::registry()
                .with(filter)
                .with(layer)
                .init();
            counter
        })
        .clone()
}

struct QueryCountingLayer {
    count: Arc<AtomicU64>,
}

impl<S> Layer<S> for QueryCountingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// RAII reset + read of the query counter — wraps a single endpoint
/// call. Reset is per-call so per-endpoint thresholds make sense.
struct QueryCount<'a> {
    counter: &'a AtomicU64,
    baseline: u64,
}

impl<'a> QueryCount<'a> {
    fn snapshot(counter: &'a AtomicU64) -> Self {
        let baseline = counter.load(Ordering::Relaxed);
        Self { counter, baseline }
    }
    fn taken(&self) -> u64 {
        self.counter.load(Ordering::Relaxed) - self.baseline
    }
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
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
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let extract = |needle: &str| -> String {
        cookies
            .iter()
            .find_map(|c| c.split(';').next()?.strip_prefix(needle).map(str::to_owned))
            .unwrap_or_default()
    };
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn get(
    app: &TestApp,
    auth: &Authed,
    path: &str,
) -> (StatusCode, serde_json::Value, Duration) {
    let started = Instant::now();
    let cookie = format!(
        "__Host-comic_session={}; __Host-comic_csrf={}",
        auth.session, auth.csrf
    );
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let elapsed = started.elapsed();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, body, elapsed)
}

async fn grant_access(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

/// Seed a single library with `series_count` series, each containing
/// `issues_per_series` issues. Issue ids are content-hash-shaped
/// (64-hex) so they pass the BLAKE3-id checks.
async fn seed_library(
    app: &TestApp,
    label: &str,
    series_count: usize,
    issues_per_series: usize,
) -> (Uuid, Vec<(Uuid, Vec<String>)>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();

    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {label}")),
        root_path: Set(format!("/tmp/perf-{label}-{lib_id}")),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(lib_id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(false),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
    }
    .insert(&db)
    .await
    .unwrap();

    let mut series_with_issues: Vec<(Uuid, Vec<String>)> = Vec::with_capacity(series_count);
    for s in 0..series_count {
        let series_id = Uuid::now_v7();
        let name = format!("{label} Series {s}");
        SeriesAM {
            id: Set(series_id),
            library_id: Set(lib_id),
            name: Set(name.clone()),
            normalized_name: Set(normalize_name(&name)),
            year: Set(Some(2020 + (s as i32 % 5))),
            volume: Set(None),
            publisher: Set(None),
            imprint: Set(None),
            status: Set("continuing".into()),
            total_issues: Set(None),
            age_rating: Set(None),
            summary: Set(None),
            language_code: Set("en".into()),
            sort_name: Set(None),
            year_end: Set(None),
            series_type: Set(None),
            aliases: Set(serde_json::json!([])),
            deck: Set(None),
            publisher_id: Set(None),
            imprint_id: Set(None),
            last_metadata_sync_at: Set(None),
            metadata_sync_paused: Set(false),
            series_group: Set(None),
            slug: Set(format!("{label}-{s}")),
            alternate_names: Set(serde_json::json!([])),
            created_at: Set(now),
            updated_at: Set(now),
            folder_path: Set(None),
            last_scanned_at: Set(None),
            match_key: Set(None),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            status_user_set_at: Set(None),
            reading_direction: Set(None),
            preserve_canonical_order: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();

        let mut issue_ids: Vec<String> = Vec::with_capacity(issues_per_series);
        for n in 0..issues_per_series {
            // Each issue id is a 64-hex content-hash; pack series-prefix
            // and a per-series sequence so ids stay unique.
            let suffix = (s * issues_per_series + n) as u32;
            let issue_id = format!("{:0>56}{:08x}", series_id.simple(), suffix);
            IssueAM {
                id: Set(issue_id.clone()),
                library_id: Set(lib_id),
                series_id: Set(series_id),
                slug: Set(format!("issue-{suffix}")),
                file_path: Set(format!("/tmp/perf-{label}/{s}/{n}.cbz")),
                file_size: Set(1),
                file_mtime: Set(now),
                state: Set("active".into()),
                content_hash: Set(issue_id.clone()),
                title: Set(Some(format!("Issue {n}"))),
                sort_number: Set(Some((n + 1) as f64)),
                number_raw: Set(Some(format!("{}", n + 1))),
                volume: Set(None),
                year: Set(Some(2020 + (s as i32 % 5))),
                month: Set(None),
                day: Set(None),
                summary: Set(None),
                notes: Set(None),
                language_code: Set(None),
                format: Set(None),
                black_and_white: Set(None),
                manga: Set(None),
                age_rating: Set(None),
                page_count: Set(Some(20)),
                pages: Set(serde_json::json!([])),
                comic_info_raw: Set(serde_json::json!({})),
                alternate_series: Set(None),
                story_arc: Set(None),
                story_arc_number: Set(None),
                characters: Set(None),
                teams: Set(None),
                locations: Set(None),
                tags: Set(None),
                genre: Set(None),
                writer: Set(None),
                penciller: Set(None),
                inker: Set(None),
                colorist: Set(None),
                letterer: Set(None),
                cover_artist: Set(None),
                editor: Set(None),
                translator: Set(None),
                publisher: Set(None),
                imprint: Set(None),
                scan_information: Set(None),
                community_rating: Set(None),
                review: Set(None),
                web_url: Set(None),
                deck: Set(None),
                store_date: Set(None),
                foc_date: Set(None),
                price: Set(None),
                sku: Set(None),
                staff_rating: Set(None),
                aliases: Set(serde_json::json!([])),
                last_metadata_sync_at: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                removed_at: Set(None),
                removal_confirmed_at: Set(None),
                superseded_by: Set(None),
                special_type: Set(None),
                hash_algorithm: Set(0),
                thumbnails_generated_at: Set(None),
                thumbnail_version: Set(0),
                thumbnails_error: Set(None),
                additional_links: Set(serde_json::json!([])),
                user_edited: Set(serde_json::json!([])),
                comicinfo_count: Set(Some(0)),
                last_rewrite_at: Set(None),
                last_rewrite_kind: Set(None),
            }
            .insert(&db)
            .await
            .unwrap();
            issue_ids.push(issue_id);
        }
        series_with_issues.push((series_id, issue_ids));
    }

    (lib_id, series_with_issues)
}

/// Seed `cbl_count` CBL lists, each holding 3 issues sampled from the
/// seeded series.
async fn seed_cbls(app: &TestApp, series_with_issues: &[(Uuid, Vec<String>)], cbl_count: usize) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    for i in 0..cbl_count {
        let list_id = Uuid::now_v7();
        let mut raw_sha = vec![0u8; 32];
        raw_sha[0] = i as u8;
        cbl_list::ActiveModel {
            id: Set(list_id),
            owner_user_id: Set(None),
            source_kind: Set("upload".into()),
            source_url: Set(None),
            catalog_source_id: Set(None),
            catalog_path: Set(None),
            github_blob_sha: Set(None),
            source_etag: Set(None),
            source_last_modified: Set(None),
            raw_sha256: Set(raw_sha),
            raw_xml: Set("<ReadingList />".into()),
            parsed_name: Set(format!("CBL {i}")),
            parsed_matchers_present: Set(false),
            num_issues_declared: Set(Some(3)),
            description: Set(None),
            imported_at: Set(now),
            last_refreshed_at: Set(None),
            last_match_run_at: Set(None),
            refresh_schedule: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            preserve_canonical_order: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();
        let series_idx = i % series_with_issues.len();
        let (_series_id, issue_ids) = &series_with_issues[series_idx];
        for (pos, issue_id) in issue_ids.iter().take(3).enumerate() {
            cbl_entry::ActiveModel {
                id: Set(Uuid::now_v7()),
                cbl_list_id: Set(list_id),
                position: Set(pos as i32),
                series_name: Set(format!("CBL {i} series")),
                issue_number: Set(format!("{}", pos + 1)),
                volume: Set(None),
                year: Set(None),
                cv_series_id: Set(None),
                cv_issue_id: Set(None),
                metron_series_id: Set(None),
                metron_issue_id: Set(None),
                matched_issue_id: Set(Some(issue_id.clone())),
                match_status: Set("matched".into()),
                match_method: Set(Some("test".into())),
                match_confidence: Set(Some(1.0)),
                ambiguous_candidates: Set(None),
                user_resolved_at: Set(None),
                matched_at: Set(Some(now)),
            }
            .insert(&db)
            .await
            .unwrap();
        }
    }
}

/// Mark a couple of issues as finished so /me/on-deck has signal.
async fn seed_progress(app: &TestApp, user_id: Uuid, issue_ids: &[String]) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    for (i, issue_id) in issue_ids.iter().take(3).enumerate() {
        progress_record::ActiveModel {
            user_id: Set(user_id),
            issue_id: Set(issue_id.clone()),
            last_page: Set(19),
            percent: Set(1.0),
            finished: Set(true),
            finished_at: Set(Some(now)),
            updated_at: Set(now - chrono::Duration::seconds(i as i64)),
            device: Set(None),
            is_backfill: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();
    }
}

const TIMEOUT: Duration = Duration::from_secs(30);

fn assert_quick(elapsed: Duration, path: &str) {
    assert!(
        elapsed < TIMEOUT,
        "{path} took {elapsed:?} (>{TIMEOUT:?}); likely a perf regression",
    );
}

/// Strict per-endpoint upper bounds on query count, calibrated from
/// the actual observed values (recorded in the comments below) plus
/// a buffer for ACL / cookie / auth-session lookups. Lowering a
/// threshold is celebration — raising one needs a code-review
/// justification.
///
/// These bounds catch *severe* regressions (N+1s, accidental
/// row-by-row fetches). Per-issue N+1s in a 100-issue seed would
/// add ~100 queries; per-series N+1s would add ~10. The thresholds
/// are tuned so either category trips the test immediately while
/// leaving room for legitimate one-off additions (a new ACL check,
/// a new metadata sub-query).
const MAX_QUERIES_ON_DECK: u64 = 50; // observed ≈ 23
const MAX_QUERIES_MARKERS: u64 = 10; // observed ≈ 2
const MAX_QUERIES_READING_LOG: u64 = 30; // observed ≈ 9
const MAX_QUERIES_SERIES: u64 = 20; // observed ≈ 5
const MAX_QUERIES_ADMIN_STATS: u64 = 20; // observed ≈ 6

#[tokio::test]
async fn realistic_dataset_endpoints_respond_correctly() {
    let counter = install_query_counter();
    let app = TestApp::spawn().await;
    let user = register(&app, "perf@example.com").await;

    // 10 series × 10 issues = 100 issues, 5 CBLs.
    let (lib_id, series_with_issues) = seed_library(&app, "perf", 10, 10).await;
    grant_access(&app, user.user_id, lib_id).await;
    seed_cbls(&app, &series_with_issues, 5).await;
    let finished_issues: Vec<String> = series_with_issues
        .iter()
        .flat_map(|(_, ids)| ids.iter().take(1).cloned())
        .collect();
    seed_progress(&app, user.user_id, &finished_issues).await;

    // Sanity check the counter is wired before we trust any per-call
    // assertion: by this point we've executed dozens of seed inserts
    // and the counter should have moved off zero.
    assert!(
        counter.load(Ordering::Relaxed) > 0,
        "sqlx::query events not observed — counter wiring is broken; \
         downstream ≤N-queries assertions would silently pass"
    );

    // ── /me/on-deck — the rails query that M5.1 + M5.2 batched.
    let snap = QueryCount::snapshot(&counter);
    let (status, body, elapsed) = get(&app, &user, "/api/me/on-deck").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array(), "on-deck items missing");
    assert_quick(elapsed, "/api/me/on-deck");
    assert_query_count(snap.taken(), MAX_QUERIES_ON_DECK, "/api/me/on-deck");

    // ── /me/markers — paginated; should walk past the first page.
    let snap = QueryCount::snapshot(&counter);
    let (status, body, elapsed) = get(&app, &user, "/api/me/markers?limit=50").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array(), "markers items missing");
    assert_quick(elapsed, "/api/me/markers");
    assert_query_count(snap.taken(), MAX_QUERIES_MARKERS, "/api/me/markers");

    // ── /me/reading-log — hydrates issues + series in one batch each.
    // Note: this endpoint uses `events`, not the uniform `items`
    // envelope — it predates the M4 envelope refactor and the audit
    // didn't include it in M4's migration list.
    let snap = QueryCount::snapshot(&counter);
    let (status, body, elapsed) = get(&app, &user, "/api/me/reading-log?limit=50").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["events"].is_array(), "reading-log events missing");
    assert_quick(elapsed, "/api/me/reading-log");
    assert_query_count(snap.taken(), MAX_QUERIES_READING_LOG, "/api/me/reading-log");

    // ── /series — `hydrate_series` lifts issue counts + covers in batches.
    let snap = QueryCount::snapshot(&counter);
    let (status, body, elapsed) = get(&app, &user, "/api/series?limit=60").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array(), "series items missing");
    let series_items = body["items"].as_array().unwrap();
    assert!(
        !series_items.is_empty(),
        "/api/series returned empty over a 100-issue library",
    );
    // Spot-check that hydrate_series populated counts.
    for s in series_items {
        let count = s["issue_count"].as_i64().unwrap_or(0);
        assert!(count > 0, "series row missing issue_count: {s}");
    }
    assert_quick(elapsed, "/api/series");
    assert_query_count(snap.taken(), MAX_QUERIES_SERIES, "/api/series");

    // ── /admin/stats/users — admin-only; the first registered user is
    // admin by Folio's bootstrap rule.
    let snap = QueryCount::snapshot(&counter);
    let (status, body, elapsed) = get(&app, &user, "/api/admin/stats/users").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array(), "admin stats items missing");
    assert_quick(elapsed, "/api/admin/stats/users");
    assert_query_count(
        snap.taken(),
        MAX_QUERIES_ADMIN_STATS,
        "/api/admin/stats/users",
    );
}

fn assert_query_count(observed: u64, max: u64, path: &str) {
    assert!(
        observed <= max,
        "{path} fired {observed} queries (>{max}); investigate for N+1 regressions. \
         Lower the threshold in perf_regressions.rs if the new value is a deliberate \
         improvement.",
    );
}
