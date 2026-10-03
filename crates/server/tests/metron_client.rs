//! Metron HTTP client integration tests (metadata-providers-1.0 M2).
//!
//! Exercises the live HTTP path of `MetronClient` against a
//! wiremock-backed mock Metron API. Mirrors the ComicVine harness
//! (`tests/comicvine_client.rs`).
//!
//! Coverage:
//! - happy path series + issue fetch with native CV/GCD ID propagation
//! - HTTP 401/403 → Unauthorized
//! - HTTP 404 → NotFound
//! - HTTP 429 → QuotaExceeded
//! - cache short-circuit (`.expect(1)` on the mock)
//! - search_series uses ?name only (year is left to the pre-filter gate)
//! - structured credit roles map straight through (no comma-splitting
//!   needed — Metron normalizes upstream)

mod common;

use common::TestApp;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use server::metadata::budget::{self, BudgetWindow};
use server::metadata::cache;
use server::metadata::identifier::Source;
use server::metadata::metron::{MetronAuth, MetronClient};
use server::metadata::provider::{IssueQuery, MetadataProvider, ProviderError, SeriesQuery};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{basic_auth, header, header_exists, method, path, query_param},
};

// ────────────────────── fixtures ──────────────────────

fn paged(results: serde_json::Value) -> serde_json::Value {
    json!({
        "count": results.as_array().map(|a| a.len()).unwrap_or(0),
        "next": null,
        "previous": null,
        "results": results,
    })
}

fn series_list_fixture() -> serde_json::Value {
    json!({
        "id": 123,
        "series": "Saga",
        "year_began": 2012,
        "issue_count": 60,
        "modified": "2024-01-15T12:34:56Z"
    })
}

fn series_detail_fixture() -> serde_json::Value {
    json!({
        "id": 123,
        "name": "Saga",
        "sort_name": "Saga",
        "volume": 1,
        "series_type": {"id": 1, "name": "Ongoing Series"},
        "publisher": {"id": 5, "name": "Image Comics"},
        "imprint": null,
        "year_began": 2012,
        "year_end": null,
        "desc": "Sci-fi epic.",
        "issue_count": 60,
        "genres": [{"id": 1, "name": "Science-Fiction"}],
        "associated": [],
        "cv_id": 12345,
        "gcd_id": 98765,
        "resource_url": "https://metron.cloud/series/saga-2012/",
        "modified": "2024-01-15T12:34:56Z"
    })
}

fn issue_detail_fixture() -> serde_json::Value {
    json!({
        "id": 456,
        "publisher": {"id": 5, "name": "Image Comics"},
        "imprint": null,
        "series": {
            "id": 123,
            "name": "Saga",
            "sort_name": "Saga",
            "volume": 1,
            "year_began": 2012,
            "series_type": {"id": 1, "name": "Ongoing Series"},
            "genres": []
        },
        "number": "1",
        "title": "Chapter One",
        "name": ["Chapter One"],
        "cover_date": "2012-03-14",
        "store_date": "2012-03-14",
        "foc_date": null,
        "price": "2.99",
        "rating": {"id": 1, "name": "Teen Plus"},
        "sku": "JAN120494",
        "isbn": "",
        "upc": "75960608437600111",
        "page": 36,
        "desc": "Premiere issue.",
        "image": "https://static.metron.cloud/saga-1.jpg",
        "cover_hash": "abc",
        "arcs": [{"id": 10, "name": "Beginning"}],
        "credits": [
            {"id": 1, "creator": "Brian K. Vaughan", "creator_id": 7, "role": [{"id": 1, "name": "Writer"}, {"id": 9, "name": "Cover"}]},
            {"id": 2, "creator": "Fiona Staples", "creator_id": 8, "role": [{"id": 2, "name": "Artist"}]}
        ],
        "characters": [{"id": 100, "name": "Hazel"}],
        "teams": [],
        "universes": [{"id": 500, "name": "Main Universe"}],
        "reprints": [],
        "variants": [
            {"name": "Cover B", "sku": "JAN120495", "upc": "75960608437600121", "image": "https://static.metron.cloud/saga-1-b.jpg"}
        ],
        "cv_id": 67890,
        "gcd_id": 11111,
        "resource_url": "https://metron.cloud/issue/saga-1-2012/",
        "modified": "2024-02-20T08:00:00Z"
    })
}

