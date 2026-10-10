//! Coverage tie-ins: compare mode uses series coverage, the opt-in
//! issue-level refresh, exact missing-issue lists from provider issue
//! lists, and "not in your library" links from coverage ranges.
//!
//! Fixture: the owner's Fantastic Four (folder "2001", the 1998 volume) —
//! ComicVine 6211 lumps the run, Metron splits #600–611 into 1713. Issue
//! lists are the **recorded** responses in `tests/fixtures/fantastic_four/`;
//! issue details are synthesised from the same recorded list rows (only
//! list pages were recorded). GCD answers empty everywhere unless a test
//! says otherwise. No test reaches a real provider.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::writers::{SetBy, set_external_id};
use std::collections::HashMap;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, Request as WmRequest, Respond, ResponseTemplate,
    matchers::{any, method, path, path_regex, query_param, query_param_is_missing},
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

fn empty_page() -> Value {
    json!({"count": 0, "next": null, "previous": null, "results": []})
}

/// Recorded Metron 1713 page, optionally extended with synthetic
/// #612–#615 (as if the provider series ran on past the folder's #611).
fn metron_1713(extended: bool) -> Value {
    let mut page = fixture("metron_issues_1713_p1.json");
    if extended {
        let last = page["results"].as_array().unwrap().last().unwrap().clone();
        for (i, n) in (612..=615).enumerate() {
            let mut row = last.clone();
            row["id"] = json!(990_000 + n);
            row["number"] = json!(n.to_string());
            row["issue"] = json!(format!("Fantastic Four (2012) #{n}"));
            row["cover_date"] = json!(format!("2013-0{}-01", i + 1));
            page["results"].as_array_mut().unwrap().push(row);
        }
        page["count"] = json!(page["results"].as_array().unwrap().len());
    }
    page
}

/// Recorded list rows keyed by provider issue id.
fn rows_by_id(pages: &[Value]) -> HashMap<String, Value> {
    pages
        .iter()
        .flat_map(|p| p["results"].as_array().unwrap().clone())
        .map(|r| (r["id"].to_string(), r))
        .collect()
}

fn id_for(rows: &HashMap<String, Value>, number_key: &str, number: &str) -> String {
    rows.iter()
        .find(|(_, r)| r[number_key].as_str() == Some(number))
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("no recorded row for #{number}"))
}

/// ComicVine `/issue/4000-<id>/` built from the recorded list row.
struct CvDetail(HashMap<String, Value>);

impl Respond for CvDetail {
    fn respond(&self, req: &WmRequest) -> ResponseTemplate {
        let id = req
            .url
            .path()
            .trim_start_matches("/issue/4000-")
            .trim_end_matches('/');
        let Some(row) = self.0.get(id) else {
            return ResponseTemplate::new(404);
        };
        ok(json!({
            "status_code": 1,
            "error": "OK",
            "results": {
                "id": row["id"],
                "name": null,
                "issue_number": row["issue_number"],
                "cover_date": row["cover_date"],
                "store_date": null,
                "deck": null,
                "description": format!("<p>Fantastic Four #{} recap.</p>", row["issue_number"].as_str().unwrap()),
                "image": null,
                "person_credits": [],
                "character_credits": [],
                "team_credits": [],
                "location_credits": [],
                "concept_credits": [],
                "object_credits": [],
                "story_arc_credits": [],
                "associated_images": [],
                "first_appearance_characters": [],
                "volume": {
                    "id": 6211,
                    "name": "Fantastic Four",
                    "start_year": "1998",
                    "site_detail_url": null,
                    "publisher": null,
                    "deck": null,
                    "description": null,
                    "image": null,
                    "count_of_issues": null,
                    "date_last_updated": null,
                    "aliases": null,
                },
                "site_detail_url": format!("https://comicvine.gamespot.com/issue/4000-{id}/"),
                "date_last_updated": "2024-02-20 08:00:00",
                "aliases": null,
            }
        }))
    }
}

/// Metron `/api/issue/<id>/` built from the recorded list row.
struct MetronDetail(HashMap<String, Value>);

impl Respond for MetronDetail {
    fn respond(&self, req: &WmRequest) -> ResponseTemplate {
        let id = req
            .url
            .path()
            .trim_start_matches("/api/issue/")
            .trim_end_matches('/');
        let Some(row) = self.0.get(id) else {
            return ResponseTemplate::new(404);
        };
        ok(json!({
            "id": row["id"],
            "publisher": {"id": 1, "name": "Marvel"},
            "series": row["series"],
            "number": row["number"],
            "cover_date": row["cover_date"],
            "store_date": row["store_date"],
            "desc": format!("Fantastic Four #{} recap.", row["number"].as_str().unwrap()),
            // No image: nothing to hash, nothing fetched off-box.
            "image": null,
            "cover_hash": row["cover_hash"],
            "resource_url": format!("https://metron.cloud/issue/{id}/"),
            "modified": row["modified"],
        }))
    }
}

