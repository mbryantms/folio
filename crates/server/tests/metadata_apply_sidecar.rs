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
use entity::{field_provenance, issue, metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::json;
use server::jobs::metadata_apply::apply_issue_inline;
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
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
        // PNG-signed so the archive crate's open-time content sniff keeps
        // the entry as a page; the label keeps each fixture distinct.
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

/// Every `RewriteIssueSidecarsJob` currently sitting in the apalis queue,
/// decoded from the storage's `{namespace}:data` hash (the stored shape is
/// apalis's `Request { args, parts }` JSON — same thing the dead-jobs admin
/// endpoint reads). Tests use this to run the job the apply enqueued.
async fn queued_rewrite_jobs(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let mut jobs: Vec<RewriteIssueSidecarsJob> = all
        .values()
        .map(|blob| {
            let v: serde_json::Value = serde_json::from_str(blob).expect("request json");
            serde_json::from_value(v["args"].clone()).expect("job args")
        })
        .collect();
    jobs.sort_by(|a, b| a.issue_id.cmp(&b.issue_id));
    jobs
}

/// Run every queued sidecar job through the worker entry point (mutex,
/// rewrite, audit, rescan enqueue, deferred metadata writes) and clear the
/// queue so a second call only sees newer jobs.
async fn run_queued_rewrite_jobs(app: &TestApp) -> usize {
    let jobs = queued_rewrite_jobs(app).await;
    let n = jobs.len();
    for job in jobs {
        server::jobs::rewrite_sidecars::handle(job, apalis::prelude::Data::new(app.state()))
            .await
            .expect("rewrite job handle");
    }
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    n
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
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
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
        outcome.sidecar_skip_reasons.is_empty(),
        "CBZ takes the sidecar path without a refusal: {:?}",
        outcome.sidecar_skip_reasons
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

    // WP-2.6 (f): `last_metadata_sync_at` is deferred until the XML is in
    // the archive — NULL right after the apply, stamped once the rewrite
    // job has run (it previously stayed NULL forever on this path, so the
    // Metadata tab's "Last metadata sync" showed "Never" after a pull).
    let before = issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("issue present");
    assert!(
        before.last_metadata_sync_at.is_none(),
        "sync stamp must wait for the rewrite to land",
    );
    assert_eq!(
        run_queued_rewrite_jobs(&app).await,
        1,
        "one rewrite job queued"
    );
    let after = issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("issue present");
    assert!(
        after.last_metadata_sync_at.is_some(),
        "writeback apply must stamp last_metadata_sync_at once the rewrite lands",
    );
    assert!(
        after.last_sidecar_rewrite_at.is_some(),
        "sidecar rewrite stamps last_sidecar_rewrite_at",
    );
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
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
    // Variant rows land only once the rewrite job has put the XML in the
    // archive (WP-2.6 (f)).
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

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
    // hotlink. Internal URL rejection is covered by
    // `apply_issue_keeps_ssrf_rejected_variant_covers_as_hotlinks`.
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
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    // Re-seed a fresh run and apply again with the same candidate.
    let (run_id2, ordinal2) = seed_issue_run(&app, &issue_id, "comicvine").await;
    let _ = apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id2, ordinal2, ApplyMode::FillMissing, false),
    )
    .await
    .expect("second apply");
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

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
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
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
    // issues; enqueued_rewrite=true. Since the Chew fix, series-row
    // scalars are ALSO persisted DB-direct (they don't survive the
    // archive round-trip), so applied_fields reports them — publisher
    // is in this payload and the seeded series has none.
    assert!(outcome.enqueued_rewrite);
    assert_eq!(outcome.composed_sidecars, 3, "all three issues composed");
    assert!(
        outcome.sidecar_skip_reasons.is_empty(),
        "no skip reasons expected: {:?}",
        outcome.sidecar_skip_reasons,
    );
    assert!(
        outcome.applied_fields.iter().any(|f| f == "publisher"),
        "series scalars persist DB-direct on the sidecar path: {:?}",
        outcome.applied_fields,
    );

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
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
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
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
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
async fn apply_issue_keeps_ssrf_rejected_variant_covers_as_hotlinks() {
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, header};
    use entity::issue_cover;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    use server::metadata::cache;
    use server::metadata::identifier::Source;
    use server::metadata::provider::VariantCoverCandidate;
    use tower::ServiceExt;

    let urls = [
        "http://127.0.0.1:1/walker.png".to_owned(),
        "http://127.0.0.1:1/mccaig.png".to_owned(),
    ];

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
            image_url: Some(urls[0].clone()),
        },
        VariantCoverCandidate {
            label: Some("McCaig".into()),
            artist_name: None,
            identifiers: vec![],
            image_url: Some(urls[1].clone()),
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
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .order_by_asc(issue_cover::Column::Ordinal)
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (idx, row) in rows.iter().enumerate() {
        assert!(
            row.local_path.is_empty(),
            "SSRF-rejected URL should remain hotlink-only"
        );
        assert!(
            row.width.is_none() && row.height.is_none(),
            "no dimensions are recorded without downloaded bytes"
        );
        assert!(row.phash.is_none(), "no perceptual hash is computed");
        assert_eq!(
            row.source_url.as_deref(),
            Some(urls[idx].as_str()),
            "provider URL is preserved for hotlink fallback"
        );
    }

    let cookie = register_admin_cookie(&app).await;

    // The list endpoint still exposes the hotlink URL for metadata-only rows.
    let list = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/issues/{issue_id}/covers"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = to_bytes(list.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let covers = payload["covers"].as_array().expect("cover list response");
    assert_eq!(covers.len(), 2);
    assert_eq!(covers[0]["image_url"], urls[0]);
    assert_eq!(covers[1]["image_url"], urls[1]);

    // No local artifact was written, so the byte endpoint returns 404.
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
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

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
async fn variant_cover_backfill_skips_ssrf_rejected_hotlink_rows() {
    use entity::issue_cover;
    use sea_orm::{EntityTrait, Set};

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
        source_url: Set(Some("http://127.0.0.1:1/v.png".into())),
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

    let outcome = server::metadata::writers::run_variant_cover_backfill(
        &app.state().db,
        &app.state().cfg().data_path,
    )
    .await
    .unwrap();
    assert_eq!(outcome.considered, 1);
    assert_eq!(outcome.stored, 0);
    assert_eq!(outcome.skipped, 1);

    let row = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.local_path.is_empty(), "backfill keeps rejected hotlink");
    assert!(row.phash.is_none(), "rejected hotlink is not hashed");
}

