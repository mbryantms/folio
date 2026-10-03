//! Owner bug (Chew #14, v0.33.0): in a writeback library, a description
//! picked in the "Fetch metadata" dialog reached the archive's
//! `<Summary>` but never `issues.summary`, so the issue page kept showing
//! the old text.
//!
//! The rewrite job records the apply's provider `field_provenance` rows
//! (WP-2.6 f) right after it queues the scoped rescan, and the scanner's
//! WP-2.5 tier gate treated any provider row as "owns the column" — so
//! the rescan refused to ingest the very XML Folio had just written.
//!
//! These tests drive the same surfaces the UI does (single-candidate
//! `POST …/metadata/apply` with `selected_fields`, and the compare-mode
//! `POST …/metadata/composite-apply` with `field_sources`), then run the
//! queued apply job → sidecar rewrite job → scoped rescan job through
//! their worker entry points. Provider details come from the metadata
//! cache, so nothing reaches a real provider.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::Utc;
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{field_provenance, issue, metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::de::DeserializeOwned;
use serde_json::json;
use server::library::scanner;
use server::metadata::cache::{self, CacheEntity};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::GenericMetadata;
use server::metadata::writers::{self, SetBy};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use uuid::Uuid;

const OLD_SUMMARY: &str = "'JUST DESSERTS,' Part Four. The issue opens up with a flashback.";
const NEW_SUMMARY: &str = "Cibopath showdown!";

// ───────── fixtures ─────────

fn comicinfo(title: &str, summary: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>{title}</Title>
  <Series>Chew</Series>
  <Number>14</Number>
  <Year>2010</Year>
  <Month>9</Month>
  <Summary>{summary}</Summary>
  <Writer>John Layman</Writer>
  <Publisher>Image</Publisher>
</ComicInfo>"#
    )
}

/// A CBZ with one PNG-signed page (the archive crate content-sniffs
/// pages) and a ComicInfo.xml. `marker` keeps the bytes distinct so a
/// rewrite of the same XML still changes the file.
fn write_cbz(path: &Path, comic_info: &str, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(comic_info.as_bytes()).unwrap();
    zw.finish().unwrap();
}

fn archive_entry(path: &Path, name: &str) -> String {
    let f = std::fs::File::open(path).unwrap();
    let mut zr = zip::ZipArchive::new(f).unwrap();
    let mut entry = zr.by_name(name).unwrap();
    let mut s = String::new();
    entry.read_to_string(&mut s).unwrap();
    s
}

fn archive_summary(path: &Path) -> Option<String> {
    let xml = archive_entry(path, "ComicInfo.xml");
    parsers::comicinfo::parse(xml.as_bytes())
        .ok()
        .and_then(|ci| ci.summary)
}

fn archive_title(path: &Path) -> Option<String> {
    let xml = archive_entry(path, "ComicInfo.xml");
    parsers::comicinfo::parse(xml.as_bytes())
        .ok()
        .and_then(|ci| ci.title)
}

fn cv_detail(description: &str) -> GenericMetadata {
    GenericMetadata {
        title: Some("Just Desserts, Part 4".into()),
        issue_number: Some("14".into()),
        description: Some(description.into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "cv14",
            "issue",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("cv14".into()),
        ..Default::default()
    }
}

fn metron_detail(description: &str) -> GenericMetadata {
    GenericMetadata {
        title: Some("Just Desserts Part Four".into()),
        issue_number: Some("14".into()),
        description: Some(description.into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::Metron,
            "m14",
            "issue",
        )],
        source_provider: Some(Source::Metron),
        source_external_id: Some("m14".into()),
        ..Default::default()
    }
}

struct Fixture {
    app: TestApp,
    _dir: tempfile::TempDir,
    lib_id: Uuid,
    issue_id: String,
    archive: PathBuf,
    admin: (String, String),
}

