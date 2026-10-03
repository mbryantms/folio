//! "Detect from providers" for every series: the detector resolves the
//! series' Metron / GCD id even when it was only ever matched through
//! ComicVine (existing link → cross-reference bridge → strict search), and
//! maps the issue ranges those providers file under a different series.
//!
//! GCD bodies for Fantastic Four (1961) are the recorded fixtures under
//! `tests/fixtures/gcd/` (see `gcd_client.rs`). The alternate series
//! ("Fantastic Four (1998 series)" #500–501, id 9999) and every Metron
//! body are synthetic and built inline. No real provider is called.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::writers::{SetBy, set_external_id};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

// ───────── fixtures + mocks ─────────

fn gcd_fixture(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gcd")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

fn empty_page() -> Value {
    json!({"count": 0, "next": null, "previous": null, "results": []})
}

fn paged(results: Value) -> Value {
    json!({
        "count": results.as_array().map(|a| a.len()).unwrap_or(0),
        "next": null,
        "previous": null,
        "results": results,
    })
}

async fn gcd_route(mock: &MockServer, route: &str, body: Value, expect: Option<u64>) {
    let m = Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(body));
    match expect {
        Some(n) => m.expect(n).mount(mock).await,
        None => m.mount(mock).await,
    }
}

/// The synthetic GCD series that carries the legacy-numbered #500–501.
fn gcd_alt_series() -> Value {
    json!({
        "api_url": "https://www.comics.org/api/series/9999/?format=json",
        "name": "Fantastic Four",
        "year_began": 1998,
        "active_issues": [
            "https://www.comics.org/api/issue/7001/?format=json",
            "https://www.comics.org/api/issue/7002/?format=json"
        ],
        "issue_descriptors": ["500", "501"],
        "publishing_format": "ongoing series",
        "publisher": "https://www.comics.org/api/publisher/78/?format=json"
    })
}

fn gcd_issue_row(number: &str, id: u32) -> Value {
    json!({
        "api_url": format!("https://www.comics.org/api/issue/{id}/?format=json"),
        "series_name": "Fantastic Four (1998 series)",
        "descriptor": number,
        "publication_date": "June 2003",
        "variant_of": null,
        "series": "https://www.comics.org/api/series/9999/?format=json"
    })
}

/// GCD routes for the FF split: the main series (recorded fixture, #1–416)
/// and the #500/#501 broad searches resolving to the alternate series.
async fn mount_gcd_split(mock: &MockServer) {
    gcd_route(mock, "/api/series/1482/", gcd_fixture("series_1482"), None).await;
    gcd_route(
        mock,
        "/api/publisher/78/",
        gcd_fixture("publisher_78"),
        None,
    )
    .await;
    gcd_route(mock, "/api/series/9999/", gcd_alt_series(), None).await;
    gcd_route(
        mock,
        "/api/series/name/Fantastic%20Four/issue/500/",
        paged(json!([gcd_issue_row("500", 7001)])),
        None,
    )
    .await;
    gcd_route(
        mock,
        "/api/series/name/Fantastic%20Four/issue/501/",
        paged(json!([gcd_issue_row("501", 7002)])),
        None,
    )
    .await;
}

