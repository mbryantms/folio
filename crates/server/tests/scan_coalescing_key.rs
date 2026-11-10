//! The per-library scan coalescing key is a lease (R-03).
//!
//! Real Redis through the harness. Pins:
//! - concurrent claims: exactly one wins, every caller reports its id;
//! - a run whose key expired and was claimed by another run does not delete
//!   the new holder's key on release;
//! - a running scan's lease keeps the key alive past its lifetime, and a
//!   holder that stops refreshing (a dead process) frees the library;
//! - a scan whose lease is lost stops through the cancel path and closes as
//!   `cancelled`; a job whose key is held by another run never scans.
//!
//! Lease lifetimes are shortened through `ScanLeaseTiming` so expiry is
//! observable in milliseconds.

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use common::TestApp;
use common::seed::seed_library;
use entity::scan_run::{Entity as ScanRunEntity, ScanRunState};
use redis::AsyncCommands;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use server::jobs::scan_lease::{self, LeaseCheck, ScanLease, ScanLeaseTiming};
use server::jobs::{CoalesceOutcome, ReleaseOutcome};
use server::library::events::ScanEvent;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

const SHORT: ScanLeaseTiming = ScanLeaseTiming {
    ttl: Duration::from_millis(400),
    refresh: Duration::from_millis(100),
};

fn in_flight_key(lib: Uuid) -> String {
    format!("scan:in_flight:{lib}")
}

fn write_minimal_cbz(path: &Path, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&unique_marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

/// A library with one series folder holding one archive.
async fn library_with_one_issue(app: &TestApp, tmp: &Path) -> Uuid {
    let folder = tmp.join("Lease Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Lease 001.cbz"), 7);
    seed_library(&app.state().db, tmp).await
}

async fn issue_count(app: &TestApp, lib: Uuid) -> u64 {
    entity::issue::Entity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib))
        .count(&app.state().db)
        .await
        .unwrap()
}

async fn run_row(app: &TestApp, id: Uuid) -> entity::scan_run::Model {
    ScanRunEntity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("scan run row")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_exactly_one_wins() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&app.state().db, tmp.path()).await;
    let state = app.state();

    let outcomes = futures::future::join_all((0..16).map(|_| {
        let jobs = state.jobs.clone();
        tokio::spawn(async move { jobs.coalesce_scan(lib, false).await.unwrap() })
    }))
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect::<Vec<_>>();

    let winners: Vec<Uuid> = outcomes
        .iter()
        .filter_map(|o| match o {
            CoalesceOutcome::Enqueued { scan_id } => Some(*scan_id),
            CoalesceOutcome::Coalesced { .. } => None,
        })
        .collect();
    assert_eq!(winners.len(), 1, "exactly one claim wins: {outcomes:?}");
    let winner = winners[0];
    assert!(
        outcomes.iter().all(|o| o.scan_id() == winner),
        "every coalesced caller reports the winner's id: {outcomes:?}"
    );
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(winner));
    let runs = ScanRunEntity::find()
        .filter(entity::scan_run::Column::LibraryId.eq(lib))
        .count(&state.db)
        .await
        .unwrap();
    assert_eq!(runs, 1, "one queued run, not one per caller");
}