/// Writeback (or not) library holding `Chew (2009)/Chew 014.cbz`, first
/// ingested by a real scan so provenance starts file-tier, exactly as on
/// the owner's install.
async fn fixture(writeback: bool) -> Fixture {
    let app = TestApp::spawn_with_providers("cv-key", "metron-user", "metron-pass").await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Chew (2009)");
    std::fs::create_dir_all(&folder).unwrap();
    let archive = folder.join("Chew 014.cbz");
    write_cbz(&archive, &comicinfo("Just Desserts", OLD_SUMMARY), 1);
    let mut seed = LibrarySeed::new(dir.path());
    if writeback {
        seed = seed.with_sidecar_writeback();
    }
    let lib_id = seed.insert(&app.state().db).await;
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    let row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.summary.as_deref(), Some(OLD_SUMMARY));
    let admin = register_admin(&app).await;
    Fixture {
        app,
        _dir: dir,
        lib_id,
        issue_id: row.id,
        archive,
        admin,
    }
}

async fn register_admin(app: &TestApp) -> (String, String) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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
        .map(|c| c.split(';').next().unwrap_or("").to_owned())
        .collect();
    let csrf = cookies
        .iter()
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .expect("csrf cookie")
        .to_owned();
    (cookies.join("; "), csrf)
}

/// A completed issue-scope search run with a ComicVine (ordinal 0) and a
/// Metron (ordinal 1) candidate whose details sit in the provider cache.
async fn seed_run(f: &Fixture, cv_desc: &str, metron_desc: &str) -> Uuid {
    let db = &f.app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(f.issue_id.clone())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into(), "metron".into()]),
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
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    for (ordinal, source, ext) in [(0, "comicvine", "cv14"), (1, "metron", "m14")] {
        metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(ordinal),
            source: Set(source.into()),
            external_id: Set(ext.into()),
            bucket: Set("high".into()),
            score: Set(90.0),
            score_breakdown: Set(json!({})),
            candidate: Set(json!({
                "source": source,
                "external_id": ext,
                "external_url": null,
                "issue_number": "14",
                "name": "Chew #14",
                "cover_date": null,
                "series_name": "Chew",
                "series_year": 2009,
                "series_external_id": null,
                "cover_image_url": null,
            })),
            applied_at: Set(None),
        }
        .insert(db)
        .await
        .unwrap();
    }
    cache::put(
        db,
        Source::ComicVine,
        CacheEntity::Issue,
        "cv14",
        &cv_detail(cv_desc),
    )
    .await
    .unwrap();
    cache::put(
        db,
        Source::Metron,
        CacheEntity::Issue,
        "m14",
        &metron_detail(metron_desc),
    )
    .await
    .unwrap();
    run_id
}

async fn post(f: &Fixture, path: &str, body: serde_json::Value) -> StatusCode {
    let (cookie, csrf) = &f.admin;
    let resp = f
        .app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    if !status.is_success() {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        panic!("{path} → {status}: {}", String::from_utf8_lossy(&bytes));
    }
    status
}

async fn issue_row(f: &Fixture) -> issue::Model {
    issue::Entity::find_by_id(f.issue_id.clone())
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap()
}

/// Pop every job queued on an apalis Redis storage (decoded from its
/// `{namespace}:data` hash, the `Request { args, parts }` JSON) and clear
/// the hash so a later drain only sees newer jobs.
async fn drain<T: DeserializeOwned>(app: &TestApp, data_hash: String) -> Vec<T> {
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    all.values()
        .map(|blob| {
            let v: serde_json::Value = serde_json::from_str(blob).expect("request json");
            serde_json::from_value(v["args"].clone()).expect("job args")
        })
        .collect()
}

/// Run the pipeline the workers would: queued apply job(s) → sidecar
/// rewrite job(s) (which queue the scoped rescan and then record the
/// apply's provenance) → the scoped rescan job(s).
async fn run_pipeline(f: &Fixture) {
    let state = f.app.state();
    let data = || apalis::prelude::Data::new(f.app.state());
    let apply_hash = state
        .jobs
        .metadata_apply_issue_storage
        .get_config()
        .job_data_hash();
    for job in drain::<server::jobs::metadata_apply::ApplyIssueJob>(&f.app, apply_hash).await {
        server::jobs::metadata_apply::handle_issue(job, data())
            .await
            .unwrap();
    }
    let rewrite_hash = state
        .jobs
        .rewrite_issue_sidecars_storage
        .get_config()
        .job_data_hash();
    let rewrites =
        drain::<server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob>(&f.app, rewrite_hash)
            .await;
    assert_eq!(rewrites.len(), 1, "one sidecar rewrite queued");
    for job in rewrites {
        server::jobs::rewrite_sidecars::handle(job, data())
            .await
            .unwrap();
    }
    let scan_hash = state.jobs.scan_series_storage.get_config().job_data_hash();
    let scans = drain::<server::jobs::scan_series::Job>(&f.app, scan_hash).await;
    assert_eq!(scans.len(), 1, "the rewrite queued one scoped rescan");
    for job in scans {
        assert!(
            job.force,
            "post-rewrite rescan bypasses the size+mtime skip"
        );
        server::jobs::scan_series::handle(job, data())
            .await
            .unwrap();
    }
}

