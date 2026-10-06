//! WP-7.8: provider links and external targets.
//!
//! - Metron issue reprints persisted by the apply (DB-direct and sidecar
//!   writeback), with user precedence, the provider-id resolution and the
//!   later "reprinted issue scanned in" resolution, and the WP-7.6 reprint
//!   roll-up turning them into `collects` suggestions.
//! - Metron series `associated` recorded as external rows; local targets
//!   become `provider_associated` suggestions refined by series type;
//!   promotion when the series appears (provider row → suggestion, user
//!   row → manual pair).
//! - The `external` list on `GET /series/{slug}/relationships` (ACL), the
//!   admin create / delete endpoints (audit, dismissal memory, 403).
//! - The metadata cache schema-version bump.
//!
//! No network: provider details are pre-seeded into `metadata_cache`
//! (the apply reads through it), like the other apply tests.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{
    field_provenance, issue_reprint, library_user_access, metadata_run, metadata_run_candidate,
    series, series_external_relationship as ext, series_relationship,
    series_relationship_suggestion as sug,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    PaginatorTrait, QueryFilter, Set, Statement, Unchanged,
};
use serde_json::{Value, json};
use server::jobs::metadata_apply::{apply_issue_inline, apply_series_inline};
use server::jobs::relationship_suggest;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::cache;
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::{GenericMetadata, ProviderSeriesRef, ReprintCandidate};
use server::metadata::writers::{self, CoverOverwritePolicy, SetBy};
use std::io::{Cursor, Write};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

// ───── scaffolding ─────

fn args(run_id: Uuid, mode: ApplyMode, override_user: bool) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal: 0,
        mode,
        apply_cover: false,
        cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
        override_user_edits: override_user,
        actor_id: None,
        // A preview selection that doesn't list `reprints` (there is no
        // preview row for them) must not block them.
        selected_fields: Some(["title".to_owned()].into_iter().collect()),
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