struct Mocks {
    cv: MockServer,
    metron: MockServer,
    gcd: MockServer,
    cv_rows: HashMap<String, Value>,
    metron_rows: HashMap<String, Value>,
}

async fn mount(extended_1713: bool) -> Mocks {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;

    let cv_pages = [
        fixture("cv_issues_6211_p1.json"),
        fixture("cv_issues_6211_p2.json"),
    ];
    for (offset, page) in ["0", "100"].iter().zip(cv_pages.iter()) {
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:6211"))
            .and(query_param("offset", *offset))
            .respond_with(ok(page.clone()))
            .mount(&cv)
            .await;
    }
    let cv_rows = rows_by_id(&cv_pages);
    Mock::given(method("GET"))
        .and(path_regex(r"^/issue/4000-\d+/?$"))
        .respond_with(CvDetail(cv_rows.clone()))
        .mount(&cv)
        .await;
    Mock::given(method("GET"))
        .and(path("/issues"))
        .respond_with(ok(json!({"status_code": 1, "error": "OK", "results": []})))
        .mount(&cv)
        .await;

    let m1711 = [
        fixture("metron_issues_1711_p1.json"),
        fixture("metron_issues_1711_p2.json"),
    ];
    let m1713 = metron_1713(extended_1713);
    for (sid, page, body) in [
        ("1711", "1", m1711[0].clone()),
        ("1711", "2", m1711[1].clone()),
        ("1713", "1", m1713.clone()),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/issue/"))
            .and(query_param("series_id", sid))
            .and(query_param("page", page))
            .respond_with(ok(body))
            .mount(&metron)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("name", "Fantastic Four"))
        .respond_with(ok(fixture("metron_series_search_ff.json")))
        .mount(&metron)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param_is_missing("page"))
        .respond_with(ok(empty_page()))
        .mount(&metron)
        .await;
    let metron_rows = rows_by_id(&[m1711[0].clone(), m1711[1].clone(), m1713]);
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/issue/\d+/?$"))
        .respond_with(MetronDetail(metron_rows.clone()))
        .mount(&metron)
        .await;

    Mock::given(any())
        .respond_with(ok(empty_page()))
        .mount(&gcd)
        .await;

    Mocks {
        cv,
        metron,
        gcd,
        cv_rows,
        metron_rows,
    }
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

async fn total_requests(m: &Mocks) -> usize {
    count(&m.cv, |_| true).await + count(&m.metron, |_| true).await + count(&m.gcd, |_| true).await
}

fn has_param(r: &WmRequest, k: &str) -> bool {
    r.url.query_pairs().any(|(key, _)| key == k)
}

/// By-number issue lookups (`/issues?filter=issue_number:…`), the second
/// leg of a ComicVine by-name search.
async fn cv_searches(m: &Mocks) -> usize {
    count(&m.cv, |r| r.url.path() == "/issues").await
}

async fn metron_searches(m: &Mocks) -> usize {
    count(&m.metron, |r| {
        r.url.path() == "/api/issue/" && has_param(r, "number")
    })
    .await
}

// ───────── local library ─────────

struct Ff {
    series_id: Uuid,
    library_id: Uuid,
    slug: String,
    /// `number_raw → issue id`.
    issues: HashMap<String, String>,
    _tmp: tempfile::TempDir,
}

/// The owner's 173 FF issues `(number_raw, year, month)`.
fn ff_rows() -> Vec<(String, i32, i32)> {
    std::fs::read_to_string(format!("{FIX}/local_issues.psv"))
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
        .collect()
}

/// Seed "Fantastic Four" (2001, Marvel) with `issues`; with `links`,
/// accept ComicVine 6211 + Metron 1711 and the Metron #600–611 → 1713
/// range.
async fn seed_ff(app: &TestApp, issues: &[(String, i32, i32)], links: bool) -> Ff {
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Fantastic Four");
    seed.year = Some(2001);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    let mut ids = HashMap::new();
    for (i, (raw, y, m)) in issues.iter().enumerate() {
        let p = tmp.path().join(format!("ff-{raw}.cbz"));
        let sort: f64 = raw.parse().unwrap_or(i as f64);
        let id = IssueSeed::new(
            lib,
            series_id,
            &p,
            format!("ff {lib} {raw}").as_bytes(),
            sort,
        )
        .insert(&db)
        .await;
        db.execute_unprepared(&format!(
            "UPDATE issues SET number_raw = '{raw}', year = {y}, month = {m} WHERE id = '{id}'"
        ))
        .await
        .unwrap();
        ids.insert(raw.clone(), id);
    }
    if links {
        for (src, id) in [(Source::ComicVine, "6211"), (Source::Metron, "1711")] {
            set_external_id(
                &db,
                "series",
                &series_id.to_string(),
                &Identifier::with_canonical_url(src, id.to_owned(), "series"),
                SetBy::Provider(src),
            )
            .await
            .unwrap();
        }
        insert_range(app, series_id, "600", "611").await;
    }
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    Ff {
        series_id,
        library_id: lib,
        slug,
        issues: ids,
        _tmp: tmp,
    }
}

