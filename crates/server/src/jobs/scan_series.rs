//! Per-series scan job (spec §3 trigger row "Manual series scan").
//!
//! The handler runs the narrow folder-scoped scan path
//! ([`crate::library::scanner::scan_series_folder`]) when the payload
//! identifies one specific series folder. When the payload only carries a
//! `library_id` (defensive fallback for malformed enqueues), it coalesces
//! into a full library scan via the existing in-flight gate.
//!
//! Sources of payloads:
//!   - `POST /series/{id}/scan` — manual rescan from the UI
//!   - `POST /issues/{id}/scan` — issue-level "Refresh metadata"
//!   - File-watch events (Milestone 9) — debounced per-folder writes
//!
//! Series identity (Milestone 6) means the same `(library_id, series_id,
//! folder_path)` tuple maps to one row in the DB, so the per-folder ingest
//! pipeline produces identical state regardless of which trigger pushed
//! the job.

use crate::state::AppState;
use apalis::prelude::*;
use entity::series;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Trigger discriminator for the History tab. Defaults to `Series` so
/// payloads enqueued by older binaries (file-watch events from before this
/// field landed) still deserialize cleanly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    #[default]
    Series,
    Issue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub library_id: Uuid,
    pub series_id: Option<Uuid>,
    /// Folder path that triggered the scan (file-watch / manual rescan).
    /// When `None` we look up the series's folder_path; if both are None we
    /// fall back to a coalesced full library scan.
    pub folder_path: Option<String>,
    /// What clicked the trigger — surfaced in `scan_runs.kind` for History.
    /// Optional for backward-compat with payloads enqueued before this
    /// field existed; `None` is treated as `Series`.
    #[serde(default)]
    pub kind: Option<JobKind>,
    /// When `kind == Issue`, the originating issue id. Recorded on the
    /// scan run so the History row links back to the right issue page.
    #[serde(default)]
    pub issue_id: Option<String>,
    /// Bypass the per-file size+mtime fast path so every archive is
    /// re-parsed from disk. Defaults to false for back-compat with payloads
    /// enqueued by older binaries (file-watch events) where forcing would
    /// be wasteful. Manual scans set it to true.
    #[serde(default)]
    pub force: bool,
    /// Scan run id allocated at enqueue time so the UI can show `queued`
    /// immediately and the worker transitions the same row to `running`.
    #[serde(default)]
    pub scan_run_id: Option<Uuid>,
}

