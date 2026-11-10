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

use common::TestApp;
use common::seed::seed_library;
use entity::scan_run::{Entity as ScanRunEntity, ScanRunState};
use redis::AsyncCommands;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use server::jobs::scan_lease::{self, LeaseCheck, ScanLease, ScanLeaseTiming};
use server::jobs::{CoalesceOutcome, ReleaseOutcome};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
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
