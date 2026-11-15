//! Release calendar (upcoming-releases 1.0, part C): the Metron list
//! pass, the enrich sweep and the admin endpoints, against a wiremock
//! Metron.
//!
//! Covers:
//! - a disabled calendar sends nothing;
//! - a pass lists every week, pages, dedupes on the issue id and upserts;
//! - the budget gate stops a pass, which then resumes at the same week
//!   without listing the earlier ones again;
//! - a complete pass removes what Metron no longer lists, keeps a
//!   release that moved, and prunes long-past rows;
//! - a hard failure records a backoff; a quota refusal only pauses;
//! - the enrich sweep fills publisher + ComicVine id and re-asks later
//!   for a series that had none;
//! - `/admin/upcoming/*` is admin-only and `sync` is audited.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, NaiveDate, Utc};
use common::TestApp;
use entity::{release_calendar_issue, release_calendar_series, release_calendar_state};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::jobs::release_calendar as job;
use server::metadata::rate_limit;
use server::upcoming::calendar::{self, EnrichOutcome, PassOutcome, Pause};
use server::upcoming::{self, Window};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

// ────────────────────── harness ──────────────────────

async fn spawn(mock: &MockServer) -> TestApp {
    TestApp::spawn_with_metron_and_gcd(mock.uri(), mock.uri()).await
}

fn enable(app: &TestApp) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.upcoming_enabled = true;
    app.state().replace_cfg(cfg);
}

fn todays_window() -> Window {
    upcoming::window(Utc::now().date_naive(), 8)
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn issue_row(id: i64, series: (i64, &str), number: &str, store_date: NaiveDate) -> Value {
    json!({
        "id": id,
        "series": {"id": series.0, "name": series.1, "volume": 1, "year_began": 2024},
        "number": number,
        "issue": format!("{} #{number}", series.1),
        "cover_date": "2026-12-01",
        "store_date": iso(store_date),
        "image": format!("https://static.metron.cloud/media/issue/{id}.jpg"),
        "cover_hash": "c3d4",
        "modified": "2026-11-01T10:00:00Z"
    })
}

fn page(rows: Vec<Value>, next: bool) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "count": rows.len(),
        "next": if next { json!("https://metron.cloud/api/issue/?page=2") } else { Value::Null },
        "previous": null,
        "results": rows,
    }))
}

/// Every week not given its own mock answers with an empty page.
async fn empty_weeks(mock: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .respond_with(page(vec![], false))
        .mount(mock)
        .await;
}

fn week_mock(monday: NaiveDate) -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("store_date_range_after", iso(monday)))
        .and(query_param(
            "store_date_range_before",
            iso(monday + Duration::days(6)),
        ))
}

async fn refill_minute(app: &TestApp) {
    let mut conn = app.state().jobs.redis.clone();
    rate_limit::refill(&mut conn, &rate_limit::METRON_MIN)
        .await
        .unwrap();
}

async fn spend_minute(app: &TestApp, n: u32) {
    let mut conn = app.state().jobs.redis.clone();
    for _ in 0..n {
        rate_limit::reserve(&mut conn, &rate_limit::METRON_MIN)
            .await
            .unwrap();
    }
}

async fn state_row(app: &TestApp) -> release_calendar_state::Model {
    calendar::load_state(&app.state().db).await.unwrap()
}

async fn issue(app: &TestApp, id: i64) -> Option<release_calendar_issue::Model> {
    release_calendar_issue::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
}

async fn series(app: &TestApp, id: i64) -> Option<release_calendar_series::Model> {
    release_calendar_series::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
}

async fn seed_issue(app: &TestApp, id: i64, series_id: i64, store_date: NaiveDate) {
    let db = &app.state().db;
    let then = (Utc::now() - Duration::days(1)).fixed_offset();
    if release_calendar_series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .is_none()
    {
        release_calendar_series::ActiveModel {
            metron_series_id: Set(series_id),
            name: Set(format!("Series {series_id}")),
            created_at: Set(then),
            updated_at: Set(then),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
    }
    release_calendar_issue::ActiveModel {
        metron_issue_id: Set(id),
        metron_series_id: Set(series_id),
        number: Set("1".into()),
        store_date: Set(store_date),
        cover_date: Set(None),
        image_url: Set(None),
        cover_hash: Set(None),
        metron_modified: Set(None),
        first_seen_at: Set(then),
        last_seen_at: Set(then),
    }
    .insert(db)
    .await
    .unwrap();
}

// ────────────────────── list pass ──────────────────────

#[tokio::test]
async fn a_disabled_calendar_asks_metron_nothing() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(page(vec![], false))
        .expect(0)
        .mount(&mock)
        .await;
    let app = spawn(&mock).await;

    assert_eq!(
        calendar::list_pass(&app.state()).await.unwrap(),
        PassOutcome::Disabled
    );
    assert_eq!(
        calendar::enrich(&app.state()).await.unwrap(),
        EnrichOutcome::default()
    );
    assert!(!calendar::ready(&app.state()));
}