async fn seed_run(db: &DatabaseConnection, scope: &str, entity_id: &str, ext_id: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set(scope.into()),
        scope_entity_id: Set(Some(entity_id.into())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into()]),
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
        source: Set("metron".into()),
        external_id: Set(ext_id.into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": scope})),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

fn cbz(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

async fn issue(
    db: &DatabaseConnection,
    dir: &std::path::Path,
    lib: Uuid,
    series: Uuid,
    label: &str,
    n: f64,
) -> String {
    IssueSeed::new(
        lib,
        series,
        &dir.join(format!("{label}.cbz")),
        &cbz(label),
        n,
    )
    .insert(db)
    .await
}

async fn set_id(db: &DatabaseConnection, entity_type: &str, id: &str, source: Source, ext: &str) {
    writers::set_external_id(
        db,
        entity_type,
        id,
        &Identifier::with_canonical_url(source, ext, entity_type),
        SetBy::Provider(source),
    )
    .await
    .unwrap();
}

async fn set_series_type(db: &DatabaseConnection, id: Uuid, t: &str) {
    series::ActiveModel {
        id: Unchanged(id),
        series_type: Set(Some(t.into())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

fn reprint(label: &str, metron_id: &str) -> ReprintCandidate {
    ReprintCandidate {
        label: label.into(),
        identifiers: vec![Identifier::with_canonical_url(
            Source::Metron,
            metron_id,
            "issue",
        )],
    }
}

fn metron_issue(id: &str, reprints: Vec<ReprintCandidate>) -> GenericMetadata {
    GenericMetadata {
        title: Some("Volume One".into()),
        issue_number: Some("1".into()),
        reprints,
        identifiers: vec![Identifier::with_canonical_url(Source::Metron, id, "issue")],
        source_provider: Some(Source::Metron),
        source_external_id: Some(id.into()),
        ..Default::default()
    }
}

async fn reprint_rows(db: &DatabaseConnection, issue_id: &str) -> Vec<issue_reprint::Model> {
    let mut rows = issue_reprint::Entity::find()
        .filter(issue_reprint::Column::IssueId.eq(issue_id))
        .all(db)
        .await
        .unwrap();
    rows.sort_by(|a, b| a.reprinted_label.cmp(&b.reprinted_label));
    rows
}

async fn provenance(db: &DatabaseConnection, issue_id: &str, field: &str) -> Option<String> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(issue_id))
        .filter(field_provenance::Column::Field.eq(field))
        .one(db)
        .await
        .unwrap()
        .map(|p| p.set_by)
}

async fn pin_user(db: &DatabaseConnection, issue_id: &str, field: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
         VALUES ('issue', $1, $2, 'user', now()) \
         ON CONFLICT (entity_type, entity_id, field) DO UPDATE SET set_by = 'user'",
        [issue_id.into(), field.into()],
    ))
    .await
    .unwrap();
}

/// A "Saga TPB" (collected edition) issue reprinting Saga #1 (local,
/// Metron 501) and Saga #2 (Metron 502, not in the library yet).
struct ReprintFixture {
    lib: Uuid,
    tpb: Uuid,
    saga: Uuid,
    tpb_issue: String,
    saga_1: String,
    dir: tempfile::TempDir,
}

async fn reprint_fixture(db: &DatabaseConnection, writeback: bool) -> ReprintFixture {
    let dir = tempdir().unwrap();
    let mut seed = LibrarySeed::new(dir.path());
    if writeback {
        seed = seed.with_sidecar_writeback();
    }
    let lib = seed.insert(db).await;
    let saga = SeriesSeed::new(lib, "Saga").insert(db).await;
    let tpb = SeriesSeed::new(lib, "Saga TPB").insert(db).await;
    let saga_1 = issue(db, dir.path(), lib, saga, "saga-1", 1.0).await;
    let tpb_issue = issue(db, dir.path(), lib, tpb, "saga-tpb-1", 1.0).await;
    set_id(db, "issue", &saga_1, Source::Metron, "501").await;
    ReprintFixture {
        lib,
        tpb,
        saga,
        tpb_issue,
        saga_1,
        dir,
    }
}

async fn put_issue_detail(db: &DatabaseConnection, d: &GenericMetadata) {
    cache::put(
        db,
        Source::Metron,
        cache::CacheEntity::Issue,
        d.source_external_id.as_deref().unwrap(),
        d,
    )
    .await
    .unwrap();
}

async fn suggestions(db: &DatabaseConnection, from: Uuid, to: Uuid) -> Vec<sug::Model> {
    sug::Entity::find()
        .filter(sug::Column::FromSeriesId.eq(from))
        .filter(sug::Column::ToSeriesId.eq(to))
        .all(db)
        .await
        .unwrap()
}

// ───── reprints ─────

#[tokio::test]
async fn reprints_persist_on_db_direct_apply_and_roll_up_to_collects() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = reprint_fixture(&db, false).await;
    put_issue_detail(
        &db,
        &metron_issue(
            "777",
            vec![reprint("Saga #1", "501"), reprint("Saga #2", "502")],
        ),
    )
    .await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    let outcome = apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::FillMissing, false),
    )
    .await
    .expect("apply");
    assert!(outcome.junctions_touched.contains(&"reprints".to_owned()));
    assert!(outcome.applied_fields.contains(&"reprints".to_owned()));

    let rows = reprint_rows(&db, &f.tpb_issue).await;
    assert_eq!(rows.len(), 2);
    // Saga #1 resolved through its Metron id; Saga #2 kept as a label with
    // its provider id pending.
    assert_eq!(rows[0].reprinted_label.as_deref(), Some("Saga #1"));
    assert_eq!(
        rows[0].reprinted_issue_id.as_deref(),
        Some(f.saga_1.as_str())
    );
    assert_eq!(rows[1].reprinted_label.as_deref(), Some("Saga #2"));
    assert_eq!(rows[1].reprinted_issue_id, None);
    assert_eq!(rows[1].reprinted_source.as_deref(), Some("metron"));
    assert_eq!(rows[1].reprinted_external_id.as_deref(), Some("502"));
    assert_eq!(
        provenance(&db, &f.tpb_issue, "reprints").await.as_deref(),
        Some("metron")
    );

    // Saga #2 is scanned in and matched: the external-id hook resolves the
    // pending row.
    let saga_2 = issue(&db, f.dir.path(), f.lib, f.saga, "saga-2", 2.0).await;
    set_id(&db, "issue", &saga_2, Source::Metron, "502").await;
    let rows = reprint_rows(&db, &f.tpb_issue).await;
    assert_eq!(rows[1].reprinted_issue_id.as_deref(), Some(saga_2.as_str()));

    // WP-7.6's reprint roll-up now proposes TPB collects Saga #1-2.
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.by_source.get("reprint_rollup"), Some(&1));
    let s = suggestions(&db, f.tpb, f.saga).await;
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].kind, "collects");
    assert_eq!(s[0].to_range.as_deref(), Some("1-2"));
    assert_eq!(s[0].coverage.as_deref(), Some("full"));
    assert_eq!(s[0].status, "pending");
}