#[tokio::test]
async fn apply_series_with_writeback_persists_uncarriable_series_fields() {
    // Chew regression: ComicInfo/MetronInfo only carry *issue*-level
    // summaries, and no schema has a slot for a series deck or alias
    // list — so the sidecar round-trip can't deliver them and the
    // scanner never writes `series.summary`. Pre-fix, a series-scope
    // apply on a writeback library rewrote every archive but the
    // series page's description never changed. These fields must be
    // written DB-direct (the sanctioned "XML can't carry it"
    // exception), honoring fill/replace semantics.
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Chew")
        .insert(&app.state().db)
        .await;
    let payload = build_cbz_bytes("chew-page-1");
    let cbz = dir.path().join("chew-001.cbz");
    IssueSeed::new(lib_id, series_id, &cbz, &payload, 1.0)
        .insert(&app.state().db)
        .await;

    use server::metadata::cache;
    use server::metadata::identifier::Source;
    let series_payload = server::metadata::provider::GenericMetadata {
        series_name: Some("Chew".into()),
        publisher: Some("Image Comics".into()),
        description: Some("Tony Chu is a cibopathic detective.".into()),
        deck: Some("Crime drama with a culinary twist.".into()),
        aliases: vec!["Chew: The Series".into()],
        series_sort_name: Some("Chew".into()),
        series_type: Some("ongoing".into()),
        year_began: Some(2009),
        year_end: Some(2016),
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("4050-27155".into()),
        ..Default::default()
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "27155",
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
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    for ordinal in [0, 1] {
        metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(ordinal),
            source: Set("comicvine".into()),
            external_id: Set("27155".into()),
            bucket: Set("high".into()),
            score: Set(95.0),
            score_breakdown: Set(json!({})),
            candidate: Set(json!({"kind": "series"})),
            applied_at: Set(None),
        }
        .insert(&app.state().db)
        .await
        .unwrap();
    }

    // Fill-missing: empty series summary/deck/aliases get the provider
    // values even though the archive round-trip can't carry them.
    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, 0, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_series fill");
    assert!(outcome.enqueued_rewrite);
    assert!(
        outcome.applied_fields.iter().any(|f| f == "description"),
        "description must be in applied_fields: {:?}",
        outcome.applied_fields,
    );

    let row = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("series row");
    assert_eq!(
        row.summary.as_deref(),
        Some("Tony Chu is a cibopathic detective."),
        "series summary must land despite the XML round-trip",
    );
    assert_eq!(
        row.deck.as_deref(),
        Some("Crime drama with a culinary twist."),
    );
    assert_eq!(row.aliases, json!(["Chew: The Series"]));
    // Full DB-direct parity: every scalar the legacy path applies must
    // land on the writeback path too — the series row is refreshed by
    // neither the composed XML nor the rescan for any of these.
    assert_eq!(row.sort_name.as_deref(), Some("Chew"));
    assert_eq!(row.series_type.as_deref(), Some("ongoing"));
    assert_eq!(row.year_end, Some(2016));
    assert_eq!(row.publisher.as_deref(), Some("Image Comics"));

    // Replace-all: a provider-updated description overwrites the
    // previous (non-user) value on a second apply.
    let updated = server::metadata::provider::GenericMetadata {
        description: Some("Updated description from provider.".into()),
        ..series_payload
    };
    cache::put(
        &app.state().db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "27155",
        &updated,
    )
    .await
    .unwrap();
    server::jobs::metadata_apply::apply_series_inline(
        &app.state(),
        series_id,
        args(run_id, 1, ApplyMode::ReplaceAll, false),
    )
    .await
    .expect("apply_series replace");
    let row = entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("series row");
    assert_eq!(
        row.summary.as_deref(),
        Some("Updated description from provider."),
        "replace-all must overwrite the provider-set summary",
    );
}

