//! Restarting the server inside apalis's orphan window.
//!
//! apalis-redis refuses to register a worker whose name it saw alive within
//! the orphan window ("worker is still active within threshold"), and the
//! monitor had no restart policy, so a server restarted a few seconds after
//! the previous one stopped came up with those workers dead: scans sat
//! "queued" until the next restart (seen in dev, 2026-11-04). `JobRuntime`
//! now releases its own stale registrations before the monitor starts.

mod common;

use std::time::Duration;

use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use entity::issue::Entity as IssueEntity;
use redis::AsyncCommands;
use sea_orm::EntityTrait;
use server::jobs::post_scan::{ThumbsJob, enqueue_thumb_job};
use server::jobs::{JobRuntime, storage_config};
use tokio_util::sync::CancellationToken;

const WORKER: &str = "post_scan_thumbs";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn redis(url: &str) -> redis::aio::MultiplexedConnection {
    redis::Client::open(url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_frees_the_name_and_requeues_held_jobs() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let cfg = storage_config::<ThumbsJob>();
    let mut conn = redis(&state.cfg().redis_url).await;

    // What a killed server leaves: a just-seen registration holding a job.
    let inflight = format!("{}:{WORKER}", cfg.inflight_jobs_set());
    let _: () = conn
        .zadd(cfg.workers_set(), WORKER, now_secs())
        .await
        .unwrap();
    let _: () = conn.sadd(&inflight, "held-task").await.unwrap();

    let runtime = JobRuntime::new(&state.cfg().redis_url, state.db.clone())
        .await
        .expect("job runtime");
    let requeued = runtime.release_stale_workers().await.unwrap();
    assert_eq!(requeued, 1);

    let score: Option<i64> = conn.zscore(cfg.workers_set(), WORKER).await.unwrap();
    assert_eq!(score, None, "registration released");
    let held: bool = conn.exists(&inflight).await.unwrap();
    assert!(!held, "in-flight set cleared");
    let active: Vec<String> = conn.lrange(cfg.active_jobs_list(), 0, -1).await.unwrap();
    assert_eq!(
        active,
        vec!["held-task".to_owned()],
        "job back on the queue"
    );

    // Idempotent: nothing left to release.
    assert_eq!(runtime.release_stale_workers().await.unwrap(), 0);
    let _: () = conn.del(cfg.active_jobs_list()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workers_start_despite_a_fresh_registration_from_a_killed_server() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let db = &state.db;
    let cfg = storage_config::<ThumbsJob>();

    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = seed_series(db, lib, "Restart").await;
    let issue_id = seed_issue(
        db,
        lib,
        series,
        &tmp.path().join("Restart 001.cbz"),
        b"definitely-not-a-zip-restart",
        1.0,
    )
    .await;

    // The previous server's worker was alive a moment ago.
    let mut conn = redis(&state.cfg().redis_url).await;
    let _: () = conn
        .zadd(cfg.workers_set(), WORKER, now_secs())
        .await
        .unwrap();

    let runtime = JobRuntime::new(&state.cfg().redis_url, db.clone())
        .await
        .expect("job runtime");
    let shutdown = CancellationToken::new();
    let monitor = tokio::spawn({
        let (state, shutdown) = (state.clone(), shutdown.clone());
        async move { runtime.run(state, shutdown).await }
    });

    assert!(enqueue_thumb_job(&state, ThumbsJob::cover(issue_id.clone())).await);
    // Well under the orphan window: without the release this worker never
    // registers and the job is never consumed.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = IssueEntity::find_by_id(issue_id.clone())
                .one(db)
                .await
                .unwrap()
                .expect("issue still present");
            if row.thumbnails_error.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("thumbnail worker never consumed the job after a quick restart");

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(30), monitor)
        .await
        .expect("monitor did not stop after cancellation")
        .expect("monitor task panicked");
}
