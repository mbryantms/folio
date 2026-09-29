//! Search-time cover pHash cache (WP-2.9, audit DI-16).
//!
//! Coverage:
//! - put/get round trip, 30-day TTL, expired-row sweep
//! - `fetch_and_hash_cover` serves a cached hash with **zero network**
//!   (the URL's host is unresolvable, so any fetch attempt would `None`)
//! - a series search scores a candidate's cover from the cache alone

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed, seed_issue};
use image::{ImageBuffer, Rgb};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use server::metadata::comicvine::ComicVineClient;
use server::metadata::cover_hash_cache::{self, CoverHashes};
use server::metadata::identifier::Source;
use server::metadata::matcher::{SeriesQueryFacts, Thresholds};
use server::metadata::orchestrator::{self, PreFilter, StartRunArgs, StoredQuery};
use server::metadata::phash;
use server::metadata::provider::MetadataProvider;
use server::util::ssrf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

/// `.invalid` is reserved (RFC 6761) and never resolves — a fetch of
/// this URL can only ever fail, so a `Some` hash proves the cache hit.
const UNRESOLVABLE_COVER: &str = "https://covers.invalid/saga-1.jpg";

fn client() -> reqwest::Client {
    ssrf::shared_public_client("folio-test", Duration::from_secs(2), 2)
}

#[tokio::test]
async fn put_get_round_trip_ttl_and_sweep() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let hashes = CoverHashes {
        phash: 0x1234_5678_9abc_def0,
        dhash: 42,
        ahash: -7,
    };
    cover_hash_cache::put(db, UNRESOLVABLE_COVER, hashes)
        .await
        .expect("put");
    let hit = cover_hash_cache::get(db, UNRESOLVABLE_COVER)
        .await
        .expect("get")
        .expect("hit");
    assert_eq!(hit, hashes);
    // Re-put updates in place (primary key = url).
    cover_hash_cache::put(db, UNRESOLVABLE_COVER, CoverHashes { phash: 1, ..hashes })
        .await
        .expect("re-put");
    assert_eq!(
        cover_hash_cache::get(db, UNRESOLVABLE_COVER)
            .await
            .unwrap()
            .unwrap()
            .phash,
        1
    );

    // Age the row past the TTL: it's a miss, and the sweep drops it.
    entity::metadata_cover_hash::Entity::update_many()
        .col_expr(
            entity::metadata_cover_hash::Column::FetchedAt,
            Expr::value(Utc::now() - chrono::Duration::days(31)),
        )
        .filter(entity::metadata_cover_hash::Column::Url.eq(UNRESOLVABLE_COVER))
        .exec(db)
        .await
        .unwrap();
    assert!(
        cover_hash_cache::get(db, UNRESOLVABLE_COVER)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(cover_hash_cache::sweep_expired(db).await.unwrap(), 1);
    assert!(
        entity::metadata_cover_hash::Entity::find_by_id(UNRESOLVABLE_COVER.to_owned())
            .one(db)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn fetch_and_hash_cover_serves_cached_hash_without_network() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let http = client();

    // Unseeded: the host can't resolve, so the fetch soft-fails.
    assert_eq!(
        phash::fetch_and_hash_cover(db, &http, UNRESOLVABLE_COVER, Duration::from_secs(2)).await,
        None
    );

    cover_hash_cache::put(
        db,
        UNRESOLVABLE_COVER,
        CoverHashes {
            phash: 0x0f0f_0f0f_0f0f_0f0f,
            dhash: 0,
            ahash: 0,
        },
    )
    .await
    .unwrap();

    // Seeded: same URL, same unresolvable host — the only way this can
    // be `Some` is the cache.
    assert_eq!(
        phash::fetch_and_hash_cover(db, &http, UNRESOLVABLE_COVER, Duration::from_secs(2)).await,
        Some(0x0f0f_0f0f_0f0f_0f0f)
    );
}

/// End to end: a series search whose candidate cover is already in the
/// cache scores the cover (Hamming 0 against the local cover) without
/// any cover download — the candidate URL's host is unresolvable, so a
/// download attempt would have left `cover_hamming` at `None`.
#[tokio::test]
async fn series_search_scores_cover_from_cache_without_download() {
    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 1,
            "error": "OK",
            "results": [{
                "id": 100,
                "name": "Saga",
                "start_year": "2012",
                "publisher": { "id": 99, "name": "Image Comics" },
                "image": { "original_url": UNRESOLVABLE_COVER },
                "count_of_issues": 60,
                "site_detail_url": "https://comicvine.gamespot.com/volume/4050-100/",
            }]
        })))
        .expect(2)
        .mount(&cv_mock)
        .await;

    let app = TestApp::spawn().await;
    let db = &app.state().db;

    // Local series with a hashed primary cover.
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(db).await;
    let series_id = SeriesSeed::new(lib, "Saga").insert(db).await;
    let issue_id = seed_issue(
        db,
        lib,
        series_id,
        &dir.path().join("saga-001.cbz"),
        b"cbz",
        1.0,
    )
    .await;
    let img = image::DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 120, |x, y| {
        Rgb([x as u8 * 3, y as u8, 200])
    }));
    phash::upsert_archive_cover_hashes(db, &issue_id, "thumbs/saga/cover.webp", &img)
        .await
        .expect("seed local cover");
    let local = phash::series_representative_phash(db, series_id)
        .await
        .unwrap()
        .expect("local phash");

    // The candidate's cover is "already hashed": identical to the local one.
    cover_hash_cache::put(
        db,
        UNRESOLVABLE_COVER,
        CoverHashes {
            phash: local,
            dhash: 0,
            ahash: 0,
        },
    )
    .await
    .unwrap();

    let providers: Vec<Arc<dyn MetadataProvider>> = vec![Arc::new(ComicVineClient::with_base_url(
        "k".into(),
        cv_mock.uri(),
        app.state().jobs.redis.clone(),
    ))];
    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: Some(2012),
        publisher: Some("Image Comics".into()),
        volume: None,
    };
    // Two searches back to back: both score the cover from the cache.
    for _ in 0..2 {
        let run_id = orchestrator::start_run(
            db,
            StartRunArgs {
                scope: orchestrator::scope::SERIES,
                scope_entity_id: Some(series_id.to_string()),
                library_id: None,
                triggered_by: None,
                trigger_kind: orchestrator::trigger_kind::MANUAL,
                providers: &[Source::ComicVine],
                query: StoredQuery::Series(facts.clone()),
                batch_id: None,
            },
        )
        .await
        .expect("start_run");
        let ranked = orchestrator::run_series_search(
            db,
            run_id,
            &providers,
            &facts,
            Thresholds::new(75.0, 70.0),
            &PreFilter::default(),
            3,
            Some(series_id),
        )
        .await
        .expect("search");
        assert_eq!(ranked.len(), 1);
        assert_eq!(
            ranked[0].score.cover_hamming,
            Some(0),
            "cover scored from the cache: {:?}",
            ranked[0].score
        );
    }
}
