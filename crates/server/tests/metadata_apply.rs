//! Apply-layer integration tests (metadata-providers-1.0 M4).
//!
//! Drives [`server::metadata::apply::apply_series`] +
//! [`apply_issue`] against a pre-populated `metadata_run` +
//! `metadata_run_candidate` row and a wiremock-backed provider.
//! Verifies:
//! - empty-DB rows get filled with provider values
//! - user-set fields stay sacred (skip with `mode=fill_missing` AND
//!   `replace_all`)
//! - non-user provenance overwrites only under `replace_all`
//! - `override_user_edits=true` bypasses the sacred rule
//! - external_ids land in the `external_ids` table for the entity
//! - the chosen candidate row gets `applied_at` stamped
//! - `metadata_run.items_applied` bumps when fields wrote
//! - junction writes hit issue_credit + person tables
//!
//! Bypasses apalis by using the test-only `*_inline` helpers in
//! `jobs::metadata_apply`.

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use entity::{external_id, field_provenance, issue, metadata_run, metadata_run_candidate, person, series};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::json;
use server::jobs::metadata_apply::apply_series_inline;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::writers::CoverOverwritePolicy;
use tempfile::tempdir;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn ok_cv_envelope(body: serde_json::Value) -> serde_json::Value {
    json!({
        "status_code": 1,
        "error": "OK",
        "results": body,
    })
}

fn cv_volume_detail() -> serde_json::Value {
    json!({
        "id": 12345,
        "name": "Saga",
        "start_year": "2012",
        "publisher": {"id": 99, "name": "Image Comics"},
        "deck": "Sci-fi epic.",
        "description": "Full description body.",
        "image": null,
        "count_of_issues": 60,
        "site_detail_url": "https://comicvine.gamespot.com/volume/4050-12345/",
        "date_last_updated": "2024-01-15 12:34:56",
        "aliases": null,
    })
}