// ─────────────────────────────────────────────────────────────────
// Provider field-provenance from the sidecar branch
// (fix/field-provenance-writes). The XML can't carry "ComicVine set
// this on date X" and the scoped rescan's file-tier writes are guarded
// from overwriting provider rows, so the apply itself records the true
// source — same metadata-only exception class as variant covers.
// ─────────────────────────────────────────────────────────────────

use sea_orm::{ColumnTrait, QueryFilter};

async fn issue_prov(
    app: &TestApp,
    issue_id: &str,
) -> std::collections::HashMap<String, (String, Option<String>)> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.field, (r.set_by, r.source_external_id)))
        .collect()
}

#[tokio::test]
async fn apply_issue_with_writeback_writes_provider_field_provenance() {
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
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

    apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");
    // Provider provenance is deferred until the rewrite job lands the XML
    // (a pre-existing user pin is the only row allowed here).
    assert!(
        issue_prov(&app, &issue_id)
            .await
            .values()
            .all(|(set_by, _)| set_by == "user"),
        "no provider provenance before the rewrite runs"
    );
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    let prov = issue_prov(&app, &issue_id).await;
    // Provider-contributed fields (title, description, credits,
    // characters in the stub) get provider rows with the provider's
    // external id for the issue.
    for field in ["title", "description", "credits", "characters"] {
        assert_eq!(
            prov.get(field),
            Some(&("comicvine".to_owned(), Some("67890".to_owned()))),
            "field {field} must be attributed to the provider",
        );
    }
    // Fields the provider had nothing for keep no rows (the composer
    // kept the DB value), and page_count is excluded by design (the
    // scanner ingests the archive's real count, not the XML's claim).
    for field in ["tags", "genres", "page_count"] {
        assert!(
            !prov.contains_key(field),
            "unexpected provenance for {field}"
        );
    }
}

#[tokio::test]
async fn apply_issue_writeback_pin_suppresses_provider_provenance() {
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .with_title("My Hand-Edited Title")
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

    apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply_issue");
    // Provider provenance is deferred until the rewrite job lands the XML
    // (a pre-existing user pin is the only row allowed here).
    assert!(
        issue_prov(&app, &issue_id)
            .await
            .values()
            .all(|(set_by, _)| set_by == "user"),
        "no provider provenance before the rewrite runs"
    );
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    let prov = issue_prov(&app, &issue_id).await;
    assert_eq!(
        prov.get("title").map(|(s, _)| s.as_str()),
        Some("user"),
        "composer suppressed the provider title, so the pin must survive",
    );
    assert_eq!(
        prov.get("description").map(|(s, _)| s.as_str()),
        Some("comicvine"),
        "unpinned provider-contributed fields still attribute to the provider",
    );
}

