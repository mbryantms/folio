//! Grand Comics Database client integration tests (roadmap WP-6.1).
//!
//! Every upstream body under `tests/fixtures/gcd/` is a **recorded**
//! response from `https://www.comics.org/api/` (fetched 2026-10-01 with
//! `?format=json`, verbatim apart from a trailing newline). GCD data is
//! CC BY-SA 4.0 — <https://www.comics.org/>.
//!
//! | fixture | upstream route |
//! |---|---|
//! | `series_search_ff_1961.json` | `/api/series/name/fantastic four/year/1961/` |
//! | `series_1482.json` | `/api/series/1482/` (Fantastic Four 1961 — GCD splits it at #416) |
//! | `publisher_78.json` | `/api/publisher/78/` (Marvel) |
//! | `issue_search_ff_1_1961.json` | `/api/series/name/fantastic four/issue/1/year/1961/` |
//! | `issue_16556.json` | `/api/issue/16556/` (FF #1) |
//! | `issue_1714163.json` | `/api/issue/1714163/` (FF #1 British price variant) |
//! | `series_search_saga_2012.json` | `/api/series/name/Saga/year/2012/` |
//! | `series_63051.json` | `/api/series/63051/` (Saga, Image) |
//! | `issue_911609.json` | `/api/issue/911609/` (Saga #1) |
//! | `publisher_709.json` | `/api/publisher/709/` (Image) |
//!
//! Coverage: series/issue search (broad + narrowed), detail mapping,
//! renamed-field tolerance, the splitter enumeration, auth / quota /
//! schema-drift error classification, the Redis summary cache, the
//! search → apply round trip through the production provider factory,
//! and the admin providers/budget surface.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::jobs::metadata_apply::{apply_issue_inline, apply_series_inline};
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::gcd::GcdClient;
use server::metadata::identifier::Source;
use server::metadata::matcher::{IssueQueryFacts, SeriesQueryFacts, Thresholds};
use server::metadata::orchestrator::{self, PreFilter, StartRunArgs, StoredQuery};
use server::metadata::provider::{IssueQuery, MetadataProvider, ProviderError, SeriesQuery};
use server::metadata::writers::CoverOverwritePolicy;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{basic_auth, method, path, query_param},
};

// ────────────────────── fixtures ──────────────────────

fn fixture(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gcd")
        .join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&raw).expect("fixture JSON")
}

fn empty_page() -> Value {
    json!({"count": 0, "next": null, "previous": null, "results": []})
}

async fn mount(mock: &MockServer, route: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(mock)
        .await;
}

/// Every recorded fixture at its upstream route.
async fn mount_all(mock: &MockServer) {
    for (route, name) in [
        (
            "/api/series/name/Fantastic%20Four/year/1961/",
            "series_search_ff_1961",
        ),
        ("/api/series/1482/", "series_1482"),
        ("/api/publisher/78/", "publisher_78"),
        (
            "/api/series/name/Fantastic%20Four/issue/1/year/1961/",
            "issue_search_ff_1_1961",
        ),
        ("/api/issue/16556/", "issue_16556"),
        ("/api/issue/1714163/", "issue_1714163"),
        (
            "/api/series/name/Saga/year/2012/",
            "series_search_saga_2012",
        ),
        ("/api/series/63051/", "series_63051"),
        ("/api/issue/911609/", "issue_911609"),
        ("/api/publisher/709/", "publisher_709"),
    ] {
        mount(mock, route, fixture(name)).await;
    }
    // Name-only routes: nothing beyond the year-scoped results.
    mount(mock, "/api/series/name/Fantastic%20Four/", empty_page()).await;
    mount(mock, "/api/series/name/Saga/", empty_page()).await;
}

fn client(app: &TestApp, mock: &MockServer) -> GcdClient {
    GcdClient::with_base_url(
        "gcd-user",
        "gcd-pass",
        mock.uri(),
        app.state().jobs.redis.clone(),
    )
}

// ────────────────────── client tests ──────────────────────