#[tokio::test]
async fn a_pass_lists_every_week_pages_and_dedupes() {
    let mock = MockServer::start().await;
    let win = todays_window();
    let weeks: Vec<NaiveDate> = win.weeks().collect();
    let this_week = weeks[2];
    let next_week = weeks[3];

    // This week: two issues of one series on one page.
    week_mock(this_week)
        .respond_with(page(
            vec![
                issue_row(1001, (50, "Saga"), "70", this_week + Duration::days(2)),
                issue_row(1002, (50, "Saga"), "71", this_week + Duration::days(2)),
            ],
            false,
        ))
        .with_priority(1)
        .expect(1)
        .mount(&mock)
        .await;
    // Next week: two pages; page 2 repeats an id from page 1.
    week_mock(next_week)
        .and(query_param("page", "1"))
        .respond_with(page(
            vec![issue_row(
                2001,
                (51, "Absolute Batman"),
                "15",
                next_week + Duration::days(2),
            )],
            true,
        ))
        .with_priority(1)
        .expect(1)
        .mount(&mock)
        .await;
    week_mock(next_week)
        .and(query_param("page", "2"))
        .respond_with(page(
            vec![
                issue_row(
                    2001,
                    (51, "Absolute Batman"),
                    "15",
                    next_week + Duration::days(2),
                ),
                issue_row(
                    2002,
                    (52, "Ultimates"),
                    "1.MU",
                    next_week + Duration::days(2),
                ),
                // No store date: the calendar cannot place it.
                json!({"id": 2003, "series": {"id": 52, "name": "Ultimates"}, "number": "9"}),
            ],
            false,
        ))
        .with_priority(1)
        .expect(1)
        .mount(&mock)
        .await;
    empty_weeks(&mock).await;

    let app = spawn(&mock).await;
    enable(&app);

    let outcome = calendar::list_pass(&app.state()).await.unwrap();
    assert_eq!(
        outcome,
        PassOutcome::Finished {
            weeks: 11,
            issues: 4,
            removed: 0,
            // Eleven weeks plus the second page.
            requests: 12,
        }
    );

    let saga = issue(&app, 1001).await.expect("stored");
    assert_eq!(saga.metron_series_id, 50);
    assert_eq!(saga.number, "70");
    assert_eq!(saga.store_date, this_week + Duration::days(2));
    assert_eq!(
        saga.image_url.as_deref(),
        Some("https://static.metron.cloud/media/issue/1001.jpg")
    );
    assert_eq!(issue(&app, 2002).await.unwrap().number, "1.MU");
    assert!(issue(&app, 2003).await.is_none());
    let s = series(&app, 51).await.unwrap();
    assert_eq!(s.name, "Absolute Batman");
    assert_eq!(s.year_began, Some(2024));
    assert!(s.detail_fetched_at.is_none());

    let st = state_row(&app).await;
    assert!(st.last_finished_at.is_some());
    assert!(st.pass_next_week.is_none());
    assert!(st.last_error.is_none());
    assert!(!calendar::pass_due(&st, Utc::now().fixed_offset()));
}

#[tokio::test]
async fn the_budget_gate_stops_a_pass_and_it_resumes_at_the_same_week() {
    let mock = MockServer::start().await;
    let win = todays_window();
    let weeks: Vec<NaiveDate> = win.weeks().collect();
    // The first two weeks may be listed exactly once across both calls.
    for monday in &weeks[..2] {
        week_mock(*monday)
            .respond_with(page(vec![], false))
            .with_priority(1)
            .expect(1)
            .mount(&mock)
            .await;
    }
    empty_weeks(&mock).await;
    let app = spawn(&mock).await;
    enable(&app);

    // Ten of twenty spent: two more requests reach the reserve of eight.
    spend_minute(&app, 10).await;
    let first = calendar::list_pass(&app.state()).await.unwrap();
    assert_eq!(
        first,
        PassOutcome::Paused {
            reason: Pause::BudgetReserve,
            next_week: weeks[2],
            requests: 2,
        }
    );
    let paused = state_row(&app).await;
    assert_eq!(paused.pass_next_week, Some(weeks[2]));
    assert!(paused.last_finished_at.is_none());
    assert!(paused.next_attempt_at.is_none(), "a pause is not a failure");
    assert!(calendar::pass_due(&paused, Utc::now().fixed_offset()));

    refill_minute(&app).await;
    let second = calendar::list_pass(&app.state()).await.unwrap();
    assert_eq!(
        second,
        PassOutcome::Finished {
            weeks: 9,
            issues: 0,
            removed: 0,
            requests: 9,
        }
    );
    let done = state_row(&app).await;
    assert_eq!(
        done.pass_started_at, paused.pass_started_at,
        "the resumed pass is the same pass"
    );
    assert!(done.pass_next_week.is_none());
}

