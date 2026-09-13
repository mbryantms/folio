//! End-to-end apalis worker-loop test.
//!
//! Every other job test calls a handler directly (`post_scan::handle_thumbs(job,
//! Data(state))`), which proves the handler but not the loop around it: job
//! serialisation through `RedisStorage`, the `Monitor` + `WorkerBuilder`
//! registration in `JobRuntime::run`, worker pickup, and the `JobMetricsLayer`.
//! An `apalis` / `apalis-redis` / `redis` bump can break any of those while
//! every handler test stays green — which is why those crates sit behind a
//! coordinated human review in renovate.json. This test boots the real
//! `JobRuntime::run` against the harness Redis, enqueues through the same
//! producer the API uses, and waits for the side effect to land in Postgres.
//!
//! The job is a cover-thumbnail request on an issue whose file is not a valid
//! archive: the handler soft-fails and stamps `issues.thumbnails_error`, so
//! the assertion needs no image fixture and no encoder work.

mod common;

use std::time::Duration;

use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use entity::issue::Entity as IssueEntity;
use sea_orm::EntityTrait;
use server::jobs::JobRuntime;
use server::jobs::post_scan::{ThumbsJob, enqueue_thumb_job};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_loop_consumes_a_queued_job_and_writes_its_side_effect() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let db = &state.db;

    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = seed_series(db, lib, "Worker Loop").await;
    let issue_id = seed_issue(
        db,
        lib,
        series,
        &tmp.path().join("Worker Loop 001.cbz"),
        b"definitely-not-a-zip",
        1.0,
    )
    .await;

    let before = IssueEntity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .expect("seeded issue");
    assert!(
        before.thumbnails_error.is_none(),
        "precondition: no thumbnail error before the job runs"
    );

    // Boot the worker loop exactly as `app::serve` does (crates/server/src/app.rs),
    // on a second JobRuntime bound to the same Redis logical DB — storages are
    // namespaced by job type, so it sees what the state's producer pushes.
    let runtime = JobRuntime::new(&state.cfg().redis_url, db.clone())
        .await
        .expect("job runtime");
    let shutdown = CancellationToken::new();
    let monitor = tokio::spawn({
        let (state, shutdown) = (state.clone(), shutdown.clone());
        async move { runtime.run(state, shutdown).await }
    });

    // Producer path shared with the admin thumbnail endpoints + the scanner.
    assert!(
        enqueue_thumb_job(&state, ThumbsJob::cover(issue_id.clone())).await,
        "enqueue should accept the first request for this issue"
    );

    // Worker pickup is bounded by apalis-redis's poll interval (100 ms); the
    // budget is generous because CI runs the suite oversubscribed.
    let stamped = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let row = IssueEntity::find_by_id(issue_id.clone())
                .one(db)
                .await
                .unwrap()
                .expect("issue still present");
            if row.thumbnails_error.is_some() {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("worker never consumed the job: thumbnails_error stayed NULL for 60s");

    assert!(
        stamped.thumbnails_generated_at.is_some(),
        "handler stamps generated_at alongside the error"
    );

    // Stop the loop before TestApp drops: an orphaned worker would keep polling
    // a Redis index the pool hands to the next test.
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(30), monitor)
        .await
        .expect("monitor did not stop after cancellation")
        .expect("monitor task panicked");
}
