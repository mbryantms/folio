//! Batch direct lookups through series coverage
//! ([`server::metadata::direct_lookup`]).
//!
//! A series "Fetch metadata" batch answers each provider from the issue's
//! provider series' cached issue list plus one cached detail fetch instead
//! of a search, when that series is known (series-level external id or a
//! covering range) and lists the number with an agreeing cover date.
//! Everything else falls back to today's search.
//!
//! Fixture: the owner's Fantastic Four (folder "2001", the 1998 volume) —
//! ComicVine 6211 lumps the run, Metron splits #600–611 into 1713 (a
//! range row here). The issue lists are the **recorded** responses in
//! `tests/fixtures/fantastic_four/`; the issue *details* are synthesised
//! per request from the same recorded list rows (id, number, cover date,
//! series), since only list pages were recorded. GCD has no link, so it
//! is the "no coverage → search" provider; its searches answer empty.

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
use server::metadata::matcher::{self, IssueQueryFacts, Thresholds};
use server::metadata::provider::IssueCandidate;
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

/// Recorded list rows keyed by provider issue id.
fn rows_by_id(files: &[&str]) -> HashMap<String, Value> {
    files
        .iter()
        .flat_map(|f| fixture(f)["results"].as_array().unwrap().clone())
        .map(|r| (r["id"].to_string(), r))
        .collect()
}

/// Provider issue id for a number in recorded list rows.
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
            "image": row["image"],
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

async fn mount() -> Mocks {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;

    // ── ComicVine: recorded 6211 issue list; synthesized details;
    //    searches (`/issues` without the slash, `/search`) answer empty.
    for (offset, file) in [
        ("0", "cv_issues_6211_p1.json"),
        ("100", "cv_issues_6211_p2.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:6211"))
            .and(query_param("offset", offset))
            .respond_with(ok(fixture(file)))
            .mount(&cv)
            .await;
    }
    let cv_rows = rows_by_id(&["cv_issues_6211_p1.json", "cv_issues_6211_p2.json"]);
    Mock::given(method("GET"))
        .and(path_regex(r"^/issue/4000-\d+/?$"))
        .respond_with(CvDetail(cv_rows.clone()))
        .mount(&cv)
        .await;
    for p in ["/issues", "/search"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ok(json!({"status_code": 1, "error": "OK", "results": []})))
            .mount(&cv)
            .await;
    }

    // ── Metron: recorded 1711 / 1713 lists (paged), synthesized details;
    //    an issue search (no `page`) answers empty.
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
            .mount(&metron)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param_is_missing("page"))
        .respond_with(ok(empty_page()))
        .mount(&metron)
        .await;
    let metron_rows = rows_by_id(&[
        "metron_issues_1711_p1.json",
        "metron_issues_1711_p2.json",
        "metron_issues_1713_p1.json",
    ]);
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/issue/\d+/?$"))
        .respond_with(MetronDetail(metron_rows.clone()))
        .mount(&metron)
        .await;

    // ── GCD: unlinked, so every issue searches; everything answers empty.
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

/// Requests a mock server received whose path satisfies `pred` (and,
/// for Metron's shared `/api/issue/` path, whose query does).
async fn count(server: &MockServer, pred: impl Fn(&WmRequest) -> bool) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| pred(r))
        .count()
}

fn has_param(r: &WmRequest, k: &str) -> bool {
    r.url.query_pairs().any(|(key, _)| key == k)
}

async fn cv_searches(m: &Mocks) -> usize {
    count(&m.cv, |r| {
        r.url.path() == "/issues" || r.url.path() == "/search"
    })
    .await
}

async fn metron_searches(m: &Mocks) -> usize {
    count(&m.metron, |r| {
        r.url.path() == "/api/issue/" && has_param(r, "number")
    })
    .await
}

async fn cv_details(m: &Mocks) -> usize {
    count(&m.cv, |r| r.url.path().starts_with("/issue/4000-")).await
}

async fn metron_details(m: &Mocks) -> usize {
    count(&m.metron, |r| {
        r.url.path().starts_with("/api/issue/") && r.url.path() != "/api/issue/"
    })
    .await
}

// ───────── local library ─────────