#[tokio::test]
async fn search_series_uses_year_route_then_name_route_with_basic_auth() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/name/Fantastic%20Four/year/1961/"))
        .and(basic_auth("gcd-user", "gcd-pass"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("series_search_ff_1961")))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/name/Fantastic%20Four/"))
        .and(basic_auth("gcd-user", "gcd-pass"))
        // The name-only page repeats the same series — must dedupe.
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("series_search_ff_1961")))
        .expect(1)
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let got = client(&app, &mock)
        .search_series(&SeriesQuery {
            name: "Fantastic Four".into(),
            year: Some(1961),
            publisher: None,
            limit: 5,
        })
        .await
        .expect("search_series");
    assert_eq!(got.len(), 1, "deduped across the two routes");
    let c = &got[0];
    assert_eq!(c.source, Source::Gcd);
    assert_eq!(c.external_id, "1482");
    assert_eq!(c.name, "Fantastic Four");
    assert_eq!(c.year, Some(1961));
    // 920 active issues incl. variants → 416 distinct numbers.
    assert_eq!(c.issue_count, Some(416));
    assert_eq!(c.format.as_deref(), Some("was ongoing series"));
    assert_eq!(
        c.external_url.as_deref(),
        Some("https://www.comics.org/series/1482/")
    );
}

#[tokio::test]
async fn renamed_and_unknown_fields_still_parse() {
    // Take the recorded Saga search and simulate upstream schema churn:
    // `api_url` → `url`, `name` → `series_name`, `year_began` →
    // `start_year` re-typed as a string, plus an unknown field.
    let mut body = fixture("series_search_saga_2012");
    for item in body["results"].as_array_mut().unwrap() {
        let obj = item.as_object_mut().unwrap();
        let url = obj.remove("api_url").unwrap();
        obj.insert("url".into(), url);
        let name = obj.remove("name").unwrap();
        obj.insert("series_name".into(), name);
        let year = obj.remove("year_began").unwrap();
        obj.insert("start_year".into(), Value::String(year.to_string()));
        obj.insert("brand_new_field".into(), json!({"nested": [1, 2, 3]}));
    }
    let mock = MockServer::start().await;
    mount(&mock, "/api/series/name/Saga/year/2012/", body).await;
    mount(&mock, "/api/series/name/Saga/", empty_page()).await;
    let app = TestApp::spawn().await;
    let got = client(&app, &mock)
        .search_series(&SeriesQuery {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            limit: 50,
        })
        .await
        .expect("renamed fields must not fail the search");
    assert_eq!(got.len(), 30);
    let saga = got
        .iter()
        .find(|c| c.external_id == "63051")
        .expect("Image's Saga survives the rename");
    assert_eq!(saga.name, "Saga");
    assert_eq!(saga.year, Some(2012));
}

