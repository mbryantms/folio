//! Orchestrator integration tests (metadata-providers-1.0 M3).
//!
//! Drives [`server::metadata::orchestrator::run_series_search`] +
//! [`run_issue_search`] against wiremock-backed CV + Metron clients to
//! verify:
//! - candidates from both providers fuse and rank by score
//! - matcher buckets reach `metadata_run_candidate.bucket` round-trip
//! - run row transitions queued → searching → completed with the
//!   correct items_matched_{high,medium,low} counts
//! - quota-exhaustion across every provider yields `awaiting_quota`
//! - a hard provider error with no surviving results lands `failed`
//! - the orchestrator persists ranked candidates in score-descending
//!   order

mod common;

use common::TestApp;
use sea_orm::EntityTrait;
use serde_json::json;
use server::metadata::comicvine::ComicVineClient;
use server::metadata::identifier::Source;
use server::metadata::matcher::{IssueQueryFacts, SeriesQueryFacts};
use server::metadata::metron::MetronClient;
use server::metadata::orchestrator::{
    self, status, StartRunArgs, StoredQuery,
};
use server::metadata::provider::MetadataProvider;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn ok_envelope_cv(results: serde_json::Value) -> serde_json::Value {
    json!({
        "status_code": 1,
        "error": "OK",
        "results": results,
    })
}

fn paged_metron(results: serde_json::Value) -> serde_json::Value {
    json!({
        "count": results.as_array().map(|a| a.len()).unwrap_or(0),
        "next": null,
        "previous": null,
        "results": results,
    })
}

fn cv_volume(id: i64, name: &str, year: &str, publisher: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": name,
        "start_year": year,
        "publisher": {"id": 1, "name": publisher},
        "deck": null,
        "description": null,
        "image": null,
        "count_of_issues": null,
        "site_detail_url": null,
        "date_last_updated": null,
        "aliases": null,
    })
}

fn metron_series_list(id: i64, name: &str, year: i32) -> serde_json::Value {
    json!({
        "id": id,
        "series": name,
        "year_began": year,
        "issue_count": 60,
        "modified": "2024-01-15T12:34:56Z"
    })
}

async fn start_series_run(app: &TestApp, facts: &SeriesQueryFacts) -> Uuid {
    let providers = vec![Source::Metron, Source::ComicVine];
    orchestrator::start_run(
        &app.state().db,
        StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: Some("abcd".into()),
            library_id: None,
            triggered_by: None,
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &providers,
            query: StoredQuery::Series(facts.clone()),
        },
    )
    .await
    .expect("start_run")
}

#[tokio::test]
async fn run_series_search_fuses_two_providers_and_sorts_by_score() {
    let cv_mock = MockServer::start().await;
    let metron_mock = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope_cv(json!([
                cv_volume(100, "Saga", "2012", "Image Comics"),
                cv_volume(200, "Saga Adventures", "2015", "Other Pub"),
            ]))),
        )
        .mount(&cv_mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paged_metron(json!([
                metron_series_list(300, "Saga", 2012),
            ]))),
        )
        .mount(&metron_mock)
        .await;

    let app = TestApp::spawn().await;
    let providers: Vec<Arc<dyn MetadataProvider>> = vec![
        Arc::new(MetronClient::with_base_url(
            "u",
            "p",
            metron_mock.uri(),
            app.state().jobs.redis.clone(),
        )),
        Arc::new(ComicVineClient::with_base_url(
            "k".into(),
            cv_mock.uri(),
            app.state().jobs.redis.clone(),
        )),
    ];

    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: Some(2012),
        publisher: Some("Image Comics".into()),
        volume: None,
    };
    let run_id = start_series_run(&app, &facts).await;
    let ranked = orchestrator::run_series_search(&app.state().db, run_id, &providers, &facts, 75.0, None)
        .await
        .expect("orchestrator search");
    // 3 results: 2 from CV + 1 from Metron.
    assert_eq!(ranked.len(), 3);
    // Sorted descending by score.
    let scores: Vec<_> = ranked.iter().map(|r| r.score.total).collect();
    for w in scores.windows(2) {
        assert!(w[0] >= w[1], "candidates not sorted descending: {scores:?}");
    }
    // The exact Saga (2012) match — Metron or CV — should top the list.
    assert!(ranked[0].score.total >= 80.0);

    // Run row finalized.
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, status::COMPLETED);
    assert_eq!(run.items_total, 3);

    // Candidate rows landed in ordinal order.
    let rows = orchestrator::fetch_candidates(&app.state().db, run_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let ordinals: Vec<_> = rows.iter().map(|r| r.ordinal).collect();
    assert_eq!(ordinals, vec![0, 1, 2]);
}

