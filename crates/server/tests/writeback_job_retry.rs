//! Audit 2026-11 N-M7: the two writeback jobs no longer swallow failures.
//!
//! - `rewrite_sidecars::handle` returns `Err` for a transient failure (here
//!   an unwritable archive directory) so apalis retries it under
//!   `JOB_MAX_ATTEMPTS`; on the last budgeted attempt it marks the
//!   triggering metadata run `failed` and writes an error-severity library
//!   event before the budget wrapper dead-letters the task. A deliberate
//!   refusal (`sidecar_refusal`) still returns `Ok` and is never retried.
//! - `metadata_apply::handle_{series,issue}` requeue themselves with the
//!   shared busy-lock backoff instead of returning `Ok` when the per-entity
//!   apply mutex is held, and mark the run failed once that budget is spent.
//!
//! Every test writes only to a temp dir; no archive on disk is touched.

mod common;

use apalis::prelude::{AbortError, Attempt, Data};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{library_event, metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::de::DeserializeOwned;
use serde_json::json;
use server::jobs::busy_requeue::BUSY_MAX_ATTEMPTS;
use server::jobs::metadata_apply::{
    ApplyIssueJob, ApplySeriesJob, CoverPolicy, handle_issue, handle_series, release_issue_mutex,
    try_claim_issue_mutex, try_claim_series_mutex,
};
use server::jobs::rewrite_sidecars::{self, RewriteIssueSidecarsJob};
use server::jobs::{JOB_MAX_ATTEMPTS, with_retry_budget_attempt};
use server::metadata::apply::ApplyMode;
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;
use wiremock::MockServer;

// ───────── fixtures ─────────

/// A one-page stored CBZ with a real PNG signature so the archive crate
/// keeps the entry as a page.
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

fn cbr_fixture() -> std::path::PathBuf {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page.cbr");
    assert!(p.is_file(), "fixtures/synthetic-3page.cbr is committed");
    p
}

/// A sidecar job as the issue apply enqueues it — tied to a metadata run.
fn sidecar_job(issue_id: &str, run_id: Uuid) -> RewriteIssueSidecarsJob {
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
        triggering_run_id: Some(run_id),
        triggering_run_ordinal: Some(0),
        skip_rescan: false,
        attempt: 0,
        post_apply: None,
    }
}