#[tokio::test]
async fn apply_issue_writeback_override_retires_user_pin() {
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .with_title("My Hand-Edited Title")
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

    apply_issue_inline(
        &app.state(),
        &issue_id,
        args(run_id, ordinal, ApplyMode::FillMissing, true),
    )
    .await
    .expect("apply_issue");
    // Provider provenance is deferred until the rewrite job lands the XML
    // (a pre-existing user pin is the only row allowed here).
    assert!(
        issue_prov(&app, &issue_id)
            .await
            .values()
            .all(|(set_by, _)| set_by == "user"),
        "no provider provenance before the rewrite runs"
    );
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    let prov = issue_prov(&app, &issue_id).await;
    assert_eq!(
        prov.get("title").map(|(s, _)| s.as_str()),
        Some("comicvine"),
        "override_user_edits must retire the stale user pin so the \
         follow-up rescan can ingest the overridden value",
    );
}

// ───────── WP-2.6 (e) + (f): lock-busy requeue, CBT / CBR formats, deferred writes ─────────

/// A bare sidecar job with fixed XML payloads — what the drift-flush
/// endpoint enqueues (no provider decisions attached).
fn sidecar_job(issue_id: &str) -> RewriteIssueSidecarsJob {
    RewriteIssueSidecarsJob {
        issue_id: issue_id.to_owned(),
        comic_info_xml: "<?xml version=\"1.0\"?><ComicInfo><Title>Rewritten</Title></ComicInfo>"
            .to_owned(),
        metron_info_xml: "<?xml version=\"1.0\"?><MetronInfo><Title>Rewritten</Title></MetronInfo>"
            .to_owned(),
        suppressed_user_pins: Vec::new(),
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
        triggering_run_id: None,
        triggering_run_ordinal: None,
        skip_rescan: false,
        attempt: 0,
        post_apply: None,
    }
}

/// Minimal tar (CBT) with the given entries, PNG-signed pages included.
fn build_cbt_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut tw = tar::Builder::new(&mut buf);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tw.append_data(&mut header, *name, *bytes).unwrap();
        }
        tw.into_inner().unwrap();
    }
    buf.into_inner()
}

fn cbr_fixture() -> std::path::PathBuf {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page.cbr");
    assert!(p.is_file(), "fixtures/synthetic-3page.cbr is committed");
    p
}

async fn issue_row(app: &TestApp, issue_id: &str) -> issue::Model {
    issue::Entity::find_by_id(issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("issue present")
}

/// WP-2.6 (e) / audit DI-13: with the per-issue rewrite lock held, the
/// job is re-enqueued with `attempt + 1` instead of being silently dropped
/// (pre-fix it returned `Ok` with a log line claiming "the caller will
/// re-enqueue" — nothing did). Once the lock is released the requeued job
/// rewrites the archive.
#[tokio::test]
async fn sidecar_job_requeues_when_rewrite_lock_is_busy() {
    use server::archive_rewrite::mutex;
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;
    let before = std::fs::read(&cbz).unwrap();

    // Simulate an in-flight page edit holding the lock.
    let mut redis = app.state().jobs.redis.clone();
    let token = mutex::try_claim(&mut redis, &issue_id, mutex::EDIT_TTL_SECS)
        .await
        .unwrap()
        .expect("lock claimed");

    server::jobs::rewrite_sidecars::handle(
        sidecar_job(&issue_id),
        apalis::prelude::Data::new(app.state()),
    )
    .await
    .expect("busy lock is not an error");
    assert_eq!(
        std::fs::read(&cbz).unwrap(),
        before,
        "busy lock blocks the rewrite"
    );

    let queued = queued_rewrite_jobs(&app).await;
    assert_eq!(queued.len(), 1, "the write was re-enqueued, not dropped");
    assert_eq!(queued[0].issue_id, issue_id);
    assert_eq!(queued[0].attempt, 1, "attempt counter bumped");
    assert!(
        issue_row(&app, &issue_id)
            .await
            .last_sidecar_rewrite_at
            .is_none(),
        "no stamp without a rewrite"
    );

    // Release and let the requeued job run.
    mutex::release(&mut redis, &issue_id, &token).await;
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);
    assert_ne!(
        std::fs::read(&cbz).unwrap(),
        before,
        "requeued job rewrote the archive"
    );
    let mut a = archive::open(&cbz, archive::ArchiveLimits::default()).unwrap();
    assert!(
        String::from_utf8(a.read_entry_bytes("ComicInfo.xml").unwrap())
            .unwrap()
            .contains("<Title>Rewritten</Title>")
    );
    assert!(
        issue_row(&app, &issue_id)
            .await
            .last_sidecar_rewrite_at
            .is_some()
    );
}

