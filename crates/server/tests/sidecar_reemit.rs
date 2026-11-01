//! Audit 2026-11 A7 (D-6): library-wide sidecar re-emit. An admin starts a
//! run on a writeback library; the job walks the active issues, queues one
//! DB-composed `RewriteIssueSidecarsJob` per archive the sidecar path can
//! take (`skip_rescan = true`), skips and counts refused formats, and queues
//! one series-scoped rescan per series once its rewrites are done — never a
//! library scan. Every archive here lives in a test temp dir.

mod common;

use archive::ArchiveLimits;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{audit_log, library};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::jobs::sidecar_reemit::{self, TickOutcome};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        // Real PNG signature: readers drop image-named entries without one.
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        // An old-composer sidecar the re-emit must replace.
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(b"<?xml version=\"1.0\"?><ComicInfo><Title>Stale</Title></ComicInfo>")
            .unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn read_entry(path: &Path, name: &str) -> Option<String> {
    let mut a = archive::open(path, ArchiveLimits::default()).unwrap();
    a.read_entry_bytes(name)
        .ok()
        .map(|b| String::from_utf8(b).unwrap())
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "email": email,
                        "password": "correctly-horse-battery-staple",
                    })
                    .to_string(),
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
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", auth.csrf.clone())
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn start(app: &TestApp, auth: &Authed, lib: Uuid) -> (StatusCode, serde_json::Value) {
    send(
        app,
        auth,
        Method::POST,
        &format!("/api/libraries/{lib}/sidecar-reemit"),
    )
    .await
}

/// Number of jobs stored in a queue's data hash.
async fn hlen(app: &TestApp, data_hash: String) -> i64 {
    let mut conn = app.state().jobs.redis.clone();
    redis::cmd("HLEN")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap()
}

async fn library_scan_jobs(app: &TestApp) -> i64 {
    let hash = app.state().jobs.scan_storage.get_config().job_data_hash();
    hlen(app, hash).await
}

async fn scoped_scan_jobs(app: &TestApp) -> i64 {
    let hash = app
        .state()
        .jobs
        .scan_series_storage
        .get_config()
        .job_data_hash();
    hlen(app, hash).await
}

/// Decode and drop every queued rewrite job, without running them.
async fn take_queued_rewrites(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let data_hash = app
        .state()
        .jobs
        .rewrite_issue_sidecars_storage
        .get_config()
        .job_data_hash();
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
    let mut jobs: Vec<RewriteIssueSidecarsJob> = all
        .values()
        .map(|blob| serde_json::from_str(blob).expect("job args"))
        .collect();
    jobs.sort_by(|a, b| a.issue_id.cmp(&b.issue_id));
    jobs
}

async fn run_rewrite(app: &TestApp, job: &RewriteIssueSidecarsJob) {
    server::jobs::rewrite_sidecars::handle(
        job.clone(),
        apalis::prelude::Data::new(app.state()),
        apalis::prelude::Attempt::new_with_value(1),
    )
    .await
    .expect("rewrite job handle");
}

/// Decode, run and drop every queued rewrite job; returns them.
async fn run_queued_rewrites(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let jobs = take_queued_rewrites(app).await;
    for job in &jobs {
        run_rewrite(app, job).await;
    }
    jobs
}

/// Drive a run to its end: tick, run what it queued, repeat.
async fn drive(app: &TestApp, run_id: Uuid) -> Vec<RewriteIssueSidecarsJob> {
    let mut all = Vec::new();
    for _ in 0..50 {
        let outcome = sidecar_reemit::tick(&app.state(), run_id).await.unwrap();
        all.extend(run_queued_rewrites(app).await);
        if outcome == TickOutcome::Done {
            return all;
        }
    }
    panic!("run {run_id} did not finish");
}

struct Seeded {
    lib: Uuid,
    /// `(issue_id, path)` per CBZ, per series.
    series: Vec<(Uuid, Vec<(String, PathBuf)>)>,
    cbr: Option<(String, PathBuf)>,
}