#[tokio::test]
async fn run_series_search_yields_awaiting_quota_when_all_providers_exhausted() {
    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status_code": 107,
            "error": "Rate limit",
            "results": []
        })))
        .mount(&cv_mock)
        .await;

    let app = TestApp::spawn().await;
    let providers: Vec<Arc<dyn MetadataProvider>> = vec![Arc::new(
        ComicVineClient::with_base_url("k".into(), cv_mock.uri(), app.state().jobs.redis.clone()),
    )];
    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: None,
        publisher: None,
        volume: None,
    };
    let run_id = start_series_run(&app, &facts).await;
    let err = orchestrator::run_series_search(&app.state().db, run_id, &providers, &facts, 75.0, None)
        .await
        .expect_err("should signal QuotaExceeded");
    assert!(matches!(
        err,
        server::metadata::provider::ProviderError::QuotaExceeded { .. }
    ));
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, status::AWAITING_QUOTA);
    assert!(run.resume_after.is_some());
}

#[tokio::test]
async fn run_series_search_fails_when_provider_errors_and_no_candidates() {
    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream blew up"))
        .mount(&cv_mock)
        .await;

    let app = TestApp::spawn().await;
    let providers: Vec<Arc<dyn MetadataProvider>> = vec![Arc::new(
        ComicVineClient::with_base_url("k".into(), cv_mock.uri(), app.state().jobs.redis.clone()),
    )];
    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: None,
        publisher: None,
        volume: None,
    };
    let run_id = start_series_run(&app, &facts).await;
    let err = orchestrator::run_series_search(&app.state().db, run_id, &providers, &facts, 75.0, None)
        .await
        .expect_err("should fail");
    let _ = err;
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, status::FAILED);
    assert!(run.error_summary.is_some());
}

#[tokio::test]
async fn run_series_search_partial_failure_still_finalizes() {
    // CV errors out, Metron returns a result. The orchestrator should
    // finalize with the surviving candidate rather than failing the
    // entire run.
    let cv_mock = MockServer::start().await;
    let metron_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("CV down"))
        .mount(&cv_mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paged_metron(json!([
                metron_series_list(11, "Saga", 2012),
            ]))),
        )
        .mount(&metron_mock)
        .await;

    let app = TestApp::spawn().await;
    let providers: Vec<Arc<dyn MetadataProvider>> = vec![
        Arc::new(MetronClient::with_base_url(
            "u",
            "p",
            metron_mock.uri(),
            app.state().jobs.redis.clone(),
        )),
        Arc::new(ComicVineClient::with_base_url(
            "k".into(),
            cv_mock.uri(),
            app.state().jobs.redis.clone(),
        )),
    ];
    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: Some(2012),
        publisher: None,
        volume: None,
    };
    let run_id = start_series_run(&app, &facts).await;
    let ranked = orchestrator::run_series_search(&app.state().db, run_id, &providers, &facts, 75.0, None)
        .await
        .expect("partial success still finalizes");
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].source, Source::Metron);
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, status::COMPLETED);
}

#[tokio::test]
async fn run_issue_search_buckets_high_when_number_and_name_match() {
    let metron_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/issue/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(paged_metron(json!([{
                "id": 456,
                "series": {
                    "id": 11,
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
                "image": "https://static/saga-1.jpg",
                "modified": "2024-01-15T12:34:56Z"
            }]))),
        )
        .mount(&metron_mock)
        .await;

    let app = TestApp::spawn().await;
    let providers: Vec<Arc<dyn MetadataProvider>> = vec![Arc::new(MetronClient::with_base_url(
        "u",
        "p",
        metron_mock.uri(),
        app.state().jobs.redis.clone(),
    ))];
    let facts = IssueQueryFacts {
        series_name: "Saga".into(),
        series_year: Some(2012),
        publisher: None,
        volume: Some(1),
        issue_number: "1".into(),
    };
    let run_id = orchestrator::start_run(
        &app.state().db,
        StartRunArgs {
            scope: orchestrator::scope::ISSUE,
            scope_entity_id: Some("issue-xyz".into()),
            library_id: None,
            triggered_by: None,
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &[Source::Metron],
            query: StoredQuery::Issue(facts.clone()),
        },
    )
    .await
    .unwrap();
    let ranked = orchestrator::run_issue_search(&app.state().db, run_id, &providers, &facts, &[], 80.0, None)
        .await
        .unwrap();
    assert_eq!(ranked.len(), 1);
    // Per matcher unit tests: perfect issue match scores 87.5 with
    // missing publisher (half-credit) — HIGH at 80.0 threshold.
    assert!(ranked[0].score.total >= 80.0);
    assert_eq!(ranked[0].bucket.as_str(), "high");
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.items_matched_high, 1);
    assert_eq!(run.items_matched_medium, 0);
}