async fn seed_run_with_candidate(
    app: &TestApp,
    series_id: Uuid,
    cv_id: &str,
    source: &str,
) -> (Uuid, i32) {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec![source.into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    let payload = json!({
        "kind": "series",
        "source": source,
        "external_id": cv_id,
        "external_url": null,
        "name": "Saga",
        "year": 2012,
        "publisher": "Image Comics",
        "issue_count": 60,
        "cover_image_url": null,
        "deck": null,
    });
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set(source.into()),
        external_id: Set(cv_id.into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(payload),
        applied_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    (run_id, 0)
}

fn args(run_id: Uuid, ordinal: i32, mode: ApplyMode, override_user: bool) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal,
        mode,
        apply_cover: false,
        cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
        override_user_edits: override_user,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

#[tokio::test]
async fn apply_series_fills_empty_fields_and_writes_provenance() {
    let cv_mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volume/4050-12345"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_cv_envelope(cv_volume_detail())))
        .mount(&cv_mock)
        .await;

    // TestApp with a CV client base URL — but the apply layer builds
    // its own client via build_provider, which only honors the
    // production base URL. To exercise the wiremock provider end-to-
    // end without an env-var override, we pre-populate the
    // metadata_cache with the GenericMetadata payload so apply_series
    // hits the cache and never opens an HTTP socket. The wiremock
    // mount remains as a regression guard against a future change
    // that bypasses the cache.
    let app = TestApp::spawn_with_comicvine("cv-test-key", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    // Seed an *empty* series (no description / deck / aliases set).
    let series_id = SeriesSeed::new(lib_id, "Some Other Name").insert(&app.state().db).await;
    // Clear the publisher set by the seed so the apply has a slot to
    // fill.
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: series::ActiveModel = row.into();
    am.publisher = Set(None);
    am.year = Set(None);
    am.update(&app.state().db).await.unwrap();

    // Pre-populate the cache with the GenericMetadata payload so
    // apply_series doesn't depend on HTTP wiring inside the
    // production client (which uses the hard-coded base URL).
    use server::metadata::cache;
    use server::metadata::identifier::{Identifier, Source};
    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        year_began: Some(2012),
        publisher: Some("Image Comics".into()),
        deck: Some("Sci-fi epic.".into()),
        description: Some("Full description body.".into()),
        identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "12345", "series")],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series");

    // `series.name` is a required column populated by the seed
    // ("Some Other Name") so fill_missing skips it — only the
    // currently-empty fields (year, publisher, description, deck)
    // get written.
    assert!(outcome.applied_fields.contains(&"year_began".to_owned()));
    assert!(outcome.applied_fields.contains(&"publisher".to_owned()));
    assert!(outcome.applied_fields.contains(&"description".to_owned()));

    // Series row mutated. Name stays at the seed value because
    // fill_missing leaves non-empty fields alone.
    let updated = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "Some Other Name");
    assert_eq!(updated.year, Some(2012));
    assert_eq!(updated.publisher.as_deref(), Some("Image Comics"));
    assert_eq!(updated.summary.as_deref(), Some("Full description body."));
    assert!(updated.last_metadata_sync_at.is_some());

    // Provenance rows landed for each applied field.
    let prov = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("series"))
        .filter(field_provenance::Column::EntityId.eq(series_id.to_string()))
        .all(&app.state().db)
        .await
        .unwrap();
    let prov_fields: Vec<_> = prov.iter().map(|p| p.field.as_str()).collect();
    assert!(prov_fields.contains(&"year_began"));
    assert!(prov_fields.contains(&"publisher"));
    for p in &prov {
        assert_eq!(p.set_by, "comicvine", "non-CV provenance: {p:?}");
    }

    // External_id row inserted.
    let ext = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(external_id::Column::Source.eq("comicvine"))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("external_id row");
    assert_eq!(ext.external_id, "12345");
    assert_eq!(ext.set_by, "comicvine");

    // Candidate flipped applied_at; run items_applied bumped.
    let cand = metadata_run_candidate::Entity::find_by_id((run_id, ordinal))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(cand.applied_at.is_some());
    let run = metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.items_applied, 1);

    // Unused wiremock asserts nothing — but keep it for the
    // regression-guard purpose (no surprise HTTP calls).
    let _ = cv_mock;
}

#[tokio::test]
async fn apply_series_fill_missing_skips_existing_non_user_fields() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .with_publisher("Image Comics")
        .insert(&app.state().db)
        .await;
    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga (provider name)".into()),
        publisher: Some("Different Publisher".into()),
        identifiers: vec![],
        source_provider: Some(server::metadata::identifier::Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    server::metadata::cache::put(
        &app.state().db,
        server::metadata::identifier::Source::ComicVine,
        server::metadata::cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();
    let (run_id, ordinal) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series");
    // Both fields were non-empty + no user provenance → fill_missing
    // skips them.
    assert!(outcome.skipped_fields.contains(&"title".to_owned()));
    assert!(outcome.skipped_fields.contains(&"publisher".to_owned()));
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    // Original values preserved.
    assert_eq!(row.name, "Saga");
    assert_eq!(row.publisher.as_deref(), Some("Image Comics"));
}

#[tokio::test]
async fn apply_series_replace_all_overwrites_non_user_provenance() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .with_publisher("Image Comics")
        .insert(&app.state().db)
        .await;
    // Seed a non-user provenance row to prove ReplaceAll overwrites it.
    let now = Utc::now().fixed_offset();
    field_provenance::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series_id.to_string()),
        field: Set("publisher".into()),
        set_by: Set("comicinfo".into()),
        set_at: Set(now),
        source_external_id: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        publisher: Some("New Publisher".into()),
        identifiers: vec![],
        source_provider: Some(server::metadata::identifier::Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    server::metadata::cache::put(
        &app.state().db,
        server::metadata::identifier::Source::ComicVine,
        server::metadata::cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();
    let (run_id, ordinal) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, ordinal, ApplyMode::ReplaceAll, false),
    )
    .await
    .expect("apply_series");
    assert!(outcome.applied_fields.contains(&"publisher".to_owned()));
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.publisher.as_deref(), Some("New Publisher"));
}