/// A completed search run with one high-confidence ComicVine candidate,
/// the state an apply job finds. `scope` is `"issue"` or `"series"`.
async fn seed_run(
    app: &TestApp,
    scope: &str,
    entity_id: &str,
    candidate: serde_json::Value,
) -> Uuid {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set(scope.into()),
        scope_entity_id: Set(Some(entity_id.into())),
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
        candidate: Set(candidate),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

/// The provider detail the apply reads from `metadata_cache` — the search
/// already paid for it, so the apply never opens a socket.
fn stub_issue_payload() -> server::metadata::provider::GenericMetadata {
    use server::metadata::identifier::{Identifier, Source};
    server::metadata::provider::GenericMetadata {
        title: Some("Chapter One".into()),
        issue_number: Some("1".into()),
        description: Some("Provider summary.".into()),
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

async fn run_row(app: &TestApp, run_id: Uuid) -> metadata_run::Model {
    metadata_run::Entity::find_by_id(run_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("run row")
}

async fn events_for(app: &TestApp, entity_id: &str) -> Vec<library_event::Model> {
    library_event::Entity::find()
        .filter(library_event::Column::EntityId.eq(entity_id))
        .filter(library_event::Column::Action.eq("errored"))
        .all(&app.state().db)
        .await
        .unwrap()
}

/// Every job on an apalis storage, decoded from its `{namespace}:data`
/// hash (the `Request { args, parts }` JSON), then the hash is cleared so
/// a later call only sees newer jobs.
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
        .map(|blob| serde_json::from_str(blob).expect("job args"))
        .collect()
}

fn rewrite_hash(app: &TestApp) -> String {
    app.state()
        .jobs
        .rewrite_issue_sidecars_storage
        .get_config()
        .job_data_hash()
}

fn apply_issue_hash(app: &TestApp) -> String {
    app.state()
        .jobs
        .metadata_apply_issue_storage
        .get_config()
        .job_data_hash()
}

fn apply_series_hash(app: &TestApp) -> String {
    app.state()
        .jobs
        .metadata_apply_series_storage
        .get_config()
        .job_data_hash()
}

/// Makes a directory read-only for the test's duration so the rewrite's
/// staging file cannot be created (the transient-I/O class: a NAS gone
/// read-only, a full disk). Restored on drop so the temp dir can be
/// deleted. Panics when the process can write anyway (root): a test that
/// cannot simulate its fault must not pass vacuously — run the suite as an
/// ordinary user, as CI does.
struct ReadOnlyDir<'a>(&'a Path);

impl<'a> ReadOnlyDir<'a> {
    fn new(dir: &'a Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = dir.join(".probe");
        if std::fs::File::create(&probe).is_ok() {
            let _ = std::fs::remove_file(&probe);
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            panic!(
                "this process can create files in a 0o555 directory (running as root?); \
                 the transient-failure tests need an unprivileged user to simulate the fault"
            );
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

struct WritebackIssue {
    app: TestApp,
    dir: tempfile::TempDir,
    cbz: std::path::PathBuf,
    issue_id: String,
    run_id: Uuid,
}

async fn writeback_issue() -> WritebackIssue {
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
    let run_id = seed_run(&app, "issue", &issue_id, json!({"kind": "issue"})).await;
    WritebackIssue {
        app,
        dir,
        cbz,
        issue_id,
        run_id,
    }
}

// ───────── rewrite job: transient failures ─────────

/// A rewrite that fails for a transient reason is an `Err` apalis retries;
/// the run is not failed yet, the event is a warning, and the next attempt
/// succeeds once the fault clears. Pre-fix the handler returned `Ok`, so
/// the first attempt was also the last and the write was gone.
#[tokio::test]
async fn transient_rewrite_failure_is_retried_and_succeeds_once_the_fault_clears() {
    let f = writeback_issue().await;
    let before = std::fs::read(&f.cbz).unwrap();

    let guard = ReadOnlyDir::new(f.dir.path());
    let first = rewrite_sidecars::handle(
        sidecar_job(&f.issue_id, f.run_id),
        Data::new(f.app.state()),
        Attempt::new_with_value(1),
    )
    .await;
    assert!(
        first.is_err(),
        "an unwritable directory is a transient failure: Err for apalis"
    );
    assert_eq!(std::fs::read(&f.cbz).unwrap(), before, "original untouched");
    assert_eq!(
        run_row(&f.app, f.run_id).await.status,
        "completed",
        "the run is not failed while retries remain"
    );
    let events = events_for(&f.app, &f.issue_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].severity, "warning",
        "a retried attempt is a warning"
    );
    assert!(
        events[0].summary.contains("retrying"),
        "summary says it will retry: {}",
        events[0].summary
    );

    // The fault clears (the NAS comes back): the next attempt lands.
    drop(guard);
    rewrite_sidecars::handle(
        sidecar_job(&f.issue_id, f.run_id),
        Data::new(f.app.state()),
        Attempt::new_with_value(2),
    )
    .await
    .expect("retry succeeds once the directory is writable");
    assert_ne!(std::fs::read(&f.cbz).unwrap(), before, "archive rewritten");
    let mut a = archive::open(&f.cbz, archive::ArchiveLimits::default()).unwrap();
    assert!(
        String::from_utf8(a.read_entry_bytes("ComicInfo.xml").unwrap())
            .unwrap()
            .contains("<Title>Rewritten</Title>")
    );
    assert_eq!(run_row(&f.app, f.run_id).await.status, "completed");
}

/// The last budgeted attempt still fails: the handler marks the triggering
/// run `failed` with the error, writes an error-severity event, and the
/// budget wrapper turns the `Err` into an `AbortError` so apalis
/// dead-letters the task (visible on Background work) instead of silently
/// completing it. When the operator's Retry of the dead job then lands,
/// the run is `completed` again with the stale error cleared.
#[tokio::test]
async fn rewrite_failure_on_the_last_attempt_marks_the_run_failed_and_aborts() {
    let f = writeback_issue().await;
    let before = std::fs::read(&f.cbz).unwrap();
    let guard = ReadOnlyDir::new(f.dir.path());

    let mut worker = with_retry_budget_attempt(rewrite_sidecars::handle);
    let err = worker(
        sidecar_job(&f.issue_id, f.run_id),
        Data::new(f.app.state()),
        Attempt::new_with_value(JOB_MAX_ATTEMPTS),
    )
    .await
    .expect_err("the last attempt still fails");
    assert!(
        err.downcast_ref::<AbortError>().is_some(),
        "last budgeted failure is killed, not rescheduled: {err}"
    );
    assert_eq!(std::fs::read(&f.cbz).unwrap(), before, "original untouched");

    let run = run_row(&f.app, f.run_id).await;
    assert_eq!(run.status, "failed", "run marked failed: {run:?}");
    let summary = run.error_summary.expect("error_summary set");
    assert!(
        summary.contains(&format!("after {JOB_MAX_ATTEMPTS} attempts")),
        "{summary}"
    );
    assert!(run.finished_at.is_some());

    let events = events_for(&f.app, &f.issue_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].severity, "error");
    assert_eq!(events[0].category, "archive");
    assert!(
        events[0]
            .summary
            .contains(&format!("after {JOB_MAX_ATTEMPTS} attempts")),
        "{}",
        events[0].summary
    );
    let detail = events[0].detail.clone().expect("detail");
    assert_eq!(detail["triggering_run_id"], json!(f.run_id));

    // The share is back and the operator retries the dead job (Background
    // work pushes a fresh copy, so this is apalis attempt 1 again).
    drop(guard);
    worker(
        sidecar_job(&f.issue_id, f.run_id),
        Data::new(f.app.state()),
        Attempt::new_with_value(1),
    )
    .await
    .expect("the retried rewrite lands");
    assert_ne!(std::fs::read(&f.cbz).unwrap(), before, "archive rewritten");
    let run = run_row(&f.app, f.run_id).await;
    assert_eq!(run.status, "completed", "the run recovers: {run:?}");
    assert!(run.error_summary.is_none(), "stale error cleared");
}

/// A run whose `error_summary` the rewrite job did not write — a search
/// failure — is never flipped back to `completed` by a rewrite that lands.
#[tokio::test]
async fn successful_rewrite_leaves_a_search_failure_alone() {
    let f = writeback_issue().await;
    server::metadata::orchestrator::fail_run(&f.app.state().db, f.run_id, "provider error: 503")
        .await
        .unwrap();

    rewrite_sidecars::handle(
        sidecar_job(&f.issue_id, f.run_id),
        Data::new(f.app.state()),
        Attempt::new_with_value(1),
    )
    .await
    .expect("rewrite lands");
    let run = run_row(&f.app, f.run_id).await;
    assert_eq!(run.status, "failed");
    assert_eq!(run.error_summary.as_deref(), Some("provider error: 503"));
}

/// A `sidecar_refusal` (CBR in a library that does not allow conversion)
/// is an answer, not a fault: the handler returns `Ok`, apalis never
/// retries, nothing is requeued and the run is left alone.
#[tokio::test]
async fn sidecar_refusal_returns_ok_and_is_not_retried() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbr = dir.path().join("saga-1.cbr");
    let cbr_bytes = std::fs::read(cbr_fixture()).unwrap();
    let issue_id = IssueSeed::new(lib_id, series_id, &cbr, &cbr_bytes, 1.0)
        .insert(&app.state().db)
        .await;
    let run_id = seed_run(&app, "issue", &issue_id, json!({"kind": "issue"})).await;

    // Through the budget wrapper on the last attempt — the strongest
    // claim: even there a refusal completes the task.
    let mut worker = with_retry_budget_attempt(rewrite_sidecars::handle);
    worker(
        sidecar_job(&issue_id, run_id),
        Data::new(app.state()),
        Attempt::new_with_value(JOB_MAX_ATTEMPTS),
    )
    .await
    .expect("a refusal is Ok, never an Err to retry");

    assert_eq!(std::fs::read(&cbr).unwrap(), cbr_bytes, "file untouched");
    assert!(!cbr.with_extension("cbz").exists(), "no conversion");
    let queued = drain::<RewriteIssueSidecarsJob>(&app, rewrite_hash(&app)).await;
    assert!(queued.is_empty(), "nothing requeued: {queued:?}");
    assert_eq!(
        run_row(&app, run_id).await.status,
        "completed",
        "a refusal does not fail the run (the dispatch gate reports it)"
    );
    let events = events_for(&app, &issue_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].severity, "error");
    assert!(
        events[0].summary.contains("refused"),
        "{}",
        events[0].summary
    );
}

/// The busy-lock requeue budget is spent: the write is dropped loudly and,
/// since the job came from a metadata run, that run is marked failed.
#[tokio::test]
async fn rewrite_busy_lock_budget_exhausted_marks_the_run_failed() {
    use server::archive_rewrite::mutex;
    let f = writeback_issue().await;
    let before = std::fs::read(&f.cbz).unwrap();
    let mut redis = f.app.state().jobs.redis.clone();
    let token = mutex::try_claim(&mut redis, &f.issue_id, mutex::EDIT_TTL_SECS)
        .await
        .unwrap()
        .expect("lock claimed");

    let mut job = sidecar_job(&f.issue_id, f.run_id);
    job.attempt = BUSY_MAX_ATTEMPTS;
    rewrite_sidecars::handle(job, Data::new(f.app.state()), Attempt::new_with_value(1))
        .await
        .expect("a spent busy budget completes the task");
    mutex::release(&mut redis, &f.issue_id, &token).await;

    assert_eq!(std::fs::read(&f.cbz).unwrap(), before);
    assert!(
        drain::<RewriteIssueSidecarsJob>(&f.app, rewrite_hash(&f.app))
            .await
            .is_empty(),
        "no further requeue"
    );
    let run = run_row(&f.app, f.run_id).await;
    assert_eq!(run.status, "failed");
    assert!(
        run.error_summary
            .as_deref()
            .is_some_and(|s| s.contains("busy")),
        "{run:?}"
    );
    let events = events_for(&f.app, &f.issue_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].severity, "error");
    assert!(events[0].summary.contains("lock busy"));
}