// ────────────────────── tests ──────────────────────

#[tokio::test]
async fn search_series_uses_name_filter_only() {
    // Series search is name-only on purpose: year is NOT sent as a hard
    // `year_began` filter (it would exclude a series whose local year is
    // wrong/off-by-one). The tolerant pre_filter_series gate handles year.
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("name", "Saga"))
        .and(basic_auth("metron-user", "metron-pass"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paged(json!([series_list_fixture()]))),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("metron-user", "metron-pass", true).await;
    let client = MetronClient::with_base_url(
        "metron-user",
        "metron-pass",
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

    let candidates = client
        .search_series(&SeriesQuery {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            limit: 5,
        })
        .await
        .expect("search_series");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source, Source::Metron);
    assert_eq!(candidates[0].external_id, "123");
    assert_eq!(candidates[0].name, "Saga");
    assert_eq!(candidates[0].year, Some(2012));
    assert_eq!(candidates[0].issue_count, Some(60));
}

#[tokio::test]
async fn fetch_series_propagates_cv_and_gcd_identifiers() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(series_detail_fixture()))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let m = client.fetch_series("123").await.expect("fetch_series");
    assert_eq!(m.series_name.as_deref(), Some("Saga"));
    assert_eq!(m.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(m.genres, vec!["Science-Fiction"]);
    // Identifiers: Metron self + CV + GCD propagated for free.
    let sources: Vec<_> = m.identifiers.iter().map(|i| i.source).collect();
    assert!(sources.contains(&Source::Metron));
    assert!(sources.contains(&Source::ComicVine));
    assert!(sources.contains(&Source::Gcd));
}

#[tokio::test]
async fn fetch_issue_carries_structured_credits_and_barcodes() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/456/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_detail_fixture()))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let m = client.fetch_issue("456").await.expect("fetch_issue");
    assert_eq!(m.issue_number.as_deref(), Some("1"));
    assert_eq!(m.title.as_deref(), Some("Chapter One"));
    assert_eq!(m.price, Some(2.99));
    assert_eq!(m.page_count, Some(36));
    assert_eq!(m.age_rating.as_deref(), Some("Teen Plus"));
    // Series back-reference + sort name + volume number preserved.
    assert_eq!(m.series_name.as_deref(), Some("Saga"));
    assert_eq!(m.volume, Some(1));
    // 3 credits, with roles canonicalized at the provider boundary
    // (`canonicalize_role`): Writer + Cover→CoverArtist (exploded from the
    // first creator) + Artist→Penciller.
    assert_eq!(m.credits.len(), 3);
    assert!(m.credits.iter().any(|c| c.role == "Writer"));
    assert!(m.credits.iter().any(|c| c.role == "CoverArtist"));
    assert!(m.credits.iter().any(|c| c.role == "Penciller"));
    // Universes are Metron-only; pulled through.
    assert_eq!(m.universes.len(), 1);
    assert_eq!(m.universes[0].name, "Main Universe");
    // Variants carry the variant UPC as an Identifier.
    assert_eq!(m.variants.len(), 1);
    assert_eq!(m.variants[0].label.as_deref(), Some("Cover B"));
    // Identifiers: Metron + CV + GCD + UPC (ISBN was empty string).
    let sources: Vec<_> = m.identifiers.iter().map(|i| i.source).collect();
    assert!(sources.contains(&Source::ComicVine));
    assert!(sources.contains(&Source::Gcd));
    assert!(sources.contains(&Source::Upc));
    assert!(!sources.contains(&Source::Isbn));
}

