//! Audit 2026-11 N-M4 / N-M3 (WP A6): the two apply options the sidecar
//! (writeback) path used to ignore.
//!
//! - **`fill_missing`** (owner decision D-5): the composed sidecars keep
//!   every value the issue already has and take the provider's only where
//!   the issue is empty — scalars and junctions alike. Provenance is
//!   stamped for the filled fields only. `replace_all` stays provider-wins.
//!   A user pin wins in both modes.
//! - **`apply_cover`**: the provider's primary cover rides the rewrite
//!   job's deferred writes and becomes the active `issue_cover` primary
//!   (with its perceptual hashes) only once the rewrite landed; a failed
//!   rewrite writes no cover.
//!
//! Every test writes only to temp dirs; no real archive is touched.

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{field_provenance, issue_cover, metadata_run, metadata_run_candidate};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, Statement,
};
use serde_json::json;
use server::jobs::metadata_apply::apply_issue_inline;
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::{CreditCandidate, GenericMetadata};
use server::metadata::writers::CoverOverwritePolicy;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use uuid::Uuid;

// ───────── fixtures ─────────

/// One stored, PNG-signed page so the archive crate keeps it as a page.
fn build_cbz_bytes(label: &str) -> Vec<u8> {
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

/// A real, decodable PNG so `writers::apply_cover` computes the hashes.
fn cover_png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(48, 64, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 3) as u8, ((x + y) * 2) as u8])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// The ComicVine issue detail the apply reads from `metadata_cache`.
fn provider_payload() -> GenericMetadata {
    GenericMetadata {
        title: Some("Chapter One".into()),
        issue_number: Some("1A".into()),
        description: Some("Provider summary.".into()),
        credits: vec![
            CreditCandidate {
                name: "Brian K. Vaughan".into(),
                role: "Writer".into(),
                ordinal: None,
                identifiers: vec![],
            },
            CreditCandidate {
                name: "Fiona Staples".into(),
                role: "Penciller".into(),
                ordinal: None,
                identifiers: vec![],
            },
        ],
        genres: vec!["Science Fiction".into()],
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

fn args(run_id: Uuid, mode: ApplyMode) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal: 0,
        mode,
        apply_cover: false,
        cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
        override_user_edits: false,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

async fn seed_issue_run(app: &TestApp, issue_id: &str) -> Uuid {
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
    .insert(db)
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
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

struct Fixture {
    app: TestApp,
    dir: tempfile::TempDir,
    cbz: PathBuf,
    issue_id: String,
}

/// A writeback library holding one issue whose archive-derived row has a
/// title ("Issue 1"), a number ("1"), no summary, one writer credit in
/// `issue_credits` (+ its read-cache column) and no genres. The provider
/// detail is cached so the apply never opens a socket.
async fn fixture(payload: &GenericMetadata) -> Fixture {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(db)
        .await;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "INSERT INTO issue_credits (issue_id, role, person) VALUES ($1, 'writer', 'Jane Doe')",
        [issue_id.clone().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE issues SET writer = 'Jane Doe' WHERE id = $1",
        [issue_id.clone().into()],
    ))
    .await
    .unwrap();
    server::metadata::cache::put(
        db,
        Source::ComicVine,
        server::metadata::cache::CacheEntity::Issue,
        "67890",
        payload,
    )
    .await
    .unwrap();
    Fixture {
        app,
        dir,
        cbz,
        issue_id,
    }
}

async fn pin(app: &TestApp, issue_id: &str, field: &str) {
    field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.into()),
        field: Set(field.into()),
        set_by: Set("user".into()),
        source_external_id: Set(None),
        set_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// Every queued `RewriteIssueSidecarsJob`, with the queue cleared.
async fn take_rewrite_jobs(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: HashMap<String, String> = redis::cmd("HGETALL")
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
        .map(|blob| serde_json::from_str(blob).expect("job args"))
        .collect()
}

async fn run_job(
    app: &TestApp,
    job: RewriteIssueSidecarsJob,
    attempt: usize,
) -> Result<(), String> {
    server::jobs::rewrite_sidecars::handle(
        job,
        apalis::prelude::Data::new(app.state()),
        apalis::prelude::Attempt::new_with_value(attempt),
    )
    .await
    .map_err(|e| e.to_string())
}

/// Apply in `mode`, run the one rewrite job it queued, and return the
/// sidecars the archive now holds.
async fn apply_and_rewrite(
    f: &Fixture,
    mode: ApplyMode,
) -> (
    parsers::comicinfo::ComicInfo,
    parsers::metroninfo::MetronInfo,
) {
    let run_id = seed_issue_run(&f.app, &f.issue_id).await;
    let outcome = apply_issue_inline(&f.app.state(), &f.issue_id, args(run_id, mode))
        .await
        .expect("apply_issue");
    assert!(outcome.enqueued_rewrite, "writeback path");
    let jobs = take_rewrite_jobs(&f.app).await;
    assert_eq!(jobs.len(), 1);
    run_job(&f.app, jobs.into_iter().next().unwrap(), 1)
        .await
        .expect("rewrite lands");
    sidecars(&f.cbz)
}

fn sidecars(
    cbz: &Path,
) -> (
    parsers::comicinfo::ComicInfo,
    parsers::metroninfo::MetronInfo,
) {
    let mut a = archive::open(cbz, archive::ArchiveLimits::default()).unwrap();
    let ci = parsers::comicinfo::parse(&a.read_entry_bytes("ComicInfo.xml").unwrap()).unwrap();
    let mi = parsers::metroninfo::parse(&a.read_entry_bytes("MetronInfo.xml").unwrap()).unwrap();
    (ci, mi)
}

/// `field → set_by` for the issue's provenance rows.
async fn provenance(app: &TestApp, issue_id: &str) -> HashMap<String, String> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.field, r.set_by))
        .collect()
}