#[tokio::test]
async fn search_issue_broad_hydrates_cover_and_skips_variants() {
    let mock = MockServer::start().await;
    mount_all(&mock).await;
    // The British variant must not be hydrated on the broad path.
    Mock::given(method("GET"))
        .and(path("/api/issue/1714163/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("issue_1714163")))
        .expect(0)
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let got = client(&app, &mock)
        .search_issue(&IssueQuery {
            series_external_id: None,
            series_name: Some("Fantastic Four".into()),
            series_year: Some(1961),
            issue_number: "001".into(),
            cover_year: Some(1961),
            limit: 10,
        })
        .await
        .expect("search_issue");
    assert_eq!(got.len(), 1);
    let c = &got[0];
    assert_eq!(c.external_id, "16556");
    assert_eq!(c.issue_number.as_deref(), Some("1"));
    assert_eq!(c.series_name.as_deref(), Some("Fantastic Four"));
    assert_eq!(c.series_year, Some(1961));
    assert_eq!(c.series_external_id.as_deref(), Some("1482"));
    assert_eq!(
        c.cover_date,
        chrono::NaiveDate::from_ymd_opt(1961, 11, 1),
        "key_date picked up from the hydrated detail"
    );
    assert_eq!(
        c.cover_image_url.as_deref(),
        Some("https://files1.comics.org//img/gcd/covers_by_id/21/w400/21867.jpg")
    );
    assert_eq!(c.name.as_deref(), Some("The Fantastic Four!"));
}

#[tokio::test]
async fn search_issue_narrowed_folds_variant_cover_into_parent() {
    let mock = MockServer::start().await;
    mount_all(&mock).await;
    let app = TestApp::spawn().await;
    let got = client(&app, &mock)
        .search_issue(&IssueQuery {
            series_external_id: Some("1482".into()),
            series_name: Some("Fantastic Four".into()),
            series_year: Some(1961),
            issue_number: "1".into(),
            cover_year: None,
            limit: 10,
        })
        .await
        .expect("narrowed search");
    assert_eq!(got.len(), 1, "variant folded, not a separate candidate");
    assert_eq!(got[0].external_id, "16556");
    assert_eq!(
        got[0].alternate_cover_urls,
        vec!["https://files1.comics.org//img/gcd/covers_by_id/1157/w400/1157906.jpg".to_owned()]
    );
}

#[tokio::test]
async fn fetch_issue_maps_detail_and_caches_series_summary() {
    let mock = MockServer::start().await;
    mount(&mock, "/api/issue/16556/", fixture("issue_16556")).await;
    Mock::given(method("GET"))
        .and(path("/api/series/1482/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("series_1482")))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/publisher/78/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("publisher_78")))
        .expect(1)
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let c = client(&app, &mock);
    let m = c.fetch_issue("16556").await.expect("fetch_issue");
    assert_eq!(m.series_name.as_deref(), Some("Fantastic Four"));
    assert_eq!(m.series_external_id.as_deref(), Some("1482"));
    assert_eq!(m.year_began, Some(1961));
    assert_eq!(m.publisher.as_deref(), Some("Marvel"));
    assert_eq!(m.language_code.as_deref(), Some("en"));
    assert_eq!(m.issue_number.as_deref(), Some("1"));
    assert_eq!(m.title.as_deref(), Some("The Fantastic Four!"));
    assert_eq!(m.cover_date, chrono::NaiveDate::from_ymd_opt(1961, 11, 1));
    assert_eq!(m.store_date, chrono::NaiveDate::from_ymd_opt(1961, 8, 8));
    assert_eq!(m.page_count, Some(36));
    assert_eq!(m.price, Some(0.10));
    let has = |n: &str, r: &str| m.credits.iter().any(|c| c.name == n && c.role == r);
    assert!(has("Stan Lee", "Writer"));
    assert!(has("Stan Lee", "Editor"));
    assert!(has("Jack Kirby", "Penciller"));
    assert!(has("Jack Kirby", "CoverArtist"));
    assert!(has("Artie Simek", "Letterer"));
    assert!(has("Stan Goldberg", "Colorist"));
    assert!(
        m.teams.iter().any(|t| t.name == "Fantastic Four"),
        "teams: {:?}",
        m.teams
    );
    let mole = m
        .characters
        .iter()
        .find(|c| c.name == "Mole Man")
        .expect("Mole Man");
    assert!(mole.is_first_appearance);
    assert!(m.description.as_deref().unwrap().contains("rocket trip"));
    assert_eq!(m.identifiers[0].source, Source::Gcd);
    assert_eq!(m.identifiers[0].id, "16556");

    // Second fetch: series summary + publisher come from Redis
    // (the `.expect(1)` mocks fail the test on a second hit).
    let again = c.fetch_issue("16556").await.expect("second fetch");
    assert_eq!(again.publisher.as_deref(), Some("Marvel"));
}

#[tokio::test]
async fn fetch_series_maps_detail_with_publisher_name() {
    let mock = MockServer::start().await;
    mount_all(&mock).await;
    let app = TestApp::spawn().await;
    let m = client(&app, &mock)
        .fetch_series("63051")
        .await
        .expect("fetch_series");
    assert_eq!(m.series_name.as_deref(), Some("Saga"));
    assert_eq!(m.year_began, Some(2012));
    assert_eq!(m.year_end, None);
    assert_eq!(m.publisher.as_deref(), Some("Image"));
    assert_eq!(m.series_type.as_deref(), Some("ongoing series"));
    assert_eq!(m.format, None, "ongoing → no Format churn");
    assert_eq!(m.language_code.as_deref(), Some("en"));
    assert_eq!(m.source_provider, Some(Source::Gcd));
    assert_eq!(
        m.source_url.as_deref(),
        Some("https://www.comics.org/series/63051/")
    );
}

#[tokio::test]
async fn list_series_issue_numbers_enumerates_the_split_run() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/1482/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("series_1482")))
        .expect(1)
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let numbers = client(&app, &mock)
        .list_series_issue_numbers("1482")
        .await
        .expect("enumerate");
    // GCD splits Fantastic Four (1961) at #416 — the legacy #500+ run
    // lives in other GCD series, which auto-split maps by range.
    assert_eq!(numbers.len(), 416);
    assert_eq!(numbers.first().map(String::as_str), Some("1"));
    assert_eq!(numbers.last().map(String::as_str), Some("416"));
    assert!(!numbers.iter().any(|n| n == "600"));
}

#[tokio::test]
async fn auth_quota_and_schema_errors_classify() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/1/"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"detail": "Invalid username/password."})),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/2/"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "120")
                .set_body_json(
                    json!({"detail": "Request was throttled. Expected available in 120 seconds."}),
                ),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/3/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>maintenance</html>"))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/4/"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"detail": "Not found."})))
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let c = client(&app, &mock);
    assert!(matches!(
        c.fetch_series("1").await,
        Err(ProviderError::Unauthorized(_))
    ));
    match c.fetch_series("2").await {
        Err(ProviderError::QuotaExceeded { retry_after_secs }) => assert_eq!(retry_after_secs, 120),
        other => panic!("expected QuotaExceeded, got {other:?}"),
    }
    assert!(matches!(
        c.fetch_series("3").await,
        Err(ProviderError::InvalidResponse(_))
    ));
    assert!(matches!(
        c.fetch_series("4").await,
        Err(ProviderError::NotFound(_))
    ));
    // Last error surfaced for the admin card.
    let last = server::metadata::budget::load_last_error(&app.state().jobs.redis, Source::Gcd)
        .await
        .expect("last error recorded");
    assert!(last.message.contains("not found"), "{}", last.message);
}