/// Library + "Fantastic Four" (`year`) with issues 1, 2, 3, 416, 500, 501,
/// linked only to ComicVine (id 2045).
async fn seed_ff(app: &TestApp, year: i32) -> (Uuid, String, tempfile::TempDir) {
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Fantastic Four");
    seed.year = Some(year);
    let series_id = seed.insert(&db).await;
    for n in [1.0_f64, 2.0, 3.0, 416.0, 500.0, 501.0] {
        let p = tmp.path().join(format!("ff-{n}.cbz"));
        IssueSeed::new(lib, series_id, &p, format!("ff {n}").as_bytes(), n)
            .insert(&db)
            .await;
    }
    set_external_id(
        &db,
        "series",
        &series_id.to_string(),
        &Identifier::with_canonical_url(Source::ComicVine, "2045", "series"),
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    (series_id, slug, tmp)
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

async fn detect(app: &TestApp, cookie: &str, slug: &str) -> Value {
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
                .method(Method::POST)
                .uri(format!("/api/series/{slug}/provider-ranges/detect"))
                .header(header::COOKIE, cookie)
                .header("X-CSRF-Token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn source<'a>(body: &'a Value, src: &str) -> &'a Value {
    body["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["source"] == src)
        .unwrap_or_else(|| panic!("no {src} result in {body}"))
}

async fn series_ext(
    app: &TestApp,
    series_id: Uuid,
    src: &str,
) -> Option<entity::external_id::Model> {
    entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(entity::external_id::Column::Source.eq(src))
        .one(&app.state().db)
        .await
        .unwrap()
}

async fn ranges(app: &TestApp, series_id: Uuid) -> Vec<entity::series_provider_range::Model> {
    entity::series_provider_range::Entity::find()
        .filter(entity::series_provider_range::Column::SeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

// ───────── tests ─────────

/// A ComicVine-only series: GCD's id comes from a strict series search
/// (exact name + year, unique, and GCD's issue list carries ≥ 50% of the
/// local issues), is recorded, and the #500–501 block GCD files under a
/// separate series is mapped. A second click re-uses the recorded link —
/// no second search — and maps nothing new.
#[tokio::test]
async fn comicvine_only_series_resolves_gcd_by_search_and_maps_split() {
    let gcd = MockServer::start().await;
    mount_gcd_split(&gcd).await;
    // The series search must run exactly once across both clicks.
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/year/1961/",
        gcd_fixture("series_search_ff_1961"),
        Some(1),
    )
    .await;
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/",
        empty_page(),
        None,
    )
    .await;

    let app = TestApp::spawn_with_gcd("gcd-user", "gcd-pass", true, Some(gcd.uri())).await;
    let (series_id, slug, _tmp) = seed_ff(&app, 1961).await;
    let cookie = register_admin(&app).await;

    let body = detect(&app, &cookie, &slug).await;
    let g = source(&body, "gcd");
    assert_eq!(g["status"], "scanned", "{g}");
    assert_eq!(g["resolved_via"], "search");
    assert_eq!(g["provider_series_id"], "1482");
    assert_eq!(g["id_recorded"], true);
    assert_eq!(g["matched_local"], 4);
    assert_eq!(g["gaps"], json!(["500..501"]));
    assert_eq!(g["gap_details"][0]["status"], "mapped");
    assert_eq!(g["created"][0]["provider_series_id"], "9999");
    assert_eq!(g["created"][0]["declared_year"], 1998);
    // Metron isn't configured; ComicVine is linked but can't enumerate
    // (and isn't configured in this app).
    assert_eq!(source(&body, "metron")["status"], "not_configured");
    assert_eq!(source(&body, "comicvine")["status"], "not_configured");

    let ext = series_ext(&app, series_id, "gcd")
        .await
        .expect("gcd id recorded");
    assert_eq!(ext.external_id, "1482");
    assert_eq!(ext.set_by, "gcd");
    let rows = ranges(&app, series_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].range_low.as_deref(), rows[0].range_high.as_deref()),
        (Some("500"), Some("501"))
    );

    // Idempotent re-detect.
    let again = detect(&app, &cookie, &slug).await;
    let g = source(&again, "gcd");
    assert_eq!(g["status"], "scanned");
    assert_eq!(g["resolved_via"], "linked");
    assert_eq!(g["id_recorded"], false);
    assert_eq!(g["created"], json!([]));
    assert_eq!(g["gap_details"][0]["status"], "already_mapped");
    assert_eq!(g["gap_details"][0]["provider_series_id"], "9999");
    assert_eq!(ranges(&app, series_id).await.len(), 1);
}

/// A local year one off GCD's start year: the search hit is only
/// MEDIUM-confidence, so it's surfaced for confirmation and nothing is
/// written — no external id, no range.
#[tokio::test]
async fn medium_confidence_candidate_is_surfaced_not_written() {
    let gcd = MockServer::start().await;
    mount_gcd_split(&gcd).await;
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/year/1962/",
        gcd_fixture("series_search_ff_1961"),
        None,
    )
    .await;
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/",
        empty_page(),
        None,
    )
    .await;

    let app = TestApp::spawn_with_gcd("gcd-user", "gcd-pass", true, Some(gcd.uri())).await;
    let (series_id, slug, _tmp) = seed_ff(&app, 1962).await;
    let cookie = register_admin(&app).await;

    let body = detect(&app, &cookie, &slug).await;
    let g = source(&body, "gcd");
    assert_eq!(g["status"], "needs_confirmation", "{g}");
    assert_eq!(g["id_recorded"], false);
    assert_eq!(g["provider_series_id"], Value::Null);
    let cands = g["candidates"].as_array().unwrap();
    assert_eq!(cands[0]["external_id"], "1482");
    assert_eq!(cands[0]["reason"], "start year differs");
    assert!(series_ext(&app, series_id, "gcd").await.is_none());
    assert!(ranges(&app, series_id).await.is_empty());
}