pub async fn handle(job: Job, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    tracing::info!(
        library_id = %job.library_id,
        series_id = ?job.series_id,
        folder_path = ?job.folder_path,
        "scan_series job",
    );

    if matches!(job.kind.unwrap_or_default(), JobKind::Issue)
        && let Some(issue_id) = job.issue_id.as_deref()
    {
        match crate::library::scanner::scan_issue_file(
            &state,
            job.library_id,
            issue_id,
            job.force,
            job.scan_run_id,
        )
        .await
        {
            Ok(stats) => {
                tracing::info!(
                    library_id = %job.library_id,
                    issue_id,
                    files_added = stats.files_added,
                    files_updated = stats.files_updated,
                    files_unchanged = stats.files_unchanged,
                    "scan_issue complete (narrow)",
                );
                release_scope(&state, &job).await;
                return Ok(());
            }
            Err(e) => {
                tracing::error!(
                    library_id = %job.library_id,
                    issue_id,
                    error = %e,
                    "scan_issue narrow path failed",
                );
                release_scope(&state, &job).await;
                return Err(Error::Failed(std::sync::Arc::new(Box::new(
                    std::io::Error::other(e.to_string()),
                ))));
            }
        }
    }

    // Resolve a (series_id, folder) pair for the narrow path. If the payload
    // only gave us a folder_path, look up the series row by it; if it only
    // gave us a series_id, pull the canonical folder_path from the series.
    let resolved = match (job.series_id, job.folder_path.as_deref()) {
        (Some(sid), Some(fp)) => Some((sid, PathBuf::from(fp))),
        (Some(sid), None) => match series::Entity::find_by_id(sid).one(&state.db).await {
            Ok(Some(s)) => s.folder_path.map(|p| (sid, PathBuf::from(p))),
            Ok(None) => {
                tracing::warn!(series_id = %sid, "scan_series: series row missing — falling back to library scan");
                None
            }
            Err(e) => {
                tracing::error!(series_id = %sid, error = %e, "scan_series: series lookup failed");
                return Err(Error::Failed(std::sync::Arc::new(Box::new(
                    std::io::Error::other(e.to_string()),
                ))));
            }
        },
        (None, Some(fp)) => {
            // File-watch payload arrived without the series_id resolved.
            // Resolve via folder_path on this library; if no row exists yet,
            // a full library scan is the right answer (the new folder needs
            // discovery).
            match series::Entity::find()
                .filter(series::Column::FolderPath.eq(fp))
                .filter(series::Column::LibraryId.eq(job.library_id))
                .one(&state.db)
                .await
            {
                Ok(Some(s)) => Some((s.id, PathBuf::from(fp))),
                Ok(None) => None,
                Err(e) => {
                    tracing::error!(folder_path = fp, error = %e, "scan_series: folder lookup failed");
                    None
                }
            }
        }
        (None, None) => None,
    };

    if let Some((series_id, folder)) = resolved {
        let scan_kind = match job.kind.unwrap_or_default() {
            JobKind::Issue => crate::library::scanner::ScanKind::Issue,
            JobKind::Series => crate::library::scanner::ScanKind::Series,
        };
        let issue_id_for_run = job
            .issue_id
            .clone()
            .filter(|_| matches!(scan_kind, crate::library::scanner::ScanKind::Issue));
        match crate::library::scanner::scan_series_folder(
            &state,
            job.library_id,
            series_id,
            &folder,
            scan_kind,
            issue_id_for_run,
            job.force,
            job.scan_run_id,
        )
        .await
        {
            Ok(stats) => {
                tracing::info!(
                    library_id = %job.library_id,
                    series_id = %series_id,
                    folder = %folder.display(),
                    files_added = stats.files_added,
                    files_updated = stats.files_updated,
                    issues_removed = stats.issues_removed,
                    "scan_series complete (narrow)",
                );
                release_scope(&state, &job).await;
                return Ok(());
            }
            Err(e) => {
                tracing::error!(
                    library_id = %job.library_id,
                    series_id = %series_id,
                    folder = %folder.display(),
                    error = %e,
                    "scan_series narrow path failed",
                );
                release_scope(&state, &job).await;
                return Err(Error::Failed(std::sync::Arc::new(Box::new(
                    std::io::Error::other(e.to_string()),
                ))));
            }
        }
    }

    // Fallback: payload didn't identify a folder we can scan narrowly. Use
    // the coalesced full-library path so noisy file-watch bursts on
    // unidentifiable folders still collapse into a single in-flight scan.
    if let Err(e) = state.jobs.coalesce_scan(job.library_id, job.force).await {
        tracing::error!(
            library_id = %job.library_id,
            error = %e,
            "scan_series coalesce_scan failed",
        );
        return Err(Error::Failed(std::sync::Arc::new(Box::new(
            std::io::Error::other(e.to_string()),
        ))));
    }
    Ok(())
}

async fn release_scope(state: &AppState, job: &Job) {
    let Some(series_id) = job.series_id else {
        return;
    };
    let kind = job.kind.unwrap_or_default();
    let issue_id = job
        .issue_id
        .as_deref()
        .filter(|_| matches!(kind, JobKind::Issue));
    if let Err(e) = state
        .jobs
        .release_scoped_scan(job.library_id, series_id, kind, issue_id)
        .await
    {
        tracing::warn!(
            library_id = %job.library_id,
            series_id = %series_id,
            error = %e,
            "scan_series: release scoped coalescing key failed",
        );
    }
}
