//! M7 of `metadata-sidecar-writeback-1.0`: writeback-rollout progress
//! metric.
//!
//! The Prometheus gauge `comic_metadata_writeback_libraries_remaining`
//! exposes how many libraries still have writeback disabled — operators
//! watch it tick toward zero before approving the follow-up cleanup PR
//! that drops the legacy DB-direct apply branch.
//!
//! These tests cover the underlying query
//! [`count_libraries_without_writeback`] directly (the gauge call site
//! is a thin wrapper around it; gauges are global state and awkward to
//! assert on in parallel tests).

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use server::metadata::writeback_progress::count_libraries_without_writeback;
use tempfile::tempdir;

#[tokio::test]
async fn count_excludes_libraries_with_writeback_enabled() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;

    // Two writeback-OFF libraries (default), one writeback-ON.
    let dir1 = tempdir().unwrap();
    let dir2 = tempdir().unwrap();
    let dir3 = tempdir().unwrap();
    LibrarySeed::new(dir1.path()).insert(db).await;
    LibrarySeed::new(dir2.path()).insert(db).await;
    LibrarySeed::new(dir3.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;

    let remaining = count_libraries_without_writeback(db).await.unwrap();
    assert_eq!(remaining, 2, "two off + one on → two remaining to flip");
}

#[tokio::test]
async fn count_is_zero_when_every_library_is_migrated() {
    // Empty starting point — every library that exists has writeback on.
    // The gauge hitting zero is the signal that gates the follow-up
    // cleanup PR (drop the legacy DB-direct apply branch).
    let app = TestApp::spawn().await;
    let db = &app.state().db;

    let dir = tempdir().unwrap();
    LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;

    let remaining = count_libraries_without_writeback(db).await.unwrap();
    assert_eq!(
        remaining, 0,
        "all libraries writeback-on → zero remaining; cleanup-PR safe",
    );
}

#[tokio::test]
async fn count_is_zero_on_a_fresh_db_with_no_libraries() {
    // Edge case: a fresh deploy with no libraries yet. The gauge should
    // report 0 rather than erroring out — operators see "writeback
    // rollout complete" trivially until they create a library.
    let app = TestApp::spawn().await;
    let remaining = count_libraries_without_writeback(&app.state().db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}