async fn active_primary(app: &TestApp, issue_id: &str) -> Option<issue_cover::Model> {
    issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq(issue_cover::IssueCoverKind::Primary))
        .filter(issue_cover::Column::Ordinal.eq(0))
        .filter(issue_cover::Column::IsActive.eq(true))
        .one(&app.state().db)
        .await
        .unwrap()
}

// ───────── fill_missing ─────────

/// Scalars: the archive's non-empty title and number survive, the empty
/// summary is filled. Provenance names the provider for the summary only.
#[tokio::test]
async fn fill_missing_keeps_archive_scalars_and_fills_empty_ones() {
    let f = fixture(&provider_payload()).await;
    let (ci, mi) = apply_and_rewrite(&f, ApplyMode::FillMissing).await;

    assert_eq!(ci.title.as_deref(), Some("Issue 1"), "non-empty title kept");
    assert_eq!(mi.title.as_deref(), Some("Issue 1"));
    assert_eq!(ci.number.as_deref(), Some("1"), "non-empty number kept");
    assert_eq!(mi.number.as_deref(), Some("1"));
    assert_eq!(
        ci.summary.as_deref(),
        Some("Provider summary."),
        "empty summary filled"
    );
    assert_eq!(mi.summary.as_deref(), Some("Provider summary."));

    let prov = provenance(&f.app, &f.issue_id).await;
    assert_eq!(
        prov.get("description").map(String::as_str),
        Some("comicvine")
    );
    for kept in ["title", "number"] {
        assert!(
            !prov.contains_key(kept),
            "{kept} kept from the archive must not be stamped provider-set: {prov:?}"
        );
    }
}

/// Junctions: existing credits survive whole (no provider penciller added),
/// empty genres are filled, and only the genres get a provider row.
#[tokio::test]
async fn fill_missing_keeps_existing_credits_and_fills_empty_genres() {
    let f = fixture(&provider_payload()).await;
    let (ci, mi) = apply_and_rewrite(&f, ApplyMode::FillMissing).await;

    assert_eq!(ci.writer.as_deref(), Some("Jane Doe"), "credits kept");
    assert_eq!(ci.penciller, None, "provider credits not merged in");
    let metron_names: Vec<&str> = mi.credits.values().flatten().map(String::as_str).collect();
    assert_eq!(metron_names, vec!["Jane Doe"], "{:?}", mi.credits);
    assert_eq!(
        ci.genre.as_deref(),
        Some("Science Fiction"),
        "genres filled"
    );
    assert_eq!(mi.genres, vec!["Science Fiction".to_owned()]);

    let prov = provenance(&f.app, &f.issue_id).await;
    assert_eq!(prov.get("genres").map(String::as_str), Some("comicvine"));
    assert!(
        !prov.contains_key("credits"),
        "kept credits must not be stamped provider-set: {prov:?}"
    );
}