#[tokio::test]
async fn stale_release_leaves_the_new_holders_key() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&app.state().db, tmp.path()).await;
    let state = app.state();
    let mut conn = state.jobs.redis.clone();

    let first = state.jobs.coalesce_scan(lib, false).await.unwrap();
    assert!(!first.was_coalesced());
    // The first run's key lapses mid-scan...
    let _: () = conn.pexpire(in_flight_key(lib), 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    // ...and a second trigger claims the library.
    let second = state.jobs.coalesce_scan(lib, false).await.unwrap();
    assert!(!second.was_coalesced(), "a lapsed key is free to claim");
    assert_ne!(second.scan_id(), first.scan_id());

    // The first run finishing must not delete the second run's key.
    let out = state.jobs.release_scan(lib, first.scan_id()).await.unwrap();
    assert_eq!(out, ReleaseOutcome::NotHolder);
    assert_eq!(
        state.jobs.scan_in_flight(lib).await.unwrap(),
        Some(second.scan_id())
    );
    // A trigger now still coalesces into the second run.
    let third = state.jobs.coalesce_scan(lib, false).await.unwrap();
    assert!(third.was_coalesced());
    assert_eq!(third.scan_id(), second.scan_id());

    // The holder's release does remove it (and hands over to the follow-up
    // the third trigger queued).
    let out = state
        .jobs
        .release_scan(lib, second.scan_id())
        .await
        .unwrap();
    let ReleaseOutcome::HandedOver { scan_id: follow_up } = out else {
        panic!("expected a hand-over to the queued follow-up, got {out:?}");
    };
    assert_eq!(
        state.jobs.scan_in_flight(lib).await.unwrap(),
        Some(follow_up)
    );
    assert_eq!(
        state.jobs.release_scan(lib, follow_up).await.unwrap(),
        ReleaseOutcome::Released
    );
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), None);
}

#[tokio::test]
async fn lease_keeps_the_key_alive_and_a_dead_holder_frees_it() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&app.state().db, tmp.path()).await;
    let state = app.state();
    let mut conn = state.jobs.redis.clone();

    let claim = state.jobs.coalesce_scan(lib, false).await.unwrap();
    let token = claim.scan_id();
    // Queued: the long queued lifetime.
    let queued_pttl: i64 = conn.pttl(in_flight_key(lib)).await.unwrap();
    assert!(
        queued_pttl > SHORT.ttl.as_millis() as i64 * 10,
        "a queued claim carries the queued lifetime: {queued_pttl}"
    );

    let (lease, first) = ScanLease::start(state.jobs.redis.clone(), lib, token, SHORT).await;
    assert_eq!(first, Some(LeaseCheck::Held));
    let running_pttl: i64 = conn.pttl(in_flight_key(lib)).await.unwrap();
    assert!(
        running_pttl > 0 && running_pttl <= SHORT.ttl.as_millis() as i64,
        "a running scan's key carries the lease lifetime: {running_pttl}"
    );

    // Three lifetimes later the refresh has kept it.
    tokio::time::sleep(SHORT.ttl * 3).await;
    assert!(!lease.is_lost());
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(token));

    // The holder stops refreshing without releasing, as a dead process
    // would. The key expires within one lifetime and a new scan can start.
    lease.stop().await;
    tokio::time::sleep(SHORT.ttl + Duration::from_millis(200)).await;
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), None);
    let next = state.jobs.coalesce_scan(lib, false).await.unwrap();
    assert!(!next.was_coalesced());
    assert_ne!(next.scan_id(), token);
}

