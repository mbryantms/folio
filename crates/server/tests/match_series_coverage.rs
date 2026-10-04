//! "Match this series…" × provider coverage (coverage tie-ins PR 2).
//!
//! 1. **Hints**: `GET /series/{slug}/metadata/coverage-hints` reports, per
//!    series candidate, how many local issues that provider series lists
//!    (number + cover date). Bounded to three per request, served from the
//!    24 h issue-list cache, capped by the per-series coverage budget, and
//!    display only (scores / buckets / order unchanged).
//! 2. **After apply**: a successful series apply queues a coverage
//!    analysis seeded with the applied provider series, per
//!    `metadata.coverage_after_series_apply` (`off` | `manual_only` |
//!    `all`), deduped per series for bulk applies.
//!
//! Fixtures are the recorded Fantastic Four responses
//! (`tests/fixtures/fantastic_four/`): the owner's 173-issue folder
//! (#½, #1–70, #500–588, #600–611 + #605.1) against Metron 1711 (1998,
//! #½–588) + 1713 (2012, #600–611) and ComicVine 6211 (everything). The
//! Metron issue search for #600 is the #600 row of the recorded 1713 list.

mod common;

use apalis::prelude::Storage;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use entity::{metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, PaginatorTrait, Set};
use serde_json::{Value, json};
use server::config::CoverageAfterSeriesApply;
use server::jobs::metadata_apply::{ApplySeriesJob, CoverPolicy};
use server::jobs::provider_coverage::{self, CoverageJobState, CoverageTrigger};
use server::metadata::apply::ApplyMode;
use server::metadata::cache::{self, CacheEntity};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::{GenericMetadata, SeriesCandidate};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, Request as WmRequest, ResponseTemplate,
    matchers::{method, path, query_param},
};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fantastic_four");

fn fixture(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{FIX}/{name}"))
        .unwrap_or_else(|e| panic!("{FIX}/{name}: {e}"));
    serde_json::from_str(&raw).unwrap()
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

// ───────── providers ─────────

async fn mount_cv(cv: &MockServer) {
    for (offset, file) in [
        ("0", "cv_issues_6211_p1.json"),
        ("100", "cv_issues_6211_p2.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:6211"))
            .and(query_param("offset", offset))
            .respond_with(ok(fixture(file)))
            .mount(cv)
            .await;
    }
}

async fn mount_metron(metron: &MockServer) {
    for (sid, page, file) in [
        ("1711", "1", "metron_issues_1711_p1.json"),
        ("1711", "2", "metron_issues_1711_p2.json"),
        ("1713", "1", "metron_issues_1713_p1.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/issue/"))
            .and(query_param("series_id", sid))
            .and(query_param("page", page))
            .respond_with(ok(fixture(file)))
            .mount(metron)
            .await;
    }
    // Gap issue search for #600: the #600 row of the recorded 1713 list.
    let list = fixture("metron_issues_1713_p1.json");
    let row = list["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == "600")
        .unwrap()
        .clone();
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_name", "Fantastic Four"))
        .and(query_param("number", "600"))
        .respond_with(ok(json!({
            "count": 1, "next": null, "previous": null, "results": [row],
        })))
        .mount(metron)
        .await;
}

struct Fx {
    app: TestApp,
    cv: MockServer,
    metron: MockServer,
    gcd: MockServer,
    series_id: Uuid,
    slug: String,
    _tmp: tempfile::TempDir,
}

async fn ff() -> Fx {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    mount_cv(&cv).await;
    mount_metron(&metron).await;
    let app = TestApp::spawn_with_all_providers(cv.uri(), metron.uri(), gcd.uri()).await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Fantastic Four");
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
        let id = IssueSeed::new(lib, series_id, &p, format!("ff {raw}").as_bytes(), sort)
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
        series_id,
        slug,
        _tmp: tmp,
    }
}