// ───────── apply job: busy mutex, apply error ─────────

/// An `ApplyError` from the apply itself (here: the job names an ordinal
/// the run has no candidate for) marks the run `failed` with the error.
/// Pre-fix only the WS event fired and the run stayed `completed` with
/// nothing applied.
#[tokio::test]
async fn apply_error_marks_the_run_failed() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;
    let run_id = seed_run(&app, "issue", &issue_id, json!({"kind": "issue"})).await;

    let mut job = issue_apply_job(run_id, &issue_id);
    job.ordinal = 7;
    handle_issue(job, Data::new(app.state()))
        .await
        .expect("an apply error is recorded, not retried");

    let run = run_row(&app, run_id).await;
    assert_eq!(run.status, "failed", "{run:?}");
    assert!(
        run.error_summary
            .as_deref()
            .is_some_and(|s| s.contains("candidate not found")),
        "{run:?}"
    );
    assert_eq!(run.items_applied, 0);
}

fn issue_apply_job(run_id: Uuid, issue_id: &str) -> ApplyIssueJob {
    ApplyIssueJob {
        run_id,
        ordinal: 0,
        issue_id: issue_id.to_owned(),
        mode: ApplyMode::FillMissing,
        apply_cover: false,
        cover_overwrite_policy: CoverPolicy::Never,
        override_user_edits: false,
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
        selected_fields: None,
        override_external_id_sources: Default::default(),
        is_auto: false,
        composite: None,
        attempt: 0,
    }
}