#[tokio::test]
async fn extend_takes_back_a_free_key_and_refuses_another_holders() {
    let app = TestApp::spawn().await;
    let lib = Uuid::now_v7();
    let mut conn = app.state().jobs.redis.clone();
    let mine = Uuid::now_v7();

    // Free key: taken back with this token.
    let check = scan_lease::extend(&mut conn, lib, mine, SHORT.ttl)
        .await
        .unwrap();
    assert_eq!(check, LeaseCheck::Reclaimed);
    let held: Option<String> = conn.get(in_flight_key(lib)).await.unwrap();
    assert_eq!(held.as_deref(), Some(mine.to_string().as_str()));
    assert_eq!(
        scan_lease::extend(&mut conn, lib, mine, SHORT.ttl)
            .await
            .unwrap(),
        LeaseCheck::Held
    );

    // Another token holds it: not extended, not overwritten.
    let other = Uuid::now_v7().to_string();
    let _: () = conn.set(in_flight_key(lib), &other).await.unwrap();
    assert_eq!(
        scan_lease::extend(&mut conn, lib, mine, SHORT.ttl)
            .await
            .unwrap(),
        LeaseCheck::Lost
    );
    let held: Option<String> = conn.get(in_flight_key(lib)).await.unwrap();
    assert_eq!(held, Some(other));
    let pttl: i64 = conn.pttl(in_flight_key(lib)).await.unwrap();
    assert_eq!(pttl, -1, "another holder's key keeps its own expiry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_that_lost_its_key_stops_as_cancelled() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();
    let mut conn = state.jobs.redis.clone();

    let claim = state.jobs.coalesce_scan(lib, false).await.unwrap();
    let token = claim.scan_id();
    let (lease, first) = ScanLease::start(state.jobs.redis.clone(), lib, token, SHORT).await;
    assert_eq!(first, Some(LeaseCheck::Held));

    // Another run takes the library (its key lapsed and was re-claimed).
    let other = Uuid::now_v7();
    let _: () = conn
        .set(in_flight_key(lib), other.to_string())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !lease.is_lost() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refresh must notice the new holder"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let stats = server::library::scanner::scan_library_leased(
        &state,
        lib,
        false,
        Some(token),
        Some(lease.lost_flag()),
    )
    .await
    .expect("a superseded scan stops cleanly");
    lease.stop().await;
    assert_eq!(stats.files_added, 0);
    assert_eq!(issue_count(&app, lib).await, 0, "no folder was processed");
    let row = run_row(&app, token).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert_eq!(
        row.error.as_deref(),
        Some(server::jobs::scan::SUPERSEDED_REASON)
    );

    // Its release leaves the new holder's key.
    assert_eq!(
        state.jobs.release_scan(lib, token).await.unwrap(),
        ReleaseOutcome::NotHolder
    );
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(other));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_whose_key_another_run_holds_does_not_scan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();
    let mut conn = state.jobs.redis.clone();

    let claim = state.jobs.coalesce_scan(lib, false).await.unwrap();
    let token = claim.scan_id();
    let other = Uuid::now_v7();
    let _: () = conn
        .set(in_flight_key(lib), other.to_string())
        .await
        .unwrap();

    server::jobs::scan::run(
        &state,
        server::jobs::scan::Job {
            library_id: lib,
            scan_run_id: token,
            force: false,
            scope: None,
        },
        SHORT,
    )
    .await;

    assert_eq!(issue_count(&app, lib).await, 0);
    let row = run_row(&app, token).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert_eq!(
        row.error.as_deref(),
        Some(server::jobs::scan::SUPERSEDED_REASON)
    );
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(other));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_holds_its_key_while_scanning_and_releases_it() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();

    let claim = state.jobs.coalesce_scan(lib, false).await.unwrap();
    let token = claim.scan_id();
    server::jobs::scan::run(
        &state,
        server::jobs::scan::Job {
            library_id: lib,
            scan_run_id: token,
            force: false,
            scope: None,
        },
        SHORT,
    )
    .await;

    assert_eq!(run_row(&app, token).await.state, ScanRunState::Complete);
    assert_eq!(issue_count(&app, lib).await, 1);
    assert_eq!(
        state.jobs.scan_in_flight(lib).await.unwrap(),
        None,
        "the holder released its key"
    );
    // The lease task is gone: nothing re-creates the key afterwards.
    tokio::time::sleep(SHORT.refresh * 3).await;
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), None);
}

// ───────────── superseded runs, failed enqueues, cancel ─────────────

async fn redis_flag(app: &TestApp, key: String) -> Option<String> {
    let mut conn = app.state().jobs.redis.clone();
    conn.get(key).await.unwrap()
}

/// The queued follow-up recorded for `lib`: (queued, forced, scoped).
async fn follow_up(app: &TestApp, lib: Uuid) -> (bool, bool, bool) {
    (
        redis_flag(app, format!("scan:queued:{lib}"))
            .await
            .is_some(),
        redis_flag(app, format!("scan:queued:{lib}:force"))
            .await
            .is_some(),
        redis_flag(app, format!("scan:queued:{lib}:scoped"))
            .await
            .is_some(),
    )
}

