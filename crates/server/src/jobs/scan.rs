//! Full-library scan job.
//!
//! Worker entry point for the apalis `scan` queue. The HTTP-side handler in
//! [`crate::api::libraries::scan`] only enqueues — the actual filesystem
//! traversal happens here, via [`crate::library::scanner::scan_library`].
//! After the scan completes, the runtime calls
//! [`crate::jobs::JobRuntime::release_scan`] to clear coalescing state and
//! re-enqueue if a trigger arrived during execution.

use crate::state::AppState;
use apalis::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub library_id: Uuid,
    pub scan_run_id: Uuid,
    /// When true, bypass per-folder mtime checks (spec §4.4 force flag).
    /// Forwarded into the scanner via the upcoming `ScanContext`.
    pub force: bool,
}

/// Worker handler. A scan failure is recorded durably by the scanner's finalize
/// (the `scan_runs` row's `state='failed'` + `error`, a `ScanEvent::Failed` WS
/// event, and a `library_event` manifest row), so the handler returns `Ok` even
/// on failure: returning `Err` would only drive apalis's immediate 5× retry of
/// an expensive full-library scan, which rarely helps a deterministic failure
/// (missing mount, bad config) at the millisecond timescale of those retries.
/// The operator — or the next scheduled / file-watch trigger — re-runs it
/// deliberately. (OPS-3 follow-up.)
pub async fn handle(job: Job, state: Data<AppState>) -> Result<(), Error> {
    // Data<AppState> derefs to &AppState; clone the AppState to drop the Data wrapper.
    let state: AppState = (*state).clone();
    tracing::info!(
        library_id = %job.library_id,
        scan_run_id = %job.scan_run_id,
        force = job.force,
        "scan job started",
    );

    let scan_result = crate::library::scanner::scan_library_with_run_id(
        &state,
        job.library_id,
        job.force,
        Some(job.scan_run_id),
    )
    .await;

    // Release coalescing state regardless of success — a failed scan still
    // needs to clear the in-flight marker so the next trigger isn't blocked.
    if let Err(e) = state.jobs.release_scan(job.library_id).await {
        tracing::error!(
            library_id = %job.library_id,
            error = %e,
            "release_scan failed (coalescing keys may be stale)",
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
            Ok(())
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
            Ok(())
        }
    }
}
