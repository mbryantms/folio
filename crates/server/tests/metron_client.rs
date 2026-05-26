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
//! - search_series uses ?name + ?year_began
//! - structured credit roles map straight through (no comma-splitting
//!   needed — Metron normalizes upstream)

mod common;

use common::TestApp;
use server::metadata::cache;
use server::metadata::identifier::Source;
use server::metadata::metron::MetronClient;
use server::metadata::provider::{IssueQuery, MetadataProvider, ProviderError, SeriesQuery};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{basic_auth, method, path, query_param},
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
async fn search_series_uses_name_and_year_filters() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("name", "Saga"))
        .and(query_param("year_began", "2012"))
        .and(basic_auth("metron-user", "metron-pass"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([series_list_fixture()]))))
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
    // 3 credits: writer + cover (exploded) + artist.
    assert_eq!(m.credits.len(), 3);
    assert!(m.credits.iter().any(|c| c.role == "writer"));
    assert!(m.credits.iter().any(|c| c.role == "cover"));
    assert!(m.credits.iter().any(|c| c.role == "artist"));
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
    let client = MetronClient::with_base_url("bad", "creds", mock.uri(), app.state().jobs.redis.clone());

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
