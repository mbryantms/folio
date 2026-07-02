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

/// PERF-4: concurrent cache misses for the same key single-flight — only the
/// first (leader) caller runs `fetch`; the rest await the per-key lock and then
/// read the value the leader cached, instead of all stampeding the provider.
#[tokio::test]
async fn get_or_fetch_single_flights_concurrent_misses() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let ttl = chrono::Duration::hours(1);
    let fetches = Arc::new(AtomicU32::new(0));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        let fetches = fetches.clone();
        handles.push(tokio::spawn(async move {
            cache::get_or_fetch(
                &db,
                Source::ComicVine,
                CacheEntity::Series,
                "single-flight-key",
                ttl,
                || async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    // Hold the leader's lock long enough for the others to queue.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Ok::<_, String>(GenericMetadata {
                        series_name: Some("Coalesced".into()),
                        ..Default::default()
                    })
                },
            )
            .await
        }));
    }

    for handle in handles {
        let got = handle.await.expect("task join").expect("resolves Ok");
        assert_eq!(got.series_name.as_deref(), Some("Coalesced"));
    }
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "single-flight: only the leader should have run `fetch`",
    );
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