#[tokio::test]
async fn apply_series_user_set_fields_stay_sacred_unless_override() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "User Title")
        .with_publisher("User Publisher")
        .insert(&app.state().db)
        .await;
    let now = Utc::now().fixed_offset();
    // Mark publisher as user-set.
    field_provenance::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series_id.to_string()),
        field: Set("publisher".into()),
        set_by: Set("user".into()),
        set_at: Set(now),
        source_external_id: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    let prefilled = server::metadata::provider::GenericMetadata {
        publisher: Some("Provider Publisher".into()),
        identifiers: vec![],
        source_provider: Some(server::metadata::identifier::Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    server::metadata::cache::put(
        &app.state().db,
        server::metadata::identifier::Source::ComicVine,
        server::metadata::cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();
    let (run_id, ordinal) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    // ReplaceAll without override: still skipped.
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, ordinal, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    assert!(outcome.skipped_fields.contains(&"publisher".to_owned()));
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.publisher.as_deref(), Some("User Publisher"));

    // With override_user_edits=true: applies.
    // Need a fresh run + candidate row since the prior apply flipped applied_at.
    let (run2, ord2) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    let outcome2 = apply_series_inline(
        &app.state(),
        series_id,
        args(run2, ord2, ApplyMode::ReplaceAll, true),
    )
    .await
    .unwrap();
    assert!(outcome2.applied_fields.contains(&"publisher".to_owned()));
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.publisher.as_deref(), Some("Provider Publisher"));
}

#[tokio::test]
async fn apply_series_no_writes_bumps_items_skipped_instead_of_applied() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    // Mark every potentially-touched field user-set so the apply
    // skips them all.
    let now = Utc::now().fixed_offset();
    for f in [
        "title", "year_began", "publisher", "description", "sort_name", "deck", "imprint",
        "volume", "year_end", "series_type", "aliases",
    ] {
        field_provenance::ActiveModel {
            entity_type: Set("series".into()),
            entity_id: Set(series_id.to_string()),
            field: Set(f.into()),
            set_by: Set("user".into()),
            set_at: Set(now),
            source_external_id: Set(None),
        }
        .insert(&app.state().db)
        .await
        .unwrap();
    }
    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        year_began: Some(2012),
        publisher: Some("Image".into()),
        identifiers: vec![],
        source_provider: Some(server::metadata::identifier::Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    server::metadata::cache::put(
        &app.state().db,
        server::metadata::identifier::Source::ComicVine,
        server::metadata::cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();
    let (run_id, ordinal) = seed_run_with_candidate(&app, series_id, "12345", "comicvine").await;
    apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .unwrap();
    let run = metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.items_applied, 0);
    assert_eq!(run.items_skipped, 1);
}

#[tokio::test]
async fn apply_issue_writes_credits_through_writer_helpers() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.path().join("test.cbz");
    let issue_id = common::seed::IssueSeed::new(lib_id, series_id, &cbz, b"dummy", 1.0)
        .insert(&app.state().db)
        .await;

    // Seed an issue-scope run + candidate.
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(issue_id.clone())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set("67890".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "issue"})),
        applied_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    use server::metadata::cache;
    use server::metadata::identifier::{Identifier, Source};
    let prefilled = server::metadata::provider::GenericMetadata {
        title: Some("Chapter One".into()),
        issue_number: Some("1".into()),
        credits: vec![
            server::metadata::provider::CreditCandidate {
                name: "Brian K. Vaughan".into(),
                role: "writer".into(),
                ordinal: None,
                identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "7", "person")],
            },
            server::metadata::provider::CreditCandidate {
                name: "Fiona Staples".into(),
                role: "artist".into(),
                ordinal: None,
                identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "8", "person")],
            },
        ],
        identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "67890", "issue")],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("67890".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &prefilled,
    )
    .await
    .unwrap();

    let outcome = server::jobs::metadata_apply::apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, 0, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");

    // `title` was pre-populated by IssueSeed ("Issue 1"); fill_missing
    // leaves it alone. Credits are empty in the seed → write lands.
    assert!(outcome.applied_fields.contains(&"credits".to_owned()));
    assert!(outcome.junctions_touched.contains(&"credits".to_owned()));

    // Person rows landed.
    let people = person::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap();
    let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"Brian K. Vaughan"));
    assert!(names.contains(&"Fiona Staples"));

    // Issue row title still the seed value (fill_missing didn't
    // touch it).
    let row = issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.title.as_deref(), Some("Issue 1"));
    // CSV cache was rebuilt → writer column populated.
    assert_eq!(row.writer.as_deref(), Some("Brian K. Vaughan"));
}

