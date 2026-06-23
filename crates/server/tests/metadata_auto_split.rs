//! Auto-split integration: provider series-boundary divergence (M7).
//!
//! Drives [`server::metadata::auto_split::detect_and_map`] against a
//! wiremock-backed Metron client to verify that, after matching a local
//! series to a provider's main run, the detector finds the local issues
//! that run doesn't cover, identifies the alternate provider series that
//! does, and writes a `series_provider_range` mapping for them — once
//! (idempotent on re-run).

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::EntityTrait;
use serde_json::json;
use server::metadata::auto_split;
use server::metadata::identifier::Source;
use server::metadata::metron::MetronClient;
use std::sync::Arc;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

fn paged(results: serde_json::Value) -> serde_json::Value {
    json!({
        "count": results.as_array().map(|a| a.len()).unwrap_or(0),
        "next": null,
        "previous": null,
        "results": results,
    })
}

#[tokio::test]
async fn auto_split_detects_relaunch_block_and_creates_range() {
    let metron = MockServer::start().await;
    // Enumeration of the matched "main run" (series_id=MAIN) covers only
    // #1 and #2 — NOT the #600-601 relaunch block.
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("series_id", "MAIN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 1, "number": "1", "series": {"name": "Fantastic Four", "year_began": 1998}},
            {"id": 2, "number": "2", "series": {"name": "Fantastic Four", "year_began": 1998}},
        ]))))
        .mount(&metron)
        .await;
    // Broad search for the gap's representative issue (#600) returns the
    // separate 2012 series (id 62349) — the candidate carries the series
    // id so the cheap path resolves it without a detail fetch.
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .and(query_param("number", "600"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 20519, "number": "600",
             "series": {"id": 62349, "name": "Fantastic Four", "year_began": 2012}},
        ]))))
        .mount(&metron)
        .await;

    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib, "Fantastic Four").insert(&db).await;
    for n in [1.0_f64, 2.0, 600.0, 601.0] {
        let p = tmp.path().join(format!("ff-{n}.cbz"));
        // Unique payload per issue — the seed derives the issue id from a
        // BLAKE3 of the bytes, so identical content would collide.
        let payload = format!("ff issue {n}");
        IssueSeed::new(lib, series_id, &p, payload.as_bytes(), n)
            .insert(&db)
            .await;
    }

    let client =
        MetronClient::with_base_url("u", "p", metron.uri(), app.state().jobs.redis.clone());
    let series_row = entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();

    let provider: Arc<dyn server::metadata::provider::MetadataProvider> = Arc::new(client);
    let outcome = auto_split::detect_and_map(&db, &series_row, Source::Metron, "MAIN", &*provider)
        .await
        .unwrap();

    let created = &outcome.created;
    assert_eq!(created.len(), 1, "one alternate-series range");
    assert_eq!(created[0].provider_series_id, "62349");
    assert_eq!(created[0].range_low, "600");
    assert_eq!(created[0].range_high, "601");
    assert_eq!(created[0].declared_year, Some(2012));

    // Persisted.
    let rows = entity::series_provider_range::Entity::find()
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider_series_id, "62349");

    // Idempotent — re-running doesn't duplicate the mapping.
    let again = auto_split::detect_and_map(&db, &series_row, Source::Metron, "MAIN", &*provider)
        .await
        .unwrap();
    assert!(again.created.is_empty(), "second run creates nothing new");
    let rows = entity::series_provider_range::Entity::find()
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "still just one row");
}
