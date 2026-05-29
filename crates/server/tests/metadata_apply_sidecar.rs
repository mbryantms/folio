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
use std::io::{Cursor, Write};
use tempfile::tempdir;
use uuid::Uuid;

/// Build a minimal valid CBZ in memory — one stored (uncompressed)
/// page entry whose bytes are the `label` string. Series-scope tests
/// feed these bytes to `IssueSeed::insert`, which writes them to the
/// `.cbz` path; the inline `rewrite_one_issue` helper can then open
/// the file as a real zip.
fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
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
    }
    .insert(db)
    .await
    .unwrap();
    (run_id, 0)
}

fn stub_provider_payload_with_variants() -> server::metadata::provider::GenericMetadata {
    let mut payload = stub_provider_payload();
    payload.variants = vec![
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
        // A variant with no image URL — composer should skip it.
        server::metadata::provider::VariantCoverCandidate {
            label: Some("Ghost variant".into()),
            artist_name: None,
            identifiers: vec![],
            image_url: None,
        },
    ];
    payload
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
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "67890",
            "issue",
        )],
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
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
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
    assert!(
        outcome.enqueued_rewrite,
        "writeback path must enqueue rewrite"
    );
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
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
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

    // Subscribe before the apply so we catch the completion broadcast.
    let mut events = app.state().events.subscribe();

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

    // The DB-direct path must broadcast `metadata.applied` for this issue so
    // an open match dialog re-hydrates without a page refresh (the writeback
    // path uses the rescan's `scan.completed` instead).
    use server::library::events::ScanEvent;
    let mut saw_applied = false;
    while let Ok(evt) = events.try_recv() {
        if let ScanEvent::MetadataApplied {
            library_id,
            issue_id: evt_issue,
            ..
        } = evt
        {
            assert_eq!(library_id, lib_id);
            assert_eq!(evt_issue.as_deref(), Some(issue_id.as_str()));
            saw_applied = true;
        }
    }
    assert!(
        saw_applied,
        "DB-direct apply must broadcast a MetadataApplied event",
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
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
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
async fn apply_issue_with_writeback_writes_variant_covers_to_issue_cover_table() {
    // Variant covers travel outside the XML — they live in DB as
    // `issue_cover` rows with `kind='variant'`. The `<CoverGallery>`
    // surface needs them to actually appear in the UI; without this
    // wiring the gallery auto-hides because it only sees the primary
    // row.
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz_payload = build_cbz_bytes("saga-1");
    let issue_id = IssueSeed::new(
        lib_id,
        series_id,
        &dir.path().join("saga-1.cbz"),
        &cbz_payload,
        1.0,
    )
    .insert(&app.state().db)
    .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload_with_variants(),
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

    // Provider returned 3 variants but one had no image_url — only
    // 2 land in the table, and outcome.variants_written reflects the
    // actual insert count (not the input length).
    assert_eq!(
        outcome.variants_written, 2,
        "no-URL variant must be skipped"
    );
    use entity::issue_cover;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .filter(issue_cover::Column::IsActive.eq(true))
        .order_by_asc(issue_cover::Column::Ordinal)
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "the no-URL variant must be skipped");
    assert_eq!(
        rows[0].variant_label.as_deref(),
        Some("Cory Walker variant")
    );
    assert_eq!(
        rows[0].source_url.as_deref(),
        Some("https://cdn.example.com/saga-1-walker.jpg"),
    );
    assert_eq!(
        rows[0].ordinal, 1,
        "primary owns ordinal 0; variants start at 1"
    );
    // The fixture's `cdn.example.com` URLs are unreachable, so the
    // downloader soft-falls-back to a metadata-only row that keeps the
    // hotlink. (The success path — bytes downloaded to `local_path` — is
    // covered by `apply_issue_downloads_and_stores_variant_covers_locally`.)
    assert!(
        rows[0].local_path.is_empty(),
        "unreachable fixture URL → metadata-only fallback",
    );
    assert_eq!(
        rows[1].variant_label.as_deref(),
        Some("Dave McCaig variant")
    );
    assert_eq!(rows[1].ordinal, 2);
    assert_eq!(rows[0].source_provider.as_deref(), Some("comicvine"));
}

