//! Full-library scan job.
//!
//! Worker entry point for the apalis `scan` queue. The HTTP-side handler in
//! [`crate::api::libraries::scan`] only enqueues — the actual filesystem
//! traversal happens here, via [`crate::library::scanner::scan_library`].
//! While the scan runs, a [`ScanLease`] keeps the library's coalescing key
//! alive for this run; after the scan, the runtime calls
//! [`crate::jobs::JobRuntime::release_scan`] to release the key (only if
//! this run still holds it) and re-enqueue if a trigger arrived during
//! execution.

use crate::jobs::scan_lease::{LeaseCheck, ScanLease, ScanLeaseTiming};
use crate::state::AppState;
use apalis::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Recorded on a run that stopped because another scan of the same library
/// holds the coalescing key.
pub const SUPERSEDED_REASON: &str = "Superseded: another scan of this library is running";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub library_id: Uuid,
    pub scan_run_id: Uuid,
    /// When true, bypass per-folder mtime checks (spec §4.4 force flag).
    /// Forwarded into the scanner via the upcoming `ScanContext`.
    pub force: bool,
    /// Watcher-scoped scan (WP-3.1): the touched directories (absolute,
    /// under the library root). `None` = full-library scan. Defaults to
    /// `None` so payloads queued by older binaries still deserialize.
    #[serde(default)]
    pub scope: Option<Vec<String>>,
}

/// Worker handler. A scan failure is recorded durably by the scanner's finalize
/// (the `scan_runs` row's `state='failed'` + `error`, a `ScanEvent::Failed` WS
/// event, and a `library_event` manifest row), so the handler returns `Ok` even
/// on failure: returning `Err` would only drive apalis's retry of an expensive
/// full-library scan ([`crate::jobs::JOB_MAX_ATTEMPTS`] attempts, re-queued on
/// the next poll), which rarely helps a deterministic failure (missing mount,
/// bad config) at the timescale of those retries.
/// The operator — or the next scheduled / file-watch trigger — re-runs it
/// deliberately. (OPS-3 follow-up.)
pub async fn handle(job: Job, state: Data<AppState>) -> Result<(), BoxDynError> {
    // Data<AppState> derefs to &AppState; clone the AppState to drop the Data wrapper.
    let state: AppState = (*state).clone();
    run(&state, job, ScanLeaseTiming::PRODUCTION).await;
    Ok(())
}

/// [`handle`] with an explicit lease timing (tests pass short lifetimes).
pub async fn run(state: &AppState, job: Job, timing: ScanLeaseTiming) {
    tracing::info!(
        library_id = %job.library_id,
        scan_run_id = %job.scan_run_id,
        force = job.force,
        scope_dirs = job.scope.as_ref().map_or(0, Vec::len),
        "scan job started",
    );

    // Hold the library's coalescing key for this run while it scans. A key
    // that another run holds means that run owns the library now (this job
    // was re-queued after a restart, or its queued claim lapsed): close this
    // run as cancelled without scanning and leave the key alone.
    let (lease, first_check) = ScanLease::start(
        state.jobs.redis.clone(),
        job.library_id,
        job.scan_run_id,
        timing,
    )
    .await;
    if first_check == Some(LeaseCheck::Lost) {
        tracing::warn!(
            library_id = %job.library_id,
            scan_run_id = %job.scan_run_id,
            "scan job superseded before it started: another run holds the library",
        );
        crate::library::scanner::cancel_unstarted_run(
            state,
            job.library_id,
            job.scan_run_id,
            SUPERSEDED_REASON,
        )
        .await;
        return;
    }

    let lease_lost = lease.lost_flag();
    let scan_result = match job.scope.as_deref() {
        Some(dirs) => {
            let scope: Vec<std::path::PathBuf> =
                dirs.iter().map(std::path::PathBuf::from).collect();
            crate::library::scanner::scan_library_scoped_leased(
                state,
                job.library_id,
                &scope,
                Some(job.scan_run_id),
                Some(lease_lost),
            )
            .await
        }
        None => {
            crate::library::scanner::scan_library_leased(
                state,
                job.library_id,
                job.force,
                Some(job.scan_run_id),
                Some(lease_lost),
            )
            .await
        }
    };

    // Stop refreshing before the release, so no refresh lands after it.
    lease.stop().await;
    // Release coalescing state regardless of success — a failed scan still
    // needs to clear the in-flight marker so the next trigger isn't blocked.
    // The release is compare-and-delete: a run that lost the key leaves the
    // new holder's key in place.
    if let Err(e) = state
        .jobs
        .release_scan(job.library_id, job.scan_run_id)
        .await
    {
        tracing::error!(
            library_id = %job.library_id,
            error = %e,
            "release_scan failed (the coalescing key expires on its own)",
        );
    }

    match scan_result {
        Ok(stats) => {
            tracing::info!(
                library_id = %job.library_id,
                scan_run_id = %job.scan_run_id,
                files_added = stats.files_added,
                files_updated = stats.files_updated,
                files_unchanged = stats.files_unchanged,
                "scan job completed",
            );
        }
        Err(e) => {
            // Failure already recorded by the scanner finalize (scan_runs row +
            // WS event + manifest). Return Ok so apalis doesn't re-run the whole
            // scan 5× immediately; the run is surfaced as `failed` and a fresh
            // trigger re-runs it. (OPS-3 follow-up — see the fn doc.)
            tracing::error!(
                library_id = %job.library_id,
                scan_run_id = %job.scan_run_id,
                error = %e,
                "scan job failed (recorded in scan_runs; not retried by apalis)",
            );
            // …except when it failed before opening the run (library gone,
            // validation): the pre-inserted `queued` row is still open.
            crate::library::scanner::fail_unstarted_run(
                state,
                job.library_id,
                Some(job.scan_run_id),
                &e.to_string(),
            )
            .await;
        }
    }
}