/// Make a scoped, non-forced watcher scan the holder of `lib`'s key, as if
/// the previous holder's key had lapsed. Returns its run id.
async fn scoped_holder_takes_over(app: &TestApp, lib: Uuid) -> Uuid {
    let state = app.state();
    let mut conn = state.jobs.redis.clone();
    let _: () = conn.del(in_flight_key(lib)).await.unwrap();
    let b = state
        .jobs
        .coalesce_watch_scan(lib, vec!["/elsewhere".into()])
        .await
        .unwrap();
    assert!(!b.was_coalesced());
    b.scan_id()
}

fn forced_full_job(lib: Uuid, run: Uuid) -> server::jobs::scan::Job {
    server::jobs::scan::Job {
        library_id: lib,
        scan_run_id: run,
        force: true,
        scope: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_before_start_hands_on_a_forced_full_request() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();

    let a = state.jobs.coalesce_scan(lib, true).await.unwrap().scan_id();
    let b = scoped_holder_takes_over(&app, lib).await;
    assert_eq!(follow_up(&app, lib).await, (false, false, false));

    server::jobs::scan::run(&state, forced_full_job(lib, a), SHORT).await;

    let row = run_row(&app, a).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert_eq!(
        row.error.as_deref(),
        Some(server::jobs::scan::SUPERSEDED_REASON)
    );
    assert_eq!(issue_count(&app, lib).await, 0);
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(b));
    assert_eq!(
        follow_up(&app, lib).await,
        (true, true, false),
        "the holder's follow-up is full and forced"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn superseded_mid_scan_stops_before_reconcile_and_hands_on_its_request() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let state = app.state();
    // A series that the first scan finds and that is then deleted: a run
    // that reconciles would mark it removed.
    let gone = tmp.path().join("Gone Series (2020)");
    std::fs::create_dir_all(&gone).unwrap();
    write_minimal_cbz(&gone.join("Gone 001.cbz"), 1);
    let lib = seed_library(&state.db, tmp.path()).await;
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    // Enough new folders that phase 3 outlasts a lease refresh.
    for i in 0..120u32 {
        let folder = tmp.path().join(format!("Bulk {i:03} (2024)"));
        std::fs::create_dir_all(&folder).unwrap();
        write_minimal_cbz(&folder.join(format!("Bulk {i:03} 001.cbz")), 1000 + i);
    }
    let baseline = issue_count(&app, lib).await;

    let a = state.jobs.coalesce_scan(lib, true).await.unwrap().scan_id();
    let scan = tokio::spawn({
        let state = state.clone();
        async move { server::jobs::scan::run(&state, forced_full_job(lib, a), SHORT).await }
    });
    // Wait until phase 3 has written its first new issue, then let a
    // scoped watcher scan take the library.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while issue_count(&app, lib).await <= baseline {
        assert!(
            tokio::time::Instant::now() < deadline,
            "phase 3 never started"
        );
        assert!(
            !scan.is_finished(),
            "the scan finished before phase 3 was seen"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let b = scoped_holder_takes_over(&app, lib).await;
    scan.await.unwrap();

    let row = run_row(&app, a).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert_eq!(
        row.error.as_deref(),
        Some(server::jobs::scan::SUPERSEDED_REASON)
    );
    assert!(
        issue_count(&app, lib).await < baseline + 120,
        "the superseded scan stopped starting folders"
    );
    let gone_series = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(lib))
        .filter(entity::series::Column::Name.contains("Gone"))
        .one(&state.db)
        .await
        .unwrap()
        .expect("gone series row");
    assert!(
        gone_series.removed_at.is_none(),
        "a superseded scan must not reconcile"
    );
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(b));
    assert_eq!(follow_up(&app, lib).await, (true, true, false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_redelivered_job_closes_its_running_row() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();

    let a = state
        .jobs
        .coalesce_scan(lib, false)
        .await
        .unwrap()
        .scan_id();
    // The process died mid-scan: the row is `running`; another trigger
    // claimed the library before the job was re-delivered.
    let mut am: entity::scan_run::ActiveModel = run_row(&app, a).await.into();
    am.state = Set(ScanRunState::Running);
    am.update(&state.db).await.unwrap();
    let b = scoped_holder_takes_over(&app, lib).await;
    let mut events = state.events.subscribe();

    server::jobs::scan::run(
        &state,
        server::jobs::scan::Job {
            library_id: lib,
            scan_run_id: a,
            force: false,
            scope: None,
        },
        SHORT,
    )
    .await;

    let row = run_row(&app, a).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert!(row.ended_at.is_some());
    assert_eq!(
        row.error.as_deref(),
        Some(server::jobs::scan::SUPERSEDED_REASON)
    );
    let mut terminal = false;
    while let Ok(ev) = events.try_recv() {
        if let ScanEvent::Failed { scan_id, .. } = ev
            && scan_id == a
        {
            terminal = true;
        }
    }
    assert!(terminal, "the closed run emits its terminal event");
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), Some(b));
}