/// WP-2.6 (f): a CBT is rewritten in place through the tar writer — pages
/// keep their original names, foreign sidecars survive, both fresh
/// sidecars land, and the bookkeeping stamps are set. Pre-fix the job
/// always `Cbz::open`ed and failed on every tar.
#[tokio::test]
async fn sidecar_rewrite_handles_cbt_in_place() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let p1: &[u8] = b"\x89PNG\r\n\x1a\nONE";
    let p2: &[u8] = b"\x89PNG\r\n\x1a\nTWO";
    let cbt_bytes = build_cbt_bytes(&[
        ("Issue/x-0002.png", p2),
        ("Issue/x-0001.png", p1),
        ("CoMet.xml", b"<comet/>"),
        (
            "ComicInfo.xml",
            b"<ComicInfo><Title>Old</Title></ComicInfo>",
        ),
        ("Thumbs.db", b"junk"),
    ]);
    let cbt = dir.path().join("saga-1.cbt");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbt, &cbt_bytes, 1.0)
        .insert(&app.state().db)
        .await;

    server::jobs::rewrite_sidecars::handle(
        sidecar_job(&issue_id),
        apalis::prelude::Data::new(app.state()),
    )
    .await
    .unwrap();

    let mut a = archive::open(&cbt, archive::ArchiveLimits::default()).unwrap();
    let pages: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(
        pages,
        vec!["Issue/x-0001.png", "Issue/x-0002.png"],
        "names preserved"
    );
    assert_eq!(a.read_entry_bytes("Issue/x-0001.png").unwrap(), p1);
    assert_eq!(a.read_entry_bytes("CoMet.xml").unwrap(), b"<comet/>");
    assert!(a.find("Thumbs.db").is_none(), "junk dropped");
    let ci = String::from_utf8(a.read_entry_bytes("ComicInfo.xml").unwrap()).unwrap();
    assert!(ci.contains("<Title>Rewritten</Title>"), "{ci}");
    assert!(a.find("MetronInfo.xml").is_some(), "MetronInfo added");
    assert!(cbt.with_extension("cbt.bak").exists(), ".bak kept");

    let row = issue_row(&app, &issue_id).await;
    assert!(row.file_path.ends_with("saga-1.cbt"), "still a .cbt");
    assert_eq!(row.last_rewrite_kind.as_deref(), Some("sidecar"));
    assert!(row.last_sidecar_rewrite_at.is_some());
}

/// WP-2.6 (f): a CBR in a library that allows CBR→CBZ conversion is
/// converted first (the `.cbr` kept as `.cbr.bak`), the row is repointed
/// at the `.cbz`, and the sidecars are written into the new archive.
#[tokio::test]
async fn sidecar_rewrite_converts_cbr_when_library_allows() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .with_auto_convert_cbr_on_scan()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Thanos")
        .insert(&app.state().db)
        .await;
    let cbr = dir.path().join("Thanos 001.cbr");
    let issue_id = IssueSeed::new(
        lib_id,
        series_id,
        &cbr,
        &std::fs::read(cbr_fixture()).unwrap(),
        1.0,
    )
    .insert(&app.state().db)
    .await;

    server::jobs::rewrite_sidecars::handle(
        sidecar_job(&issue_id),
        apalis::prelude::Data::new(app.state()),
    )
    .await
    .unwrap();

    let cbz = cbr.with_extension("cbz");
    assert!(cbz.exists(), "converted .cbz written");
    assert!(!cbr.exists(), "original .cbr moved away");
    let row = issue_row(&app, &issue_id).await;
    assert_eq!(
        row.file_path,
        cbz.to_string_lossy(),
        "row repointed at the .cbz"
    );
    assert!(row.last_sidecar_rewrite_at.is_some());

    let mut a = archive::open(&cbz, archive::ArchiveLimits::default()).unwrap();
    assert_eq!(a.pages().len(), 3, "all three fixture pages carried over");
    let ci = String::from_utf8(a.read_entry_bytes("ComicInfo.xml").unwrap()).unwrap();
    assert!(ci.contains("<Title>Rewritten</Title>"), "{ci}");
    assert!(a.find("MetronInfo.xml").is_some());

    let libr = entity::library::Entity::find_by_id(lib_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        libr.cbr_convert_confirmed_at.is_some(),
        "first conversion acknowledged"
    );
}