// ────────────────────── round trips ──────────────────────

fn apply_args(run_id: Uuid, ordinal: i32) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal,
        mode: ApplyMode::FillMissing,
        apply_cover: false,
        cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
        override_user_edits: false,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

async fn spawn_gcd_app(mock: &MockServer) -> TestApp {
    TestApp::spawn_with_gcd("gcd-user", "gcd-pass", true, Some(mock.uri())).await
}

#[tokio::test]
async fn series_search_then_apply_round_trips_through_the_factory() {
    let mock = MockServer::start().await;
    mount_all(&mock).await;
    let app = spawn_gcd_app(&mock).await;
    let state = app.state();

    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id(), Source::Gcd);

    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&state.db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&state.db).await;
    {
        let row = entity::series::Entity::find_by_id(series_id)
            .one(&state.db)
            .await
            .unwrap()
            .unwrap();
        let mut am: entity::series::ActiveModel = row.into();
        am.year = Set(None);
        am.publisher = Set(None);
        am.update(&state.db).await.unwrap();
    }

    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: Some(2012),
        publisher: None,
        volume: None,
        format: None,
    };
    let run_id = orchestrator::start_run(
        &state.db,
        StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: Some(series_id.to_string()),
            library_id: Some(lib_id),
            triggered_by: None,
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &[Source::Gcd],
            query: StoredQuery::Series(facts.clone()),
            batch_id: None,
        },
    )
    .await
    .unwrap();
    let ranked = orchestrator::run_series_search(
        &state.db,
        run_id,
        &providers,
        &facts,
        Thresholds::new(80.0, 60.0),
        &PreFilter::default(),
        3,
        None,
    )
    .await
    .expect("series search");
    assert!(!ranked.is_empty());
    let rows = orchestrator::fetch_candidates(&state.db, run_id)
        .await
        .unwrap();
    // Exact-name Saga runs rank above "Batman Saga" & co.
    let top = &rows[0];
    assert_eq!(top.source, "gcd");
    let saga = rows
        .iter()
        .find(|r| r.external_id == "63051")
        .expect("Image Saga candidate persisted");

    let outcome = apply_series_inline(&state, series_id, apply_args(run_id, saga.ordinal))
        .await
        .expect("apply GCD series");
    assert!(
        outcome.applied_fields.contains(&"year_began".to_owned()),
        "{:?}",
        outcome.applied_fields
    );
    let after = entity::series::Entity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.year, Some(2012));
    assert_eq!(after.publisher.as_deref(), Some("Image"));
    let ext = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(entity::external_id::Column::Source.eq("gcd"))
        .one(&state.db)
        .await
        .unwrap()
        .expect("gcd external id written");
    assert_eq!(ext.external_id, "63051");
}