async fn apply_single(f: &Fixture, run_id: Uuid, ordinal: i32, selected: &[&str]) {
    let row = issue_row(f).await;
    post(
        f,
        &format!(
            "/api/series/{}/issues/{}/metadata/apply",
            row.series_id, row.slug
        ),
        json!({
            "run_id": run_id,
            "ordinal": ordinal,
            "mode": "fill_missing",
            "apply_cover": false,
            "cover_overwrite_policy": "when_missing",
            "override_user_edits": false,
            "selected_fields": selected,
            "override_external_id_sources": [],
        }),
    )
    .await;
}

async fn apply_composite(f: &Fixture, run_id: Uuid, field_sources: serde_json::Value) {
    let row = issue_row(f).await;
    post(
        f,
        &format!(
            "/api/series/{}/issues/{}/metadata/composite-apply",
            row.series_id, row.slug
        ),
        json!({
            "run_id": run_id,
            "field_sources": field_sources,
            "included": [0, 1],
            "mode": "fill_missing",
            "apply_cover": false,
            "cover_overwrite_policy": "when_missing",
            "override_user_edits": false,
            "override_external_id_sources": [],
        }),
    )
    .await;
}

async fn provenance(f: &Fixture, field: &str) -> Option<String> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(&f.issue_id))
        .filter(field_provenance::Column::Field.eq(field))
        .one(&f.app.state().db)
        .await
        .unwrap()
        .map(|r| r.set_by)
}

// ───────── tests ─────────

/// The owner's exact flow, single-candidate preview: tick the
/// description row and apply. The archive gets the new `<Summary>` and
/// so must `issues.summary` — then again on a second apply (the owner
/// applied twice; the second run sees the first's provider provenance).
#[tokio::test]
async fn selected_description_reaches_the_db_in_a_writeback_library() {
    let f = fixture(true).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;

    apply_single(&f, run_id, 0, &["description"]).await;
    run_pipeline(&f).await;

    assert_eq!(
        archive_summary(&f.archive).as_deref(),
        Some(NEW_SUMMARY),
        "the archive carries the picked description"
    );
    let row = issue_row(&f).await;
    assert_eq!(
        row.summary.as_deref(),
        Some(NEW_SUMMARY),
        "the scoped rescan must ingest the description Folio just wrote"
    );
    assert_eq!(
        provenance(&f, "description").await.as_deref(),
        Some("comicvine")
    );

    // Second apply of a different description: provenance is already
    // provider-tier before this rescan even starts.
    let run2 = seed_run(&f, "Second pick.", "unused").await;
    apply_single(&f, run2, 0, &["description"]).await;
    run_pipeline(&f).await;
    assert_eq!(archive_summary(&f.archive).as_deref(), Some("Second pick."));
    assert_eq!(issue_row(&f).await.summary.as_deref(), Some("Second pick."));
}

/// Once Folio has written a provider value into the archive, the archive
/// is canonical for it: a later external retag of that field is ingested
/// like any other file change (only user pins outrank the file).
#[tokio::test]
async fn retag_after_writeback_apply_follows_the_archive() {
    let f = fixture(true).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;
    apply_single(&f, run_id, 0, &["description"]).await;
    run_pipeline(&f).await;
    assert_eq!(issue_row(&f).await.summary.as_deref(), Some(NEW_SUMMARY));

    write_cbz(&f.archive, &comicinfo("Just Desserts", "Retagged."), 9);
    scanner::scan_library_with(&f.app.state(), f.lib_id, true)
        .await
        .unwrap();
    assert_eq!(issue_row(&f).await.summary.as_deref(), Some("Retagged."));
}