#[tokio::test]
async fn http_401_maps_to_unauthorized() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"detail": "invalid"})))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("bad", "creds", true).await;
    let client =
        MetronClient::with_base_url("bad", "creds", mock.uri(), app.state().jobs.redis.clone());

    let err = client
        .search_series(&SeriesQuery {
            name: "x".into(),
            year: None,
            publisher: None,
            limit: 1,
        })
        .await
        .expect_err("expected unauthorized");
    assert!(matches!(err, ProviderError::Unauthorized(_)));
}

#[tokio::test]
async fn http_404_maps_to_not_found() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"detail": "Not found."})))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let err = client.fetch_series("99999").await.expect_err("not found");
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn http_429_maps_to_quota_exceeded() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let err = client
        .search_series(&SeriesQuery {
            name: "x".into(),
            year: None,
            publisher: None,
            limit: 1,
        })
        .await
        .expect_err("expected quota");
    assert!(matches!(err, ProviderError::QuotaExceeded { .. }));
}

#[tokio::test]
async fn fetch_series_cached_round_trips_through_metadata_cache() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(series_detail_fixture()))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    let db = &app.state().db;

    let first = client.fetch_series_cached(db, "123").await.expect("first");
    let second = client.fetch_series_cached(db, "123").await.expect("second");
    assert_eq!(first.series_name, second.series_name);
    // Sanity: row landed in the cache table under the Metron key.
    let hit = cache::get(
        db,
        Source::Metron,
        cache::CacheEntity::Series,
        "123",
        chrono::Duration::hours(168),
    )
    .await
    .expect("cache lookup");
    assert!(hit.is_some());
}

#[tokio::test]
async fn search_issue_filters_by_series_id() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "123"))
        .and(query_param("number", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([{
            "id": 456,
            "series": {
                "id": 123,
                "name": "Saga",
                "sort_name": "Saga",
                "volume": 1,
                "year_began": 2012,
                "series_type": null,
                "genres": []
            },
            "number": "1",
            "name": ["Chapter One"],
            "cover_date": "2012-03-14",
            "image": "https://static.metron.cloud/saga-1.jpg",
            "modified": "2024-02-20T08:00:00Z"
        }]))))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let out = client
        .search_issue(&IssueQuery {
            series_external_id: Some("123".into()),
            series_name: None,
            series_year: None,
            issue_number: "1".into(),
            cover_year: None,
            limit: 5,
        })
        .await
        .expect("search_issue");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].external_id, "456");
    assert_eq!(out[0].series_name.as_deref(), Some("Saga"));
}

// ────────────────────── WP-2.9: token auth + budget + conditional ──────────────────────

#[tokio::test]
async fn token_auth_sends_bearer_header_and_skips_basic() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(header("authorization", "Bearer tok-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([]))))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron_token(" tok-123\n", true).await;
    // `from_config` trims the pasted token and prefers it over Basic.
    let auth = MetronAuth::from_config(&app.state().cfg()).expect("configured");
    assert!(auth.is_token());
    let client = MetronClient::with_auth(auth, mock.uri(), app.state().jobs.redis.clone());
    // The health check is what the admin "Test" button runs — it must
    // exercise the token path.
    let snap = client.health_check().await.expect("token accepted");
    assert_eq!(snap.provider, Source::Metron);
}

#[tokio::test]
async fn metron_auth_falls_back_to_basic_and_none() {
    let basic = TestApp::spawn_with_metron("u", "p", true).await;
    let auth = MetronAuth::from_config(&basic.state().cfg()).expect("basic configured");
    assert!(!auth.is_token());
    assert!(matches!(auth, MetronAuth::Basic { .. }));

    let none = TestApp::spawn().await;
    assert!(MetronAuth::from_config(&none.state().cfg()).is_none());
    assert!(
        MetronClient::from_config(&none.state().cfg(), none.state().jobs.redis.clone()).is_none()
    );
}

#[tokio::test]
async fn rate_limit_headers_are_recorded_as_budget() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-RateLimit-Burst-Limit", "20")
                .insert_header("X-RateLimit-Burst-Remaining", "17")
                .insert_header("X-RateLimit-Burst-Reset", "1700000060")
                .insert_header("X-RateLimit-Sustained-Limit", "5000")
                .insert_header("X-RateLimit-Sustained-Remaining", "4982")
                .insert_header("X-RateLimit-Sustained-Reset", "1700003600")
                .set_body_json(paged(json!([]))),
        )
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let redis = app.state().jobs.redis.clone();
    // Before any response: the budget falls back to the local day bucket.
    let pre = budget::for_provider(&redis, Source::Metron)
        .await
        .expect("bucket-derived");
    assert_eq!(pre.window, BudgetWindow::Day);
    assert_eq!(pre.limit, 5000);

    let client = MetronClient::with_base_url("u", "p", mock.uri(), redis.clone());
    client
        .search_series(&SeriesQuery {
            name: "x".into(),
            year: None,
            publisher: None,
            limit: 1,
        })
        .await
        .expect("search");

    let state = budget::load(&redis, Source::Metron).await.expect("stored");
    assert_eq!(state.windows.len(), 2);
    let headline = state.headline().expect("headline");
    assert_eq!(headline.window, BudgetWindow::Day);
    assert_eq!(headline.limit, 5000);
    assert_eq!(headline.remaining, 4982);
    assert_eq!(headline.reset_at.timestamp(), 1_700_003_600);
    // `for_provider` now prefers the upstream figure.
    let post = budget::for_provider(&redis, Source::Metron).await.unwrap();
    assert_eq!(post.remaining, 4982);
}