#[tokio::test]
async fn a_complete_pass_removes_unlisted_rows_keeps_moved_ones_and_prunes() {
    let mock = MockServer::start().await;
    let win = todays_window();
    let weeks: Vec<NaiveDate> = win.weeks().collect();
    let (week_a, week_b) = (weeks[3], weeks[5]);

    let app = spawn(&mock).await;
    enable(&app);
    // 1: listed last time, gone now. 2: listed in week A last time, in
    // week B now. 3: far in the past.
    seed_issue(&app, 1, 10, week_a + Duration::days(2)).await;
    seed_issue(&app, 2, 11, week_a + Duration::days(2)).await;
    seed_issue(&app, 3, 12, win.start - Duration::days(60)).await;

    week_mock(week_b)
        .respond_with(page(
            vec![issue_row(2, (11, "Moved"), "1", week_b + Duration::days(2))],
            false,
        ))
        .with_priority(1)
        .mount(&mock)
        .await;
    empty_weeks(&mock).await;

    let outcome = calendar::list_pass(&app.state()).await.unwrap();
    assert!(
        matches!(outcome, PassOutcome::Finished { removed: 1, .. }),
        "{outcome:?}"
    );
    assert!(issue(&app, 1).await.is_none(), "no longer listed");
    assert_eq!(
        issue(&app, 2).await.expect("moved, not removed").store_date,
        week_b + Duration::days(2)
    );
    assert!(issue(&app, 3).await.is_none(), "pruned");
    assert!(series(&app, 10).await.is_none(), "orphan series removed");
    assert!(series(&app, 12).await.is_none());
    assert_eq!(series(&app, 11).await.unwrap().name, "Moved");
}

#[tokio::test]
async fn a_hard_failure_sets_a_backoff_and_removes_nothing() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
        .expect(1)
        .mount(&mock)
        .await;
    let app = spawn(&mock).await;
    enable(&app);
    let win = todays_window();
    seed_issue(&app, 1, 10, win.start + Duration::days(16)).await;

    let outcome = calendar::list_pass(&app.state()).await.unwrap();
    assert!(matches!(outcome, PassOutcome::Failed { .. }), "{outcome:?}");
    assert!(issue(&app, 1).await.is_some());

    let st = state_row(&app).await;
    assert!(st.last_error.is_some());
    let until = st.next_attempt_at.expect("backoff set");
    assert!(until > Utc::now().fixed_offset());
    assert!(!calendar::pass_due(&st, Utc::now().fixed_offset()));

    // A second call inside the backoff does not reach Metron (`expect(1)`).
    assert_eq!(
        calendar::list_pass(&app.state()).await.unwrap(),
        PassOutcome::BackingOff { until }
    );

    calendar::clear_backoff(&app.state().db).await.unwrap();
    assert!(calendar::pass_due(
        &state_row(&app).await,
        Utc::now().fixed_offset()
    ));
}

#[tokio::test]
async fn a_quota_refusal_pauses_without_a_backoff() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
        .mount(&mock)
        .await;
    let app = spawn(&mock).await;
    enable(&app);

    let outcome = calendar::list_pass(&app.state()).await.unwrap();
    assert_eq!(
        outcome,
        PassOutcome::Paused {
            reason: Pause::Quota,
            next_week: todays_window().start,
            requests: 0,
        }
    );
    let st = state_row(&app).await;
    assert!(st.next_attempt_at.is_none());
    assert!(st.last_error.is_none());
}

// ────────────────────── enrich sweep ──────────────────────

fn series_detail(id: i64, name: &str, cv_id: Option<i64>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": id,
        "name": name,
        "sort_name": name,
        "volume": 1,
        "series_type": {"id": 13, "name": "Single Issue"},
        "publisher": {"id": 3, "name": "Image Comics"},
        "year_began": 2024,
        "issue_count": 3,
        "cv_id": cv_id,
        "gcd_id": null,
        "resource_url": format!("https://metron.cloud/series/{id}/"),
        "modified": "2026-11-01T10:00:00Z"
    }))
}