/// Seed "Fantastic Four" (2001, Marvel) with `(number, year, month)`
/// issues, link ComicVine 6211 + Metron 1711 and the Metron #600–611 →
/// 1713 range. Returns `(series_id, slug)`.
async fn seed_ff(app: &TestApp, tmp: &std::path::Path, issues: &[(&str, i32, i32)]) -> Uuid {
    let db = app.state().db.clone();
    let lib = seed_library(&db, tmp).await;
    let mut seed = SeriesSeed::new(lib, "Fantastic Four");
    seed.year = Some(2001);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    for (i, (raw, y, m)) in issues.iter().enumerate() {
        let p = tmp.join(format!("ff-{raw}.cbz"));
        let id = IssueSeed::new(
            lib,
            series_id,
            &p,
            format!("ff {raw}").as_bytes(),
            i as f64 + 1.0,
        )
        .insert(&db)
        .await;
        db.execute_unprepared(&format!(
            "UPDATE issues SET number_raw = '{raw}', year = {y}, month = {m} WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    }
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
    let now = chrono::Utc::now().fixed_offset();
    entity::series_provider_range::ActiveModel {
        id: Set(Uuid::new_v4()),
        series_id: Set(series_id),
        source: Set("metron".into()),
        provider_series_id: Set("1713".into()),
        provider_series_url: Set(None),
        provider_series_name: Set(Some("Fantastic Four".into())),
        range_low: Set(Some("600".into())),
        range_high: Set(Some("611".into())),
        declared_year: Set(Some(2012)),
        set_by: Set("cross_reference".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    series_id
}

// ───────── HTTP + job helpers ─────────

struct Authed {
    session: String,
    csrf: String,
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
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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
            .map(|c| c.trim_start_matches(prefix).to_owned())
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn send(app: &TestApp, auth: &Authed, m: Method, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(m)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("x-csrf-token", &auth.csrf)
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

/// Run every queued `SearchIssueJob` through the worker entry point, in
/// queue order (the worker's concurrency is 1).
async fn run_issue_jobs(app: &TestApp) -> usize {
    use server::jobs::metadata_search::SearchIssueJob;
    let storage = app.state().jobs.metadata_search_issue_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let jobs: Vec<SearchIssueJob> = all
        .values()
        .map(|blob| {
            let v: Value = serde_json::from_str(blob).unwrap();
            serde_json::from_value(v["args"].clone()).unwrap()
        })
        .collect();
    let n = jobs.len();
    for job in jobs {
        assert!(job.direct_lookup, "batch children may use direct lookups");
        server::jobs::metadata_search::handle_issue(job, apalis::prelude::Data::new(app.state()))
            .await
            .unwrap();
    }
    n
}

/// Start a series batch and run its children. Returns the batch id.
async fn run_batch(app: &TestApp, auth: &Authed, series_id: Uuid, expected: u64) -> Uuid {
    let (status, body) = send(
        app,
        auth,
        Method::POST,
        &format!("/api/series/{series_id}/metadata/batch?scope=all"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["items_total"].as_u64(), Some(expected));
    assert_eq!(run_issue_jobs(app).await as u64, expected);
    Uuid::parse_str(body["batch_id"].as_str().unwrap()).unwrap()
}

/// `number_raw → (source → candidate rows)` for a batch.
async fn candidates_by_number(
    app: &TestApp,
    batch_id: Uuid,
) -> HashMap<String, (Value, Vec<entity::metadata_run_candidate::Model>)> {
    let db = &app.state().db;
    let runs = entity::metadata_run::Entity::find()
        .filter(entity::metadata_run::Column::BatchId.eq(batch_id))
        .all(db)
        .await
        .unwrap();
    let mut out = HashMap::new();
    for r in runs {
        assert_eq!(r.status, "completed", "run {} {:?}", r.id, r.error_summary);
        let number = r.query.as_ref().unwrap()["issue_number"]
            .as_str()
            .unwrap()
            .to_owned();
        let cands = entity::metadata_run_candidate::Entity::find()
            .filter(entity::metadata_run_candidate::Column::RunId.eq(r.id))
            .all(db)
            .await
            .unwrap();
        out.insert(number, (r.query.clone().unwrap(), cands));
    }
    out
}

fn lookup_row<'a>(status: &'a Value, source: &str) -> &'a Value {
    status["aggregate"]["lookups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source"] == source)
        .unwrap_or_else(|| panic!("no {source} lookup row in {status}"))
}

// ───────── tests ─────────

/// "All issues" over a covered series: ComicVine and Metron answer every
/// issue from the cached issue lists (each list fetched once) with one
/// detail fetch per issue and **zero searches**; candidates carry the
/// listed provider issue ids and the coverage reason. GCD, with no
/// provider series, searches every issue. The batch header reports both.
#[tokio::test]
async fn all_issues_batch_uses_direct_lookups_for_covered_issues() {
    let m = mount().await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let issues = [
        ("1", 1998, 1),
        ("2", 1998, 2),
        ("3", 1998, 3),
        ("4", 1998, 4),
        ("5", 1998, 5),
        ("6", 1998, 6),
        ("7", 1998, 7),
        ("8", 1998, 8),
        ("600", 2012, 1),
        ("601", 2012, 2),
    ];
    let series_id = seed_ff(&app, tmp.path(), &issues).await;

    let batch_id = run_batch(&app, &auth, series_id, 10).await;

    // Zero searches on the covered providers; one detail per issue.
    assert_eq!(cv_searches(&m).await, 0, "ComicVine searched");
    assert_eq!(metron_searches(&m).await, 0, "Metron searched");
    assert_eq!(cv_details(&m).await, 10);
    assert_eq!(metron_details(&m).await, 10);
    // Each issue list was fetched once and then served from the cache:
    // CV 6211 (2 pages), Metron 1711 (2 pages) + 1713 (1 page).
    assert_eq!(
        count(&m.cv, |r| r.url.path() == "/issues/").await,
        2,
        "CV list pages"
    );
    assert_eq!(
        count(&m.metron, |r| has_param(r, "series_id")
            && has_param(r, "page"))
        .await,
        3,
        "Metron list pages"
    );
    // GCD has no provider series: it searched every issue.
    assert!(
        count(&m.gcd, |_| true).await >= 10,
        "GCD should search each issue"
    );

    // Candidates are the listed provider issues, with the coverage note.
    let cfg = app.state().cfg();
    let thresholds = Thresholds::new(
        cfg.metadata_auto_apply_threshold as f32,
        cfg.metadata_match_medium_threshold as f32,
    );
    let by_number = candidates_by_number(&app, batch_id).await;
    assert_eq!(by_number.len(), 10);
    for (number, _, _) in issues {
        let (query, cands) = &by_number[number];
        let cv = cands
            .iter()
            .find(|c| c.source == "comicvine")
            .unwrap_or_else(|| panic!("#{number}: no ComicVine candidate"));
        assert_eq!(cv.external_id, id_for(&m.cv_rows, "issue_number", number));
        let mt = cands
            .iter()
            .find(|c| c.source == "metron")
            .unwrap_or_else(|| panic!("#{number}: no Metron candidate"));
        assert_eq!(mt.external_id, id_for(&m.metron_rows, "number", number));
        for c in [cv, mt] {
            let note = &c.score_breakdown["coverage"];
            assert_eq!(
                note["reason"], "matched by series coverage (number + cover date)",
                "#{number} {}: {}",
                c.source, c.score_breakdown
            );
            assert_eq!(note["date"], "confirmed");
            // Scored by the matcher, not forced: no cover was compared, and
            // the stored bucket is exactly what the matcher gives the
            // looked-up issue against the run's facts.
            assert!(c.score_breakdown["cover_hamming"].is_null());
            let facts: IssueQueryFacts = serde_json::from_value(query.clone()).unwrap();
            let cand: IssueCandidate = serde_json::from_value(c.candidate.clone()).unwrap();
            let bucket = matcher::score_issue(&facts, &cand).bucket(thresholds);
            assert_eq!(c.bucket, bucket.as_str(), "#{number} {}", c.source);
        }
        // #600–601 resolve through the Metron range to 1713.
        let via_range = mt.score_breakdown["coverage"]["via_range"]
            .as_bool()
            .unwrap();
        assert_eq!(via_range, number.starts_with("60"), "#{number}");
        assert_eq!(
            mt.score_breakdown["coverage"]["provider_series_id"],
            if via_range { "1713" } else { "1711" }
        );
    }

    // The batch header's per-provider tally.
    let (status, body) = send(
        &app,
        &auth,
        Method::GET,
        &format!("/api/metadata/batch/{batch_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cv = lookup_row(&body, "comicvine");
    assert_eq!(
        (cv["direct"].as_i64(), cv["search"].as_i64()),
        (Some(10), Some(0))
    );
    let mt = lookup_row(&body, "metron");
    assert_eq!(
        (mt["direct"].as_i64(), mt["search"].as_i64()),
        (Some(10), Some(0))
    );
    let gcd = lookup_row(&body, "gcd");
    assert_eq!(
        (gcd["direct"].as_i64(), gcd["search"].as_i64()),
        (Some(0), Some(10))
    );
    assert_eq!(
        gcd["fallbacks"],
        json!([{"reason": "no_target", "count": 10}])
    );
    // Display order follows the coverage card: ComicVine, Metron, GCD.
    let order: Vec<&str> = body["aggregate"]["lookups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["source"].as_str().unwrap())
        .collect();
    assert_eq!(order, ["comicvine", "metron", "gcd"]);
}

/// A cover date that conflicts with every same-numbered listed issue, and
/// a number the provider series doesn't list, both fall back to the
/// ordinary search — and the run records why.
#[tokio::test]
async fn date_conflict_and_unlisted_numbers_fall_back_to_search() {
    let m = mount().await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let series_id = seed_ff(
        &app,
        tmp.path(),
        &[
            ("1", 1998, 1),
            // Listed as 1998-03; a 1985 cover can't be that issue.
            ("3", 1985, 3),
            // Neither 6211 nor 1711 lists #450.
            ("450", 2001, 6),
        ],
    )
    .await;

    let batch_id = run_batch(&app, &auth, series_id, 3).await;

    // Only #1 used a direct lookup; #3 and #450 searched (narrowed, then
    // the broad fallback when the narrowed search came back empty).
    assert_eq!(cv_details(&m).await, 1);
    assert_eq!(metron_details(&m).await, 1);
    assert!(cv_searches(&m).await >= 2, "CV searched the misses");
    assert!(metron_searches(&m).await >= 2, "Metron searched the misses");

    let db = &app.state().db;
    let runs = entity::metadata_run::Entity::find()
        .filter(entity::metadata_run::Column::BatchId.eq(batch_id))
        .all(db)
        .await
        .unwrap();
    let path_of = |number: &str, source: &str| -> Value {
        let run = runs
            .iter()
            .find(|r| r.query.as_ref().unwrap()["issue_number"] == number)
            .unwrap();
        run.query.as_ref().unwrap()["coverage_lookups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["source"] == source)
            .unwrap()
            .clone()
    };
    for source in ["comicvine", "metron"] {
        assert_eq!(path_of("1", source)["path"], "direct");
        assert_eq!(
            path_of("3", source),
            json!({"source": source, "path": "search", "fallback": "date_conflict"})
        );
        assert_eq!(
            path_of("450", source),
            json!({"source": source, "path": "search", "fallback": "not_listed"})
        );
    }

    let (_, body) = send(
        &app,
        &auth,
        Method::GET,
        &format!("/api/metadata/batch/{batch_id}"),
    )
    .await;
    let cv = lookup_row(&body, "comicvine");
    assert_eq!(
        (cv["direct"].as_i64(), cv["search"].as_i64()),
        (Some(1), Some(2))
    );
    let reasons: Vec<&str> = cv["fallbacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"date_conflict") && reasons.contains(&"not_listed"));
}

/// A single-issue search (the match dialog) keeps searching even when the
/// series is covered — only batch children take the direct path.
#[tokio::test]
async fn dialog_issue_search_still_searches() {
    let m = mount().await;
    let app = TestApp::spawn_with_all_providers(m.cv.uri(), m.metron.uri(), m.gcd.uri()).await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let series_id = seed_ff(&app, tmp.path(), &[("1", 1998, 1)]).await;
    let db = &app.state().db;
    let s = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let i = entity::issue::Entity::find()
        .filter(entity::issue::Column::SeriesId.eq(series_id))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let (status, body) = send(
        &app,
        &auth,
        Method::POST,
        &format!("/api/series/{}/issues/{}/metadata/search", s.slug, i.slug),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let storage = app.state().jobs.metadata_search_issue_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    let v: Value = serde_json::from_str(all.values().next().unwrap()).unwrap();
    assert_eq!(v["args"]["direct_lookup"], false);
}

/// A distinct, decodable PNG (solid fill).
fn png_bytes(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(32, 32, image::Rgb(rgb));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

/// The cover pHash still decides the bucket for a direct lookup: a
/// looked-up issue whose cover matches the local cover lands HIGH through
/// the ordinary ladder (no search), and one whose cover disagrees is
/// treated as a wrong mapping and falls back to the search.
#[tokio::test]
async fn cover_hash_confirms_or_rejects_a_direct_lookup() {
    use server::metadata::direct_lookup::DirectLookupCtx;
    use server::metadata::metron::MetronClient;
    use server::metadata::orchestrator::{self, SearchOpts, StartRunArgs, StoredQuery};
    use server::metadata::provider::MetadataProvider;
    use server::metadata::range_map::EffectiveTarget;
    use std::sync::Arc;

    let m = mount().await;
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib, "Fantastic Four").insert(&db).await;
    let p = tmp.path().join("ff-1.cbz");
    let issue_id = IssueSeed::new(lib, series_id, &p, b"ff 1", 1.0)
        .insert(&db)
        .await;
    let local = server::metadata::phash::phash(
        &image::load_from_memory(&png_bytes([200, 40, 40])).unwrap(),
    );
    let now = chrono::Utc::now().fixed_offset();
    entity::issue_cover::ActiveModel {
        id: Set(Uuid::now_v7()),
        issue_id: Set(issue_id.clone()),
        kind: Set("primary".into()),
        ordinal: Set(0),
        source_provider: Set(Some("archive_extracted".into())),
        source_external_id: Set(None),
        source_url: Set(None),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set(format!("covers/{issue_id}.png")),
        width: Set(Some(32)),
        height: Set(Some(32)),
        phash: Set(Some(local)),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(now),
        is_active: Set(true),
    }
    .insert(&db)
    .await
    .unwrap();

    let providers: Vec<Arc<dyn MetadataProvider>> = vec![Arc::new(MetronClient::with_base_url(
        "u",
        "p",
        m.metron.uri(),
        app.state().jobs.redis.clone(),
    ))];
    let facts = IssueQueryFacts {
        series_name: "Fantastic Four".into(),
        series_year: Some(2001),
        publisher: Some("Marvel".into()),
        volume: None,
        issue_number: "1".into(),
        issue_year: Some(1998),
        format: None,
    };
    let targets = vec![EffectiveTarget {
        source: Source::Metron,
        provider_series_id: "1711".into(),
        declared_year: None,
        provider_series_name: None,
        provider_series_url: None,
        via_range: false,
    }];
    // The recorded #1 row's cover image; never fetched (the hasher below
    // answers it from memory).
    let cover_url = m.metron_rows[&id_for(&m.metron_rows, "number", "1")]["image"]
        .as_str()
        .unwrap()
        .to_owned();
    let search = |candidate_hash: i64| {
        let facts = facts.clone();
        let issue_id = issue_id.clone();
        let (db, providers, targets, cover_url) = (
            db.clone(),
            providers.clone(),
            targets.clone(),
            cover_url.clone(),
        );
        let redis = app.state().jobs.redis.clone();
        async move {
            let run_id = orchestrator::start_run(
                &db,
                StartRunArgs {
                    scope: orchestrator::scope::ISSUE,
                    scope_entity_id: Some(issue_id.clone()),
                    library_id: None,
                    triggered_by: None,
                    trigger_kind: orchestrator::trigger_kind::MANUAL,
                    providers: &[Source::Metron],
                    query: StoredQuery::Issue(facts.clone()),
                    batch_id: None,
                },
            )
            .await
            .unwrap();
            let opts = SearchOpts {
                cover_hasher: Some(Arc::new(move |url: String| {
                    let hit = url == cover_url;
                    Box::pin(async move { hit.then_some(candidate_hash) })
                })),
                direct: Some(DirectLookupCtx {
                    redis,
                    cover_month: Some(1),
                }),
                ..SearchOpts::default()
            };
            let ranked = orchestrator::run_issue_search_with(
                &db,
                run_id,
                &providers,
                &facts,
                &targets,
                Thresholds::default(),
                3,
                Some(issue_id.as_str()),
                opts,
            )
            .await
            .expect("search");
            let run = entity::metadata_run::Entity::find_by_id(run_id)
                .one(&db)
                .await
                .unwrap()
                .unwrap();
            (ranked, run.query.unwrap()["coverage_lookups"].clone())
        }
    };

    // Same cover → HIGH through the existing ladder, no search.
    let (ranked, lookups) = search(local).await;
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].score.cover_hamming, Some(0));
    assert_eq!(ranked[0].bucket.as_str(), "high");
    assert!(ranked[0].coverage.is_some());
    assert_eq!(lookups[0]["path"], "direct");
    assert_eq!(metron_searches(&m).await, 0);

    // Every bit different → LOW on the cover → wrong mapping → search.
    let (_, lookups) = search(!local).await;
    assert_eq!(
        lookups,
        json!([{"source": "metron", "path": "search", "fallback": "rejected_by_matcher"}])
    );
    assert!(
        metron_searches(&m).await >= 1,
        "the rejected lookup searched"
    );
}