#[tokio::test]
async fn reprint_rows_resolve_on_the_suggestion_run_when_the_hook_was_missed() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = reprint_fixture(&db, false).await;
    put_issue_detail(&db, &metron_issue("777", vec![reprint("Saga #2", "502")])).await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::FillMissing, false),
    )
    .await
    .unwrap();
    // The id lands without the writer (a raw insert, as a missed hook).
    let saga_2 = issue(&db, f.dir.path(), f.lib, f.saga, "saga-2", 2.0).await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, \
                                   first_set_at, last_synced_at) \
         VALUES ('issue', $1, 'metron', '502', 'metron', now(), now())",
        [saga_2.clone().into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        reprint_rows(&db, &f.tpb_issue).await[0].reprinted_issue_id,
        None
    );
    relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(
        reprint_rows(&db, &f.tpb_issue).await[0]
            .reprinted_issue_id
            .as_deref(),
        Some(saga_2.as_str())
    );
    assert_eq!(suggestions(&db, f.tpb, f.saga).await[0].kind, "collects");
}

#[tokio::test]
async fn reprints_respect_user_precedence_and_mode() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = reprint_fixture(&db, false).await;
    put_issue_detail(&db, &metron_issue("777", vec![reprint("Saga #1", "501")])).await;

    // A user pin on `reprints` is sacred…
    pin_user(&db, &f.tpb_issue, "reprints").await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    let out = apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    assert!(out.skipped_fields.contains(&"reprints".to_owned()));
    assert!(reprint_rows(&db, &f.tpb_issue).await.is_empty());
    assert_eq!(
        provenance(&db, &f.tpb_issue, "reprints").await.as_deref(),
        Some("user")
    );

    // …unless the admin overrides.
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::ReplaceAll, true),
    )
    .await
    .unwrap();
    assert_eq!(reprint_rows(&db, &f.tpb_issue).await.len(), 1);
    assert_eq!(
        provenance(&db, &f.tpb_issue, "reprints").await.as_deref(),
        Some("metron")
    );

    // fill_missing keeps an existing set; replace_all replaces it.
    put_issue_detail(
        &db,
        &metron_issue(
            "777",
            vec![reprint("Saga #1", "501"), reprint("Saga #3", "503")],
        ),
    )
    .await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    let out = apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::FillMissing, false),
    )
    .await
    .unwrap();
    assert!(out.skipped_fields.contains(&"reprints".to_owned()));
    assert_eq!(reprint_rows(&db, &f.tpb_issue).await.len(), 1);
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    assert_eq!(reprint_rows(&db, &f.tpb_issue).await.len(), 2);
}

/// Every `RewriteIssueSidecarsJob` queued, run through the worker entry
/// point (the deferred writes land after the rewrite).
async fn run_rewrite_jobs(app: &TestApp) -> usize {
    use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let jobs: Vec<RewriteIssueSidecarsJob> = all
        .values()
        .map(|blob| {
            let v: Value = serde_json::from_str(blob).unwrap();
            serde_json::from_value(v["args"].clone()).unwrap()
        })
        .collect();
    let n = jobs.len();
    for job in jobs {
        server::jobs::rewrite_sidecars::handle(job, apalis::prelude::Data::new(app.state()))
            .await
            .unwrap();
    }
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    n
}