fn series_candidate(source: Source, id: &str, year: i32) -> SeriesCandidate {
    SeriesCandidate {
        source,
        external_id: id.into(),
        external_url: None,
        name: "Fantastic Four".into(),
        year: Some(year),
        publisher: Some("Marvel".into()),
        issue_count: None,
        cover_image_url: None,
        deck: None,
        alternate_cover_urls: Vec::new(),
        format: None,
    }
}

/// A completed series run whose candidates are, in rank order:
/// Metron 1711, ComicVine 6211, Metron 1713, GCD 11218. Each candidate's
/// series detail sits in the provider cache (an apply reads it there).
async fn seed_run(fx: &Fx) -> Uuid {
    let db = &fx.app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(fx.series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into(), "comicvine".into(), "gcd".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(0),
        items_matched_medium: Set(1),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    for (ordinal, source, id, year, bucket, score) in [
        (0, Source::Metron, "1711", 1998, "medium", 72.5),
        (1, Source::ComicVine, "6211", 1998, "medium", 70.0),
        (2, Source::Metron, "1713", 2012, "low", 41.0),
        (3, Source::Gcd, "11218", 1998, "low", 40.0),
    ] {
        metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(ordinal),
            source: Set(source.as_str().into()),
            external_id: Set(id.into()),
            bucket: Set(bucket.into()),
            score: Set(score),
            score_breakdown: Set(json!({"name": 40.0})),
            candidate: Set(serde_json::to_value(series_candidate(source, id, year)).unwrap()),
            applied_at: Set(None),
        }
        .insert(db)
        .await
        .unwrap();
        cache::put(
            db,
            source,
            CacheEntity::Series,
            id,
            &GenericMetadata {
                series_name: Some("Fantastic Four".into()),
                year_began: Some(year),
                publisher: Some("Marvel".into()),
                identifiers: vec![Identifier::with_canonical_url(source, id, "series")],
                source_provider: Some(source),
                source_external_id: Some(id.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    run_id
}

// ───────── HTTP helpers ─────────

async fn register_admin(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
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

async fn admin_id(app: &TestApp) -> Uuid {
    entity::user::Entity::find()
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn get(app: &TestApp, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, cookie)
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

async fn hints(fx: &Fx, cookie: &str, run_id: Uuid, ordinals: &str) -> (StatusCode, Value) {
    get(
        &fx.app,
        cookie,
        &format!(
            "/api/series/{}/metadata/coverage-hints?run_id={run_id}&ordinals={ordinals}",
            fx.slug
        ),
    )
    .await
}

fn hint(body: &Value, ordinal: i64) -> &Value {
    body["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["ordinal"] == ordinal)
        .unwrap_or_else(|| panic!("no hint {ordinal} in {body}"))
}

async fn count(server: &MockServer, pred: impl Fn(&WmRequest) -> bool) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| pred(r))
        .count()
}

fn param(r: &WmRequest, k: &str, v: &str) -> bool {
    r.url.query_pairs().any(|(key, val)| key == k && val == v)
}

fn set_policy(app: &TestApp, policy: CoverageAfterSeriesApply) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.metadata_coverage_after_series_apply = policy;
    app.state().replace_cfg(cfg);
}

/// The ranked candidates' identity + score + bucket, in order.
fn ranking(body: &Value) -> Vec<(String, String, String, String)> {
    body["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["source"].as_str().unwrap().to_owned(),
                c["external_id"].as_str().unwrap().to_owned(),
                c["bucket"].as_str().unwrap().to_owned(),
                c["score"].to_string(),
            )
        })
        .collect()
}

// ───────── 1. hints ─────────

#[tokio::test]
async fn top_candidates_carry_bounded_coverage_hints() {
    let fx = ff().await;
    let cookie = register_admin(&fx.app).await;
    let run_id = seed_run(&fx).await;
    let candidates_uri = format!(
        "/api/series/{}/metadata/candidates?run_id={run_id}",
        fx.slug
    );
    let (_, before) = get(&fx.app, &cookie, &candidates_uri).await;

    // Top three: Metron 1711, CV 6211, Metron 1713.
    let (st, body) = hints(&fx, &cookie, run_id, "0,1,2").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["max_per_request"], 3);
    assert_eq!(body["hints"].as_array().unwrap().len(), 3);

    let m1711 = hint(&body, 0);
    assert_eq!(m1711["status"], "computed", "{m1711}");
    assert_eq!(m1711["local_total"], 173);
    assert_eq!(m1711["covered"], 160, "{m1711}");
    assert_eq!(m1711["missing_count"], 13);
    assert_eq!(m1711["missing_runs"], json!(["#600–611"]));
    assert_eq!(m1711["date_conflicts"], 0);
    assert_eq!(m1711["requests"], 2, "two list pages");

    let cv = hint(&body, 1);
    assert_eq!(cv["status"], "computed", "{cv}");
    assert_eq!(cv["covered"], 173, "{cv}");
    assert_eq!(cv["missing_runs"], json!([]));

    let m1713 = hint(&body, 2);
    assert_eq!(m1713["status"], "computed");
    assert_eq!(m1713["covered"], 13, "{m1713}");
    assert_eq!(m1713["missing_count"], 160);

    // Requests: exactly the three issue lists (Metron 2 + 1 pages, CV 2),
    // nothing for GCD (ordinal 3 wasn't asked for).
    assert_eq!(
        count(&fx.metron, |r| param(r, "series_id", "1711")).await,
        2
    );
    assert_eq!(
        count(&fx.metron, |r| param(r, "series_id", "1713")).await,
        1
    );
    assert_eq!(count(&fx.cv, |_| true).await, 2);
    assert_eq!(count(&fx.gcd, |_| true).await, 0);

    // Again: served from the issue-list cache, no new requests.
    let (_, again) = hints(&fx, &cookie, run_id, "0,1,2").await;
    for o in 0..3 {
        assert_eq!(hint(&again, o)["requests"], 0);
        assert_eq!(hint(&again, o)["covered"], hint(&body, o)["covered"]);
    }
    assert_eq!(count(&fx.metron, |_| true).await, 3);
    assert_eq!(count(&fx.cv, |_| true).await, 2);

    // Bounded: at most three per request; unknown ordinals 404.
    let (st, _) = hints(&fx, &cookie, run_id, "0,1,2,3").await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = hints(&fx, &cookie, run_id, "").await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = hints(&fx, &cookie, run_id, "9").await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Budget: GCD's per-series hint budget spent ⇒ not computed, no request.
    let mut redis = fx.app.state().jobs.redis.clone();
    let key = format!("metadata:coverage_hint:spent:v1:{}:gcd", fx.series_id);
    let _: () = redis::cmd("SET")
        .arg(&key)
        .arg(30)
        .query_async(&mut redis)
        .await
        .unwrap();
    let (_, gcd) = hints(&fx, &cookie, run_id, "3").await;
    let g = hint(&gcd, 3);
    assert_eq!(g["status"], "not_computed", "{g}");
    assert_eq!(g["reason"], "budget");
    assert_eq!(count(&fx.gcd, |_| true).await, 0);

    // Display only: ranking, scores and buckets are untouched.
    let (_, after) = get(&fx.app, &cookie, &candidates_uri).await;
    assert_eq!(ranking(&before), ranking(&after));
    assert_eq!(before["match_outcome"], after["match_outcome"]);
}

// ───────── 2. coverage after a series apply ─────────

fn apply_job(run_id: Uuid, series_id: Uuid, ordinal: i32, actor: Uuid) -> ApplySeriesJob {
    ApplySeriesJob {
        run_id,
        ordinal,
        series_id,
        mode: ApplyMode::FillMissing,
        apply_cover: false,
        cover_overwrite_policy: CoverPolicy::Never,
        override_user_edits: false,
        actor_id: Some(actor),
        actor_ip: None,
        actor_ua: None,
        selected_fields: None,
        override_external_id_sources: Default::default(),
        is_auto: false,
        composite: None,
        bulk: false,
    }
}

async fn run_apply(fx: &Fx, job: ApplySeriesJob) {
    server::jobs::metadata_apply::handle_series(job, apalis::prelude::Data::new(fx.app.state()))
        .await
        .unwrap();
}

async fn coverage_jobs_waiting(app: &TestApp) -> i64 {
    let mut storage = app.state().jobs.provider_coverage_storage.clone();
    storage.len().await.unwrap()
}

#[tokio::test]
async fn manual_series_apply_queues_coverage_seeded_with_the_match() {
    let fx = ff().await;
    let cookie = register_admin(&fx.app).await;
    let actor = admin_id(&fx.app).await;
    let run_id = seed_run(&fx).await;
    // The dialog's hints warm Metron 1711's issue list.
    let (st, _) = hints(&fx, &cookie, run_id, "0").await;
    assert_eq!(st, StatusCode::OK);
    let metron_before = count(&fx.metron, |_| true).await;
    assert_eq!(metron_before, 2);

    run_apply(&fx, apply_job(run_id, fx.series_id, 0, actor)).await;

    let rec = provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
        .await
        .expect("a coverage job was queued");
    assert_eq!(rec.state, CoverageJobState::Queued);
    assert_eq!(rec.trigger, CoverageTrigger::SeriesMatch);
    assert!(!rec.auto_accept, "default: present for confirmation");
    assert_eq!(rec.seeds.len(), 1);
    assert_eq!(rec.seeds[0].source, Source::Metron);
    assert_eq!(rec.seeds[0].provider_series_id, "1711");
    assert_eq!(rec.sources, vec![Source::Metron]);
    assert_eq!(coverage_jobs_waiting(&fx.app).await, 1);

    provider_coverage::process(&fx.app.state(), rec.job_id)
        .await
        .unwrap();

    // Only Metron analysed; the seed is the main; #600–611 proposed as a
    // range onto 1713, found by one gap issue search.
    let (st, body) = get(
        &fx.app,
        &cookie,
        &format!("/api/series/{}/provider-coverage/analysis", fx.slug),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["trigger"], "series_match");
    let providers = body["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1, "{body}");
    let m = &providers[0];
    assert_eq!(m["source"], "metron");
    assert_eq!(m["seeded_series_id"], "1711");
    assert_eq!(m["main_series_id"], "1711");
    assert_eq!(m["uncovered"], json!([]), "{m}");
    let ranges = m["proposed_ranges"].as_array().unwrap();
    assert_eq!(ranges.len(), 1, "{m}");
    assert_eq!(ranges[0]["provider_series_id"], "1713");
    assert_eq!(ranges[0]["low"], "600");
    assert_eq!(ranges[0]["high"], "611");
    assert_eq!(ranges[0]["status"], "new");
    assert_eq!(m["confidence"], "high", "{m}");

    // Cost: the 1711 list was cached by the hint ⇒ one issue search + the
    // 1713 list. No series-name search, no other provider touched.
    let metron_spent = count(&fx.metron, |_| true).await - metron_before;
    assert_eq!(metron_spent, 2, "search #600 + list 1713");
    assert_eq!(m["requests"], 2);
    assert_eq!(
        count(&fx.metron, |r| r.url.path() == "/api/series/").await,
        0
    );
    assert_eq!(count(&fx.cv, |_| true).await, 0);
    assert_eq!(count(&fx.gcd, |_| true).await, 0);

    // Nothing written without the admin's Accept.
    let ranges_written = entity::series_provider_range::Entity::find()
        .count(&fx.app.state().db)
        .await
        .unwrap();
    assert_eq!(ranges_written, 0);
    assert!(rec.auto_accepted.is_empty());

    // The admin accepts: the range is written.
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned();
    let resp = fx
        .app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/series/{}/provider-coverage/accept", fx.slug))
                .header(header::COOKIE, &cookie)
                .header("X-CSRF-Token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"source": "metron"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ranges_written = entity::series_provider_range::Entity::find()
        .all(&fx.app.state().db)
        .await
        .unwrap();
    assert_eq!(ranges_written.len(), 1);
    assert_eq!(ranges_written[0].provider_series_id, "1713");
}

#[tokio::test]
async fn auto_accept_setting_lets_high_confidence_results_write() {
    let fx = ff().await;
    let actor = {
        let _ = register_admin(&fx.app).await;
        admin_id(&fx.app).await
    };
    let mut cfg = (*fx.app.state().cfg()).clone();
    cfg.metadata_coverage_auto_accept = true;
    fx.app.state().replace_cfg(cfg);
    let run_id = seed_run(&fx).await;
    run_apply(&fx, apply_job(run_id, fx.series_id, 0, actor)).await;
    let rec = provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
        .await
        .unwrap();
    assert!(rec.auto_accept);
    provider_coverage::process(&fx.app.state(), rec.job_id)
        .await
        .unwrap();
    let rec = provider_coverage::load(&fx.app.state().jobs.redis, rec.job_id)
        .await
        .unwrap();
    assert_eq!(rec.auto_accepted.len(), 1, "{:?}", rec.auto_accepted);
    let ranges = entity::series_provider_range::Entity::find()
        .all(&fx.app.state().db)
        .await
        .unwrap();
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].provider_series_id, "1713");
    assert_eq!(ranges[0].set_by, "cross_reference");
}

#[tokio::test]
async fn setting_off_disables_and_manual_only_skips_bulk() {
    let fx = ff().await;
    let _ = register_admin(&fx.app).await;
    let actor = admin_id(&fx.app).await;
    let run_id = seed_run(&fx).await;

    set_policy(&fx.app, CoverageAfterSeriesApply::Off);
    run_apply(&fx, apply_job(run_id, fx.series_id, 0, actor)).await;
    assert!(
        provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
            .await
            .is_none(),
        "off: no coverage job"
    );

    // manual_only (default): a bulk apply and an automatic one don't queue.
    set_policy(&fx.app, CoverageAfterSeriesApply::ManualOnly);
    let mut bulk = apply_job(run_id, fx.series_id, 0, actor);
    bulk.bulk = true;
    run_apply(&fx, bulk).await;
    let mut auto = apply_job(run_id, fx.series_id, 0, actor);
    auto.is_auto = true;
    auto.actor_id = None;
    run_apply(&fx, auto).await;
    assert!(
        provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
            .await
            .is_none(),
        "manual_only: bulk / auto applies don't queue"
    );
    assert_eq!(coverage_jobs_waiting(&fx.app).await, 0);
    assert_eq!(count(&fx.metron, |_| true).await, 0);
}

#[tokio::test]
async fn all_queues_bulk_applies_once_per_series() {
    let fx = ff().await;
    let _ = register_admin(&fx.app).await;
    let actor = admin_id(&fx.app).await;
    let run_id = seed_run(&fx).await;
    set_policy(&fx.app, CoverageAfterSeriesApply::All);

    // A bulk apply of Metron 1711, then (same batch) ComicVine 6211, then
    // an automatic apply of 1711 again: one job, both providers seeded.
    let mut a = apply_job(run_id, fx.series_id, 0, actor);
    a.bulk = true;
    run_apply(&fx, a).await;
    let first = provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
        .await
        .unwrap();
    assert_eq!(first.trigger, CoverageTrigger::BulkSeriesMatch);
    let mut b = apply_job(run_id, fx.series_id, 1, actor);
    b.bulk = true;
    run_apply(&fx, b).await;
    let mut c = apply_job(run_id, fx.series_id, 0, actor);
    c.is_auto = true;
    c.actor_id = None;
    run_apply(&fx, c).await;

    assert_eq!(
        coverage_jobs_waiting(&fx.app).await,
        1,
        "deduped per series"
    );
    let rec = provider_coverage::latest_for_series(&fx.app.state().jobs.redis, fx.series_id)
        .await
        .unwrap();
    assert_eq!(rec.job_id, first.job_id);
    let mut seeded: Vec<(Source, String)> = rec
        .seeds
        .iter()
        .map(|s| (s.source, s.provider_series_id.clone()))
        .collect();
    seeded.sort_by_key(|(s, _)| s.as_str());
    assert_eq!(
        seeded,
        vec![
            (Source::ComicVine, "6211".to_owned()),
            (Source::Metron, "1711".to_owned()),
        ]
    );
    assert_eq!(
        rec.analysed_sources(),
        vec![Source::ComicVine, Source::Metron]
    );

    // Bounded queue: with MAX_QUEUED_AFTER_APPLY jobs waiting, another
    // series' bulk apply doesn't queue.
    let mut storage = fx.app.state().jobs.provider_coverage_storage.clone();
    for _ in 1..provider_coverage::MAX_QUEUED_AFTER_APPLY {
        storage
            .push(provider_coverage::ProviderCoverageJob {
                job_id: Uuid::new_v4(),
            })
            .await
            .unwrap();
    }
    let db = fx.app.state().db.clone();
    let lib = entity::series::Entity::find_by_id(fx.series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .library_id;
    let other = SeriesSeed::new(lib, "Fantastic Four Annual")
        .insert(&db)
        .await;
    let got =
        provider_coverage::enqueue_after_series_apply(&fx.app.state(), other, run_id, None, false)
            .await;
    assert!(got.is_none(), "queue full: skipped");
    // A manual apply still queues.
    let got = provider_coverage::enqueue_after_series_apply(
        &fx.app.state(),
        other,
        run_id,
        Some(actor),
        true,
    )
    .await;
    assert!(matches!(got, Some((_, true))));
}

// ───────── 3. the settings chain ─────────

async fn patch_settings(app: &TestApp, cookie: &str, body: Value) -> (StatusCode, Value) {
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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

#[tokio::test]
async fn coverage_settings_round_trip_through_admin_settings() {
    let app = TestApp::spawn().await;
    let cookie = register_admin(&app).await;
    let cfg = app.state().cfg();
    assert_eq!(
        cfg.metadata_coverage_after_series_apply,
        CoverageAfterSeriesApply::ManualOnly,
        "default"
    );
    assert!(!cfg.metadata_coverage_auto_accept, "default");

    let (st, body) = patch_settings(
        &app,
        &cookie,
        json!({
            "metadata.coverage_after_series_apply": "all",
            "metadata.coverage_auto_accept": true,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let cfg = app.state().cfg();
    assert_eq!(
        cfg.metadata_coverage_after_series_apply,
        CoverageAfterSeriesApply::All
    );
    assert!(cfg.metadata_coverage_auto_accept);
    let values = body["values"].as_array().unwrap();
    let v = |k: &str| {
        values
            .iter()
            .find(|v| v["key"] == k)
            .unwrap_or_else(|| panic!("{k} not in values"))["value"]
            .clone()
    };
    assert_eq!(v("metadata.coverage_after_series_apply"), "all");
    assert_eq!(v("metadata.coverage_auto_accept"), true);

    // Wrong JSON type: rejected before the write.
    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({"metadata.coverage_auto_accept": "yes"}),
    )
    .await;
    assert!(st.is_client_error(), "{st}");

    // Unknown mode: falls back to the safe default.
    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({"metadata.coverage_after_series_apply": "sometimes"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        app.state().cfg().metadata_coverage_after_series_apply,
        CoverageAfterSeriesApply::ManualOnly
    );

    // `off`, then clearing the row reverts to the default.
    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({"metadata.coverage_after_series_apply": "off"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        app.state().cfg().metadata_coverage_after_series_apply,
        CoverageAfterSeriesApply::Off
    );
    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({"metadata.coverage_after_series_apply": null}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        app.state().cfg().metadata_coverage_after_series_apply,
        CoverageAfterSeriesApply::ManualOnly
    );
}