fn series_apply_job(run_id: Uuid, series_id: Uuid) -> ApplySeriesJob {
    ApplySeriesJob {
        run_id,
        ordinal: 0,
        series_id,
        mode: ApplyMode::FillMissing,
        apply_cover: false,
        cover_overwrite_policy: CoverPolicy::Never,
        override_user_edits: false,
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
        selected_fields: None,
        override_external_id_sources: Default::default(),
        is_auto: false,
        composite: None,
        bulk: false,
        attempt: 0,
    }
}

/// With the apply mutex held by another claimant the job requeues itself
/// (`attempt + 1`, same run and ordinal) instead of returning `Ok` and
/// vanishing; once the lock is released the requeued job applies the
/// candidate. The provider is never asked again: the job names the stored
/// candidate and the detail comes from the cache the search filled, so a
/// requeue costs no quota.
#[tokio::test]
async fn apply_job_requeues_while_the_mutex_is_held_and_completes_after_release() {
    let provider = MockServer::start().await;
    let app = TestApp::spawn_with_comicvine_at("k", provider.uri()).await;
    let dir = tempdir().unwrap();
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
        &stub_issue_payload(),
    )
    .await
    .unwrap();
    let run_id = seed_run(&app, "issue", &issue_id, json!({"kind": "issue"})).await;

    // Another worker holds the issue's apply mutex.
    assert!(
        try_claim_issue_mutex(&app.state(), &issue_id)
            .await
            .unwrap(),
        "test claims the lock"
    );

    handle_issue(issue_apply_job(run_id, &issue_id), Data::new(app.state()))
        .await
        .expect("a busy lock is not an error");

    let queued = drain::<ApplyIssueJob>(&app, apply_issue_hash(&app)).await;
    assert_eq!(queued.len(), 1, "the apply was requeued, not dropped");
    assert_eq!(queued[0].attempt, 1, "busy counter bumped");
    assert_eq!(queued[0].run_id, run_id, "same run");
    assert_eq!(queued[0].ordinal, 0, "same stored candidate");
    let run = run_row(&app, run_id).await;
    assert_eq!(run.status, "completed", "not failed while requeues remain");
    assert_eq!(run.items_applied, 0, "nothing applied yet");

    // Lock released: the requeued job runs and applies.
    release_issue_mutex(&app.state(), &issue_id).await;
    handle_issue(queued.into_iter().next().unwrap(), Data::new(app.state()))
        .await
        .expect("requeued apply completes");

    let run = run_row(&app, run_id).await;
    assert_eq!(run.items_applied, 1, "{run:?}");
    let cand = metadata_run_candidate::Entity::find()
        .filter(metadata_run_candidate::Column::RunId.eq(run_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(cand.applied_at.is_some(), "candidate stamped applied");
    let issue = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    // The seed leaves `summary` empty, so fill_missing writes the provider's.
    assert_eq!(issue.summary.as_deref(), Some("Provider summary."));
    assert!(
        provider.received_requests().await.unwrap().is_empty(),
        "no provider request across the requeue: the cached detail is reused"
    );
    assert!(
        drain::<ApplyIssueJob>(&app, apply_issue_hash(&app))
            .await
            .is_empty(),
        "nothing left on the queue"
    );
}

/// The busy budget is spent while the series mutex is still held: the run
/// is marked failed with a clear message and a `metadata.errored` library
/// event is written, so the batch view shows the dropped child instead of
/// a complete batch.
#[tokio::test]
async fn apply_job_with_an_exhausted_busy_budget_marks_the_run_failed() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let run_id = seed_run(
        &app,
        "series",
        &series_id.to_string(),
        json!({"kind": "series", "name": "Saga"}),
    )
    .await;

    assert!(
        try_claim_series_mutex(&app.state(), series_id)
            .await
            .unwrap()
    );
    let mut job = series_apply_job(run_id, series_id);
    job.attempt = BUSY_MAX_ATTEMPTS;
    handle_series(job, Data::new(app.state()))
        .await
        .expect("a spent busy budget completes the task");

    assert!(
        drain::<ApplySeriesJob>(&app, apply_series_hash(&app))
            .await
            .is_empty(),
        "no further requeue"
    );
    let run = run_row(&app, run_id).await;
    assert_eq!(run.status, "failed", "{run:?}");
    assert!(run.finished_at.is_some());
    let summary = run.error_summary.expect("error_summary set");
    assert!(
        summary.contains("lock still held") && summary.contains(&BUSY_MAX_ATTEMPTS.to_string()),
        "{summary}"
    );
    let events = events_for(&app, &series_id.to_string()).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].category, "metadata");
    assert_eq!(events[0].severity, "error");
    assert_eq!(events[0].entity_type.as_deref(), Some("series"));
    assert!(
        events[0].summary.contains("lock busy"),
        "{}",
        events[0].summary
    );
    assert_eq!(
        events[0]
            .detail
            .as_ref()
            .and_then(|d| d.get("run_id"))
            .cloned(),
        Some(json!(run_id))
    );
}