#[tokio::test]
async fn reprints_persist_after_the_rewrite_in_writeback_libraries() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = reprint_fixture(&db, true).await;
    put_issue_detail(
        &db,
        &metron_issue(
            "777",
            vec![reprint("Saga #1", "501"), reprint("Saga #2", "502")],
        ),
    )
    .await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    let out = apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::FillMissing, false),
    )
    .await
    .unwrap();
    assert!(out.enqueued_rewrite);
    // Nothing lands before the archive holds the new XML…
    assert!(reprint_rows(&db, &f.tpb_issue).await.is_empty());
    assert_eq!(run_rewrite_jobs(&app).await, 1);
    // …then the deferred write does, with provider provenance.
    let rows = reprint_rows(&db, &f.tpb_issue).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].reprinted_issue_id.as_deref(),
        Some(f.saga_1.as_str())
    );
    assert_eq!(rows[1].reprinted_external_id.as_deref(), Some("502"));
    assert_eq!(
        provenance(&db, &f.tpb_issue, "reprints").await.as_deref(),
        Some("metron")
    );

    // A user pin keeps the writeback path off them too.
    pin_user(&db, &f.tpb_issue, "reprints").await;
    put_issue_detail(&db, &metron_issue("777", vec![reprint("Saga #9", "509")])).await;
    let run = seed_run(&db, "issue", &f.tpb_issue, "777").await;
    apply_issue_inline(
        &app.state(),
        &f.tpb_issue,
        args(run, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    run_rewrite_jobs(&app).await;
    assert_eq!(
        reprint_rows(&db, &f.tpb_issue).await.len(),
        2,
        "pinned set kept"
    );
    let _ = f.tpb;
}

// ───── associated → suggestions / external rows ─────

fn metron_series(
    id: &str,
    name: &str,
    series_type: &str,
    links: &[(&str, &str)],
) -> GenericMetadata {
    GenericMetadata {
        series_name: Some(name.into()),
        series_type: Some(series_type.into()),
        related_series: links
            .iter()
            .map(|(id, label)| {
                let name = label
                    .rsplit_once(" (")
                    .map_or(label.to_string(), |(n, _)| n.to_owned());
                let year = label
                    .rsplit_once(" (")
                    .and_then(|(_, y)| y.trim_end_matches(')').parse().ok());
                ProviderSeriesRef {
                    source: Source::Metron,
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                    name,
                    year,
                    url: None,
                }
            })
            .collect(),
        identifiers: vec![Identifier::with_canonical_url(Source::Metron, id, "series")],
        source_provider: Some(Source::Metron),
        source_external_id: Some(id.into()),
        ..Default::default()
    }
}

async fn put_series_detail(db: &DatabaseConnection, d: &GenericMetadata) {
    cache::put(
        db,
        Source::Metron,
        cache::CacheEntity::Series,
        d.source_external_id.as_deref().unwrap(),
        d,
    )
    .await
    .unwrap();
}

async fn ext_rows(db: &DatabaseConnection, from: Uuid) -> Vec<ext::Model> {
    let mut v = ext::Entity::find()
        .filter(ext::Column::FromSeriesId.eq(from))
        .all(db)
        .await
        .unwrap();
    v.sort_by(|a, b| a.provider_series_id.cmp(&b.provider_series_id));
    v
}

/// Local "Saga" (ongoing, Metron 100) and "Saga TPB" (trade paperback,
/// Metron 900). Metron lists Saga as associated with the TPB (local), a
/// "Saga Annual (2015)" (901) and "Saga (2018)" (902), neither local.
struct AssocFixture {
    lib: Uuid,
    saga: Uuid,
    tpb: Uuid,
    _dir: tempfile::TempDir,
}

async fn assoc_fixture(app: &TestApp, writeback: bool) -> AssocFixture {
    let db = app.state().db.clone();
    let dir = tempdir().unwrap();
    let mut seed = LibrarySeed::new(dir.path());
    if writeback {
        seed = seed.with_sidecar_writeback();
    }
    let lib = seed.insert(&db).await;
    let saga = SeriesSeed::new(lib, "Saga").insert(&db).await;
    let tpb = SeriesSeed::new(lib, "Saga Deluxe").insert(&db).await;
    set_series_type(&db, tpb, "Trade Paperback").await;
    set_id(&db, "series", &tpb.to_string(), Source::Metron, "900").await;
    put_series_detail(
        &db,
        &metron_series(
            "100",
            "Saga",
            "Ongoing Series",
            &[
                ("900", "Saga Deluxe (2014)"),
                ("901", "Saga Annual (2015)"),
                ("902", "Saga (2018)"),
                ("100", "Saga (2012)"), // a self-link is ignored
            ],
        ),
    )
    .await;
    let run = seed_run(&db, "series", &saga.to_string(), "100").await;
    apply_series_inline(&app.state(), saga, args(run, ApplyMode::FillMissing, false))
        .await
        .expect("series apply");
    AssocFixture {
        lib,
        saga,
        tpb,
        _dir: dir,
    }
}

#[tokio::test]
async fn associated_records_external_rows_and_local_suggestions() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = assoc_fixture(&app, false).await;

    let rows = ext_rows(&db, f.saga).await;
    let summary: Vec<(&str, &str, Option<i32>, bool)> = rows
        .iter()
        .map(|r| {
            (
                r.provider_series_id.as_str(),
                r.kind.as_str(),
                r.provider_year,
                r.promoted_series_id.is_some(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            // The TPB is local: stored, marked promoted.
            ("900", "collected_in", Some(2014), true),
            // "… Annual" by name → Saga has_annual it.
            ("901", "has_annual", Some(2015), false),
            ("902", "see_also", Some(2018), false),
        ]
    );
    assert!(rows.iter().all(|r| r.set_by == "provider"));
    assert_eq!(rows[2].evidence["source"], "metron");
    assert_eq!(rows[2].evidence["field"], "associated");
    assert_eq!(rows[2].evidence["ids"], json!(["100", "902"]));
    assert_eq!(rows[2].provider_series_name.as_deref(), Some("Saga"));
    assert!(
        rows[2]
            .provider_series_url
            .as_deref()
            .is_some_and(|u| u.contains("metron.cloud"))
    );
    // `associated` is no longer an alias source.
    let saga_row = series::Entity::find_by_id(f.saga)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saga_row.aliases, json!([]));

    // The local pair becomes a suggestion: the TPB collects Saga (series
    // types refine Metron's untyped link), medium bucket, with evidence.
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.by_source.get("provider_associated"), Some(&1));
    let s = suggestions(&db, f.tpb, f.saga).await;
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].kind, "collects");
    assert_eq!(s[0].bucket, "medium");
    let ev = &s[0].evidence["sources"][0];
    assert_eq!(ev["source"], "provider_associated");
    assert_eq!(ev["provider"], "metron");
    assert_eq!(ev["field"], "associated");
    // Re-applying is idempotent and keeps the promotion mark.
    let run = seed_run(&db, "series", &f.saga.to_string(), "100").await;
    apply_series_inline(
        &app.state(),
        f.saga,
        args(run, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    assert_eq!(ext_rows(&db, f.saga).await.len(), 3);
}

#[tokio::test]
async fn associated_annual_series_type_refines_to_annual_of() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;
    let main = SeriesSeed::new(lib, "Hellboy").insert(&db).await;
    set_series_type(&db, main, "Ongoing Series").await;
    let specials = SeriesSeed::new(lib, "Hellboy Specials").insert(&db).await;
    set_id(&db, "series", &main.to_string(), Source::Metron, "300").await;
    put_series_detail(
        &db,
        &metron_series(
            "301",
            "Hellboy Specials",
            "Annual Series",
            &[("300", "Hellboy (1994)")],
        ),
    )
    .await;
    let run = seed_run(&db, "series", &specials.to_string(), "301").await;
    apply_series_inline(
        &app.state(),
        specials,
        args(run, ApplyMode::FillMissing, false),
    )
    .await
    .unwrap();
    relationship_suggest::run(&db, lib).await.unwrap();
    let s = suggestions(&db, specials, main).await;
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!(s[0].kind, "annual_of");
    assert!(s[0].confidence >= 0.7);
}