#[tokio::test]
async fn claim_is_undone_when_the_run_cannot_be_queued() {
    let app = TestApp::spawn().await;
    let state = app.state();
    // No such library: the run row insert fails its foreign key.
    let lib = Uuid::now_v7();
    assert!(state.jobs.coalesce_scan(lib, false).await.is_err());
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), None);
}

#[tokio::test]
async fn hand_over_enqueue_failure_frees_the_key_and_keeps_the_follow_up() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&app.state().db, tmp.path()).await;
    let state = app.state();

    let a = state
        .jobs
        .coalesce_scan(lib, false)
        .await
        .unwrap()
        .scan_id();
    assert!(
        state
            .jobs
            .coalesce_scan(lib, true)
            .await
            .unwrap()
            .was_coalesced()
    );
    assert_eq!(follow_up(&app, lib).await, (true, true, false));
    // The follow-up's run row cannot be inserted once the library is gone.
    entity::library::Entity::delete_by_id(lib)
        .exec(&state.db)
        .await
        .unwrap();

    assert!(state.jobs.release_scan(lib, a).await.is_err());
    assert_eq!(
        state.jobs.scan_in_flight(lib).await.unwrap(),
        None,
        "key freed"
    );
    assert_eq!(
        follow_up(&app, lib).await,
        (true, true, false),
        "the follow-up stays recorded for the next holder"
    );
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
                    r#"{"email":"lease-admin@example.com","password":"correctly-horse-battery"}"#,
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
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let extract = |needle: &str| -> String {
        cookies
            .iter()
            .find_map(|c| c.split(';').next()?.strip_prefix(needle).map(str::to_owned))
            .expect(needle)
    };
    let _ = to_bytes(resp.into_body(), usize::MAX).await;
    (
        extract("__Host-comic_session="),
        extract("__Host-comic_csrf="),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_queued_run_releases_its_claim() {
    let app = TestApp::spawn().await;
    let (session, csrf) = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = library_with_one_issue(&app, tmp.path()).await;
    let state = app.state();
    let slug = entity::library::Entity::find_by_id(lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .slug;

    let a = state
        .jobs
        .coalesce_scan(lib, false)
        .await
        .unwrap()
        .scan_id();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/libraries/{slug}/scan-runs/{a}/cancel"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .header("X-CSRF-Token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(state.jobs.scan_in_flight(lib).await.unwrap(), None);

    // A new scan claims the library at once.
    let next = state.jobs.coalesce_scan(lib, false).await.unwrap();
    assert!(!next.was_coalesced());

    // The cancelled job, picked up later, does nothing.
    server::jobs::scan::run(
        &state,
        server::jobs::scan::Job {
            library_id: lib,
            scan_run_id: a,
            force: false,
            scope: None,
        },
        SHORT,
    )
    .await;
    let row = run_row(&app, a).await;
    assert_eq!(row.state, ScanRunState::Cancelled);
    assert_eq!(row.error.as_deref(), Some("Cancelled by admin"));
    assert_eq!(issue_count(&app, lib).await, 0);
    assert_eq!(
        state.jobs.scan_in_flight(lib).await.unwrap(),
        Some(next.scan_id())
    );
    assert_eq!(follow_up(&app, lib).await, (false, false, false));
}