/// A library (flags per arguments) with two series of CBZ issues and,
/// optionally, a CBR the sidecar path refuses (no conversion allowed).
async fn seed(
    app: &TestApp,
    dir: &Path,
    archive_writeback: bool,
    metadata_writeback: bool,
    with_cbr: bool,
) -> Seeded {
    let db = &app.state().db;
    let mut lib = LibrarySeed::new(dir);
    lib.allow_archive_writeback = archive_writeback;
    lib.metadata_writeback_enabled = metadata_writeback;
    let lib_id = lib.insert(db).await;
    let mut series = Vec::new();
    for (name, n) in [("Saga", 2usize), ("Paper Girls", 1)] {
        let series_id = SeriesSeed::new(lib_id, name).insert(db).await;
        let mut issues = Vec::new();
        for i in 1..=n {
            let path = dir.join(format!("{}-{i}.cbz", name.replace(' ', "-")));
            let id = IssueSeed::new(
                lib_id,
                series_id,
                &path,
                // Unique bytes per library: the issue id derives from them.
                &build_cbz_bytes(&format!("{lib_id}-{name}-{i}")),
                i as f64,
            )
            .with_title(format!("{name} Title {i}"))
            .insert(db)
            .await;
            issues.push((id, path));
        }
        series.push((series_id, issues));
    }
    let cbr = if with_cbr {
        let series_id = series[0].0;
        let path = dir.join("Saga-9.cbr");
        // Never opened: the extension alone is refused.
        let bytes = format!("Rar!\x1a\x07\x01\x00{lib_id}");
        let id = IssueSeed::new(lib_id, series_id, &path, bytes.as_bytes(), 9.0)
            .insert(db)
            .await;
        Some((id, path))
    } else {
        None
    };
    Seeded {
        lib: lib_id,
        series,
        cbr,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_unless_both_writeback_flags_are_on() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    for (archive, metadata) in [(false, false), (true, false), (false, true)] {
        let dir = tempdir().unwrap();
        let s = seed(&app, dir.path(), archive, metadata, false).await;
        let (status, body) = start(&app, &admin, s.lib).await;
        assert_eq!(status, StatusCode::CONFLICT, "{archive}/{metadata}: {body}");
        assert_eq!(body["error"]["code"], "writeback_disabled", "{body}");
    }
    assert_eq!(
        hlen(
            &app,
            app.state()
                .jobs
                .sidecar_reemit_storage
                .get_config()
                .job_data_hash()
        )
        .await,
        0,
        "no run queued"
    );
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.sidecars.reemit"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(rows.is_empty(), "a refusal is not audited as a run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_admin_gets_403() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let dir = tempdir().unwrap();
    let s = seed(&app, dir.path(), true, true, false).await;
    let (status, body) = start(&app, &user, s.lib).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(
        &app,
        &user,
        Method::GET,
        &format!("/api/libraries/{}/sidecar-reemit", s.lib),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_rewrites_every_archive_skips_refused_and_rescans_per_series() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let s = seed(&app, dir.path(), true, true, true).await;

    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"], "running");
    assert_eq!(body["total"], 4, "3 CBZ + 1 CBR: {body}");
    let run_id: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();

    // One run per library at a time.
    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "reemit_in_progress");

    // The audit row names the run.
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.sidecars.reemit"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].target_id.as_deref(),
        Some(s.lib.to_string().as_str())
    );
    assert_eq!(rows[0].payload["run_id"], run_id.to_string());
    assert_eq!(rows[0].payload["total"], 4);

    let jobs = drive(&app, run_id).await;

    // One rewrite per CBZ, composed from the database, no per-issue
    // rescan, nothing deferred, tagged with the run.
    assert_eq!(jobs.len(), 3, "{jobs:?}");
    for job in &jobs {
        assert!(job.skip_rescan);
        assert!(job.post_apply.is_none());
        assert_eq!(job.reemit.as_ref().map(|t| t.run_id), Some(run_id));
    }
    let cbr_id = s.cbr.as_ref().unwrap().0.clone();
    assert!(
        jobs.iter().all(|j| j.issue_id != cbr_id),
        "CBR never queued"
    );

    // Each archive now holds the current composer's output, not the stale
    // sidecar it shipped with.
    for (_, issues) in &s.series {
        for (_, path) in issues {
            let ci = read_entry(path, "ComicInfo.xml").expect("ComicInfo.xml");
            assert!(!ci.contains("<Title>Stale</Title>"), "{ci}");
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let (name, n) = stem.rsplit_once('-').unwrap();
            let want = format!("<Title>{} Title {n}</Title>", name.replace('-', " "));
            assert!(ci.contains(&want), "{want} in {ci}");
            let mi = read_entry(path, "MetronInfo.xml").expect("MetronInfo.xml");
            assert!(mi.contains("<MetronInfo"), "{mi}");
        }
    }

    // Counts: 3 rewritten, the CBR skipped as refused, nothing failed.
    let run = sidecar_reemit::load_run(&app.state(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(run.enqueued, 3);
    assert_eq!(run.rewritten, 3);
    assert_eq!(run.refused, 1);
    assert_eq!(run.failed, 0);
    assert_eq!(run.outstanding, 0);

    // One series-scoped rescan per series, never a library scan.
    assert_eq!(library_scan_jobs(&app).await, 0, "no unscoped library scan");
    assert_eq!(scoped_scan_jobs(&app).await, 2, "one rescan per series");

    // The library is free again, and the status endpoint reports the run.
    let (status, body) = send(
        &app,
        &admin,
        Method::GET,
        &format!("/api/libraries/{}/sidecar-reemit", s.lib),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["run_id"], run_id.to_string());
    assert_eq!(body["run"]["rewritten"], 3);
    assert_eq!(body["run"]["skipped_refused"], 1);

    // Background work lists it with its counts.
    let (status, body) = send(&app, &admin, Method::GET, "/api/admin/background-work").await;
    assert_eq!(status, StatusCode::OK);
    let runs = body["sidecar_reemits"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{body}");
    assert_eq!(runs[0]["run"]["status"], "completed");
    assert_eq!(runs[0]["run"]["skipped_refused"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ticks_never_queue_more_than_the_outstanding_cap() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Long Run").insert(db).await;
    let n = sidecar_reemit::MAX_OUTSTANDING as usize + 3;
    for i in 1..=n {
        let path = dir.path().join(format!("long-{i}.cbz"));
        IssueSeed::new(
            lib_id,
            series_id,
            &path,
            &build_cbz_bytes(&format!("l{i}")),
            i as f64,
        )
        .insert(db)
        .await;
    }
    let (status, body) = start(&app, &admin, lib_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();

    // Without the rewrites running, ticks stop at the cap.
    sidecar_reemit::tick(&app.state(), run_id).await.unwrap();
    let outcome = sidecar_reemit::tick(&app.state(), run_id).await.unwrap();
    assert_eq!(outcome, TickOutcome::Wait, "throttled at the cap");
    let queued = hlen(
        &app,
        app.state()
            .jobs
            .rewrite_issue_sidecars_storage
            .get_config()
            .job_data_hash(),
    )
    .await;
    assert_eq!(queued, sidecar_reemit::MAX_OUTSTANDING);

    // The series' rescan waits for its last rewrite.
    assert_eq!(scoped_scan_jobs(&app).await, 0);
    let jobs = drive(&app, run_id).await;
    assert_eq!(jobs.len(), n, "every issue rewritten exactly once");
    assert_eq!(scoped_scan_jobs(&app).await, 1, "one rescan for the series");
    assert_eq!(library_scan_jobs(&app).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_writeback_off_aborts_the_run() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let s = seed(&app, dir.path(), true, true, false).await;
    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();

    let lib = library::Entity::find_by_id(s.lib)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: library::ActiveModel = lib.into();
    am.metadata_writeback_enabled = Set(false);
    am.update(&app.state().db).await.unwrap();

    let outcome = sidecar_reemit::tick(&app.state(), run_id).await.unwrap();
    assert_eq!(outcome, TickOutcome::Done);
    let run = sidecar_reemit::load_run(&app.state(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "aborted");
    assert_eq!(run.enqueued, 0);
    // Nothing was queued, nothing touched.
    assert!(run_queued_rewrites(&app).await.is_empty());
    for (_, issues) in &s.series {
        for (_, path) in issues {
            let ci = read_entry(path, "ComicInfo.xml").unwrap();
            assert!(ci.contains("<Title>Stale</Title>"), "{ci}");
        }
    }
}

/// An operator Retry of a dead-lettered rewrite reports a second time: the
/// count moves from failed to rewritten instead of being counted twice, and
/// the series still gets its rescan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_report_for_the_same_issue_moves_the_count() {
    use server::jobs::sidecar_reemit::{RewriteEnd, on_rewrite_finished};

    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let s = seed(&app, dir.path(), true, true, false).await;
    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run_id: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();

    assert_eq!(
        sidecar_reemit::tick(&app.state(), run_id).await.unwrap(),
        TickOutcome::Wait,
        "the whole library fits one tick; the run drains"
    );
    let jobs = take_queued_rewrites(&app).await;
    assert_eq!(jobs.len(), 3);
    // The first Saga rewrite gives up for good (its last attempt).
    let saga = s.series[0].0;
    let failed = jobs
        .iter()
        .find(|j| j.reemit.as_ref().unwrap().series_id == saga)
        .unwrap()
        .clone();
    on_rewrite_finished(
        &app.state(),
        failed.reemit.as_ref().unwrap(),
        &failed.issue_id,
        RewriteEnd::Failed,
    )
    .await;
    for job in jobs.iter().filter(|j| j.issue_id != failed.issue_id) {
        run_rewrite(&app, job).await;
    }
    let run = sidecar_reemit::load_run(&app.state(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((run.rewritten, run.failed, run.outstanding), (2, 1, 0));
    assert_eq!(scoped_scan_jobs(&app).await, 2, "both series rescanned");

    // The run completes with the failure on record...
    assert_eq!(
        sidecar_reemit::tick(&app.state(), run_id).await.unwrap(),
        TickOutcome::Done
    );
    // ...then the operator retries the dead job and it lands.
    run_rewrite(&app, &failed).await;
    let run = sidecar_reemit::load_run(&app.state(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(
        (run.rewritten, run.failed),
        (3, 0),
        "moved, not double counted"
    );
    // Running it once more changes nothing.
    run_rewrite(&app, &failed).await;
    let run = sidecar_reemit::load_run(&app.state(), run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((run.rewritten, run.failed), (3, 0));
    assert_eq!(library_scan_jobs(&app).await, 0);
}

/// A run whose tick chain died reads as stalled once its library claim
/// lapses, and a new start supersedes it instead of answering 409 forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_run_is_superseded_by_a_new_start() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let dir = tempdir().unwrap();
    let s = seed(&app, dir.path(), true, true, false).await;
    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let old: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();

    // The claim lapses (no tick for ACTIVE_TTL_SECS).
    let mut conn = app.state().jobs.redis.clone();
    let _: i64 = redis::cmd("DEL")
        .arg(format!("sidecar_reemit:active:{}", s.lib))
        .query_async(&mut conn)
        .await
        .unwrap();
    let (status, body) = send(
        &app,
        &admin,
        Method::GET,
        &format!("/api/libraries/{}/sidecar-reemit", s.lib),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["stalled"], true, "{body}");

    let (status, body) = start(&app, &admin, s.lib).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let new: Uuid = body["run_id"].as_str().unwrap().parse().unwrap();
    assert_ne!(old, new);
    let stale = sidecar_reemit::load_run(&app.state(), old)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale.status, "aborted");
    // A leftover tick of the old run stops instead of taking the claim back.
    assert_eq!(
        sidecar_reemit::tick(&app.state(), old).await.unwrap(),
        TickOutcome::Done
    );
    let (_, body) = send(
        &app,
        &admin,
        Method::GET,
        &format!("/api/libraries/{}/sidecar-reemit", s.lib),
    )
    .await;
    assert_eq!(body["run"]["run_id"], new.to_string());
    assert_eq!(body["run"]["stalled"], false);
}