/// A field the user left unticked in the single-candidate preview is not
/// written into the archive (the sidecar path used to compose the whole
/// provider payload regardless of `selected_fields`).
#[tokio::test]
async fn unselected_fields_stay_out_of_the_archive() {
    let f = fixture(true).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;

    apply_single(&f, run_id, 0, &["description"]).await;
    run_pipeline(&f).await;

    assert_eq!(
        archive_title(&f.archive).as_deref(),
        Some("Just Desserts"),
        "title was not selected; the file keeps its own"
    );
    let row = issue_row(&f).await;
    assert_eq!(row.title.as_deref(), Some("Just Desserts"));
    assert_eq!(row.summary.as_deref(), Some(NEW_SUMMARY));
}

/// Compare mode: the description comes from Metron while ComicVine is
/// the primary candidate (it supplies the title).
#[tokio::test]
async fn composite_description_from_second_provider_reaches_the_db() {
    let f = fixture(true).await;
    let run_id = seed_run(&f, "ComicVine's description.", NEW_SUMMARY).await;

    apply_composite(
        &f,
        run_id,
        json!([
            {"field": "description", "ordinal": 1},
            {"field": "title", "ordinal": 0},
        ]),
    )
    .await;
    run_pipeline(&f).await;

    assert_eq!(archive_summary(&f.archive).as_deref(), Some(NEW_SUMMARY));
    let row = issue_row(&f).await;
    assert_eq!(row.summary.as_deref(), Some(NEW_SUMMARY));
    assert_eq!(row.title.as_deref(), Some("Just Desserts, Part 4"));
    assert_eq!(
        provenance(&f, "description").await.as_deref(),
        Some("metron")
    );
    assert_eq!(provenance(&f, "title").await.as_deref(), Some("comicvine"));
}

/// User > provider > file still holds in a writeback library: a pinned
/// description is neither replaced by an apply nor by an external retag.
#[tokio::test]
async fn user_pinned_description_survives_apply_and_retag() {
    let f = fixture(true).await;
    let db = &f.app.state().db;
    let row = issue_row(&f).await;
    let mut am: issue::ActiveModel = row.into();
    am.summary = Set(Some("My own words.".into()));
    am.update(db).await.unwrap();
    writers::write_field_provenance(
        db,
        "issue",
        &f.issue_id,
        server::metadata::MetadataField::Description,
        SetBy::User,
        None,
    )
    .await
    .unwrap();

    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;
    apply_single(&f, run_id, 0, &["description"]).await;
    run_pipeline(&f).await;

    assert_eq!(
        archive_summary(&f.archive).as_deref(),
        Some("My own words."),
        "composer keeps the user's value"
    );
    assert_eq!(
        issue_row(&f).await.summary.as_deref(),
        Some("My own words.")
    );
    assert_eq!(provenance(&f, "description").await.as_deref(), Some("user"));

    // An external tool retags the archive with something else.
    write_cbz(&f.archive, &comicinfo("Just Desserts", "Retagged."), 9);
    scanner::scan_library_with(&f.app.state(), f.lib_id, true)
        .await
        .unwrap();
    assert_eq!(
        issue_row(&f).await.summary.as_deref(),
        Some("My own words."),
        "a file value never replaces a user pin"
    );
}

/// A provider value the archive does NOT carry (applied DB-direct — e.g.
/// a CBR the sidecar path refused, or before writeback was switched on)
/// still survives a rescan in a writeback library: WP-2.5 protection
/// only yields once Folio has written the XML.
#[tokio::test]
async fn writeback_library_keeps_provider_value_the_archive_never_received() {
    let f = fixture(true).await;
    let db = &f.app.state().db;
    let row = issue_row(&f).await;
    let mut am: issue::ActiveModel = row.into();
    am.summary = Set(Some("DB-direct provider summary.".into()));
    am.update(db).await.unwrap();
    writers::write_field_provenance(
        db,
        "issue",
        &f.issue_id,
        server::metadata::MetadataField::Description,
        SetBy::Provider(Source::ComicVine),
        None,
    )
    .await
    .unwrap();

    write_cbz(&f.archive, &comicinfo("Just Desserts", "Retagged."), 9);
    scanner::scan_library_with(&f.app.state(), f.lib_id, true)
        .await
        .unwrap();
    assert_eq!(
        issue_row(&f).await.summary.as_deref(),
        Some("DB-direct provider summary."),
        "never sidecar-rewritten → the provider value is protected"
    );
}