#[tokio::test]
async fn enrich_fills_publisher_and_comicvine_id_and_asks_again_later() {
    let mock = MockServer::start().await;
    let app = spawn(&mock).await;
    enable(&app);
    let soon = Utc::now().date_naive() + Duration::days(9);
    seed_issue(&app, 1, 50, soon).await;
    seed_issue(&app, 2, 51, soon).await;

    Mock::given(method("GET"))
        .and(path("/api/series/50/"))
        .respond_with(series_detail(50, "Saga", Some(4050)))
        .expect(1)
        .mount(&mock)
        .await;
    // First answer for 51: Metron has not linked it to ComicVine yet.
    let unlinked = Mock::given(method("GET"))
        .and(path("/api/series/51/"))
        .respond_with(series_detail(51, "New Number One", None))
        .expect(1)
        .mount_as_scoped(&mock)
        .await;

    let first = calendar::enrich(&app.state()).await.unwrap();
    assert_eq!(
        first,
        EnrichOutcome {
            fetched: 2,
            paused: false
        }
    );
    let saga = series(&app, 50).await.unwrap();
    assert_eq!(saga.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(saga.series_type.as_deref(), Some("Single Issue"));
    assert_eq!(saga.cv_series_id, Some(4050));
    assert!(saga.detail_fetched_at.is_some());
    let new_one = series(&app, 51).await.unwrap();
    assert_eq!(new_one.cv_series_id, None);
    assert!(new_one.detail_fetched_at.is_some());

    // Nothing is due: 50 is done and 51 was asked a moment ago.
    assert_eq!(
        calendar::enrich(&app.state()).await.unwrap(),
        EnrichOutcome::default()
    );
    drop(unlinked);

    // Four days on, 51 is asked again, past the cached answer.
    let mut aged: release_calendar_series::ActiveModel = new_one.into();
    aged.detail_fetched_at = Set(Some((Utc::now() - Duration::days(4)).fixed_offset()));
    aged.update(&app.state().db).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/series/51/"))
        .respond_with(series_detail(51, "New Number One", Some(9051)))
        .expect(1)
        .mount(&mock)
        .await;
    assert_eq!(calendar::enrich(&app.state()).await.unwrap().fetched, 1);
    assert_eq!(series(&app, 51).await.unwrap().cv_series_id, Some(9051));
}

/// A fresh calendar holds a few hundred series and a tick fetches ten:
/// the ones releasing soonest go first, whatever their ids.
#[tokio::test]
async fn enrich_takes_the_soonest_releases_first() {
    let mock = MockServer::start().await;
    let app = spawn(&mock).await;
    enable(&app);
    let today = Utc::now().date_naive();
    // Twelve series; the higher the id, the sooner the release.
    for n in 0..12i64 {
        seed_issue(&app, 100 + n, 60 + n, today + Duration::days(40 - 3 * n)).await;
    }
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(r"^/api/series/\d+/$"))
        .respond_with(series_detail(1, "Any", Some(1)))
        .mount(&mock)
        .await;

    let tick = calendar::enrich(&app.state()).await.unwrap();
    assert_eq!(u64::from(tick.fetched), calendar::ENRICH_PER_TICK);
    for id in 62..=71 {
        assert!(
            series(&app, id).await.unwrap().detail_fetched_at.is_some(),
            "series {id} releases among the ten soonest"
        );
    }
    for id in [60, 61] {
        assert!(
            series(&app, id).await.unwrap().detail_fetched_at.is_none(),
            "series {id} releases last and waits for the next tick"
        );
    }
}

#[tokio::test]
async fn enrich_stops_at_the_budget_reserve() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(series_detail(50, "Saga", Some(4050)))
        .expect(0)
        .mount(&mock)
        .await;
    let app = spawn(&mock).await;
    enable(&app);
    seed_issue(&app, 1, 50, Utc::now().date_naive() + Duration::days(9)).await;

    spend_minute(&app, 12).await;
    assert_eq!(
        calendar::enrich(&app.state()).await.unwrap(),
        EnrichOutcome {
            fetched: 0,
            paused: true
        }
    );
    assert!(series(&app, 50).await.unwrap().detail_fetched_at.is_none());
}

