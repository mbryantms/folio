//! ComicVine HTTP client integration tests (metadata-providers-1.0 M1).
//!
//! Exercises the live HTTP path of `ComicVineClient` against a
//! wiremock-backed mock CV API. Pattern mirrors the OIDC tests
//! (`tests/oidc.rs`): per-test mock server, real Redis via
//! testcontainers (for the token bucket), real DB via testcontainers
//! (for the cache table).
//!
//! Coverage:
//! - happy path: search_series + fetch_series + fetch_issue
//! - envelope status_code mapping: 100 (auth), 101 (not found), 107 (rate limit)
//! - velocity cap: two back-to-back calls have ≥1s gap (CV's per-sec rule)
//! - response cache: second fetch hits cache, no second wiremock request

mod common;

use common::TestApp;
use server::metadata::cache;
use server::metadata::comicvine::ComicVineClient;
use server::metadata::identifier::Source;
use server::metadata::provider::{IssueQuery, MetadataProvider, ProviderError, SeriesQuery};
use serde_json::json;
use std::time::Instant;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

// ────────────────────── fixtures ──────────────────────

fn ok_envelope(results: serde_json::Value) -> serde_json::Value {
    json!({
        "status_code": 1,
        "error": "OK",
        "number_of_total_results": 1,
        "number_of_page_results": 1,
        "limit": 100,
        "offset": 0,
        "results": results,
    })
}

fn cv_volume_fixture(id: i64, name: &str, year: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "start_year": year,
        "publisher": { "id": 99, "name": "Image Comics" },
        "deck": "Sci-fi epic",
        "description": "<p>A long, ongoing space opera.</p>",
        "image": {
            "icon_url": "https://cdn/icon.jpg",
            "medium_url": "https://cdn/medium.jpg",
            "screen_url": "https://cdn/screen.jpg",
            "super_url": "https://cdn/super.jpg",
            "original_url": "https://cdn/original.jpg",
            "thumb_url": "https://cdn/thumb.jpg",
        },
        "count_of_issues": 60,
        "site_detail_url": "https://comicvine.gamespot.com/volume/4050-12345/",
        "date_last_updated": "2024-01-15 12:34:56",
        "aliases": "Alias One\nAlias Two",
    })
}

fn cv_issue_fixture(id: i64, number: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": "First Issue",
        "issue_number": number,
        "cover_date": "2012-03-14",
        "store_date": "2012-03-12",
        "deck": "Short blurb",
        "description": "Full HTML body.",
        "image": {
            "super_url": "https://cdn/super.jpg",
            "original_url": "https://cdn/original.jpg",
            "icon_url": null,
            "medium_url": null,
            "screen_url": null,
            "thumb_url": null
        },
        "person_credits": [
            {"id": 7, "name": "Brian K. Vaughan", "role": "writer, cover", "site_detail_url": null},
            {"id": 8, "name": "Fiona Staples", "role": "artist", "site_detail_url": null},
        ],
        "character_credits": [
            {"id": 100, "name": "Hazel", "site_detail_url": null},
        ],
        "team_credits": [],
        "location_credits": [],
        "concept_credits": [],
        "object_credits": [],
        "story_arc_credits": [
            {"id": 200, "name": "The Will", "site_detail_url": null},
        ],
        "associated_images": [
            {"original_url": "https://cdn/variant-b.jpg", "id": 501, "caption": "Cover B", "image_tags": "Cover"},
            {"original_url": "https://cdn/variant-c.jpg", "id": 502, "caption": null, "image_tags": "Cover"},
        ],
        "first_appearance_characters": [
            {"id": 100, "name": "Hazel", "site_detail_url": null},
        ],
        "volume": {
            "id": 12345,
            "name": "Saga",
            "start_year": "2012",
            "site_detail_url": null,
            "publisher": null,
            "deck": null,
            "description": null,
            "image": null,
            "count_of_issues": null,
            "date_last_updated": null,
            "aliases": null,
        },
        "site_detail_url": "https://comicvine.gamespot.com/issue/4000-67890/",
        "date_last_updated": "2024-02-20 08:00:00",
        "aliases": null,
    })
}

// ────────────────────── tests ──────────────────────