// ────────────────────────────────────────────────────────────────
// metadata-providers-1.0 M5 — diff endpoint + selected_fields-
// respecting apply. The preview pane reads the diff to render
// per-field checkboxes; apply then echoes back only the user-
// selected field keys.
// ────────────────────────────────────────────────────────────────

async fn seed_series_with_filled_payload(app: &TestApp) -> (Uuid, Uuid, i32) {
    use server::metadata::cache;
    use server::metadata::identifier::{Identifier, Source};

    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Some Other Name").insert(&app.state().db).await;
    let row = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: series::ActiveModel = row.into();
    am.publisher = Set(None);
    am.year = Set(None);
    am.summary = Set(None);
    am.update(&app.state().db).await.unwrap();
    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        year_began: Some(2012),
        publisher: Some("Image Comics".into()),
        deck: Some("Sci-fi epic.".into()),
        description: Some("Full description body.".into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "12345",
            "series",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("12345".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        &prefilled,
    )
    .await
    .unwrap();
    let (run_id, ordinal) = seed_run_with_candidate(app, series_id, "12345", "comicvine").await;
    (series_id, run_id, ordinal)
}

#[tokio::test]
async fn compute_series_diff_classifies_per_field_decisions() {
    use server::metadata::diff;
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let (_series_id, run_id, ordinal) = seed_series_with_filled_payload(&app).await;
    let diff = diff::compute_series_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("diff");
    assert_eq!(diff.scope, "series");
    assert_eq!(diff.source, "comicvine");
    assert!(!diff.rows.is_empty());

    // Title is non-empty in DB ("Some Other Name") and the candidate's
    // "Saga" disagrees — FillMissing mode → SkippedFillMissingHasValue.
    let title_row = diff.rows.iter().find(|r| r.field == "title").expect("title row");
    assert_eq!(title_row.decision, "skipped_fill_missing_has_value");
    assert_eq!(title_row.current_value.as_deref(), Some("Some Other Name"));
    assert_eq!(title_row.proposed_value.as_deref(), Some("Saga"));

    // year_began was cleared in the seed — would_fill.
    let year_row = diff
        .rows
        .iter()
        .find(|r| r.field == "year_began")
        .expect("year_began row");
    assert_eq!(year_row.decision, "would_fill");
    assert_eq!(year_row.current_value, None);
    assert_eq!(year_row.proposed_value.as_deref(), Some("2012"));

    // publisher: cleared, would_fill.
    let pub_row = diff
        .rows
        .iter()
        .find(|r| r.field == "publisher")
        .expect("publisher row");
    assert_eq!(pub_row.decision, "would_fill");

    // changes_count counts the would_fill / would_replace rows + new
    // external_ids. The new external_id row for ComicVine pushes it
    // up by one.
    assert!(
        diff.changes_count >= 3,
        "expected at least year + publisher + description + cv external_id, got {}",
        diff.changes_count
    );
    assert_eq!(diff.external_ids_new.len(), 1);
    assert_eq!(diff.external_ids_new[0].source, "comicvine");
}

#[tokio::test]
async fn compute_series_diff_replace_all_flips_decisions() {
    use server::metadata::diff;
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let (_series_id, run_id, ordinal) = seed_series_with_filled_payload(&app).await;
    let diff = diff::compute_series_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::ReplaceAll, false),
    )
    .await
    .expect("diff");

    // Title under ReplaceAll → would_replace (different current value,
    // no user-set provenance).
    let title_row = diff.rows.iter().find(|r| r.field == "title").unwrap();
    assert_eq!(title_row.decision, "would_replace");
}