#[tokio::test]
async fn the_sweep_queues_one_list_pass_when_one_is_due() {
    let mock = MockServer::start().await;
    empty_weeks(&mock).await;
    let app = spawn(&mock).await;
    let clear_queued = async || {
        let mut conn = app.state().jobs.redis.clone();
        let _: () = redis::AsyncCommands::del(&mut conn, "release_calendar:sync:queued")
            .await
            .unwrap();
    };

    // Off: the sweep queues nothing, so this trigger is the first.
    job::sweep(&app.state()).await;
    assert!(job::enqueue(&app.state(), "test").await);
    // One pass queued at a time: a second trigger adds nothing.
    assert!(!job::enqueue(&app.state(), "test").await);
    clear_queued().await;

    // On, and no pass has ever finished: the sweep queues the due one.
    enable(&app);
    job::sweep(&app.state()).await;
    assert!(
        !job::enqueue(&app.state(), "test").await,
        "the sweep already queued the due pass"
    );

    // The worker clears the key and runs the pass; after that nothing
    // is due, so the next sweep queues nothing.
    clear_queued().await;
    assert!(matches!(
        job::process(&app.state(), "test").await,
        Some(PassOutcome::Finished { .. })
    ));
    job::sweep(&app.state()).await;
    assert!(job::enqueue(&app.state(), "test").await);
}

#[tokio::test]
async fn the_sweep_leaves_the_budget_to_a_stopped_list_pass() {
    let mock = MockServer::start().await;
    // No series detail may be asked for while the pass is unfinished.
    Mock::given(method("GET"))
        .and(path("/api/series/50/"))
        .respond_with(series_detail(50, "Saga", Some(4050)))
        .expect(0)
        .mount(&mock)
        .await;
    empty_weeks(&mock).await;
    let app = spawn(&mock).await;
    enable(&app);
    seed_issue(&app, 1, 50, Utc::now().date_naive() + Duration::days(9)).await;

    spend_minute(&app, 10).await;
    assert!(matches!(
        calendar::list_pass(&app.state()).await.unwrap(),
        PassOutcome::Paused { .. }
    ));
    refill_minute(&app).await;

    job::sweep(&app.state()).await;
    assert!(series(&app, 50).await.unwrap().detail_fetched_at.is_none());
    assert!(
        !job::enqueue(&app.state(), "test").await,
        "the sweep queued the stopped pass"
    );
}

// ────────────────────── admin endpoints ──────────────────────

struct Authed {
    session: String,
    csrf: String,
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .unwrap()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    user: Option<&Authed>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method.clone()).uri(uri);
    if let Some(u) = user {
        req = req.header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                u.session, u.csrf
            ),
        );
        if method != Method::GET {
            req = req.header("X-CSRF-Token", &u.csrf);
        }
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn admin_endpoints_are_admin_only() {
    let mock = MockServer::start().await;
    let app = spawn(&mock).await;
    let _admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;

    for (m, uri) in [
        (Method::GET, "/api/admin/upcoming/status"),
        (Method::POST, "/api/admin/upcoming/sync"),
    ] {
        let (anon, _) = http(&app, m.clone(), uri, None).await;
        assert!(
            anon == StatusCode::UNAUTHORIZED || anon == StatusCode::FORBIDDEN,
            "{uri}: anonymous got {anon}"
        );
        let (status, body) = http(&app, m, uri, Some(&reader)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
    }
}

#[tokio::test]
async fn status_reports_the_calendar_and_sync_queues_and_audits() {
    let mock = MockServer::start().await;
    let app = spawn(&mock).await;
    let admin = register(&app, "admin@example.com").await;
    let win = todays_window();
    let this_week = win.start + Duration::weeks(2);
    seed_issue(&app, 1, 50, this_week + Duration::days(2)).await;
    seed_issue(&app, 2, 50, this_week + Duration::days(9)).await;

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/admin/upcoming/status",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["metron_ready"], true);
    assert_eq!(body["weeks_ahead"], 8);
    let cal = &body["calendar"];
    assert_eq!(cal["window_start"], iso(win.start));
    assert_eq!(cal["window_end"], iso(win.end));
    assert_eq!(cal["issues"], 2);
    assert_eq!(cal["series"], 1);
    assert_eq!(cal["series_pending_detail"], 1);
    assert_eq!(
        cal["weeks"],
        json!([
            {"week_start": iso(this_week), "issues": 1},
            {"week_start": iso(this_week + Duration::weeks(1)), "issues": 1},
        ])
    );
    assert!(cal["last_finished_at"].is_null());

    // Off: nothing is queued and nothing is audited.
    let (status, body) = http(&app, Method::POST, "/api/admin/upcoming/sync", Some(&admin)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "conflict");

    enable(&app);
    let (status, body) = http(&app, Method::POST, "/api/admin/upcoming/sync", Some(&admin)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], true);
    let (status, body) = http(&app, Method::POST, "/api/admin/upcoming/sync", Some(&admin)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["queued"], false, "one pass queued at a time");

    let audited = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.upcoming.sync"))
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(audited, 2);
}