#[tokio::test]
async fn associated_is_recorded_in_writeback_libraries_too() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = assoc_fixture(&app, true).await;
    assert_eq!(ext_rows(&db, f.saga).await.len(), 3);
}

#[tokio::test]
async fn provider_row_promotes_to_a_suggestion_when_the_series_appears() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = assoc_fixture(&app, false).await;
    // "Saga (2018)" is scanned in and matched to Metron 902.
    let saga2018 = SeriesSeed::new(f.lib, "Saga 2018").insert(&db).await;
    set_id(&db, "series", &saga2018.to_string(), Source::Metron, "902").await;
    let row = ext_rows(&db, f.saga)
        .await
        .into_iter()
        .find(|r| r.provider_series_id == "902")
        .unwrap();
    assert_eq!(row.promoted_series_id, Some(saga2018), "hook marks it");
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let (a, b) = if f.saga < saga2018 {
        (f.saga, saga2018)
    } else {
        (saga2018, f.saga)
    };
    let s = suggestions(&db, a, b).await;
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, "see_also");
    // Nothing is created without review.
    assert_eq!(
        series_relationship::Entity::find()
            .filter(series_relationship::Column::FromSeriesId.eq(f.saga))
            .count(&db)
            .await
            .unwrap(),
        0
    );

    // The match goes away → the mark clears on the next run.
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM external_ids WHERE entity_type = 'series' AND entity_id = $1",
        [saga2018.to_string().into()],
    ))
    .await
    .unwrap();
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let row = ext::Entity::find_by_id(row.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.promoted_series_id, None);
}