/// WP-2.6 (f): a CBR in a library that has NOT allowed conversion is
/// refused by the sidecar path *at dispatch* — the apply falls back to the
/// DB-direct branch with the reason on the outcome, no job is queued, and
/// the archive is untouched. Pre-fix the job was queued, failed at open,
/// and the run was already marked applied.
#[tokio::test]
async fn apply_issue_cbr_without_conversion_falls_back_to_db_direct() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Thanos")
        .insert(&app.state().db)
        .await;
    let cbr = dir.path().join("Thanos 001.cbr");
    let cbr_bytes = std::fs::read(cbr_fixture()).unwrap();
    let issue_id = IssueSeed::new(lib_id, series_id, &cbr, &cbr_bytes, 1.0)
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

    assert!(!outcome.enqueued_rewrite, "sidecar path refused");
    assert_eq!(
        outcome.sidecar_skip_reasons.len(),
        1,
        "{:?}",
        outcome.sidecar_skip_reasons
    );
    assert!(
        outcome.sidecar_skip_reasons[0].contains("CBR")
            && outcome.sidecar_skip_reasons[0].contains("applied DB-direct"),
        "{:?}",
        outcome.sidecar_skip_reasons
    );
    // FillMissing skips the seeded title; the empty summary is filled.
    assert!(
        outcome.applied_fields.iter().any(|f| f == "description"),
        "DB-direct apply ran: {:?}",
        outcome.applied_fields
    );
    assert!(
        queued_rewrite_jobs(&app).await.is_empty(),
        "no rewrite job queued"
    );
    assert_eq!(std::fs::read(&cbr).unwrap(), cbr_bytes, "archive untouched");
    let row = issue_row(&app, &issue_id).await;
    assert_eq!(
        row.summary.as_deref(),
        Some("Provider summary."),
        "provider summary landed DB-direct"
    );
    assert!(row.file_path.ends_with(".cbr"), "not converted");
    assert!(
        issue_prov(&app, &issue_id)
            .await
            .get("description")
            .is_some_and(|(s, _)| s == "comicvine"),
        "DB-direct provenance written",
    );
}

fn cb7_fixture(name: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    assert!(
        p.is_file(),
        "fixtures/{name} is committed (make-cb7-fixture.py)"
    );
    p
}

/// WP-6.5: a CB7 in a library that allows CB7→CBZ conversion is converted
/// first (the `.cb7` kept as `.cb7.bak`), the row is repointed at the
/// `.cbz`, and the sidecars are written into the new archive. Uses the
/// solid fixture so the one-pass 7z decode is on the path.
#[tokio::test]
async fn sidecar_rewrite_converts_cb7_when_library_allows() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .with_auto_convert_cb7_on_scan()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Thanos")
        .insert(&app.state().db)
        .await;
    let cb7 = dir.path().join("Thanos 001.cb7");
    let cb7_bytes = std::fs::read(cb7_fixture("synthetic-3page-solid.cb7")).unwrap();
    let issue_id = IssueSeed::new(lib_id, series_id, &cb7, &cb7_bytes, 1.0)
        .insert(&app.state().db)
        .await;

    server::jobs::rewrite_sidecars::handle(
        sidecar_job(&issue_id),
        apalis::prelude::Data::new(app.state()),
    )
    .await
    .unwrap();

    let cbz = cb7.with_extension("cbz");
    assert!(cbz.exists(), "converted .cbz written");
    assert!(!cb7.exists(), "original .cb7 moved away");
    assert_eq!(
        std::fs::read(cb7.with_extension("cb7.bak")).unwrap(),
        cb7_bytes,
        "original kept byte-for-byte as .cb7.bak"
    );
    let row = issue_row(&app, &issue_id).await;
    assert_eq!(
        row.file_path,
        cbz.to_string_lossy(),
        "row repointed at the .cbz"
    );
    assert!(row.last_sidecar_rewrite_at.is_some());

    let mut a = archive::open(&cbz, archive::ArchiveLimits::default()).unwrap();
    assert_eq!(a.pages().len(), 3, "all three fixture pages carried over");
    let ci = String::from_utf8(a.read_entry_bytes("ComicInfo.xml").unwrap()).unwrap();
    assert!(ci.contains("<Title>Rewritten</Title>"), "{ci}");
    assert!(a.find("MetronInfo.xml").is_some());
    assert!(
        a.read_entry_bytes("notes.txt")
            .unwrap()
            .starts_with(b"foreign entry"),
        "foreign entry preserved through conversion + rewrite"
    );

    // CB7 has no page-editor path: the CBR confirm gate stays untouched.
    let libr = entity::library::Entity::find_by_id(lib_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(libr.cbr_convert_confirmed_at.is_none());
}