/// Non-writeback libraries keep WP-2.5 exactly: a provider-set summary
/// survives a retag + force rescan even when the issue carries a sidecar
/// rewrite stamp from a time the library had writeback on.
#[tokio::test]
async fn non_writeback_library_keeps_wp25_protection() {
    let f = fixture(false).await;
    let db = &f.app.state().db;
    writers::write_field_provenance(
        db,
        "issue",
        &f.issue_id,
        server::metadata::MetadataField::Description,
        SetBy::Provider(Source::ComicVine),
        None,
    )
    .await
    .unwrap();
    let row = issue_row(&f).await;
    let mut am: issue::ActiveModel = row.into();
    am.summary = Set(Some("Provider summary.".into()));
    // Stamp *after* the provenance row: under the writeback rule this
    // would mean "the archive carries it" — it must not matter here.
    am.last_sidecar_rewrite_at = Set(Some(Utc::now().fixed_offset()));
    am.update(db).await.unwrap();

    write_cbz(&f.archive, &comicinfo("New file title", "Retagged."), 9);
    scanner::scan_library_with(&f.app.state(), f.lib_id, true)
        .await
        .unwrap();
    let row = issue_row(&f).await;
    assert_eq!(row.summary.as_deref(), Some("Provider summary."));
    assert_eq!(
        row.title.as_deref(),
        Some("New file title"),
        "file-tier columns still refresh"
    );
}

/// `selected_fields` contract shared by both apply paths: ABSENT means
/// "apply everything" (one-click apply, legacy clients); an EMPTY list
/// means the user unticked every row and applies nothing. The web's
/// one-click apply used to send `[]`, which the DB-direct gate (rightly)
/// read as "nothing", so one-click silently applied nothing outside
/// writeback libraries.
async fn apply_single_with(f: &Fixture, run_id: Uuid, selected: Option<&[&str]>, mode: &str) {
    let row = issue_row(f).await;
    let mut body = json!({
        "run_id": run_id,
        "ordinal": 0,
        "mode": mode,
        "apply_cover": false,
        "cover_overwrite_policy": "when_missing",
        "override_user_edits": false,
        "override_external_id_sources": [],
    });
    if let Some(sel) = selected {
        body["selected_fields"] = json!(sel);
    }
    post(
        f,
        &format!(
            "/api/series/{}/issues/{}/metadata/apply",
            row.series_id, row.slug
        ),
        body,
    )
    .await;
}

/// DB-direct libraries: the apply job writes the columns itself (no
/// sidecar rewrite, no rescan), so only the apply queue is drained.
async fn run_apply_only(f: &Fixture) {
    let state = f.app.state();
    let apply_hash = state
        .jobs
        .metadata_apply_issue_storage
        .get_config()
        .job_data_hash();
    for job in drain::<server::jobs::metadata_apply::ApplyIssueJob>(&f.app, apply_hash).await {
        server::jobs::metadata_apply::handle_issue(job, apalis::prelude::Data::new(f.app.state()))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn empty_selection_writes_nothing_in_writeback_library() {
    let f = fixture(true).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;

    apply_single_with(&f, run_id, Some(&[]), "replace_all").await;
    run_pipeline(&f).await;

    assert_eq!(
        archive_summary(&f.archive).as_deref(),
        Some(OLD_SUMMARY),
        "every row unticked: the archive keeps its own description"
    );
    assert_eq!(issue_row(&f).await.summary.as_deref(), Some(OLD_SUMMARY));
}

#[tokio::test]
async fn absent_selection_applies_everything_db_direct() {
    let f = fixture(false).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;

    // What the web's one-click apply sends: no `selected_fields` at all.
    apply_single_with(&f, run_id, None, "replace_all").await;
    run_apply_only(&f).await;

    assert_eq!(issue_row(&f).await.summary.as_deref(), Some(NEW_SUMMARY));
}

#[tokio::test]
async fn empty_selection_applies_nothing_db_direct() {
    let f = fixture(false).await;
    let run_id = seed_run(&f, NEW_SUMMARY, "unused").await;

    apply_single_with(&f, run_id, Some(&[]), "replace_all").await;
    run_apply_only(&f).await;

    assert_eq!(issue_row(&f).await.summary.as_deref(), Some(OLD_SUMMARY));
}