/// `replace_all` is provider-wins, as before: every provider value lands
/// and is attributed to the provider.
#[tokio::test]
async fn replace_all_still_takes_every_provider_value() {
    let f = fixture(&provider_payload()).await;
    let (ci, mi) = apply_and_rewrite(&f, ApplyMode::ReplaceAll).await;

    assert_eq!(ci.title.as_deref(), Some("Chapter One"));
    assert_eq!(mi.title.as_deref(), Some("Chapter One"));
    assert_eq!(ci.number.as_deref(), Some("1A"));
    assert_eq!(ci.summary.as_deref(), Some("Provider summary."));
    assert_eq!(ci.writer.as_deref(), Some("Brian K. Vaughan"));
    assert_eq!(ci.penciller.as_deref(), Some("Fiona Staples"));
    assert_eq!(ci.genre.as_deref(), Some("Science Fiction"));

    let prov = provenance(&f.app, &f.issue_id).await;
    for field in ["title", "number", "description", "credits", "genres"] {
        assert_eq!(
            prov.get(field).map(String::as_str),
            Some("comicvine"),
            "{field} attributed to the provider: {prov:?}"
        );
    }
}

/// A user pin wins in both modes — including a pin on an *empty* field,
/// which fill-missing must not treat as a gap to fill. The unpinned,
/// non-empty number is the fill-missing control.
#[tokio::test]
async fn user_pins_are_never_overwritten_in_either_mode() {
    for mode in [ApplyMode::FillMissing, ApplyMode::ReplaceAll] {
        let f = fixture(&provider_payload()).await;
        pin(&f.app, &f.issue_id, "title").await;
        // The user cleared the summary on purpose.
        pin(&f.app, &f.issue_id, "summary").await;
        let (ci, mi) = apply_and_rewrite(&f, mode).await;

        assert_eq!(ci.title.as_deref(), Some("Issue 1"), "{mode:?}");
        assert_eq!(mi.title.as_deref(), Some("Issue 1"), "{mode:?}");
        assert_eq!(
            ci.summary, None,
            "pinned empty summary stays empty ({mode:?})"
        );
        assert_eq!(mi.summary, None, "{mode:?}");
        let expected_number = match mode {
            ApplyMode::FillMissing => "1",
            ApplyMode::ReplaceAll => "1A",
        };
        assert_eq!(ci.number.as_deref(), Some(expected_number), "{mode:?}");

        let prov = provenance(&f.app, &f.issue_id).await;
        assert_eq!(
            prov.get("title").map(String::as_str),
            Some("user"),
            "{mode:?}"
        );
        assert_eq!(
            prov.get("summary").map(String::as_str),
            Some("user"),
            "{mode:?}"
        );
        assert!(
            !prov.contains_key("description"),
            "the pinned summary gets no provider row ({mode:?}): {prov:?}"
        );
    }
}

/// Fill-missing keeps the archive's own `<Notes>` instead of replacing them
/// with the Folio audit line; provider-wins still writes the audit line. A
/// stale tagger trace is bookkeeping, not a value: fill-missing replaces it
/// too, so an old `[CVDB n]` never sits beside the re-matched ids.
#[tokio::test]
async fn fill_missing_keeps_existing_notes_but_not_a_stale_trace() {
    let own = "Scanned by Somebody";
    let trace = "Tagged with ComicTagger 1.3.5 on 2020-01-01. [CVDB111]";
    for (mode, notes, keeps) in [
        (ApplyMode::FillMissing, own, true),
        (ApplyMode::ReplaceAll, own, false),
        (ApplyMode::FillMissing, trace, false),
    ] {
        let f = fixture(&provider_payload()).await;
        let db = &f.app.state().db;
        db.execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "UPDATE issues SET notes = $2 WHERE id = $1",
            [f.issue_id.clone().into(), notes.into()],
        ))
        .await
        .unwrap();
        let (ci, _) = apply_and_rewrite(&f, mode).await;
        let written = ci.notes.unwrap_or_default();
        assert_eq!(written == notes, keeps, "{mode:?} {notes}: {written}");
        assert_eq!(
            written.contains("Tagged with Folio"),
            !keeps,
            "{mode:?} {notes}: {written}"
        );
        assert!(!written.contains("CVDB111"), "{mode:?}: {written}");
    }
}