#[tokio::test]
async fn expired_detail_row_is_revalidated_with_if_modified_since() {
    let mock = MockServer::start().await;
    let last_modified = "Wed, 12 Feb 2026 10:30:00 GMT";
    // First fetch: unconditional → 200 + Last-Modified.
    // `header_exists` rather than an exact match: wiremock's exact header
    // matcher splits values on commas, which an HTTP-date contains. The
    // sent value is asserted through the cached row below instead.
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .and(header_exists("if-modified-since"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Last-Modified", last_modified)
                .set_body_json(series_detail_fixture()),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = &app.state().db;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let first = client
        .fetch_series_cached(db, "123")
        .await
        .expect("first fetch");
    assert_eq!(first.series_name.as_deref(), Some("Saga"));
    let row = entity::metadata_cache::Entity::find_by_id((
        "metron".to_owned(),
        "series".to_owned(),
        "123".to_owned(),
    ))
    .one(db)
    .await
    .unwrap()
    .expect("cached row");
    assert_eq!(row.last_modified.as_deref(), Some(last_modified));
    assert!(row.etag.is_none());

    // Inside the TTL the cache answers alone (no HTTP at all).
    let hit = client
        .fetch_series_cached(db, "123")
        .await
        .expect("cache hit");
    assert_eq!(hit.series_name.as_deref(), Some("Saga"));

    // Age the row past the 168h series TTL; the next fetch must send
    // If-Modified-Since, get the 304, and serve the stored body.
    entity::metadata_cache::Entity::update_many()
        .col_expr(
            entity::metadata_cache::Column::FetchedAt,
            Expr::value(chrono::Utc::now() - chrono::Duration::days(10)),
        )
        .filter(entity::metadata_cache::Column::ExternalId.eq("123"))
        .exec(db)
        .await
        .unwrap();
    let revalidated = client
        .fetch_series_cached(db, "123")
        .await
        .expect("304 path");
    assert_eq!(revalidated.series_name.as_deref(), Some("Saga"));
    assert_eq!(revalidated.year_began, Some(2012));

    // The 304 refreshed `fetched_at`: a plain TTL read is a hit again.
    let fresh = cache::get(
        db,
        Source::Metron,
        cache::CacheEntity::Series,
        "123",
        chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    assert!(fresh.is_some(), "fetched_at refreshed by the 304");
}

#[tokio::test]
async fn changed_detail_row_is_refetched_when_upstream_sends_200() {
    let mock = MockServer::start().await;
    // The upstream answers a conditional request with a new body + a
    // newer validator; the cache must store the new payload.
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .and(header_exists("if-modified-since"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Last-Modified", "Thu, 13 Feb 2026 10:30:00 GMT")
                .set_body_json({
                    let mut v = series_detail_fixture();
                    v["name"] = json!("Saga (Deluxe)");
                    v
                }),
        )
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/123/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Last-Modified", "Wed, 12 Feb 2026 10:30:00 GMT")
                .set_body_json(series_detail_fixture()),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = &app.state().db;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    client.fetch_series_cached(db, "123").await.expect("first");
    entity::metadata_cache::Entity::update_many()
        .col_expr(
            entity::metadata_cache::Column::FetchedAt,
            Expr::value(chrono::Utc::now() - chrono::Duration::days(10)),
        )
        .exec(db)
        .await
        .unwrap();
    let second = client
        .fetch_series_cached(db, "123")
        .await
        .expect("refetch");
    assert_eq!(second.series_name.as_deref(), Some("Saga (Deluxe)"));
    let row = entity::metadata_cache::Entity::find_by_id((
        "metron".to_owned(),
        "series".to_owned(),
        "123".to_owned(),
    ))
    .one(db)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        row.last_modified.as_deref(),
        Some("Thu, 13 Feb 2026 10:30:00 GMT")
    );
}

#[tokio::test]
async fn last_error_is_recorded_then_cleared_on_success() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
        .up_to_n_times(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([]))))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let redis = app.state().jobs.redis.clone();
    let client = MetronClient::with_base_url("u", "p", mock.uri(), redis.clone());
    let q = SeriesQuery {
        name: "x".into(),
        year: None,
        publisher: None,
        limit: 1,
    };
    let err = client.search_series(&q).await.expect_err("401");
    assert!(matches!(err, ProviderError::Unauthorized(_)));
    let last = budget::load_last_error(&redis, Source::Metron)
        .await
        .expect("recorded");
    assert!(last.message.contains("credentials"), "{}", last.message);

    client.search_series(&q).await.expect("second call ok");
    assert!(
        budget::load_last_error(&redis, Source::Metron)
            .await
            .is_none()
    );
}