#[tokio::test]
async fn apply_issue_variant_covers_idempotent_no_dupes() {
    // Re-applying must NOT accumulate stale variant rows. The writer
    // deactivates the previous variant set before inserting the fresh
    // one.
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz_payload = build_cbz_bytes("saga-1");
    let issue_id = IssueSeed::new(
        lib_id,
        series_id,
        &dir.path().join("saga-1.cbz"),
        &cbz_payload,
        1.0,
    )
    .insert(&app.state().db)
    .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &stub_provider_payload_with_variants(),
    )
    .await
    .unwrap();

    let (run_id, ordinal) = seed_issue_run(&app, &issue_id, "comicvine").await;
    let _ = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("first apply");

    // Re-seed a fresh run and apply again with the same candidate.
    let (run_id2, ordinal2) = seed_issue_run(&app, &issue_id, "comicvine").await;
    let _ = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id2, ordinal2, ApplyMode::FillMissing, false),
    )
    .await
    .expect("second apply");

    use entity::issue_cover;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let active_rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .filter(issue_cover::Column::IsActive.eq(true))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(
        active_rows.len(),
        2,
        "second apply must not accumulate variant rows",
    );
    let inactive_rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .filter(issue_cover::Column::IsActive.eq(false))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(
        inactive_rows.len(),
        0,
        "variants are presentational; re-apply deletes prior rows (no audit trail needed)",
    );
}

#[tokio::test]
async fn apply_series_with_writeback_enabled_composes_per_issue_and_triggers_one_rescan() {
    // M4: series-scope apply walks every active issue, composes XMLs,
    // and reports `composed_sidecars=N`. We seed 3 issues and assert
    // they're all counted.
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let payloads: Vec<Vec<u8>> = (1..=3)
        .map(|n| build_cbz_bytes(&format!("saga-page-{n}")))
        .collect();
    for (n, payload) in (1..=3).zip(payloads.iter()) {
        let cbz = dir.path().join(format!("saga-{n:03}.cbz"));
        IssueSeed::new(lib_id, series_id, &cbz, payload, n as f64)
            .insert(&app.state().db)
            .await;
    }

    // Cache series-level provider detail.
    use server::metadata::cache;
    use server::metadata::identifier::Source;
    let series_payload = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        publisher: Some("Image Comics".into()),
        volume: Some(1),
        year_began: Some(2012),
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("4050-12345".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        &series_payload,
    )
    .await
    .unwrap();

    // Series-scope run + candidate.
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
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
        external_id: Set("12345".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "series"})),
        applied_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, 0, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series");

    // M4 path signals: composed_sidecars matches the three eligible
    // issues; enqueued_rewrite=true; legacy applied_fields empty.
    assert!(outcome.enqueued_rewrite);
    assert_eq!(outcome.composed_sidecars, 3, "all three issues composed");
    assert!(
        outcome.sidecar_skip_reasons.is_empty(),
        "no skip reasons expected: {:?}",
        outcome.sidecar_skip_reasons,
    );
    assert!(outcome.applied_fields.is_empty());

    // Run counts bumped on the apply (as if a single candidate was
    // applied — items_applied=1, not 3, since the run is series-scope).
    let run = metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run present");
    assert_eq!(run.items_applied, 1);
}

#[tokio::test]
async fn apply_series_with_writeback_skips_removed_issues() {
    // Only `state IN ('ok','recovered')` rows are eligible. Removed +
    // malformed issues are skipped entirely (no entry in skip_reasons
    // either — they aren't *attempted*).
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;

    let ok_payload = build_cbz_bytes("saga-ok");
    let removed_payload = build_cbz_bytes("saga-removed");
    let cbz_ok = dir.path().join("saga-001.cbz");
    IssueSeed::new(lib_id, series_id, &cbz_ok, &ok_payload, 1.0)
        .insert(&app.state().db)
        .await;
    let cbz_removed = dir.path().join("saga-002.cbz");
    IssueSeed::new(lib_id, series_id, &cbz_removed, &removed_payload, 2.0)
        .with_state("removed")
        .insert(&app.state().db)
        .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    let series_payload = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga".into()),
        source_provider: Some(Source::ComicVine),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        &series_payload,
    )
    .await
    .unwrap();

    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
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
        external_id: Set("12345".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "series"})),
        applied_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, 0, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series");

    assert_eq!(
        outcome.composed_sidecars, 1,
        "only the 'ok' issue is composed"
    );
    assert!(outcome.sidecar_skip_reasons.is_empty());
}