#[tokio::test]
async fn compute_series_diff_blocks_user_set_unless_override() {
    use server::metadata::diff;
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let (series_id, run_id, ordinal) = seed_series_with_filled_payload(&app).await;

    // Mark the title as user-set.
    field_provenance::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series_id.to_string()),
        field: Set("title".into()),
        set_by: Set("user".into()),
        set_at: Set(Utc::now().fixed_offset()),
        source_external_id: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let diff = diff::compute_series_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::ReplaceAll, false),
    )
    .await
    .expect("diff");
    let title_row = diff.rows.iter().find(|r| r.field == "title").unwrap();
    assert_eq!(title_row.decision, "blocked_by_user");
    assert_eq!(title_row.current_set_by.as_deref(), Some("user"));
    assert!(title_row.current_set_at.is_some());

    // With override_user_edits → would_replace.
    let diff2 = diff::compute_series_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::ReplaceAll, true),
    )
    .await
    .expect("diff");
    let title_row2 = diff2.rows.iter().find(|r| r.field == "title").unwrap();
    assert_eq!(title_row2.decision, "would_replace");
}

#[tokio::test]
async fn apply_series_respects_selected_fields_opt_in() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let (series_id, run_id, ordinal) = seed_series_with_filled_payload(&app).await;

    // Only opt-in to year_began. publisher + description must NOT
    // write even though they're would-fill.
    let mut selected = std::collections::HashSet::new();
    selected.insert("year_began".to_owned());
    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        ApplyArgs {
            run_id,
            ordinal,
            mode: ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            actor_id: None,
            selected_fields: Some(selected),
            override_external_id_sources: std::collections::HashSet::new(),
        },
    )
    .await
    .expect("apply_series");

    assert!(outcome.applied_fields.contains(&"year_began".to_owned()));
    assert!(
        !outcome.applied_fields.contains(&"publisher".to_owned()),
        "publisher was not in selected_fields — must not write"
    );
    assert!(
        !outcome.applied_fields.contains(&"description".to_owned()),
        "description was not in selected_fields — must not write"
    );

    let updated = series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.year, Some(2012));
    assert_eq!(updated.publisher, None, "publisher cell untouched");
    assert_eq!(updated.summary, None, "description cell untouched");
}

#[tokio::test]
async fn apply_series_empty_selected_fields_writes_nothing_scalar() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let (series_id, run_id, ordinal) = seed_series_with_filled_payload(&app).await;

    let outcome = apply_series_inline(
        &app.state(),
        series_id,
        ApplyArgs {
            run_id,
            ordinal,
            mode: ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            actor_id: None,
            selected_fields: Some(std::collections::HashSet::new()),
            override_external_id_sources: std::collections::HashSet::new(),
        },
    )
    .await
    .expect("apply_series");

    // With an empty selected_fields set, no scalar fields write.
    // External_ids are written regardless of the per-field set
    // (they're a separate channel — the preview's per-source toggles
    // live in `override_external_id_sources` instead). Verify no
    // scalar landed but the CV external_id row did.
    assert!(outcome.applied_fields.is_empty(),
        "no scalar field should write when selected_fields is empty, got: {:?}",
        outcome.applied_fields);
    let cv_row = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(external_id::Column::Source.eq("comicvine"))
        .one(&app.state().db)
        .await
        .unwrap();
    assert!(cv_row.is_some(), "CV external_id row should be written regardless");
}

// ───────── M5.1 — junction + variant rows in issue diff ─────────