/// Metron resolves through its curated `cv_id` cross-reference (one
/// request), and that row's `gcd_id` resolves GCD without any GCD search.
/// Both enumerate; Metron carries #500 in the main run while GCD doesn't,
/// so the providers disagree — reported, and both mappings coexist.
#[tokio::test]
async fn metron_bridge_resolves_both_and_reports_disagreement() {
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    mount_gcd_split(&gcd).await;
    // No GCD series search may run: the bridge supplies the id.
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/year/1961/",
        gcd_fixture("series_search_ff_1961"),
        Some(0),
    )
    .await;

    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("cv_id", "2045"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([{
            "id": 1711, "series": "Fantastic Four (1961)", "year_began": 1961,
            "issue_count": 417, "cv_id": 2045, "gcd_id": 1482
        }]))))
        .expect(1)
        .mount(&metron)
        .await;
    // Metron's main run lists #1–3, #416 and #500 (not #501).
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "1711"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 1, "number": "1"}, {"id": 2, "number": "2"}, {"id": 3, "number": "3"},
            {"id": 416, "number": "416"}, {"id": 500, "number": "500"}
        ]))))
        .mount(&metron)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("number", "501"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 9501, "number": "501",
             "series": {"id": 62349, "name": "Fantastic Four", "year_began": 2012}}
        ]))))
        .mount(&metron)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "62349"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 9501, "number": "501"}, {"id": 9502, "number": "502"}
        ]))))
        .mount(&metron)
        .await;

    let app = TestApp::spawn_with_metron_and_gcd(metron.uri(), gcd.uri()).await;
    let (series_id, slug, _tmp) = seed_ff(&app, 1961).await;
    let cookie = register_admin(&app).await;

    let body = detect(&app, &cookie, &slug).await;
    let m = source(&body, "metron");
    assert_eq!(m["status"], "scanned", "{m}");
    assert_eq!(m["resolved_via"], "bridge");
    assert_eq!(m["provider_series_id"], "1711");
    assert_eq!(m["gaps"], json!(["501..501"]));
    assert_eq!(m["created"][0]["provider_series_id"], "62349");

    let g = source(&body, "gcd");
    assert_eq!(g["status"], "scanned", "{g}");
    assert_eq!(g["resolved_via"], "bridge");
    assert_eq!(g["provider_series_id"], "1482");
    assert_eq!(g["gaps"], json!(["500..501"]));
    assert_eq!(g["created"][0]["provider_series_id"], "9999");

    assert_eq!(body["agreement"]["agree"], false);
    let summary = body["agreement"]["summary"].as_str().unwrap();
    assert!(
        summary.contains("Metron #501") && summary.contains("#500–501"),
        "{summary}"
    );

    // Both ids recorded through the writer; GCD's attested by Metron.
    assert_eq!(
        series_ext(&app, series_id, "metron").await.unwrap().set_by,
        "metron"
    );
    let g_ext = series_ext(&app, series_id, "gcd").await.unwrap();
    assert_eq!(
        (g_ext.external_id.as_str(), g_ext.set_by.as_str()),
        ("1482", "metron")
    );
    // One range per provider — they don't conflict.
    let mut rows: Vec<(String, String)> = ranges(&app, series_id)
        .await
        .into_iter()
        .map(|r| (r.source, r.provider_series_id))
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("gcd".into(), "9999".into()),
            ("metron".into(), "62349".into())
        ]
    );
}

/// A provider failure is reported for that provider only; the request
/// still succeeds and nothing is written for it.
#[tokio::test]
async fn provider_errors_are_reported_per_source() {
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    // Metron rejects the credentials on the cross-reference lookup.
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&metron)
        .await;
    // GCD search works and finds the series normally.
    mount_gcd_split(&gcd).await;
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/year/1961/",
        gcd_fixture("series_search_ff_1961"),
        None,
    )
    .await;
    gcd_route(
        &gcd,
        "/api/series/name/Fantastic%20Four/",
        empty_page(),
        None,
    )
    .await;

    let app = TestApp::spawn_with_metron_and_gcd(metron.uri(), gcd.uri()).await;
    let (series_id, slug, _tmp) = seed_ff(&app, 1961).await;
    let cookie = register_admin(&app).await;

    let body = detect(&app, &cookie, &slug).await;
    let m = source(&body, "metron");
    assert_eq!(m["status"], "error", "{m}");
    assert!(m["error"].as_str().unwrap().contains("credentials"), "{m}");
    assert!(series_ext(&app, series_id, "metron").await.is_none());
    // GCD still ran to completion.
    let g = source(&body, "gcd");
    assert_eq!(g["status"], "scanned", "{g}");
    assert_eq!(g["created"].as_array().unwrap().len(), 1);
    assert_eq!(body["agreement"], Value::Null);
}