async fn insert_range(app: &TestApp, series_id: Uuid, low: &str, high: &str) {
    let now = chrono::Utc::now().fixed_offset();
    entity::series_provider_range::ActiveModel {
        id: Set(Uuid::new_v4()),
        series_id: Set(series_id),
        source: Set("metron".into()),
        provider_series_id: Set("1713".into()),
        provider_series_url: Set(None),
        provider_series_name: Set(Some("Fantastic Four".into())),
        range_low: Set(Some(low.into())),
        range_high: Set(Some(high.into())),
        declared_year: Set(Some(2012)),
        set_by: Set("cross_reference".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

// ───────── HTTP + job helpers ─────────

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

async fn call(
    app: &TestApp,
    cookie: &str,
    m: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned();
    let mut b = Request::builder()
        .method(m)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("X-CSRF-Token", csrf);
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(b.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every queued `SearchIssueJob` (not run).
async fn queued_issue_jobs(app: &TestApp) -> Vec<server::jobs::metadata_search::SearchIssueJob> {
    let storage = app.state().jobs.metadata_search_issue_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    all.values()
        .map(|blob| {
            let v: Value = serde_json::from_str(blob).unwrap();
            serde_json::from_value(v["args"].clone()).unwrap()
        })
        .collect()
}

async fn run_job(app: &TestApp, job: server::jobs::metadata_search::SearchIssueJob) {
    server::jobs::metadata_search::handle_issue(job, apalis::prelude::Data::new(app.state()))
        .await
        .unwrap();
}

async fn candidates(app: &TestApp, run_id: Uuid) -> Vec<entity::metadata_run_candidate::Model> {
    let mut c = entity::metadata_run_candidate::Entity::find()
        .filter(entity::metadata_run_candidate::Column::RunId.eq(run_id))
        .all(&app.state().db)
        .await
        .unwrap();
    c.sort_by_key(|c| c.ordinal);
    c
}

async fn patch_settings(app: &TestApp, cookie: &str, body: Value) -> (StatusCode, Value) {
    call(
        app,
        cookie,
        Method::PATCH,
        "/api/admin/settings",
        Some(body),
    )
    .await
}

async fn library_slug(app: &TestApp, id: Uuid) -> String {
    entity::library::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug
}

/// Warm the 24 h issue-list cache the way an analysis / batch would.
async fn warm_lists(app: &TestApp, lists: &[(Source, &str)]) {
    for (src, id) in lists {
        server::metadata::coverage::provider_issues(&app.state(), *src, id)
            .await
            .unwrap_or_else(|e| panic!("{src:?} {id}: {e}"));
    }
}

// ───────── 1. compare mode uses coverage ─────────

/// The match dialog's search adds each provider's coverage-assigned issue
/// (ComicVine: 6211's #600; Metron: the range's 1713 #600) next to the
/// search results — the search still runs, so alternatives stay — and the
/// compare view defaults to those candidates. GCD has no coverage: it
/// falls back to today's search (and finds nothing here, so no column).
#[tokio::test]
async fn compare_mode_uses_coverage_assigned_candidates() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &[("600".into(), 2012, 1)], true).await;
    let issue_id = &ff.issues["600"];
    let issue_slug = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug;

    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!(
            "/api/series/{}/issues/{issue_slug}/metadata/search",
            ff.slug
        ),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    let run_id = Uuid::parse_str(body["run_id"].as_str().unwrap()).unwrap();
    let jobs = queued_issue_jobs(&app).await;
    assert_eq!(jobs.len(), 1);
    assert!(!jobs[0].direct_lookup, "the dialog keeps searching");
    assert!(jobs[0].coverage_candidates, "…and adds the coverage issue");
    for job in jobs {
        run_job(&app, job).await;
    }

    let cands = candidates(&app, run_id).await;
    let cv_600 = id_for(&m.cv_rows, "issue_number", "600");
    let metron_600 = id_for(&m.metron_rows, "number", "600");
    let cov = |src: &str| {
        cands
            .iter()
            .find(|c| c.source == src && c.score_breakdown.get("coverage").is_some())
            .unwrap_or_else(|| panic!("no {src} coverage candidate in {cands:?}"))
    };
    assert_eq!(cov("comicvine").external_id, cv_600);
    assert_eq!(
        cov("comicvine").score_breakdown["coverage"]["provider_series_id"],
        "6211"
    );
    assert_eq!(cov("metron").external_id, metron_600);
    assert_eq!(
        cov("metron").score_breakdown["coverage"]["provider_series_id"],
        "1713"
    );
    assert_eq!(cov("metron").score_breakdown["coverage"]["via_range"], true);

    // Additive: the searches still ran (alternatives), and GCD — with no
    // provider series — searched (today's behaviour).
    assert!(metron_searches(&m).await >= 1, "Metron still searched");
    assert!(count(&m.gcd, |_| true).await >= 1, "GCD searched");
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let lookups = run.query.unwrap()["coverage_lookups"].clone();
    let gcd = lookups
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["source"] == "gcd")
        .unwrap()
        .clone();
    assert_eq!(gcd["path"], "search");
    assert_eq!(gcd["fallback"], "no_target");

    // Compare view (no explicit columns): one column per provider, the
    // coverage candidate, labelled.
    let (st, diff) = call(
        &app,
        &cookie,
        Method::GET,
        &format!(
            "/api/series/{}/issues/{issue_slug}/metadata/composite-diff?run_id={run_id}",
            ff.slug
        ),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{diff}");
    let cols = diff["providers"].as_array().unwrap();
    let col = |src: &str| {
        cols.iter()
            .find(|c| c["source"] == src)
            .unwrap_or_else(|| panic!("no {src} column in {diff}"))
    };
    assert_eq!(col("comicvine")["external_id"], cv_600.as_str());
    assert_eq!(col("comicvine")["via_coverage"], true);
    assert_eq!(col("metron")["external_id"], metron_600.as_str());
    assert_eq!(col("metron")["via_coverage"], true);
    assert!(cols.iter().all(|c| c["source"] != "gcd"));
}

/// Without coverage (no provider series known) the dialog search is
/// today's search: no coverage candidate, every provider searched.
#[tokio::test]
async fn compare_mode_without_coverage_is_todays_search() {
    let m = mount(false).await;
    // A by-name ComicVine search first finds the volumes with that name,
    // then asks each for the number.
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ok(json!({
            "status_code": 1, "error": "OK",
            "results": [{"id": 6211, "name": "Fantastic Four", "start_year": "1961"}]
        })))
        .mount(&m.cv)
        .await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &[("600".into(), 2012, 1)], false).await;
    let issue_slug = entity::issue::Entity::find_by_id(ff.issues["600"].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!(
            "/api/series/{}/issues/{issue_slug}/metadata/search",
            ff.slug
        ),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    let run_id = Uuid::parse_str(body["run_id"].as_str().unwrap()).unwrap();
    for job in queued_issue_jobs(&app).await {
        run_job(&app, job).await;
    }
    assert!(
        candidates(&app, run_id)
            .await
            .iter()
            .all(|c| c.score_breakdown.get("coverage").is_none())
    );
    assert!(cv_searches(&m).await >= 1);
    assert!(metron_searches(&m).await >= 1);
    // No issue list or detail was read: nothing to look up.
    assert_eq!(
        count(&m.metron, |r| has_param(r, "series_id")).await,
        0,
        "no Metron issue list"
    );
}

// ───────── 2. issue-level refresh ─────────

/// Off (the default): the library refresh makes no issue batch and no
/// provider call.
#[tokio::test]
async fn issue_refresh_off_makes_no_calls() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), true).await;
    assert!(!app.state().cfg().metadata_issue_refresh_enabled);
    let lib = library_slug(&app, ff.library_id).await;
    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/libraries/{lib}/metadata/refresh?scope=stale"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["issue_refresh"]["enabled"], false, "{body}");
    assert_eq!(body["issue_refresh"]["issues_selected"], 0);
    assert!(body["issue_refresh"]["batch_id"].is_null());
    assert!(queued_issue_jobs(&app).await.is_empty());
    let batches = entity::metadata_batch::Entity::find()
        .filter(entity::metadata_batch::Column::Scope.eq("issue_refresh"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(batches.is_empty());
    assert_eq!(total_requests(&m).await, 0, "no provider call");
}

/// On: stale covered issues, never-synced first then the oldest sync, at
/// most the cap per provider; recently synced issues are left alone; the
/// runs form one `issue_refresh` batch whose jobs ask only the covering
/// providers, by direct lookup only — a miss is never searched.
#[tokio::test]
async fn issue_refresh_on_is_bounded_and_stale_first() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    // 1–110, 500–588, 600–611 = 211 issues (#71–110 aren't in any list).
    let rows: Vec<(String, i32, i32)> = (1..=110)
        .chain(500..=588)
        .chain(600..=611)
        .map(|n| {
            let (y, mo) = if n >= 600 { (2012, 1) } else { (1998, 1) };
            (n.to_string(), y, mo)
        })
        .collect();
    assert_eq!(rows.len(), 211);
    let ff = seed_ff(&app, &rows, true).await;
    let db = &app.state().db;
    // #1–9 synced last month (fresh: excluded); #10–21 synced in 2020-01…12
    // (stale, oldest first); the other 190 never synced.
    for n in 1..=9 {
        db.execute_unprepared(&format!(
            "UPDATE issues SET last_metadata_sync_at = NOW() - interval '30 days' WHERE id = '{}'",
            ff.issues[&n.to_string()]
        ))
        .await
        .unwrap();
    }
    for (i, n) in (10..=21).enumerate() {
        db.execute_unprepared(&format!(
            "UPDATE issues SET last_metadata_sync_at = '2020-{:02}-01T00:00:00Z' WHERE id = '{}'",
            i + 1,
            ff.issues[&n.to_string()]
        ))
        .await
        .unwrap();
    }

    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({"metadata.issue_refresh_enabled": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let lib = library_slug(&app, ff.library_id).await;
    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/libraries/{lib}/metadata/refresh?scope=stale"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    let out = &body["issue_refresh"];
    assert_eq!(out["enabled"], true, "{body}");
    assert_eq!(out["issues_selected"], 200, "{out}");
    let per: HashMap<String, u64> = out["per_provider"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            assert_eq!(p["cap"], 200);
            (
                p["source"].as_str().unwrap().to_owned(),
                p["issues"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(per["comicvine"], 200);
    assert_eq!(per["metron"], 200);
    assert_eq!(per["gcd"], 0, "GCD has no coverage here");
    assert_eq!(out["jobs_enqueued"], 200);
    let batch_id = Uuid::parse_str(out["batch_id"].as_str().unwrap()).unwrap();
    let batch = entity::metadata_batch::Entity::find_by_id(batch_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.scope, "issue_refresh");
    assert_eq!(batch.items_total, 200);
    assert_eq!(batch.trigger_kind, "bulk_action");

    // Which issues: never-synced (190) + the 10 oldest stale ones
    // (#10–19); #20–21 (Nov / Dec 2020) wait for the next run; the fresh
    // #1–9 aren't eligible.
    let jobs = queued_issue_jobs(&app).await;
    assert_eq!(jobs.len(), 200);
    let picked: std::collections::HashSet<&str> =
        jobs.iter().map(|j| j.facts.issue_number.as_str()).collect();
    for n in 10..=19 {
        assert!(picked.contains(n.to_string().as_str()), "#{n} picked");
    }
    for n in [1, 9, 20, 21] {
        assert!(!picked.contains(n.to_string().as_str()), "#{n} not picked");
    }
    for j in &jobs {
        assert_eq!(
            j.direct_only.as_deref(),
            Some(&[Source::ComicVine, Source::Metron][..]),
            "only the covering providers"
        );
    }
    let runs = entity::metadata_run::Entity::find()
        .filter(entity::metadata_run::Column::BatchId.eq(batch_id))
        .all(db)
        .await
        .unwrap();
    assert_eq!(runs.len(), 200);
    assert!(runs.iter().all(|r| r.trigger_kind == "bulk_action"));
    assert!(runs.iter().all(|r| {
        let mut p = r.providers.clone();
        p.sort();
        p == vec!["comicvine".to_owned(), "metron".to_owned()]
    }));

    // Run three of them: listed (#10, #600 via the range) and unlisted
    // (#71) — direct lookups only, no search ever.
    for n in ["10", "600", "71"] {
        let job = jobs
            .iter()
            .find(|j| j.facts.issue_number == n)
            .unwrap()
            .clone();
        run_job(&app, job).await;
    }
    assert_eq!(cv_searches(&m).await, 0);
    assert_eq!(metron_searches(&m).await, 0);
    assert_eq!(count(&m.gcd, |_| true).await, 0);
    let run_for = |n: &str| {
        let id = &ff.issues[n];
        runs.iter()
            .find(|r| r.scope_entity_id.as_deref() == Some(id.as_str()))
            .unwrap()
            .id
    };
    let c71 = candidates(&app, run_for("71")).await;
    assert!(
        c71.is_empty(),
        "an unlisted number is recorded, not searched"
    );
    let c600 = candidates(&app, run_for("600")).await;
    assert_eq!(c600.len(), 2, "{c600:?}");
    assert!(
        c600.iter()
            .all(|c| c.score_breakdown.get("coverage").is_some())
    );

    // A second click right away re-proposes nothing it just asked.
    let (_, again) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/libraries/{lib}/metadata/refresh?scope=stale"),
        None,
    )
    .await;
    assert_eq!(again["issue_refresh"]["issues_selected"], 2, "{again}");
}

/// The cap is per provider. ComicVine covers every issue (6211), Metron
/// only #600 (the 1713 range; no Metron main here). With a cap of 2,
/// ComicVine's quota goes to #1–2; #3 has no provider left and waits; #600
/// still goes to Metron alone.
#[tokio::test]
async fn issue_refresh_cap_is_per_provider() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let ff = seed_ff(
        &app,
        &[
            ("1".into(), 1998, 1),
            ("2".into(), 1998, 2),
            ("3".into(), 1998, 3),
            ("600".into(), 2012, 1),
        ],
        false,
    )
    .await;
    set_external_id(
        &app.state().db,
        "series",
        &ff.series_id.to_string(),
        &Identifier::with_canonical_url(Source::ComicVine, "6211".to_owned(), "series"),
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();
    insert_range(&app, ff.series_id, "600", "611").await;
    let picks = server::metadata::refresh::select_issue_refresh(
        &app.state().db,
        ff.library_id,
        &[Source::ComicVine, Source::Metron],
        2,
        180,
    )
    .await
    .unwrap();
    let got: Vec<(&str, &Vec<Source>)> = picks
        .iter()
        .map(|p| (p.issue_id.as_str(), &p.sources))
        .collect();
    assert_eq!(
        got,
        vec![
            (ff.issues["1"].as_str(), &vec![Source::ComicVine]),
            (ff.issues["2"].as_str(), &vec![Source::ComicVine]),
            (ff.issues["600"].as_str(), &vec![Source::Metron]),
        ]
    );
}

/// `metadata.issue_refresh_enabled` / `metadata.issue_refresh_per_provider_cap`
/// through registry → Config → apply → admin settings, with range checks.
#[tokio::test]
async fn issue_refresh_settings_round_trip() {
    let app = TestApp::spawn().await;
    let cookie = register_admin(&app).await;
    let cfg = app.state().cfg();
    assert!(!cfg.metadata_issue_refresh_enabled, "off by default");
    assert_eq!(cfg.metadata_issue_refresh_per_provider_cap, 200);

    let (st, body) = patch_settings(
        &app,
        &cookie,
        json!({
            "metadata.issue_refresh_enabled": true,
            "metadata.issue_refresh_per_provider_cap": 50,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let cfg = app.state().cfg();
    assert!(cfg.metadata_issue_refresh_enabled);
    assert_eq!(cfg.metadata_issue_refresh_per_provider_cap, 50);
    let values = body["values"].as_array().unwrap();
    let v = |k: &str| {
        values
            .iter()
            .find(|v| v["key"] == k)
            .unwrap_or_else(|| panic!("{k} not in values"))["value"]
            .clone()
    };
    assert_eq!(v("metadata.issue_refresh_enabled"), true);
    assert_eq!(v("metadata.issue_refresh_per_provider_cap"), 50);

    for bad in [json!(0), json!(1001), json!("many")] {
        let (st, _) = patch_settings(
            &app,
            &cookie,
            json!({"metadata.issue_refresh_per_provider_cap": bad}),
        )
        .await;
        assert!(st.is_client_error(), "{bad}: {st}");
    }
    assert_eq!(
        app.state().cfg().metadata_issue_refresh_per_provider_cap,
        50
    );

    let (st, _) = patch_settings(
        &app,
        &cookie,
        json!({
            "metadata.issue_refresh_enabled": null,
            "metadata.issue_refresh_per_provider_cap": null,
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cfg = app.state().cfg();
    assert!(!cfg.metadata_issue_refresh_enabled);
    assert_eq!(cfg.metadata_issue_refresh_per_provider_cap, 200);
}

// ───────── 3. exact missing issues ─────────

async fn collection(app: &TestApp, cookie: &str, slug: &str) -> Value {
    let (st, body) = call(
        app,
        cookie,
        Method::GET,
        &format!("/api/series/{slug}/collection"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body
}

/// The owner's FF folder: with ComicVine 6211 and Metron 1711 + the
/// #600–611 → 1713 range accepted and their lists cached, the report is a
/// provider manifest — nothing missing (no more #71–499), no provider
/// request made by the report.
#[tokio::test]
async fn fantastic_four_collection_uses_the_provider_manifest() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), true).await;
    warm_lists(
        &app,
        &[
            (Source::ComicVine, "6211"),
            (Source::Metron, "1711"),
            (Source::Metron, "1713"),
        ],
    )
    .await;
    let before = total_requests(&m).await;

    let r = collection(&app, &cookie, &ff.slug).await;
    assert_eq!(total_requests(&m).await, before, "the report never fetches");
    assert_eq!(
        r["expected_source"], "provider_manifest",
        "{}",
        r["manifest"]
    );
    assert_eq!(r["main_run"]["missing"], json!([]), "no #71–499");
    assert_eq!(r["main_run"]["possibly_missing"], json!([]));
    assert_eq!(r["main_run"]["trailing_missing"], 0);
    let man = &r["manifest"];
    assert_eq!(man["used"], true);
    assert_eq!(man["missing"], json!([]));
    assert_eq!(man["possibly_missing"], json!([]));
    assert!(man["note"].is_null());
    let providers = man["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2);
    let metron = providers.iter().find(|p| p["source"] == "metron").unwrap();
    assert_eq!(metron["loaded"], true);
    assert_eq!(metron["listed_count"], 173, "{metron}");
    let segs = metron["series"].as_array().unwrap();
    assert_eq!(segs[1]["provider_series_id"], "1713");
    assert_eq!(segs[1]["via_range"], true);
    assert_eq!(segs[1]["range_low"], "600");
}

/// Agreement vs disagreement: a number every provider lists is missing;
/// one only some list (Metron's range stops at #610, so Metron no longer
/// expects #611 while ComicVine does) is possibly missing, with each
/// provider's view.
#[tokio::test]
async fn missing_needs_every_provider_possibly_missing_otherwise() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let rows: Vec<_> = ff_rows()
        .into_iter()
        .filter(|(n, _, _)| n != "50" && n != "611")
        .collect();
    let ff = seed_ff(&app, &rows, false).await;
    let db = &app.state().db;
    for (src, id) in [(Source::ComicVine, "6211"), (Source::Metron, "1711")] {
        set_external_id(
            db,
            "series",
            &ff.series_id.to_string(),
            &Identifier::with_canonical_url(src, id.to_owned(), "series"),
            SetBy::Provider(src),
        )
        .await
        .unwrap();
    }
    insert_range(&app, ff.series_id, "600", "610").await;
    warm_lists(
        &app,
        &[
            (Source::ComicVine, "6211"),
            (Source::Metron, "1711"),
            (Source::Metron, "1713"),
        ],
    )
    .await;

    let r = collection(&app, &cookie, &ff.slug).await;
    assert_eq!(r["expected_source"], "provider_manifest");
    assert_eq!(r["main_run"]["missing"], json!([50]));
    assert_eq!(r["main_run"]["possibly_missing"], json!([611]));
    let man = &r["manifest"];
    assert_eq!(man["missing"], json!(["50"]));
    assert_eq!(
        man["possibly_missing"],
        json!([{
            "number": "611",
            "providers": [
                {"source": "comicvine", "listing": "listed"},
                {"source": "metron", "listing": "not_listed"},
            ],
        }])
    );
}

/// Accepted coverage but no cached list: the report says so and keeps the
/// interpolated fallback — it never fetches the list itself.
#[tokio::test]
async fn uncached_provider_list_is_reported_not_fetched() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), true).await;

    let r = collection(&app, &cookie, &ff.slug).await;
    assert_eq!(total_requests(&m).await, 0, "no provider request");
    assert_eq!(r["expected_source"], "series_total");
    let man = &r["manifest"];
    assert_eq!(man["used"], false);
    assert_eq!(
        man["note"],
        "ComicVine, Metron provider lists not loaded — run Analyze coverage"
    );
    // The interpolated fallback is unchanged: #71–499 inferred.
    let missing = r["main_run"]["missing"].as_array().unwrap();
    assert!(missing.contains(&json!(71)) && missing.contains(&json!(499)));

    // Partly cached: ComicVine votes, Metron can't confirm → its numbers
    // are only "possibly missing".
    warm_lists(&app, &[(Source::ComicVine, "6211")]).await;
    let r = collection(&app, &cookie, &ff.slug).await;
    assert_eq!(r["expected_source"], "provider_manifest");
    assert_eq!(r["main_run"]["missing"], json!([]));
    assert_eq!(
        r["manifest"]["note"],
        "Metron provider list not loaded — run Analyze coverage"
    );

    // No accepted coverage at all: no manifest, interpolation as before.
    let plain = seed_ff(&app, &[("1".into(), 1998, 1), ("5".into(), 1998, 5)], false).await;
    let r = collection(&app, &cookie, &plain.slug).await;
    assert_eq!(r["expected_source"], "series_total");
    assert!(r["manifest"].is_null());
    assert_eq!(r["main_run"]["missing"], json!([2, 3, 4]));
}

// ───────── 4. "not in your library" links ─────────

async fn analyze(app: &TestApp, cookie: &str, slug: &str) -> Value {
    let (st, job) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/analyze"),
        Some(json!({"auto_accept": false})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{job}");
    let job_id = Uuid::parse_str(job["job_id"].as_str().unwrap()).unwrap();
    server::jobs::provider_coverage::process(&app.state(), job_id)
        .await
        .unwrap();
    let (st, body) = call(
        app,
        cookie,
        Method::GET,
        &format!("/api/series/{slug}/provider-coverage/analysis"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "done", "{body}");
    body
}

async fn accept_metron(app: &TestApp, cookie: &str, slug: &str) -> Value {
    let (st, out) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/accept"),
        Some(json!({"source": "metron"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    out
}

async fn externals(app: &TestApp, cookie: &str, slug: &str) -> Vec<Value> {
    let (st, body) = call(
        app,
        cookie,
        Method::GET,
        &format!("/api/series/{slug}/relationships"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    body["external"].as_array().unwrap().clone()
}

async fn ext_rows(
    app: &TestApp,
    series_id: Uuid,
) -> Vec<entity::series_external_relationship::Model> {
    entity::series_external_relationship::Entity::find()
        .filter(entity::series_external_relationship::Column::FromSeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

/// Accepting Metron's coverage writes the #600–611 → 1713 range; 1713
/// also lists #612–615, which the folder lacks → "Continued by Fantastic
/// Four (2012) — not in your library · Has #612–615". Re-accepting doesn't
/// duplicate it; dismissing it is remembered.
#[tokio::test]
async fn coverage_range_creates_a_not_in_library_link() {
    let m = mount(true).await;
    let app = TestApp::spawn_with_metron_and_gcd(m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), false).await;

    let body = analyze(&app, &cookie, &ff.slug).await;
    let metron = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == "metron")
        .unwrap();
    assert_eq!(metron["main_series_id"], "1711", "{metron}");
    let out = accept_metron(&app, &cookie, &ff.slug).await;
    assert_eq!(out["ranges_created"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["external_links"], 1, "{out}");

    let ext = externals(&app, &cookie, &ff.slug).await;
    assert_eq!(ext.len(), 1, "{ext:?}");
    assert_eq!(ext[0]["kind"], "continued_by");
    assert_eq!(ext[0]["kind_label"], "Continued by");
    assert_eq!(ext[0]["source"], "metron");
    assert_eq!(ext[0]["provider_series_id"], "1713");
    assert_eq!(ext[0]["name"], "Fantastic Four");
    assert_eq!(ext[0]["year"], 2012);
    assert_eq!(ext[0]["set_by"], "provider");
    assert_eq!(ext[0]["note"], "Has #612–615");

    // Re-accept: refreshed in place, not duplicated.
    accept_metron(&app, &cookie, &ff.slug).await;
    assert_eq!(ext_rows(&app, ff.series_id).await.len(), 1);

    // Dismiss (a provider link is dismissed, not deleted) → never re-created.
    let id = ext[0]["id"].as_str().unwrap();
    let (st, _) = call(
        &app,
        &cookie,
        Method::DELETE,
        &format!("/api/series/{}/external-relationships/{id}", ff.slug),
        None,
    )
    .await;
    assert!(st.is_success(), "{st}");
    accept_metron(&app, &cookie, &ff.slug).await;
    let rows = ext_rows(&app, ff.series_id).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].dismissed_at.is_some(), "dismissal kept");
    assert!(externals(&app, &cookie, &ff.slug).await.is_empty());
}

/// No duplicate of a Metron `associated` link: an `associated` row for the
/// same provider series wins over the coverage row, whichever comes first.
#[tokio::test]
async fn coverage_links_do_not_duplicate_associated_links() {
    use server::metadata::provider::ProviderSeriesRef;
    use server::relationships::external::record_provider_links;

    let m = mount(true).await;
    let app = TestApp::spawn_with_metron_and_gcd(m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), false).await;
    analyze(&app, &cookie, &ff.slug).await;
    accept_metron(&app, &cookie, &ff.slug).await;
    let rows = ext_rows(&app, ff.series_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].evidence["field"], "coverage");

    // A Metron series apply now records 1713 as `associated`.
    let db = &app.state().db;
    let series = entity::series::Entity::find_by_id(ff.series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let link = ProviderSeriesRef {
        source: Source::Metron,
        id: "1713".into(),
        label: "Fantastic Four (2012)".into(),
        name: "Fantastic Four".into(),
        year: Some(2012),
        url: None,
    };
    record_provider_links(db, &series, Source::Metron, Some("1711"), None, &[link])
        .await
        .unwrap();
    let rows = ext_rows(&app, ff.series_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].evidence["field"], "associated");

    // A later coverage accept leaves it alone.
    let out = accept_metron(&app, &cookie, &ff.slug).await;
    assert_eq!(out["external_links"], 0);
    let rows = ext_rows(&app, ff.series_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].evidence["field"], "associated");
}

/// A range series the folder holds completely (the recorded 1713 lists
/// only #600–611) is no "missing volume": no link.
#[tokio::test]
async fn fully_owned_range_series_gets_no_link() {
    let m = mount(false).await;
    let app = TestApp::spawn_with_metron_and_gcd(m.metron.uri(), m.gcd.uri()).await;
    let cookie = register_admin(&app).await;
    let ff = seed_ff(&app, &ff_rows(), false).await;
    analyze(&app, &cookie, &ff.slug).await;
    let out = accept_metron(&app, &cookie, &ff.slug).await;
    assert_eq!(out["ranges_created"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["external_links"], 0);
    assert!(ext_rows(&app, ff.series_id).await.is_empty());
}