#[tokio::test]
async fn apply_series_writeback_disabled_takes_legacy_path() {
    // Library defaults: both writeback toggles OFF → legacy series
    // apply path runs (touches series row, applied_fields non-empty).
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    let series_payload = server::metadata::provider::GenericMetadata {
        series_name: Some("Saga (filled from provider)".into()),
        publisher: Some("Image Comics".into()),
        year_began: Some(2012),
        source_provider: Some(Source::ComicVine),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "12345",
        &series_payload,
    )
    .await
    .unwrap();

    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
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
        external_id: Set("12345".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "series"})),
        applied_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, 0, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series");

    assert!(
        !outcome.enqueued_rewrite,
        "writeback OFF must NOT enqueue rewrites",
    );
    assert_eq!(outcome.composed_sidecars, 0);
    // Legacy path filled the title via writers::*.
    assert!(
        !outcome.applied_fields.is_empty(),
        "legacy path must populate applied_fields: {:?}",
        outcome.applied_fields,
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
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
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

/// Encode a small valid PNG in memory so the mock CDN serves a
/// decodable image (the production `image` dep isn't visible to this
/// integration-test crate, hence the `image` dev-dependency).
fn tiny_png() -> Vec<u8> {
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    let img = RgbImage::from_fn(8, 12, |x, y| Rgb([(x * 20) as u8, (y * 10) as u8, 40]));
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

/// Register the first user (→ admin) and return a `Cookie` header value
/// carrying the session. Sufficient for GET requests (no CSRF needed).
async fn register_admin_cookie(app: &TestApp) -> String {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use tower::ServiceExt;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"cover-admin@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|c| c.split(';').next().unwrap_or("").to_owned())
        .collect::<Vec<_>>()
        .join("; ")
}

#[tokio::test]
async fn apply_issue_downloads_and_stores_variant_covers_locally() {
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use entity::issue_cover;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    use server::metadata::cache;
    use server::metadata::identifier::Source;
    use server::metadata::provider::VariantCoverCandidate;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Mock CDN serving real PNG bytes for two variants.
    let cdn = MockServer::start().await;
    let png = tiny_png();
    for name in ["walker.png", "mccaig.png"] {
        Mock::given(method("GET"))
            .and(path(format!("/{name}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(png.clone()),
            )
            .mount(&cdn)
            .await;
    }

    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = build_cbz_bytes("saga-1");
    let issue_id = IssueSeed::new(lib_id, series_id, &dir.path().join("saga-1.cbz"), &cbz, 1.0)
        .insert(&app.state().db)
        .await;

    let mut payload = stub_provider_payload();
    payload.variants = vec![
        VariantCoverCandidate {
            label: Some("Walker".into()),
            artist_name: None,
            identifiers: vec![],
            image_url: Some(format!("{}/walker.png", cdn.uri())),
        },
        VariantCoverCandidate {
            label: Some("McCaig".into()),
            artist_name: None,
            identifiers: vec![],
            image_url: Some(format!("{}/mccaig.png", cdn.uri())),
        },
    ];
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Issue,
        "67890",
        &payload,
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
    assert_eq!(outcome.variants_written, 2);

    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .order_by_asc(issue_cover::Column::Ordinal)
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let data_path = app.state().cfg().data_path.clone();
    for row in &rows {
        assert!(
            !row.local_path.is_empty(),
            "variant downloaded to local storage"
        );
        assert!(
            row.width.is_some() && row.height.is_some(),
            "source dimensions recorded"
        );
        assert!(row.phash.is_some(), "perceptual hash computed");
        assert!(
            data_path.join(&row.local_path).exists(),
            "cover file written to disk: {}",
            row.local_path,
        );
    }

    // The byte endpoint serves the stored cover to an authorized user.
    let cookie = register_admin_cookie(&app).await;
    let cover_id = rows[0].id;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{issue_id}/covers/{cover_id}"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let served = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        served.as_ref(),
        png.as_slice(),
        "served bytes match the downloaded image"
    );

    // Unknown cover id → 404.
    let missing = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{issue_id}/covers/{}", Uuid::now_v7()))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn variant_cover_backfill_downloads_hotlink_rows() {
    use entity::issue_cover;
    use sea_orm::{EntityTrait, Set};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let cdn = MockServer::start().await;
    let png = tiny_png();
    Mock::given(method("GET"))
        .and(path("/v.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(png.clone()),
        )
        .mount(&cdn)
        .await;

    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = build_cbz_bytes("saga-1");
    let issue_id = IssueSeed::new(lib_id, series_id, &dir.path().join("saga-1.cbz"), &cbz, 1.0)
        .insert(&app.state().db)
        .await;

    // Seed a legacy hotlink-only variant row (no local_path).
    let cover_id = Uuid::now_v7();
    issue_cover::ActiveModel {
        id: Set(cover_id),
        issue_id: Set(issue_id.clone()),
        kind: Set("variant".into()),
        ordinal: Set(1),
        source_provider: Set(Some("comicvine".into())),
        source_external_id: Set(None),
        source_url: Set(Some(format!("{}/v.png", cdn.uri()))),
        variant_label: Set(Some("Hotlinked".into())),
        variant_artist_person_id: Set(None),
        local_path: Set(String::new()),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(Utc::now().fixed_offset()),
        is_active: Set(true),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let data_path = app.state().cfg().data_path.clone();
    let outcome =
        server::metadata::writers::run_variant_cover_backfill(&app.state().db, &data_path)
            .await
            .unwrap();
    assert_eq!(outcome.considered, 1);
    assert_eq!(outcome.stored, 1);

    let row = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.local_path.is_empty(), "backfill populated local_path");
    assert!(row.phash.is_some(), "backfill computed perceptual hash");
    assert!(
        data_path.join(&row.local_path).exists(),
        "backfilled file on disk"
    );
}
