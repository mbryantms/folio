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

/// Worker handler. Returning `Err` puts the job in apalis's failed-jobs set;
/// the spec treats per-library failures as recoverable (§12.2) — we log and
/// allow apalis to surface the error in `scan_runs.error`.
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
            tracing::error!(
                library_id = %job.library_id,
                scan_run_id = %job.scan_run_id,
                error = %e,
                "scan job failed",
            );
            Err(Error::Failed(std::sync::Arc::new(Box::new(
                std::io::Error::other(e.to_string()),
            ))))
        }
    }
}
