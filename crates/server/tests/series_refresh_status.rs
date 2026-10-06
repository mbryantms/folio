//! Guided "Refresh this series…" (coverage tie-ins PR 3):
//! `GET /api/series/{slug}/metadata/refresh-status`.
//!
//! The endpoint aggregates what the flow's steps leave behind — the latest
//! series run + apply, the coverage job record (Redis) and the series'
//! latest metadata batch — into a `resume_step`, and estimates the
//! per-issue step's direct lookups vs searches per provider. It is
//! read-only: no provider is called and nothing is written.
//!
//! Fixtures: the owner's 173-issue Fantastic Four folder
//! (`tests/fixtures/fantastic_four/local_issues.psv`) against wiremock
//! ComicVine / Metron / GCD servers with the recorded responses mounted
//! (none of them may be hit).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use entity::{metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set};
use serde_json::{Value, json};
use server::config::CoverageAfterSeriesApply;
use server::jobs::provider_coverage::{self, CoverageJobState, CoverageTrigger, JobRecord};
use server::metadata::identifier::Source;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fantastic_four");

fn fixture(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{FIX}/{name}"))
        .unwrap_or_else(|e| panic!("{FIX}/{name}: {e}"));
    serde_json::from_str(&raw).unwrap()
}

struct Fx {
    app: TestApp,
    cv: MockServer,
    metron: MockServer,
    gcd: MockServer,
    library_id: Uuid,
    series_id: Uuid,
    slug: String,
    _tmp: tempfile::TempDir,
}