// ───────── primary cover ─────────

fn cover_payload(url: &str) -> GenericMetadata {
    GenericMetadata {
        cover_image_url: Some(url.into()),
        ..provider_payload()
    }
}

async fn apply_with_cover(f: &Fixture) -> RewriteIssueSidecarsJob {
    let run_id = seed_issue_run(&f.app, &f.issue_id).await;
    let mut a = args(run_id, ApplyMode::FillMissing);
    a.apply_cover = true;
    let outcome = apply_issue_inline(&f.app.state(), &f.issue_id, a)
        .await
        .expect("apply_issue");
    assert!(outcome.cover_deferred, "cover rides the rewrite job");
    assert!(
        active_primary(&f.app, &f.issue_id).await.is_none(),
        "no cover before the rewrite runs"
    );
    let jobs = take_rewrite_jobs(&f.app).await;
    assert_eq!(jobs.len(), 1);
    jobs.into_iter().next().unwrap()
}

/// `apply_cover` on a writeback library: the provider cover becomes the
/// active primary `issue_cover` row, hashed, once the rewrite lands.
#[tokio::test]
async fn apply_cover_sets_the_primary_cover_after_the_rewrite() {
    let url = format!("https://covers.example.com/{}.png", Uuid::now_v7());
    server::metadata::writers::stub_cover_fetch_for_tests(&url, cover_png());
    let f = fixture(&cover_payload(&url)).await;

    let job = apply_with_cover(&f).await;
    run_job(&f.app, job, 1).await.expect("rewrite lands");

    let cover = active_primary(&f.app, &f.issue_id)
        .await
        .expect("primary cover written after the rewrite");
    assert_eq!(cover.source_url.as_deref(), Some(url.as_str()));
    assert_eq!(cover.source_provider.as_deref(), Some("comicvine"));
    assert_eq!(cover.source_external_id.as_deref(), Some("67890"));
    assert!(
        cover.phash.is_some() && cover.dhash.is_some() && cover.ahash.is_some(),
        "hashes computed at write time: {cover:?}"
    );
    let on_disk = f.app.state().cfg().data_path.join(&cover.local_path);
    assert_eq!(std::fs::read(on_disk).unwrap(), cover_png());
}

/// A rewrite that fails writes no cover; the retry that lands writes it.
#[tokio::test]
async fn apply_cover_is_not_written_when_the_rewrite_fails() {
    let url = format!("https://covers.example.com/{}.png", Uuid::now_v7());
    server::metadata::writers::stub_cover_fetch_for_tests(&url, cover_png());
    let f = fixture(&cover_payload(&url)).await;
    let job = apply_with_cover(&f).await;

    {
        let _guard = ReadOnlyDir::new(f.dir.path());
        let first = run_job(&f.app, job.clone(), 1).await;
        assert!(first.is_err(), "unwritable directory: transient failure");
    }
    assert!(
        active_primary(&f.app, &f.issue_id).await.is_none(),
        "a failed rewrite must not write the cover"
    );

    run_job(&f.app, job, 2).await.expect("retry lands");
    assert!(
        active_primary(&f.app, &f.issue_id).await.is_some(),
        "the landed retry writes the cover"
    );
}

/// Makes a directory read-only so the rewrite's staging file cannot be
/// created (same seam as `writeback_job_retry.rs`). Panics when the
/// process can write anyway (root), so the test never passes vacuously.
struct ReadOnlyDir<'a>(&'a Path);

impl<'a> ReadOnlyDir<'a> {
    fn new(dir: &'a Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = dir.join(".probe");
        if std::fs::File::create(&probe).is_ok() {
            let _ = std::fs::remove_file(&probe);
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            panic!("this process can create files in a 0o555 directory (running as root?)");
        }
        Self(dir)
    }
}

impl Drop for ReadOnlyDir<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
    }
}