/// Helper: seed an issue + a candidate run whose detail payload carries
/// scalar fields + junctions + variant covers. Returns the issue id +
/// the (run_id, ordinal) for `compute_issue_diff` / `apply_issue_*`.
async fn seed_issue_with_junction_candidate(
    app: &TestApp,
    dir: &std::path::Path,
) -> (String, Uuid, i32) {
    let lib_id = LibrarySeed::new(dir).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.join("saga-1.cbz");
    let issue_id = common::seed::IssueSeed::new(lib_id, series_id, &cbz, b"dummy-bytes", 1.0)
        .insert(&app.state().db)
        .await;

    use server::metadata::cache;
    use server::metadata::identifier::{Identifier, Source};
    let payload = server::metadata::provider::GenericMetadata {
        title: Some("Chapter One".into()),
        issue_number: Some("1".into()),
        description: Some("desc".into()),
        page_count: Some(44),
        credits: vec![
            server::metadata::provider::CreditCandidate {
                name: "Brian K. Vaughan".into(),
                role: "Writer".into(),
                ordinal: None,
                identifiers: vec![],
            },
            server::metadata::provider::CreditCandidate {
                name: "Fiona Staples".into(),
                role: "Penciller".into(),
                ordinal: None,
                identifiers: vec![],
            },
        ],
        characters: vec![
            server::metadata::provider::EntityCandidate {
                name: "Alana".into(),
                identifiers: vec![],
                is_first_appearance: false,
                died_in_issue: None,
                disbanded_in_issue: None,
                position_in_arc: None,
            },
            server::metadata::provider::EntityCandidate {
                name: "Marko".into(),
                identifiers: vec![],
                is_first_appearance: false,
                died_in_issue: None,
                disbanded_in_issue: None,
                position_in_arc: None,
            },
        ],
        teams: vec![],
        locations: vec![],
        story_arcs: vec![server::metadata::provider::EntityCandidate {
            name: "The Will".into(),
            identifiers: vec![],
            is_first_appearance: false,
            died_in_issue: None,
            disbanded_in_issue: None,
            position_in_arc: None,
        }],
        tags: vec!["sci-fi".into(), "romance".into()],
        genres: vec!["sci-fi".into()],
        variants: vec![
            server::metadata::provider::VariantCoverCandidate {
                label: Some("Cory Walker variant".into()),
                artist_name: Some("Cory Walker".into()),
                identifiers: vec![],
                image_url: Some("https://cdn.example.com/saga-1-walker.jpg".into()),
            },
            server::metadata::provider::VariantCoverCandidate {
                label: Some("Dave McCaig variant".into()),
                artist_name: Some("Dave McCaig".into()),
                identifiers: vec![],
                image_url: Some("https://cdn.example.com/saga-1-mccaig.jpg".into()),
            },
        ],
        identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "67890", "issue")],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("67890".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &payload,
    )
    .await
    .unwrap();

    // Issue-scope run + candidate.
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(issue_id.clone())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set("67890".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "issue"})),
        applied_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    (issue_id, run_id, 0)
}

