//! Matching-accuracy-1.0 M0: orchestrator stamps one
//! `metadata_match_outcome` row per completed search run, and the
//! prune sweep keeps the table within retention. Anchors the
//! before/after baseline so M2 / M4 tuning has a measurable delta.

mod common;

use common::TestApp;
use entity::metadata_match_outcome;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use server::metadata::comicvine::ComicVineClient;
use server::metadata::identifier::Source;
use server::metadata::match_outcome::{self, MatchOutcomeKind};
use server::metadata::matcher::SeriesQueryFacts;
use server::metadata::orchestrator::{self, StartRunArgs, StoredQuery};
use server::metadata::provider::MetadataProvider;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn ok_envelope_cv(results: serde_json::Value) -> serde_json::Value {
    json!({"status_code": 1, "error": "OK", "results": results})
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

async fn start_series_run(app: &TestApp, facts: &SeriesQueryFacts) -> Uuid {
    orchestrator::start_run(
        &app.state().db,
        StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: Some("abcd".into()),
            library_id: None,
            triggered_by: None,
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &[Source::ComicVine],
            query: StoredQuery::Series(facts.clone()),
        },
    )
    .await
    .expect("start_run")
}

#[tokio::test]
async fn orchestrator_stamps_match_outcome_on_completed_run() {
    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope_cv(json!([
                cv_volume(100, "Saga", "2012", "Image Comics"),
                cv_volume(200, "Saga Adventures", "2015", "Other"),
            ]))),
        )
        .mount(&cv_mock)
        .await;

    let app = TestApp::spawn().await;
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
    let run_id = start_series_run(&app, &facts).await;
    let ranked =
        orchestrator::run_series_search(&app.state().db, run_id, &providers, &facts, 75.0, None)
            .await
            .expect("orchestrator");
    assert_eq!(ranked.len(), 2);

    let rows = metadata_match_outcome::Entity::find()
        .filter(metadata_match_outcome::Column::RunId.eq(run_id))
        .all(&app.state().db)
        .await
        .expect("query");
    assert_eq!(rows.len(), 1, "exactly one outcome row per run");
    let row = &rows[0];
    assert_eq!(row.scope, orchestrator::scope::SERIES);
    assert_eq!(row.candidate_count, 2);
    assert!(
        row.top_score >= row.second_score.unwrap_or(0.0),
        "top_score must be >= second_score (sorted descending)",
    );
    assert!(row.second_score.is_some(), "two candidates => runner-up");
    // No phash plumbed yet (M0 stub); both are None.
    assert!(row.top_hamming.is_none());
    assert!(row.second_hamming.is_none());

    // M0 classifies by Confidence; with the strong matcher the top
    // bucket lands medium-or-better, so the outcome is "multi" of
    // some flavor — exact bucket varies with weights, but the row
    // must exist and decode to one of the five vocabulary strings.
    let parsed = match row.outcome_kind.as_str() {
        "single_good" => MatchOutcomeKind::SingleGood,
        "multi_good" => MatchOutcomeKind::MultiGood,
        "single_bad_cover" => MatchOutcomeKind::SingleBadCover,
        "multi_bad_cover" => MatchOutcomeKind::MultiBadCover,
        "no_match" => MatchOutcomeKind::NoMatch,
        other => panic!("unexpected outcome_kind: {other}"),
    };
    // Two candidates → must be the multi flavor.
    assert!(matches!(
        parsed,
        MatchOutcomeKind::MultiGood | MatchOutcomeKind::MultiBadCover
    ));
}

#[tokio::test]
async fn prune_removes_rows_older_than_cutoff() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;

    // Seed an empty run so the FK constraint passes.
    let facts = SeriesQueryFacts {
        name: "Saga".into(),
        year: None,
        publisher: None,
        volume: None,
    };
    let run_id = start_series_run(&app, &facts).await;

    // Hand-insert two outcome rows: one fresh (today), one 100d old.
    use sea_orm::{ActiveModelTrait, Set};
    let fresh = metadata_match_outcome::ActiveModel {
        id: Set(Uuid::new_v4()),
        run_id: Set(run_id),
        scope: Set("series".into()),
        outcome_kind: Set("no_match".into()),
        top_score: Set(0.0),
        top_hamming: Set(None),
        second_score: Set(None),
        second_hamming: Set(None),
        candidate_count: Set(0),
        created_at: Set(chrono::Utc::now().into()),
    };
    fresh.insert(db).await.expect("insert fresh");
    let stale_ts = chrono::Utc::now() - chrono::Duration::days(100);
    let stale = metadata_match_outcome::ActiveModel {
        id: Set(Uuid::new_v4()),
        run_id: Set(run_id),
        scope: Set("series".into()),
        outcome_kind: Set("no_match".into()),
        top_score: Set(0.0),
        top_hamming: Set(None),
        second_score: Set(None),
        second_hamming: Set(None),
        candidate_count: Set(0),
        created_at: Set(stale_ts.into()),
    };
    stale.insert(db).await.expect("insert stale");

    let deleted = match_outcome::prune(db, 90).await.expect("prune");
    assert_eq!(deleted, 1, "exactly the stale row should be pruned");

    let remaining = metadata_match_outcome::Entity::find()
        .filter(metadata_match_outcome::Column::RunId.eq(run_id))
        .all(db)
        .await
        .expect("query");
    assert_eq!(remaining.len(), 1);
    // Fresh row survived.
    assert!(
        (chrono::Utc::now() - chrono::DateTime::<chrono::Utc>::from(remaining[0].created_at))
            .num_days()
            < 1
    );
}
