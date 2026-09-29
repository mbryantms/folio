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

// ───────── WP-2.9: validators + revalidation ─────────

#[tokio::test]
async fn get_or_revalidate_304_keeps_body_and_refreshes_ttl() {
    use server::metadata::cache::Validators;
    use server::metadata::provider::ConditionalFetch;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let validators = Validators {
        etag: Some("\"v1\"".into()),
        last_modified: None,
    };
    cache::put_with_validators(
        db,
        Source::Metron,
        CacheEntity::Issue,
        "77",
        &GenericMetadata {
            title: Some("Chapter One".into()),
            ..Default::default()
        },
        &validators,
    )
    .await
    .unwrap();
    // Stale by TTL, but still readable with its validators.
    let (stale, v) = cache::get_stale(db, Source::Metron, CacheEntity::Issue, "77")
        .await
        .unwrap()
        .expect("stale row");
    assert_eq!(stale.title.as_deref(), Some("Chapter One"));
    assert_eq!(v, validators);

    let calls = Arc::new(AtomicUsize::new(0));
    let seen: Arc<std::sync::Mutex<Option<Validators>>> = Arc::new(std::sync::Mutex::new(None));
    let got = cache::get_or_revalidate(
        db,
        Source::Metron,
        CacheEntity::Issue,
        "77",
        chrono::Duration::zero(),
        |sent| {
            let calls = calls.clone();
            let seen = seen.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                *seen.lock().unwrap() = sent;
                Ok::<_, ()>(ConditionalFetch::NotModified)
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(got.title.as_deref(), Some("Chapter One"));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one conditional fetch");
    assert_eq!(seen.lock().unwrap().as_ref(), Some(&validators));
    // `fetched_at` was touched: a normal TTL read is a hit now.
    assert!(
        cache::get(
            db,
            Source::Metron,
            CacheEntity::Issue,
            "77",
            chrono::Duration::minutes(5)
        )
        .await
        .unwrap()
        .is_some()
    );
}

#[tokio::test]
async fn get_or_revalidate_fresh_body_replaces_payload_and_validators() {
    use server::metadata::cache::Validators;
    use server::metadata::provider::ConditionalFetch;

    let app = TestApp::spawn().await;
    let db = &app.state().db;
    cache::put_with_validators(
        db,
        Source::Metron,
        CacheEntity::Series,
        "5",
        &GenericMetadata {
            series_name: Some("Old".into()),
            ..Default::default()
        },
        &Validators {
            etag: None,
            last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".into()),
        },
    )
    .await
    .unwrap();
    let got = cache::get_or_revalidate(
        db,
        Source::Metron,
        CacheEntity::Series,
        "5",
        chrono::Duration::zero(),
        |_sent| async move {
            Ok::<_, ()>(ConditionalFetch::Fresh {
                payload: Box::new(GenericMetadata {
                    series_name: Some("New".into()),
                    ..Default::default()
                }),
                validators: Validators {
                    etag: Some("\"v2\"".into()),
                    last_modified: None,
                },
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(got.series_name.as_deref(), Some("New"));
    let (row, v) = cache::get_stale(db, Source::Metron, CacheEntity::Series, "5")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.series_name.as_deref(), Some("New"));
    assert_eq!(v.etag.as_deref(), Some("\"v2\""));
    assert!(v.last_modified.is_none());
}

#[tokio::test]
async fn get_or_revalidate_without_row_fetches_unconditionally() {
    use server::metadata::cache::Validators;
    use server::metadata::provider::ConditionalFetch;
    use std::sync::Arc;
    use std::sync::Mutex;

    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let seen: Arc<Mutex<Vec<Option<Validators>>>> = Arc::new(Mutex::new(Vec::new()));
    let got = cache::get_or_revalidate(
        db,
        Source::ComicVine,
        CacheEntity::Issue,
        "none",
        chrono::Duration::hours(1),
        |sent| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(sent);
                Ok::<_, ()>(ConditionalFetch::Fresh {
                    payload: Box::new(GenericMetadata {
                        title: Some("Fresh".into()),
                        ..Default::default()
                    }),
                    validators: Validators::default(),
                })
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(got.title.as_deref(), Some("Fresh"));
    assert_eq!(seen.lock().unwrap().as_slice(), &[None]);
}
