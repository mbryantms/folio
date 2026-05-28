//! Tests for the metadata_cache table + cache helpers
//! (metadata-providers-1.0 M1).

mod common;

use common::TestApp;
use server::metadata::cache::{self, CacheEntity};
use server::metadata::identifier::Source;
use server::metadata::provider::GenericMetadata;

#[tokio::test]
async fn cache_round_trips_payload() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let payload = GenericMetadata {
        series_name: Some("Saga".into()),
        year_began: Some(2012),
        publisher: Some("Image Comics".into()),
        source_provider: Some(Source::ComicVine),
        ..Default::default()
    };
    cache::put(
        db,
        Source::ComicVine,
        CacheEntity::Series,
        "12345",
        &payload,
    )
    .await
    .expect("put");
    let got = cache::get(
        db,
        Source::ComicVine,
        CacheEntity::Series,
        "12345",
        chrono::Duration::hours(168),
    )
    .await
    .expect("get")
    .expect("hit");
    assert_eq!(got.series_name.as_deref(), Some("Saga"));
    assert_eq!(got.year_began, Some(2012));
}

#[tokio::test]
async fn cache_misses_when_stale() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let payload = GenericMetadata {
        series_name: Some("Saga".into()),
        ..Default::default()
    };
    cache::put(
        db,
        Source::ComicVine,
        CacheEntity::Series,
        "55555",
        &payload,
    )
    .await
    .expect("put");
    // Negative TTL guarantees every row is stale.
    let got = cache::get(
        db,
        Source::ComicVine,
        CacheEntity::Series,
        "55555",
        chrono::Duration::seconds(-1),
    )
    .await
    .expect("get");
    assert!(got.is_none(), "expected stale miss");
}

#[tokio::test]
async fn cache_returns_none_for_unknown_key() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let got = cache::get(
        db,
        Source::Metron,
        CacheEntity::Issue,
        "no-such-id",
        chrono::Duration::hours(24),
    )
    .await
    .expect("get");
    assert!(got.is_none());
}

#[tokio::test]
async fn purge_provider_removes_only_matching_rows() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let payload = GenericMetadata::default();
    cache::put(db, Source::ComicVine, CacheEntity::Series, "a", &payload)
        .await
        .unwrap();
    cache::put(db, Source::ComicVine, CacheEntity::Issue, "b", &payload)
        .await
        .unwrap();
    cache::put(db, Source::Metron, CacheEntity::Series, "c", &payload)
        .await
        .unwrap();

    let removed = cache::purge_provider(db, Source::ComicVine)
        .await
        .expect("purge");
    assert_eq!(removed, 2);
    // Metron row survives.
    let metron = cache::get(
        db,
        Source::Metron,
        CacheEntity::Series,
        "c",
        chrono::Duration::hours(168),
    )
    .await
    .expect("get")
    .expect("hit");
    assert!(metron.series_name.is_none());
}