/// The FF folder, with the recorded ComicVine / Metron issue lists mounted
/// so an accidental provider call would succeed (and be counted).
async fn ff() -> Fx {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    for (offset, file) in [
        ("0", "cv_issues_6211_p1.json"),
        ("100", "cv_issues_6211_p2.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:6211"))
            .and(query_param("offset", offset))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(file)))
            .mount(&cv)
            .await;
    }
    for (sid, page, file) in [
        ("1711", "1", "metron_issues_1711_p1.json"),
        ("1711", "2", "metron_issues_1711_p2.json"),
        ("1713", "1", "metron_issues_1713_p1.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/issue/"))
            .and(query_param("series_id", sid))
            .and(query_param("page", page))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(file)))
            .mount(&metron)
            .await;
    }
    let app = TestApp::spawn_with_all_providers(cv.uri(), metron.uri(), gcd.uri()).await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let library_id = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(library_id, "Fantastic Four");
    seed.year = Some(2001);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    let rows: Vec<(String, i32, i32)> = std::fs::read_to_string(format!("{FIX}/local_issues.psv"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            (
                f[0].to_owned(),
                f[1].parse().unwrap(),
                f[2].parse().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows.len(), 173);
    for (raw, y, m) in &rows {
        let sort: f64 = raw.parse().unwrap();
        let p = tmp.path().join(format!("ff-{raw}.cbz"));
        let id = IssueSeed::new(
            library_id,
            series_id,
            &p,
            format!("ff {raw}").as_bytes(),
            sort,
        )
        .insert(&db)
        .await;
        db.execute_unprepared(&format!(
            "UPDATE issues SET number_raw = '{raw}', year = {y}, month = {m} WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    }
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    Fx {
        app,
        cv,
        metron,
        gcd,
        library_id,
        series_id,
        slug,
        _tmp: tmp,
    }
}

// ───────── HTTP helpers ─────────

async fn register(app: &TestApp, email: &str) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": "correctly-horse-battery"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .filter(|c| c.starts_with("__Host-comic_session=") || c.starts_with("__Host-comic_csrf="))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn send(app: &TestApp, cookie: &str, m: Method, uri: &str) -> (StatusCode, Value) {
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap_or_default()
        .to_owned();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(m)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("X-CSRF-Token", csrf)
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

async fn status(fx: &Fx, cookie: &str) -> Value {
    let (st, body) = send(
        &fx.app,
        cookie,
        Method::GET,
        &format!("/api/series/{}/metadata/refresh-status", fx.slug),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body
}

async fn provider_requests(fx: &Fx) -> usize {
    let mut n = 0;
    for s in [&fx.cv, &fx.metron, &fx.gcd] {
        n += s.received_requests().await.unwrap_or_default().len();
    }
    n
}

/// `(direct, search)` for `source` in the estimate for `scope`.
fn estimate(body: &Value, scope: &str, source: &str) -> (i64, i64) {
    let e = body["fetch_estimate"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["scope"] == scope)
        .unwrap_or_else(|| panic!("no {scope} estimate in {body}"));
    let p = e["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == source)
        .unwrap_or_else(|| panic!("no {source} in {e}"));
    (p["direct"].as_i64().unwrap(), p["search"].as_i64().unwrap())
}

// ───────── seeding ─────────

async fn link(fx: &Fx, source: &str, id: &str, set_by: &str) {
    fx.app
        .state()
        .db
        .execute_unprepared(&format!(
            "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, first_set_at, last_synced_at) \
             VALUES ('series', '{}', '{source}', '{id}', '{set_by}', now(), now())",
            fx.series_id
        ))
        .await
        .unwrap();
}

async fn range(fx: &Fx, source: &str, id: &str, low: &str, high: &str) {
    fx.app
        .state()
        .db
        .execute_unprepared(&format!(
            "INSERT INTO series_provider_range (id, series_id, source, provider_series_id, range_low, range_high, declared_year, set_by, first_set_at, last_synced_at) \
             VALUES ('{}', '{}', '{source}', '{id}', '{low}', '{high}', 2012, 'cross_reference', now(), now())",
            Uuid::now_v7(),
            fx.series_id
        ))
        .await
        .unwrap();
}

/// A completed series run whose Metron 1711 candidate was applied
/// `minutes_ago`.
async fn applied_run(fx: &Fx, minutes_ago: i64) -> Uuid {
    let db = &fx.app.state().db;
    let at = (Utc::now() - Duration::minutes(minutes_ago)).fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(fx.series_id.to_string())),
        library_id: Set(Some(fx.library_id)),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into()]),
        status: Set("completed".into()),
        started_at: Set(at),
        finished_at: Set(Some(at)),
        items_total: Set(1),
        items_matched_high: Set(0),
        items_matched_medium: Set(1),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(1),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("metron".into()),
        external_id: Set("1711".into()),
        bucket: Set("medium".into()),
        score: Set(72.5),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({})),
        applied_at: Set(Some(at)),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

async fn save_coverage_job(fx: &Fx, minutes_ago: i64, state: CoverageJobState) -> Uuid {
    let requested_at = Utc::now() - Duration::minutes(minutes_ago);
    let rec = JobRecord {
        job_id: Uuid::now_v7(),
        series_id: fx.series_id,
        actor_id: Uuid::now_v7(),
        state,
        auto_accept: false,
        requested_at,
        started_at: Some(requested_at),
        finished_at: (state == CoverageJobState::Done).then_some(requested_at),
        error: None,
        providers: Vec::new(),
        auto_accepted: Vec::new(),
        trigger: CoverageTrigger::SeriesMatch,
        seeds: Vec::new(),
        sources: vec![Source::Metron],
    };
    provider_coverage::save(&fx.app.state().jobs.redis, &rec)
        .await
        .unwrap();
    rec.job_id
}

// ───────── tests ─────────

#[tokio::test]
async fn unmatched_series_resumes_at_match_and_every_issue_searches() {
    let fx = ff().await;
    let cookie = register(&fx.app, "admin@example.com").await;

    let body = status(&fx, &cookie).await;
    assert_eq!(body["series_id"], fx.series_id.to_string());
    assert_eq!(body["resume_step"], "match", "{body}");
    assert_eq!(body["series_match"]["links"], json!([]));
    assert_eq!(body["series_match"]["latest_run"], Value::Null);
    assert_eq!(body["series_match"]["applied_at"], Value::Null);
    assert_eq!(body["coverage"], Value::Null);
    assert_eq!(body["batch"], Value::Null);
    assert_eq!(body["coverage_after_series_apply"], "manual_only");

    // Both scopes, all three providers, nothing direct without a target.
    let scopes: Vec<&str> = body["fetch_estimate"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["scope"].as_str().unwrap())
        .collect();
    assert_eq!(scopes, ["all", "incomplete"]);
    assert_eq!(body["fetch_estimate"][0]["issues"], 173);
    let sources: Vec<&str> = body["fetch_estimate"][0]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, ["comicvine", "metron", "gcd"]);
    for src in ["comicvine", "metron", "gcd"] {
        assert_eq!(estimate(&body, "all", src), (0, 173), "{src}");
    }
    // Seeded issues carry no metadata → every one is "missing or partial".
    assert_eq!(body["fetch_estimate"][1]["issues"], 173);

    assert_eq!(
        provider_requests(&fx).await,
        0,
        "read-only: no provider call"
    );
}

#[tokio::test]
async fn linked_series_estimates_direct_lookups_per_provider() {
    let fx = ff().await;
    let cookie = register(&fx.app, "admin@example.com").await;
    link(&fx, "comicvine", "6211", "user").await;
    link(&fx, "metron", "1711", "provider:metron").await;
    range(&fx, "metron", "1713", "600", "611").await;

    let body = status(&fx, &cookie).await;
    let links = body["series_match"]["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{body}");
    assert!(links.iter().any(|l| l["source"] == "comicvine"
        && l["external_id"] == "6211"
        && l["set_by"] == "user"));
    // A link alone (no apply / coverage / batch in the window) still
    // starts at the match step, where "keep current match" is offered.
    assert_eq!(body["resume_step"], "match");

    // ComicVine: the series id covers every issue; Metron: 1711 plus the
    // #600–611 range → 1713; GCD has no series → every issue searches.
    assert_eq!(estimate(&body, "all", "comicvine"), (173, 0));
    assert_eq!(estimate(&body, "all", "metron"), (173, 0));
    assert_eq!(estimate(&body, "all", "gcd"), (0, 173));
    assert_eq!(provider_requests(&fx).await, 0);
}

#[tokio::test]
async fn resume_follows_apply_then_coverage_then_batch() {
    let fx = ff().await;
    let cookie = register(&fx.app, "admin@example.com").await;

    // 1. A series match applied 30 minutes ago → coverage.
    let run_id = applied_run(&fx, 30).await;
    let body = status(&fx, &cookie).await;
    assert_eq!(body["resume_step"], "coverage", "{body}");
    assert_eq!(
        body["series_match"]["latest_run"]["run_id"],
        run_id.to_string()
    );
    assert_eq!(body["series_match"]["latest_run"]["status"], "completed");
    assert!(body["series_match"]["applied_at"].is_string());
    assert_eq!(body["coverage"], Value::Null);

    // 2. The seeded coverage job it queued → still coverage, job reported.
    let job_id = save_coverage_job(&fx, 25, CoverageJobState::Running).await;
    let body = status(&fx, &cookie).await;
    assert_eq!(body["resume_step"], "coverage");
    assert_eq!(body["coverage"]["job_id"], job_id.to_string());
    assert_eq!(body["coverage"]["state"], "running");
    assert_eq!(body["coverage"]["trigger"], "series_match");
    assert_eq!(body["coverage"]["sources"], json!(["metron"]));

    // 3. Another series' batch doesn't count.
    let other = SeriesSeed::new(fx.library_id, "Daredevil")
        .insert(&fx.app.state().db)
        .await;
    let other_issue = IssueSeed::new(
        fx.library_id,
        other,
        &fx._tmp.path().join("dd-1.cbz"),
        b"dd 1",
        1.0,
    )
    .insert(&fx.app.state().db)
    .await;
    fx.app
        .state()
        .db
        .execute_unprepared(&format!(
            "UPDATE issues SET number_raw = '1' WHERE id = '{other_issue}'"
        ))
        .await
        .unwrap();
    let other_slug = entity::series::Entity::find_by_id(other)
        .one(&fx.app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let (st, _) = send(
        &fx.app,
        &cookie,
        Method::POST,
        &format!("/api/series/{other_slug}/metadata/batch"),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(status(&fx, &cookie).await["batch"], Value::Null);

    // 4. This series' "All issues" batch (children queued) → fetch.
    let (st, created) = send(
        &fx.app,
        &cookie,
        Method::POST,
        &format!("/api/series/{}/metadata/batch", fx.slug),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["items_total"], 173);
    let body = status(&fx, &cookie).await;
    assert_eq!(body["resume_step"], "fetch", "{body}");
    assert_eq!(body["batch"]["batch_id"], created["batch_id"]);
    assert_eq!(body["batch"]["items_total"], 173);
    assert_eq!(body["batch"]["unfinished"], 173);

    // 5. Every child finished → review.
    fx.app
        .state()
        .db
        .execute_unprepared(&format!(
            "UPDATE metadata_run SET status = 'completed' WHERE batch_id = '{}'",
            created["batch_id"].as_str().unwrap()
        ))
        .await
        .unwrap();
    let body = status(&fx, &cookie).await;
    assert_eq!(body["resume_step"], "review", "{body}");
    assert_eq!(body["batch"]["unfinished"], 0);

    // 6. A newer series match restarts the flow at coverage: the batch
    //    is older than it.
    applied_run(&fx, 0).await;
    let body = status(&fx, &cookie).await;
    assert_eq!(body["resume_step"], "coverage", "{body}");

    assert_eq!(provider_requests(&fx).await, 0);
}

#[tokio::test]
async fn state_older_than_the_window_is_ignored() {
    let fx = ff().await;
    let cookie = register(&fx.app, "admin@example.com").await;
    applied_run(&fx, 25 * 60).await;
    let body = status(&fx, &cookie).await;
    assert_eq!(body["series_match"]["applied_at"], Value::Null);
    assert!(body["series_match"]["latest_run"].is_object(), "any age");
    assert_eq!(body["resume_step"], "match");
}

#[tokio::test]
async fn reports_the_coverage_after_apply_setting() {
    let fx = ff().await;
    let cookie = register(&fx.app, "admin@example.com").await;
    let mut cfg = (*fx.app.state().cfg()).clone();
    cfg.metadata_coverage_after_series_apply = CoverageAfterSeriesApply::Off;
    fx.app.state().replace_cfg(cfg);
    assert_eq!(
        status(&fx, &cookie).await["coverage_after_series_apply"],
        "off"
    );
}

#[tokio::test]
async fn admin_only() {
    let fx = ff().await;
    let _admin = register(&fx.app, "admin@example.com").await;
    let user = register(&fx.app, "user@example.com").await;
    fx.app
        .state()
        .db
        .execute_unprepared("UPDATE users SET role = 'user' WHERE email = 'user@example.com'")
        .await
        .unwrap();
    let (st, body) = send(
        &fx.app,
        &user,
        Method::GET,
        &format!("/api/series/{}/metadata/refresh-status", fx.slug),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"]["code"].is_string());
}