#[tokio::test]
async fn issue_search_then_apply_round_trips_through_the_factory() {
    let mock = MockServer::start().await;
    mount_all(&mock).await;
    let app = spawn_gcd_app(&mock).await;
    let state = app.state();

    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&state.db).await;
    let series_id = SeriesSeed::new(lib_id, "Fantastic Four")
        .insert(&state.db)
        .await;
    let cbz = dir.path().join("ff-001.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&state.db)
        .await;

    let facts = IssueQueryFacts {
        series_name: "Fantastic Four".into(),
        series_year: Some(1961),
        publisher: None,
        volume: None,
        issue_number: "1".into(),
        issue_year: Some(1961),
        format: None,
    };
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    let run_id = orchestrator::start_run(
        &state.db,
        StartRunArgs {
            scope: orchestrator::scope::ISSUE,
            scope_entity_id: Some(issue_id.clone()),
            library_id: Some(lib_id),
            triggered_by: None,
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &[Source::Gcd],
            query: StoredQuery::Issue(facts.clone()),
            batch_id: None,
        },
    )
    .await
    .unwrap();
    let ranked = orchestrator::run_issue_search(
        &state.db,
        run_id,
        &providers,
        &facts,
        &[],
        Thresholds::new(80.0, 60.0),
        3,
        None,
    )
    .await
    .expect("issue search");
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].external_id, "16556");

    let outcome = apply_issue_inline(&state, &issue_id, apply_args(run_id, 0))
        .await
        .expect("apply GCD issue");
    assert!(
        outcome.applied_fields.contains(&"credits".to_owned()),
        "{:?}",
        outcome.applied_fields
    );
    // Credits landed in the junction with the canonical roles GCD's
    // free-text story credits were mapped onto.
    let credits = entity::issue_credit::Entity::find()
        .filter(entity::issue_credit::Column::IssueId.eq(issue_id.clone()))
        .all(&state.db)
        .await
        .unwrap();
    let roles: std::collections::HashSet<_> = credits.iter().map(|c| c.role.as_str()).collect();
    for role in [
        "Writer",
        "Penciller",
        "Inker",
        "Colorist",
        "Letterer",
        "Editor",
        "CoverArtist",
    ] {
        assert!(roles.contains(role), "missing {role}: {roles:?}");
    }
    let ext = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("issue"))
        .filter(entity::external_id::Column::EntityId.eq(issue_id.clone()))
        .filter(entity::external_id::Column::Source.eq("gcd"))
        .one(&state.db)
        .await
        .unwrap()
        .expect("gcd issue id written");
    assert_eq!(ext.external_id, "16556");
}

// ────────────────────── admin surface ──────────────────────

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

fn csrf_of(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned()
}

async fn call(app: &TestApp, cookie: &str, m: Method, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(m)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("X-CSRF-Token", csrf_of(cookie))
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

#[tokio::test]
async fn admin_providers_list_shows_gcd_budget_bar() {
    let mock = MockServer::start().await;
    let app = spawn_gcd_app(&mock).await;
    let cookie = register_admin(&app).await;
    let (status, body) = call(&app, &cookie, Method::GET, "/api/admin/metadata/providers").await;
    assert_eq!(status, StatusCode::OK);
    let gcd = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "gcd")
        .expect("gcd row")
        .clone();
    assert_eq!(gcd["label"], "Grand Comics Database");
    assert_eq!(gcd["configured"], true);
    assert_eq!(gcd["enabled"], true);
    assert_eq!(gcd["quota"]["remaining_hour"], 100);
    assert_eq!(gcd["quota"]["remaining_day"], 2000);
    assert_eq!(gcd["budget"]["limit"], 2000);
    assert_eq!(gcd["budget"]["window"], "day");
}

#[tokio::test]
async fn admin_test_endpoint_exercises_gcd_health_check() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/name/__folio_health_check__/"))
        .and(basic_auth("gcd-user", "gcd-pass"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_page()))
        .expect(1)
        .mount(&mock)
        .await;
    let app = spawn_gcd_app(&mock).await;
    let cookie = register_admin(&app).await;
    let (status, body) = call(
        &app,
        &cookie,
        Method::POST,
        "/api/admin/metadata/providers/gcd/test",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    let audit = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.metadata.providers.test"))
        .filter(entity::audit_log::Column::TargetId.eq("gcd"))
        .one(&app.state().db)
        .await
        .unwrap();
    assert!(audit.is_some(), "test call is audit-logged");
}

#[tokio::test]
async fn admin_test_endpoint_400s_without_gcd_credentials() {
    let app = TestApp::spawn().await;
    let cookie = register_admin(&app).await;
    let (status, body) = call(
        &app,
        &cookie,
        Method::POST,
        "/api/admin/metadata/providers/gcd/test",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "metadata.no_credentials");
}