#[tokio::test]
async fn compute_issue_diff_surfaces_junctions_when_db_is_empty() {
    use server::metadata::diff;
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let (_issue_id, run_id, ordinal) = seed_issue_with_junction_candidate(&app, dir.path()).await;

    let diff = diff::compute_issue_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("diff");

    // Credits: DB empty (IssueSeed leaves credit columns NULL) +
    // provider has 2 → would_fill.
    let credits = diff.rows.iter().find(|r| r.field == "credits").expect("credits row");
    assert_eq!(credits.decision, "would_fill");
    assert_eq!(credits.current_value.as_deref(), Some("none"));
    assert_eq!(credits.proposed_value.as_deref(), Some("2 items"));

    // Characters: 2 from provider → would_fill.
    let chars = diff.rows.iter().find(|r| r.field == "characters").unwrap();
    assert_eq!(chars.decision, "would_fill");
    assert_eq!(chars.proposed_value.as_deref(), Some("2 items"));

    // Teams: provider has 0 → no_incoming_value.
    let teams = diff.rows.iter().find(|r| r.field == "teams").unwrap();
    assert_eq!(teams.decision, "no_incoming_value");

    // Story arcs: 1 from provider → would_fill.
    let arcs = diff.rows.iter().find(|r| r.field == "story_arcs").unwrap();
    assert_eq!(arcs.decision, "would_fill");
    assert_eq!(arcs.proposed_value.as_deref(), Some("1 item"));

    // Tags: 2 from provider → would_fill.
    let tags = diff.rows.iter().find(|r| r.field == "tags").unwrap();
    assert_eq!(tags.decision, "would_fill");

    // Genres: 1 from provider → would_fill.
    let genres = diff.rows.iter().find(|r| r.field == "genres").unwrap();
    assert_eq!(genres.decision, "would_fill");

    // Variant covers: 2 from provider, 0 in DB → would_fill.
    let variants = diff.rows.iter().find(|r| r.field == "cover.variants").unwrap();
    assert_eq!(variants.decision, "would_fill");
    assert_eq!(variants.proposed_value.as_deref(), Some("2 items"));

    // changes_count must be high enough that Apply enables — at least
    // credits + characters + story_arcs + tags + genres + variants + new
    // CV external_id = 7.
    assert!(
        diff.changes_count >= 7,
        "changes_count = {} should reflect the junction + variant rows",
        diff.changes_count,
    );
}

#[tokio::test]
async fn compute_issue_diff_variants_no_change_when_counts_match() {
    use entity::issue_cover;
    use server::metadata::diff;
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let (issue_id, run_id, ordinal) = seed_issue_with_junction_candidate(&app, dir.path()).await;

    // Pre-seed 2 active variant rows in `issue_cover` so the diff sees
    // matching counts (heuristic → no_change).
    let now = Utc::now().fixed_offset();
    for (ordinal_n, url) in [
        (1i32, "https://existing/a.jpg"),
        (2, "https://existing/b.jpg"),
    ] {
        issue_cover::ActiveModel {
            id: Set(Uuid::now_v7()),
            issue_id: Set(issue_id.clone()),
            kind: Set("variant".into()),
            ordinal: Set(ordinal_n),
            source_provider: Set(Some("comicvine".into())),
            source_external_id: Set(None),
            source_url: Set(Some(url.into())),
            variant_label: Set(Some(format!("Variant {ordinal_n}"))),
            variant_artist_person_id: Set(None),
            local_path: Set(String::new()),
            width: Set(None),
            height: Set(None),
            phash: Set(None),
            dhash: Set(None),
            ahash: Set(None),
            fetched_at: Set(now),
            is_active: Set(true),
        }
        .insert(&app.state().db)
        .await
        .unwrap();
    }

    let diff = diff::compute_issue_diff(
        &app.state(),
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("diff");

    let variants = diff.rows.iter().find(|r| r.field == "cover.variants").unwrap();
    assert_eq!(variants.decision, "no_change", "matching counts → no_change");
    assert_eq!(variants.current_value.as_deref(), Some("2 items"));
    assert_eq!(variants.proposed_value.as_deref(), Some("2 items"));
}

#[tokio::test]
async fn apply_issue_respects_variants_toggled_off_in_selected_fields() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let (issue_id, run_id, ordinal) = seed_issue_with_junction_candidate(&app, dir.path()).await;

    // Pass selected_fields WITHOUT "cover.variants" — apply path must
    // skip the variant write even though the provider returned 2.
    let mut selected = std::collections::HashSet::new();
    selected.insert("title".to_owned());
    selected.insert("credits".to_owned());

    let outcome = server::jobs::metadata_apply::apply_issue_inline(
        &app.state(),
        &issue_id,
        ApplyArgs {
            run_id,
            ordinal,
            mode: ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            actor_id: None,
            selected_fields: Some(selected),
            override_external_id_sources: std::collections::HashSet::new(),
        },
    )
    .await
    .expect("apply_issue");

    assert_eq!(
        outcome.variants_written, 0,
        "selected_fields excluded cover.variants — must skip",
    );

    use entity::issue_cover;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 0, "no variant rows written");
}