#[tokio::test]
async fn promotion_through_the_cached_id_bridge() {
    // The external row names Metron 902; the local series is matched only
    // to ComicVine 4444. Metron's cached detail of 902 lists cv 4444.
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = assoc_fixture(&app, false).await;
    let mut d902 = metron_series("902", "Saga", "Ongoing Series", &[]);
    d902.identifiers.push(Identifier::with_canonical_url(
        Source::ComicVine,
        "4444",
        "series",
    ));
    put_series_detail(&db, &d902).await;
    let saga2018 = SeriesSeed::new(f.lib, "Saga 2018").insert(&db).await;
    set_id(
        &db,
        "series",
        &saga2018.to_string(),
        Source::ComicVine,
        "4444",
    )
    .await;
    let row = ext_rows(&db, f.saga)
        .await
        .into_iter()
        .find(|r| r.provider_series_id == "902")
        .unwrap();
    assert_eq!(row.promoted_series_id, Some(saga2018));
}

// ───── HTTP: GET external, admin create / delete ─────

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let user = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: user.id,
    }
}

async fn call(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method.clone()).uri(path).header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

async fn grant(db: &DatabaseConnection, user_id: Uuid, library_id: Uuid) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn slug_of(db: &DatabaseConnection, id: Uuid) -> String {
    series::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug
}

