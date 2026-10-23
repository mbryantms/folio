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
//!
//! It also pins **pickup latency**. apalis 1.0 workers park after an empty
//! fetch and only wake on the heartbeat (30 s) unless a poll strategy is
//! attached; `JobRuntime` attaches `JOB_POLL_INTERVAL`. The second job here
//! is pushed once the worker has gone idle, which is exactly the parked case
//! — on prod (2026-10-23) every queue hop sat for up to 30 s and a metadata
//! apply took about a minute end to end.

mod common;

use std::time::Duration;

use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use entity::issue::Entity as IssueEntity;
use sea_orm::EntityTrait;
use server::jobs::post_scan::{ThumbsJob, enqueue_thumb_job};
use server::jobs::{JOB_POLL_INTERVAL, JobRuntime};
use tokio_util::sync::CancellationToken;

/// Upper bound on push → side-effect for one job. Generous for an
/// oversubscribed CI runner, but a fraction of the 30 s heartbeat so the
/// un-polled regression cannot pass.
const PICKUP_BUDGET: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_loop_consumes_a_queued_job_and_writes_its_side_effect() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let db = &state.db;

    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = seed_series(db, lib, "Worker Loop").await;
    let mut issue_ids = Vec::new();
    for n in 1..=2u32 {
        issue_ids.push(
            seed_issue(
                db,
                lib,
                series,
                &tmp.path().join(format!("Worker Loop 00{n}.cbz")),
                // Distinct bytes: the issue id is the content hash.
                format!("definitely-not-a-zip-{n}").as_bytes(),
                f64::from(n),
            )
            .await,
        );
    }
    let issue_id = issue_ids[0].clone();

    for id in &issue_ids {
        let before = IssueEntity::find_by_id(id.clone())
            .one(db)
            .await
            .unwrap()
            .expect("seeded issue");
        assert!(
            before.thumbnails_error.is_none(),
            "precondition: no thumbnail error before the job runs"
        );
    }

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

    // Worker pickup is bounded by `JOB_POLL_INTERVAL` (plus worker boot for
    // this first job); the budget is generous because CI runs the suite
    // oversubscribed, but far below the 30 s heartbeat.
    let wait_stamped = |id: String| async move {
        let started = std::time::Instant::now();
        let row = tokio::time::timeout(PICKUP_BUDGET, async {
            loop {
                let row = IssueEntity::find_by_id(id.clone())
                    .one(db)
                    .await
                    .unwrap()
                    .expect("issue still present");
                if row.thumbnails_error.is_some() {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "worker never consumed the job within {PICKUP_BUDGET:?}: \
                 thumbnails_error stayed NULL (poll interval {JOB_POLL_INTERVAL:?})"
            )
        });
        (row, started.elapsed())
    };
    let (stamped, _first_latency) = wait_stamped(issue_id.clone()).await;

    assert!(
        stamped.thumbnails_generated_at.is_some(),
        "handler stamps generated_at alongside the error"
    );

    // The parked case: the worker has drained the queue and gone idle. A
    // job pushed now must be fetched on the poll timer, not the heartbeat.
    tokio::time::sleep(JOB_POLL_INTERVAL * 3).await;
    assert!(
        enqueue_thumb_job(&state, ThumbsJob::cover(issue_ids[1].clone())).await,
        "enqueue should accept the request for the second issue"
    );
    let (_, idle_latency) = wait_stamped(issue_ids[1].clone()).await;
    assert!(
        idle_latency < PICKUP_BUDGET,
        "idle worker pickup took {idle_latency:?}"
    );

    // Stop the loop before TestApp drops: an orphaned worker would keep polling
    // a Redis index the pool hands to the next test.
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(30), monitor)
        .await
        .expect("monitor did not stop after cancellation")
        .expect("monitor task panicked");
}