/// WP-6.5: a CB7 in a library that has NOT allowed CB7 conversion is
/// refused at dispatch — even when the library *has* allowed CBR conversion
/// (the flags are deliberately separate). The apply falls back to the
/// DB-direct branch with the reason on the outcome and the file untouched.
#[tokio::test]
async fn apply_issue_cb7_without_conversion_falls_back_to_db_direct() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .with_auto_convert_cbr_on_scan()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Thanos")
        .insert(&app.state().db)
        .await;
    let cb7 = dir.path().join("Thanos 001.cb7");
    let cb7_bytes = std::fs::read(cb7_fixture("synthetic-3page.cb7")).unwrap();
    let issue_id = IssueSeed::new(lib_id, series_id, &cb7, &cb7_bytes, 1.0)
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

    assert!(!outcome.enqueued_rewrite, "sidecar path refused");
    assert_eq!(
        outcome.sidecar_skip_reasons.len(),
        1,
        "{:?}",
        outcome.sidecar_skip_reasons
    );
    assert!(
        outcome.sidecar_skip_reasons[0].contains("CB7")
            && outcome.sidecar_skip_reasons[0].contains("auto_convert_cb7_on_scan")
            && outcome.sidecar_skip_reasons[0].contains("applied DB-direct"),
        "{:?}",
        outcome.sidecar_skip_reasons
    );
    assert!(
        queued_rewrite_jobs(&app).await.is_empty(),
        "no rewrite job queued"
    );
    assert_eq!(std::fs::read(&cb7).unwrap(), cb7_bytes, "archive untouched");
    let row = issue_row(&app, &issue_id).await;
    assert!(row.file_path.ends_with(".cb7"), "not converted");
    assert_eq!(
        row.summary.as_deref(),
        Some("Provider summary."),
        "provider summary landed DB-direct"
    );
}

/// WP-2.6 (f) / audit DI-10: when the rewrite fails (here: the "archive"
/// isn't a zip at all), none of the deferred metadata writes happen — no
/// provider provenance, no variant rows, no `last_metadata_sync_at` — so
/// the DB never attributes provider values that never reached the file.
#[tokio::test]
async fn failed_rewrite_writes_no_provenance_variants_or_sync_stamp() {
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
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, b"definitely-not-a-zip", 1.0)
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
    assert!(outcome.enqueued_rewrite);

    // The job runs, fails at open, and is audited — not retried, and
    // none of the deferred writes land.
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    assert!(
        issue_prov(&app, &issue_id).await.is_empty(),
        "no provenance after a failed rewrite"
    );
    use entity::issue_cover;
    use sea_orm::{ColumnTrait, QueryFilter};
    let variants = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(
        variants.is_empty(),
        "no variant rows after a failed rewrite"
    );
    let row = issue_row(&app, &issue_id).await;
    assert!(
        row.last_metadata_sync_at.is_none(),
        "no sync stamp after a failed rewrite"
    );
    assert!(row.last_sidecar_rewrite_at.is_none());
    assert_eq!(
        std::fs::read(&cbz).unwrap(),
        b"definitely-not-a-zip",
        "file untouched"
    );
}