async fn audit_rows(db: &DatabaseConnection, action: &str) -> Vec<entity::audit_log::Model> {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .all(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn external_links_in_the_relationships_get_with_acl() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let f = assoc_fixture(&app, false).await;
    let slug = slug_of(&db, f.saga).await;
    let path = format!("/api/series/{slug}/relationships");

    // No grant → 404 (series gate), nothing leaks.
    let (st, _) = call(&app, Method::GET, &path, &reader, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    grant(&db, reader.user_id, f.lib).await;
    let (st, body) = call(&app, Method::GET, &path, &reader, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let ext = body["external"].as_array().unwrap();
    // The TPB link resolved locally → left to the suggestion engine.
    let names: Vec<(&str, &str)> = ext
        .iter()
        .map(|e| {
            (
                e["provider_series_id"].as_str().unwrap(),
                e["kind_label"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(names, vec![("901", "Has annual"), ("902", "See also")]);
    assert_eq!(ext[1]["name"], "Saga");
    assert_eq!(ext[1]["year"], 2018);
    assert_eq!(ext[1]["source"], "metron");
    assert_eq!(ext[1]["source_label"], "Metron");
    assert_eq!(ext[1]["set_by"], "provider");
    assert_eq!(ext[1]["group"], "story");
    assert!(ext[1]["local_series"].is_null());

    // The series detail count includes them.
    let (_, detail) = call(
        &app,
        Method::GET,
        &format!("/api/series/{slug}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(detail["relationship_count"], 2);

    // A user link whose target is matched to a series in a library the
    // reader can't see: plain external row for the reader, with the local
    // series for the admin (until the next run promotes it).
    let dir2 = tempdir().unwrap();
    let lib2 = LibrarySeed::new(dir2.path()).insert(&db).await;
    let hidden = SeriesSeed::new(lib2, "Saga Hidden").insert(&db).await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, \
                                   first_set_at, last_synced_at) \
         VALUES ('series', $1, 'comicvine', '5555', 'comicvine', now(), now())",
        [hidden.to_string().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO series_external_relationship \
           (id, from_series_id, kind, source, provider_series_id, provider_series_name, set_by) \
         VALUES ($1, $2, 'continued_by', 'comicvine', '5555', 'Saga Hidden', 'user')",
        [Uuid::now_v7().into(), f.saga.into()],
    ))
    .await
    .unwrap();
    let (_, body) = call(&app, Method::GET, &path, &reader, None).await;
    let row = body["external"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["provider_series_id"] == "5555")
        .cloned()
        .unwrap();
    assert!(row["local_series"].is_null(), "reader can't see it");
    assert_eq!(row["kind_label"], "Continued by");
    let (_, body) = call(&app, Method::GET, &path, &admin, None).await;
    let row = body["external"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["provider_series_id"] == "5555")
        .cloned()
        .unwrap();
    assert_eq!(row["local_series"]["id"], hidden.to_string());
}

#[tokio::test]
async fn admin_creates_and_removes_external_links() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let f = assoc_fixture(&app, false).await;
    grant(&db, reader.user_id, f.lib).await;
    let slug = slug_of(&db, f.saga).await;
    let base = format!("/api/series/{slug}/external-relationships");

    let body = json!({
        "kind": "continued_by", "qualifier": "relaunch",
        "source": "metron", "provider_series_id": "903",
        "name": "Saga: The Next Arc", "year": 2024,
    });
    // Non-admins can't.
    let (st, _) = call(&app, Method::POST, &base, &reader, Some(body.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, created) = call(&app, Method::POST, &base, &admin, Some(body.clone())).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let ext_view = &created["external"];
    assert!(created["relationship"].is_null());
    assert_eq!(ext_view["kind"], "continued_by");
    assert_eq!(ext_view["kind_label"], "Continued by");
    assert_eq!(ext_view["qualifier"], "relaunch");
    assert_eq!(ext_view["set_by"], "user");
    assert!(ext_view["url"].as_str().unwrap().contains("metron.cloud"));
    let audits = audit_rows(&db, "admin.series.external_relationship.create").await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].payload["provider_series_id"], "903");

    // Idempotent: the same link again is 200, no second audit row.
    let (st, _) = call(&app, Method::POST, &base, &admin, Some(body.clone())).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        audit_rows(&db, "admin.series.external_relationship.create")
            .await
            .len(),
        1
    );

    // Validation: field-level 422.
    let (st, err) = call(
        &app,
        Method::POST,
        &base,
        &admin,
        Some(
            json!({"kind": "see_also", "qualifier": "relaunch", "source": "gcd",
                    "provider_series_id": "abc", "name": " ", "year": 3000}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<&str> = err["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    for f in ["provider_series_id", "name", "year", "qualifier"] {
        assert!(fields.contains(&f), "{fields:?}");
    }

    // Remove the user link: deleted, audited.
    let id = ext_view["id"].as_str().unwrap();
    let (st, _) = call(&app, Method::DELETE, &format!("{base}/{id}"), &reader, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&app, Method::DELETE, &format!("{base}/{id}"), &admin, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        ext::Entity::find_by_id(Uuid::parse_str(id).unwrap())
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );

    // Remove a provider link: dismissed, never re-created by an apply.
    let provider_row = ext_rows(&db, f.saga)
        .await
        .into_iter()
        .find(|r| r.provider_series_id == "902")
        .unwrap();
    let (st, _) = call(
        &app,
        Method::DELETE,
        &format!("{base}/{}", provider_row.id),
        &admin,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let deletes = audit_rows(&db, "admin.series.external_relationship.delete").await;
    assert_eq!(deletes.len(), 2);
    assert!(deletes.iter().any(|a| a.payload["dismissed"] == true));
    let run = seed_run(&db, "series", &f.saga.to_string(), "100").await;
    apply_series_inline(
        &app.state(),
        f.saga,
        args(run, ApplyMode::ReplaceAll, false),
    )
    .await
    .unwrap();
    let row = ext::Entity::find_by_id(provider_row.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.dismissed_at.is_some(), "dismissal survives a re-apply");
    let (_, body) = call(
        &app,
        Method::GET,
        &format!("/api/series/{slug}/relationships"),
        &admin,
        None,
    )
    .await;
    assert!(
        !body["external"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["provider_series_id"] == "902")
    );
    // Second delete of a dismissed row → 404.
    let (st, _) = call(
        &app,
        Method::DELETE,
        &format!("{base}/{}", provider_row.id),
        &admin,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_link_promotes_to_a_pair_when_the_series_appears() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let admin = register(&app, "admin@example.com").await;
    let f = assoc_fixture(&app, false).await;
    let slug = slug_of(&db, f.saga).await;
    let (st, created) = call(
        &app,
        Method::POST,
        &format!("/api/series/{slug}/external-relationships"),
        &admin,
        Some(
            json!({"kind": "continued_by", "qualifier": "relaunch", "source": "metron",
                    "provider_series_id": "903", "name": "Saga II"}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let ext_id = Uuid::parse_str(created["external"]["id"].as_str().unwrap()).unwrap();

    // The series is scanned in and matched: the hook creates the pair.
    let next = SeriesSeed::new(f.lib, "Saga II").insert(&db).await;
    set_id(&db, "series", &next.to_string(), Source::Metron, "903").await;
    assert!(
        ext::Entity::find_by_id(ext_id)
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
    let edges = series_relationship::Entity::find()
        .filter(series_relationship::Column::FromSeriesId.is_in([f.saga, next]))
        .all(&db)
        .await
        .unwrap();
    let mut kinds: Vec<(Uuid, String, Option<String>, String)> = edges
        .into_iter()
        .map(|e| (e.from_series_id, e.kind, e.qualifier, e.source))
        .collect();
    kinds.sort();
    let mut want = vec![
        (
            f.saga,
            "continued_by".to_owned(),
            Some("relaunch".to_owned()),
            "manual".to_owned(),
        ),
        (
            next,
            "continues".to_owned(),
            Some("relaunch".to_owned()),
            "manual".to_owned(),
        ),
    ];
    want.sort();
    assert_eq!(kinds, want);
    let edge = series_relationship::Entity::find()
        .filter(series_relationship::Column::FromSeriesId.eq(f.saga))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(edge.created_by, Some(admin.user_id));

    // Adding a link to a provider series that's already local creates the
    // pair right away.
    let (st, created) = call(
        &app,
        Method::POST,
        &format!("/api/series/{slug}/external-relationships"),
        &admin,
        Some(json!({"kind": "see_also", "source": "metron",
                    "provider_series_id": "900", "name": "Saga Deluxe"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert!(created["external"].is_null());
    assert_eq!(created["relationship"]["kind"], "see_also");
    assert_eq!(created["relationship"]["series"]["id"], f.tpb.to_string());
}

#[tokio::test]
async fn user_link_promotes_on_the_suggestion_run_when_the_hook_was_missed() {
    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let db = app.state().db.clone();
    let f = assoc_fixture(&app, false).await;
    let next = SeriesSeed::new(f.lib, "Saga II").insert(&db).await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, \
                                   first_set_at, last_synced_at) \
         VALUES ('series', $1, 'metron', '903', 'metron', now(), now())",
        [next.to_string().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO series_external_relationship \
           (id, from_series_id, kind, source, provider_series_id, provider_series_name, set_by) \
         VALUES ($1, $2, 'sequel_of', 'metron', '903', 'Saga II', 'user')",
        [Uuid::now_v7().into(), f.saga.into()],
    ))
    .await
    .unwrap();
    relationship_suggest::run(&db, f.lib).await.unwrap();
    assert!(
        series_relationship::Entity::find()
            .filter(series_relationship::Column::FromSeriesId.eq(f.saga))
            .filter(series_relationship::Column::Kind.eq("sequel_of"))
            .one(&db)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        ext::Entity::find()
            .filter(ext::Column::ProviderSeriesId.eq("903"))
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
}

// ───── cache ─────

#[tokio::test]
async fn cache_rows_from_the_previous_schema_version_are_misses() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let d = metron_series("100", "Saga", "Ongoing Series", &[("902", "Saga (2018)")]);
    put_series_detail(&db, &d).await;
    let hit = cache::get(
        &db,
        Source::Metron,
        cache::CacheEntity::Series,
        "100",
        chrono::Duration::days(1),
    )
    .await
    .unwrap()
    .expect("current version is a hit");
    assert_eq!(hit.related_series.len(), 1);
    assert_eq!(hit.related_series[0].id, "902");
    assert_eq!(hit.related_series[0].year, Some(2018));
    assert_eq!(cache::CACHE_SCHEMA_VERSION, 2);

    // A payload stamped by the pre-WP-7.8 mapping is re-fetched, not
    // trusted (its `associated` was always empty).
    db.execute_raw(Statement::from_string(
        DbBackend::Postgres,
        "UPDATE metadata_cache SET schema_version = 1",
    ))
    .await
    .unwrap();
    assert!(
        cache::get(
            &db,
            Source::Metron,
            cache::CacheEntity::Series,
            "100",
            chrono::Duration::days(1),
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        cache::get_stale(&db, Source::Metron, cache::CacheEntity::Series, "100")
            .await
            .unwrap()
            .is_none()
    );
}