/// Provider coverage listing: issue ids, canonical numbers and cover dates
/// from `/api/issue/?series_id=`, every page walked; a page cap returns a
/// partial list flagged `complete = false`.
#[tokio::test]
async fn list_series_issues_carries_ids_and_cover_dates() {
    use server::metadata::provider::IssueListOpts;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "200"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 3, "next": "https://metron.cloud/api/issue/?page=2", "previous": null,
            "results": [
                {"id": 1, "number": "001", "cover_date": "1998-11-01",
                 "series": {"id": 200, "name": "Daredevil", "year_began": 1998}},
                {"id": 2, "number": "2", "cover_date": "1998-12-01"},
            ]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "200"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "count": 3, "next": null, "previous": null,
            "results": [{"id": 3, "number": "3", "cover_date": null}]
        })))
        .mount(&mock)
        .await;
    let app = TestApp::spawn().await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());

    let list = client
        .list_series_issues("200", &IssueListOpts::default())
        .await
        .unwrap();
    assert!(list.complete);
    assert_eq!(list.requests, 2);
    assert_eq!(list.series_name.as_deref(), Some("Daredevil"));
    assert_eq!(list.year_began, Some(1998));
    let numbers: Vec<&str> = list.issues.iter().map(|i| i.number.as_str()).collect();
    assert_eq!(numbers, ["1", "2", "3"], "canonical numbers");
    assert_eq!(list.issues[0].external_id.as_deref(), Some("1"));
    assert_eq!(
        list.issues[0].cover_date,
        chrono::NaiveDate::from_ymd_opt(1998, 11, 1)
    );
    assert_eq!(list.issues[2].cover_date, None);

    let capped = client
        .list_series_issues(
            "200",
            &IssueListOpts {
                date_hint: Vec::new(),
                max_pages: 1,
            },
        )
        .await
        .unwrap();
    assert!(!capped.complete);
    assert_eq!(capped.issues.len(), 2);
}