#[tokio::test]
async fn search_series_returns_normalized_candidates() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .and(query_param("api_key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(json!([cv_volume_fixture(12345, "Saga", "2012")]))),
        )
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
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
    let c = &candidates[0];
    assert_eq!(c.source, Source::ComicVine);
    assert_eq!(c.external_id, "12345");
    assert_eq!(c.name, "Saga");
    assert_eq!(c.year, Some(2012));
    assert_eq!(c.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(c.cover_image_url.as_deref(), Some("https://cdn/super.jpg"));
}

#[tokio::test]
async fn fetch_issue_explodes_multi_role_credits() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issue/4000-67890"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(cv_issue_fixture(67890, "1"))),
        )
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

    let metadata = client.fetch_issue("67890").await.expect("fetch_issue");
    assert_eq!(metadata.issue_number.as_deref(), Some("1"));
    assert_eq!(metadata.title.as_deref(), Some("First Issue"));
    // "writer, cover" exploded into two rows; "artist" stays one. Roles
    // are canonicalized to ComicInfo names at the provider boundary:
    // writer→Writer, cover→CoverArtist, artist→Penciller.
    assert_eq!(metadata.credits.len(), 3);
    assert!(metadata.credits.iter().any(|c| c.role == "Writer"));
    assert!(metadata.credits.iter().any(|c| c.role == "CoverArtist"));
    assert!(metadata.credits.iter().any(|c| c.role == "Penciller"));
    // Series back-reference preserved.
    assert_eq!(metadata.series_name.as_deref(), Some("Saga"));
    assert_eq!(metadata.year_began, Some(2012));
    // External identifiers carry both issue + series CV ids.
    assert_eq!(metadata.identifiers.len(), 2);
    assert!(metadata.identifiers.iter().any(|i| i.id == "67890"));
    assert!(metadata.identifiers.iter().any(|i| i.id == "12345"));
    // Characters + story_arcs propagated with CV person ids.
    assert_eq!(metadata.characters.len(), 1);
    assert_eq!(metadata.story_arcs.len(), 1);
    assert_eq!(metadata.characters[0].name, "Hazel");
    // Hazel is flagged a first appearance via first_appearance_characters.
    assert!(metadata.characters[0].is_first_appearance);
    // Variant covers pulled from associated_images.
    assert_eq!(metadata.variants.len(), 2);
    assert_eq!(
        metadata.variants[0].image_url.as_deref(),
        Some("https://cdn/variant-b.jpg")
    );
    assert_eq!(metadata.variants[0].label.as_deref(), Some("Cover B"));
    assert_eq!(metadata.variants[0].identifiers[0].id, "501");
}

#[tokio::test]
async fn status_code_100_maps_to_unauthorized() {
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 100,
            "error": "Invalid API Key",
            "results": []
        })),
    )
    .mount(&mock)
    .await;

    let app = TestApp::spawn_with_comicvine("bad-key", true).await;
    let client = ComicVineClient::with_base_url(
        "bad-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

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
async fn status_code_101_maps_to_not_found() {
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 101,
            "error": "Object Not Found",
            "results": []
        })),
    )
    .mount(&mock)
    .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

    let err = client.fetch_series("99999").await.expect_err("not found");
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn status_code_107_maps_to_quota_exceeded() {
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(
        ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 107,
            "error": "Rate limit",
            "results": []
        })),
    )
    .mount(&mock)
    .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

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
async fn velocity_cap_enforces_one_second_floor() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!([]))))
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

    let q = SeriesQuery {
        name: "warmup".into(),
        year: None,
        publisher: None,
        limit: 1,
    };
    // Warm up so the velocity-tracker has a baseline.
    client.search_series(&q).await.expect("warmup");
    let start = Instant::now();
    client.search_series(&q).await.expect("second call");
    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(1000),
        "second call too fast: {elapsed:?}; velocity cap not enforced",
    );
}

#[tokio::test]
async fn fetch_series_cached_round_trips_through_metadata_cache() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-12345"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_envelope(cv_volume_fixture(12345, "Saga", "2012"))),
        )
        // Expect exactly one upstream hit even across two fetches.
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );
    let db = &app.state().db;

    let first = client
        .fetch_series_cached(db, "12345")
        .await
        .expect("first fetch");
    let second = client
        .fetch_series_cached(db, "12345")
        .await
        .expect("second fetch");
    // Cache hit returns the same body shape; expect(1) on the mock
    // asserts only one HTTP request was made.
    assert_eq!(first.series_name, second.series_name);
    assert_eq!(first.series_name.as_deref(), Some("Saga"));
    // Sanity: row landed in the cache table.
    let hit = cache::get(
        db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        chrono::Duration::hours(168),
    )
    .await
    .expect("cache lookup");
    assert!(hit.is_some(), "expected cache row after first fetch");
}

#[tokio::test]
async fn search_issue_filters_by_volume_when_id_known() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/issues"))
        .and(query_param("filter", "issue_number:1,volume:12345"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(json!([cv_issue_fixture(67890, "1")]))),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_comicvine("test-key", true).await;
    let client = ComicVineClient::with_base_url(
        "test-key".into(),
        mock.uri(),
        app.state().jobs.redis.clone(),
    );

    let out = client
        .search_issue(&IssueQuery {
            series_external_id: Some("12345".into()),
            series_name: None,
            series_year: None,
            issue_number: "1".into(),
            cover_year: None,
            limit: 5,
        })
        .await
        .expect("search_issue");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].external_id, "67890");
}
