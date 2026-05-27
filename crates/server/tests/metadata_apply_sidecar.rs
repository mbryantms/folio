//! M3 of `metadata-sidecar-writeback-1.0`: integration tests for the
//! XML-first apply path.
//!
//! Covers the flag-gated dispatch in
//! [`server::metadata::apply::apply_issue`]:
//!
//!   - Library with `metadata_writeback_enabled=true` AND
//!     `allow_archive_writeback=true` → composer runs, sidecar job is
//!     pushed, `ApplyOutcome.enqueued_rewrite=true`.
//!   - Library with the master toggle OFF (or the metadata toggle OFF)
//!     → legacy DB-direct path runs (covered by the existing
//!     `metadata_apply.rs` suite; verified here by asserting the
//!     `applied_fields` shape).
//!   - User-pinned fields surface in `ApplyOutcome.suppressed_user_pins`.
//!   - The chosen candidate row gets `applied_at` stamped + the run's
//!     `items_applied` bumps even though the actual entity rows
//!     haven't been touched yet (the scoped rescan does that).

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{field_provenance, metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::json;
use server::jobs::metadata_apply::apply_issue_inline;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::writers::CoverOverwritePolicy;
use tempfile::tempdir;
use uuid::Uuid;

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

async fn seed_issue_run(app: &TestApp, issue_id: &str, source: &str) -> (Uuid, i32) {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(issue_id.into())),
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
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set(source.into()),
        external_id: Set("67890".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "issue"})),
        applied_at: Set(None),
        dismissed_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    (run_id, 0)
}

fn stub_provider_payload() -> server::metadata::provider::GenericMetadata {
    use server::metadata::identifier::{Identifier, Source};
    server::metadata::provider::GenericMetadata {
        title: Some("Chapter One".into()),
        issue_number: Some("1".into()),
        description: Some("Provider summary.".into()),
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
        characters: vec![server::metadata::provider::EntityCandidate {
            name: "Alana".into(),
            identifiers: vec![],
            is_first_appearance: false,
            died_in_issue: None,
            disbanded_in_issue: None,
            position_in_arc: None,
        }],
        identifiers: vec![Identifier::with_canonical_url(Source::ComicVine, "67890", "issue")],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("67890".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn apply_issue_with_writeback_enabled_enqueues_rewrite() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy-bytes", 1.0)
        .insert(&app.state().db)
        .await;

    // Pre-cache provider detail so the apply path doesn't reach out.
    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload(),
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_issue_run(&app, &issue_id, "comicvine").await;

    let outcome = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");

    // M3 path signals: rewrite enqueued, legacy field arrays empty.
    assert!(outcome.enqueued_rewrite, "writeback path must enqueue rewrite");
    assert!(
        outcome.applied_fields.is_empty(),
        "writeback path doesn't touch entity rows directly; applied_fields stays empty: {:?}",
        outcome.applied_fields,
    );
    assert!(outcome.suppressed_user_pins.is_empty());

    // Candidate flipped + run counts updated even though DB rows
    // weren't touched (the scoped rescan will catch up).
    let cand = metadata_run_candidate::Entity::find_by_id((run_id, ordinal))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("candidate present");
    assert!(cand.applied_at.is_some(), "applied_at must be stamped");

    let run = metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run present");
    assert_eq!(run.items_applied, 1, "items_applied bumps on enqueue");
    assert_eq!(run.items_skipped, 0);
}

#[tokio::test]
async fn apply_issue_writeback_disabled_takes_legacy_path() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    // Default LibrarySeed has both flags off.
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy-bytes", 1.0)
        .insert(&app.state().db)
        .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload(),
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_issue_run(&app, &issue_id, "comicvine").await;

    let outcome = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");

    assert!(
        !outcome.enqueued_rewrite,
        "writeback OFF must NOT enqueue a sidecar rewrite",
    );
    // Legacy path writes credits via writers::*; applied_fields must
    // include "credits" since the IssueSeed left them empty.
    assert!(
        outcome.applied_fields.contains(&"credits".to_owned()),
        "legacy path wrote credits: {:?}",
        outcome.applied_fields,
    );
}

#[tokio::test]
async fn apply_issue_writeback_surfaces_suppressed_user_pins() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy-bytes", 1.0)
        .with_title("My Hand-Edited Title")
        .insert(&app.state().db)
        .await;

    // Plant a user pin on `title` for the issue.
    let now = Utc::now().fixed_offset();
    field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.clone()),
        field: Set("title".into()),
        set_by: Set("user".into()),
        source_external_id: Set(None),
        set_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload(),
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_issue_run(&app, &issue_id, "comicvine").await;

    let outcome = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");

    assert!(outcome.enqueued_rewrite);
    assert!(
        outcome.suppressed_user_pins.contains(&"title".to_owned()),
        "title pin must surface: {:?}",
        outcome.suppressed_user_pins,
    );
}

#[tokio::test]
async fn apply_issue_override_user_edits_collapses_pins() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(&app.state().db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"dummy-bytes", 1.0)
        .with_title("Pinned Title")
        .insert(&app.state().db)
        .await;

    let now = Utc::now().fixed_offset();
    field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.clone()),
        field: Set("title".into()),
        set_by: Set("user".into()),
        source_external_id: Set(None),
        set_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload(),
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_issue_run(&app, &issue_id, "comicvine").await;

    // override_user_edits=true → composer behaves as provider-wins.
    let outcome = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, true),
    )
    .await
    .expect("apply_issue");

    assert!(outcome.enqueued_rewrite);
    assert!(
        outcome.suppressed_user_pins.is_empty(),
        "override_user_edits must zero the suppressed-pins set: {:?}",
        outcome.suppressed_user_pins,
    );
}
