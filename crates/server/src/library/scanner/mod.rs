//! Library scanner — orchestrates the spec's per-phase pipeline.
//!
//! Library Scanner v1, spec §4 (the scan loop). Each phase lives in its own
//! sibling module:
//!   - `validate`   — §4.2 sanity checks before starting
//!   - `enumerate`  — §4.3 list children, find series folders + layout violations
//!   - `process`    — §4.5 + §6 per-file pipeline (hash, parse, upsert)
//!
//! Reconciliation, identity merging, and post-scan jobs land in later
//! milestones; their hooks are TODOs in `scan_library`.

pub mod enumerate;
pub mod metadata_rollup;
pub mod process;
pub mod reconcile_status;
pub mod stats;
pub mod validate;

use crate::state::AppState;
use chrono::Utc;
use entity::{
    issue, library,
    scan_run::{ActiveModel as ScanRunAM, Entity as ScanRunEntity},
    series,
};
use futures::StreamExt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub use stats::ScanStats;

use crate::library::events::ScanEvent;
use crate::library::health::HealthCollector;

/// Public entry point used by both the apalis worker (Milestone 2) and the
/// per-series scan handler. `force=true` bypasses the per-folder mtime skip
/// (spec §4.4 "Force rescan").
pub async fn scan_library(state: &AppState, library_id: Uuid) -> anyhow::Result<ScanStats> {
    scan_library_with(state, library_id, false).await
}

pub async fn scan_library_with(
    state: &AppState,
    library_id: Uuid,
    force: bool,
) -> anyhow::Result<ScanStats> {
    scan_library_with_run_id(state, library_id, force, None).await
}

pub async fn scan_library_with_run_id(
    state: &AppState,
    library_id: Uuid,
    force: bool,
    requested_scan_id: Option<Uuid>,
) -> anyhow::Result<ScanStats> {
    let lib = library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("library not found"))?;

    // ───── Phase 1: validate (§4.2) ─────
    if let Err(e) = validate::validate_library(state, &lib).await {
        tracing::error!(library_id = %library_id, error = %e, "library validation failed");
        return Err(anyhow::Error::new(e));
    }

    let scan_id =
        open_scan_run(state, library_id, "library", None, None, requested_scan_id).await?;
    state.events.emit(ScanEvent::Started {
        library_id,
        scan_id,
        at: chrono::Utc::now(),
    });

    let started = Instant::now();
    let mut stats = ScanStats::default();
    let now = Utc::now().fixed_offset();
    let mut health =
        HealthCollector::new(library_id, scan_id, now).with_events(state.events.clone());

    let result = run_phases(state, &lib, scan_id, now, force, &mut stats, &mut health).await;

    finalize_run(state, &lib, scan_id, started, &mut stats, health, &result).await?;

    // Bump library.last_scan_at on full scans only — per-series scans leave
    // it alone so the scheduler still knows when a true library-wide pass
    // last ran.
    let mut lib_am: library::ActiveModel = lib.into();
    lib_am.last_scan_at = Set(Some(Utc::now().fixed_offset()));
    lib_am.update(&state.db).await?;

    result.map(|()| stats)
}

/// What kind of trigger fired this per-folder scan? Surfaces in
/// `scan_runs.kind` so the History tab can filter library / series / issue
/// scans separately. Issue scans are functionally identical to series
/// scans — same code path, same scope — but we record the trigger so the
/// admin can see who clicked which button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
    Library,
    Series,
    Issue,
}

impl ScanKind {
    fn as_db_str(self) -> &'static str {
        match self {
            ScanKind::Library => "library",
            ScanKind::Series => "series",
            ScanKind::Issue => "issue",
        }
    }
}

#[derive(Debug, Clone)]
struct PlannedFolder {
    path: PathBuf,
    archives: Vec<PathBuf>,
    known_series_id: Option<Uuid>,
    skipped_unchanged: bool,
}

#[derive(Debug, Default, Clone)]
struct ScanPlan {
    folders: Vec<PlannedFolder>,
    files_at_root: u64,
    empty_folders: u64,
    skipped_unchanged: u64,
    total_archives: u64,
}

impl ScanPlan {
    fn total_work(&self) -> u64 {
        (self.folders.len() as u64)
            .saturating_add(self.total_archives)
            .saturating_add(2)
            .max(1)
    }
}

#[derive(Debug, Clone)]
struct ProgressState {
    kind: &'static str,
    completed: u64,
    total: u64,
    series_scanned: u64,
    series_total: u64,
    series_skipped_unchanged: u64,
    files_total: u64,
    root_files: u64,
    empty_folders: u64,
    health_issues: u64,
}

impl ProgressState {
    fn new(kind: ScanKind, total: u64, series_total: u64, files_total: u64) -> Self {
        Self {
            kind: kind.as_db_str(),
            completed: 0,
            total: total.max(1),
            series_scanned: 0,
            series_total,
            series_skipped_unchanged: 0,
            files_total,
            root_files: 0,
            empty_folders: 0,
            health_issues: 0,
        }
    }
}

#[derive(Debug)]
struct LiveProgressTracker {
    started: Instant,
    files_processed: AtomicU64,
    folders_processed: AtomicU64,
    files_seen: AtomicU64,
    files_added: AtomicU64,
    files_updated: AtomicU64,
    files_unchanged: AtomicU64,
    files_skipped: AtomicU64,
    files_duplicate: AtomicU64,
    issues_removed: AtomicU64,
    bytes_hashed: AtomicU64,
}

impl LiveProgressTracker {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            files_processed: AtomicU64::new(0),
            folders_processed: AtomicU64::new(0),
            files_seen: AtomicU64::new(0),
            files_added: AtomicU64::new(0),
            files_updated: AtomicU64::new(0),
            files_unchanged: AtomicU64::new(0),
            files_skipped: AtomicU64::new(0),
            files_duplicate: AtomicU64::new(0),
            issues_removed: AtomicU64::new(0),
            bytes_hashed: AtomicU64::new(0),
        }
    }

    fn record_folder_done(&self) {
        self.folders_processed.fetch_add(1, Ordering::Relaxed);
    }

    fn record_files_done(&self, count: u64) {
        self.files_processed.fetch_add(count, Ordering::Relaxed);
    }

    fn record_file_seen(&self) {
        self.files_seen.fetch_add(1, Ordering::Relaxed);
    }

    fn record_file_done(&self, before: &ScanStats, after: &ScanStats) {
        self.files_processed.fetch_add(1, Ordering::Relaxed);
        add_delta(&self.files_added, before.files_added, after.files_added);
        add_delta(
            &self.files_updated,
            before.files_updated,
            after.files_updated,
        );
        add_delta(
            &self.files_unchanged,
            before.files_unchanged,
            after.files_unchanged,
        );
        add_delta(
            &self.files_skipped,
            before.files_skipped,
            after.files_skipped,
        );
        add_delta(
            &self.files_duplicate,
            before.files_duplicate,
            after.files_duplicate,
        );
        add_delta(
            &self.issues_removed,
            before.issues_removed,
            after.issues_removed,
        );
        add_delta(&self.bytes_hashed, before.bytes_hashed, after.bytes_hashed);
    }

    fn progress_snapshot(&self, base: &ProgressState) -> ProgressState {
        let mut progress = base.clone();
        let files_processed = self.files_processed.load(Ordering::Relaxed);
        let folders_processed = self.folders_processed.load(Ordering::Relaxed);
        progress.completed = files_processed
            .saturating_add(folders_processed)
            .min(progress.total);
        progress.series_scanned = folders_processed.min(progress.series_total);
        progress
    }

    fn stats_snapshot(&self) -> ScanStats {
        let mut stats = ScanStats {
            files_seen: self.files_seen.load(Ordering::Relaxed),
            files_added: self.files_added.load(Ordering::Relaxed),
            files_updated: self.files_updated.load(Ordering::Relaxed),
            files_unchanged: self.files_unchanged.load(Ordering::Relaxed),
            files_skipped: self.files_skipped.load(Ordering::Relaxed),
            files_duplicate: self.files_duplicate.load(Ordering::Relaxed),
            issues_removed: self.issues_removed.load(Ordering::Relaxed),
            bytes_hashed: self.bytes_hashed.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            ..ScanStats::default()
        };
        stats.finalize_rates();
        stats
    }
}

fn add_delta(counter: &AtomicU64, before: u64, after: u64) {
    counter.fetch_add(after.saturating_sub(before), Ordering::Relaxed);
}

/// Per-series narrow scan path (Milestone 3 — spec §3 "Manual series scan",
/// §4.4 "force per-folder rescan"). Runs the same per-folder pipeline as a
/// full scan but skips the library-wide enumerate + reconcile passes,
/// substituting a series-scoped reconcile so siblings whose folder wasn't
/// rescanned are left untouched.
///
/// Forces `process_folder` to bypass the mtime fast-path: callers
/// (`POST /series/{id}/scan`, file-watch events, `POST /issues/{id}/scan`)
/// invoke this because the user explicitly asked for a refresh, so honoring
/// "skip if folder mtime ≤ last_scanned_at" would defeat the request.
///
/// `kind` distinguishes a series-page click from an issue-page click in the
/// History tab; behavior is identical in both cases. `issue_id` is honored
/// only when `kind == ScanKind::Issue` and lets the History row link back
/// to the originating issue.
#[allow(clippy::too_many_arguments)]
pub async fn scan_series_folder(
    state: &AppState,
    library_id: Uuid,
    series_id: Uuid,
    folder: &std::path::Path,
    kind: ScanKind,
    issue_id: Option<String>,
    // Bypass the per-file size+mtime fast path so every archive in the
    // folder is re-parsed. Use when the user explicitly clicked "Scan
    // series" — they expect a fresh ingest, not the auto-skip behavior
    // that's safe for cron / file-watch triggers.
    force: bool,
    requested_scan_id: Option<Uuid>,
) -> anyhow::Result<ScanStats> {
    let lib = library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("library not found"))?;

    // Light validation: folder must still exist and live under the library
    // root. Falls through to a "no-op" scan run when missing — the next
    // reconcile picks up the soft-delete (matches `validate.rs` doc note).
    if !validate::folder_still_exists(folder) {
        anyhow::bail!(
            "series folder no longer exists on disk: {}",
            folder.display()
        );
    }
    let root_canon = std::fs::canonicalize(&lib.root_path)
        .map_err(|e| anyhow::anyhow!("library root unreadable: {e}"))?;
    let folder_canon = std::fs::canonicalize(folder)
        .map_err(|e| anyhow::anyhow!("series folder unreadable: {e}"))?;
    if !folder_canon.starts_with(&root_canon) {
        anyhow::bail!(
            "series folder is not inside the library root: {}",
            folder_canon.display()
        );
    }

    let scan_id = open_scan_run(
        state,
        library_id,
        kind.as_db_str(),
        Some(series_id),
        issue_id.clone().filter(|_| matches!(kind, ScanKind::Issue)),
        requested_scan_id,
    )
    .await?;
    state.events.emit(ScanEvent::Started {
        library_id,
        scan_id,
        at: chrono::Utc::now(),
    });

    let started = Instant::now();
    let mut stats = ScanStats::default();
    let now = Utc::now().fixed_offset();
    let mut health =
        HealthCollector::new_scoped(library_id, scan_id, now).with_events(state.events.clone());

    let result = run_series_phases(
        state,
        &lib,
        scan_id,
        series_id,
        &folder_canon,
        &mut stats,
        &mut health,
        force,
    )
    .await;

    finalize_run(state, &lib, scan_id, started, &mut stats, health, &result).await?;

    result.map(|()| stats)
}

/// True issue-scoped metadata refresh. This path re-ingests only the issue's
/// current file path, records the scan as `kind='issue'`, and enqueues only
/// the refreshed issue's cover thumbnail. It deliberately avoids series
/// reconciliation and library-wide post-scan fanout.
pub async fn scan_issue_file(
    state: &AppState,
    library_id: Uuid,
    issue_id: &str,
    // Bypass the per-file size+mtime fast path. The "Scan issue" button
    // in the UI passes `true` because the user explicitly asked for a
    // re-parse — useful when the parser learned new fields (ComicVine ID,
    // etc.) but the file's mtime hasn't moved.
    force: bool,
    requested_scan_id: Option<Uuid>,
) -> anyhow::Result<ScanStats> {
    let lib = library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("library not found"))?;
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("issue not found"))?;
    if row.library_id != library_id {
        anyhow::bail!("issue does not belong to library");
    }

    let scan_id = open_scan_run(
        state,
        library_id,
        ScanKind::Issue.as_db_str(),
        Some(row.series_id),
        Some(row.id.clone()),
        requested_scan_id,
    )
    .await?;
    state.events.emit(ScanEvent::Started {
        library_id,
        scan_id,
        at: chrono::Utc::now(),
    });

    let started = Instant::now();
    let mut stats = ScanStats::default();
    let now = Utc::now().fixed_offset();
    let mut health =
        HealthCollector::new_scoped(library_id, scan_id, now).with_events(state.events.clone());
    let result = run_issue_phase(state, scan_id, &lib, &row, &mut stats, &mut health, force).await;

    finalize_run(state, &lib, scan_id, started, &mut stats, health, &result).await?;
    result.map(|()| stats)
}

/// Insert a fresh `scan_runs` row in `state='running'` and return its id.
/// `kind` / `series_id` / `issue_id` flow through to the History tab's
/// filter chips and target-link cells.
async fn open_scan_run(
    state: &AppState,
    library_id: Uuid,
    kind: &str,
    series_id: Option<Uuid>,
    issue_id: Option<String>,
    requested_scan_id: Option<Uuid>,
) -> anyhow::Result<Uuid> {
    let scan_id = requested_scan_id.unwrap_or_else(Uuid::now_v7);
    let now = Utc::now().fixed_offset();
    if let Some(existing) = ScanRunEntity::find_by_id(scan_id).one(&state.db).await? {
        if existing.state == "queued" {
            let mut am: ScanRunAM = existing.into();
            am.state = Set("running".to_owned());
            am.started_at = Set(now);
            am.error = Set(None);
            am.update(&state.db).await?;
        }
        return Ok(scan_id);
    }
    let am = ScanRunAM {
        id: Set(scan_id),
        library_id: Set(library_id),
        state: Set("running".into()),
        started_at: Set(now),
        ended_at: Set(None),
        stats: Set(serde_json::to_value(ScanStats::default())?),
        error: Set(None),
        kind: Set(kind.to_owned()),
        series_id: Set(series_id),
        issue_id: Set(issue_id),
    };
    am.insert(&state.db).await?;
    Ok(scan_id)
}

/// Persist health issues, close the `scan_runs` row, emit the final WS event
/// (Completed | Failed), and record Prometheus metrics. Shared by full and
/// per-series scans so both paths produce identical observability output.
async fn finalize_run(
    state: &AppState,
    lib: &library::Model,
    scan_id: Uuid,
    started: Instant,
    stats: &mut ScanStats,
    health: HealthCollector,
    result: &anyhow::Result<()>,
) -> anyhow::Result<()> {
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    stats.finalize_rates();
    let stats_json = serde_json::to_value(&stats)?;
    let library_id = lib.id;

    let health_count = health.count();
    if let Err(e) = health.finalize(&state.db).await {
        tracing::error!(
            library_id = %library_id,
            scan_id = %scan_id,
            error = %e,
            "scanner: health finalize failed",
        );
    }
    if health_count > 0 {
        tracing::info!(
            library_id = %library_id,
            scan_id = %scan_id,
            health_issues = health_count,
            "scanner: health issues recorded",
        );
    }

    let (final_state, error) = match result {
        Ok(()) => ("complete".to_string(), None),
        Err(e) => ("failed".to_string(), Some(e.to_string())),
    };

    let mut close: ScanRunAM = ScanRunEntity::find_by_id(scan_id)
        .one(&state.db)
        .await?
        .expect("scan run row")
        .into();
    close.state = Set(final_state);
    close.ended_at = Set(Some(Utc::now().fixed_offset()));
    close.stats = Set(stats_json);
    close.error = Set(error);
    close.update(&state.db).await?;

    match result {
        Ok(()) => state.events.emit(ScanEvent::Completed {
            library_id,
            scan_id,
            added: stats.files_added,
            updated: stats.files_updated,
            removed: stats.issues_removed,
            duration_ms: stats.elapsed_ms,
        }),
        Err(e) => state.events.emit(ScanEvent::Failed {
            library_id,
            scan_id,
            error: e.to_string(),
        }),
    }

    let lib_label = library_id.to_string();
    let result_label = if result.is_ok() { "complete" } else { "failed" };
    metrics::histogram!(
        "comic_scan_duration_seconds",
        "library_id" => lib_label.clone(),
        "result" => result_label,
    )
    .record(stats.elapsed_ms as f64 / 1000.0);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label.clone(),
        "action" => "added",
    )
    .increment(stats.files_added);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label.clone(),
        "action" => "updated",
    )
    .increment(stats.files_updated);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label.clone(),
        "action" => "skipped",
    )
    .increment(stats.files_skipped + stats.files_unchanged);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label.clone(),
        "action" => "removed",
    )
    .increment(stats.issues_removed);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label.clone(),
        "action" => "malformed",
    )
    .increment(stats.files_malformed);
    metrics::counter!(
        "comic_scan_files_total",
        "library_id" => lib_label,
        "action" => "duplicate",
    )
    .increment(stats.files_duplicate);

    Ok(())
}

fn stats_json_with_progress(
    stats: &ScanStats,
    progress: Option<(&ProgressState, &'static str, &'static str, Option<&str>)>,
) -> anyhow::Result<serde_json::Value> {
    let mut json = serde_json::to_value(stats)?;
    if let Some((progress, phase, unit, current_label)) = progress
        && let Some(obj) = json.as_object_mut()
    {
        obj.insert(
            "progress".to_owned(),
            serde_json::json!({
                "kind": progress.kind,
                "phase": phase,
                "unit": unit,
                "completed": progress.completed,
                "total": progress.total,
                "current_label": current_label,
                "health_issues": progress.health_issues,
                "series_scanned": progress.series_scanned,
                "series_total": progress.series_total,
                "series_skipped_unchanged": progress.series_skipped_unchanged,
                "files_total": progress.files_total,
                "root_files": progress.root_files,
                "empty_folders": progress.empty_folders,
                "phase_elapsed_ms": stats.phase_timings_ms.get(phase).copied(),
                "files_per_sec": stats.files_per_sec,
                "bytes_per_sec": stats.bytes_per_sec,
                "skipped_folders": progress.series_skipped_unchanged,
            }),
        );
    }
    Ok(json)
}

#[allow(clippy::too_many_arguments)]
async fn emit_progress(
    state: &AppState,
    library_id: Uuid,
    scan_id: Uuid,
    progress: &ProgressState,
    phase: &'static str,
    unit: &'static str,
    current_label: Option<String>,
    stats: &ScanStats,
) {
    state.events.emit(ScanEvent::Progress {
        library_id,
        scan_id,
        kind: progress.kind,
        phase,
        unit,
        completed: progress.completed.min(progress.total),
        total: progress.total,
        current_label: current_label.clone(),
        files_seen: stats.files_seen,
        files_added: stats.files_added,
        files_updated: stats.files_updated,
        files_unchanged: stats.files_unchanged,
        files_skipped: stats.files_skipped,
        files_duplicate: stats.files_duplicate,
        issues_removed: stats.issues_removed,
        health_issues: progress.health_issues,
        series_scanned: progress.series_scanned,
        series_total: progress.series_total,
        series_skipped_unchanged: progress.series_skipped_unchanged,
        files_total: progress.files_total,
        root_files: progress.root_files,
        empty_folders: progress.empty_folders,
        elapsed_ms: (stats.elapsed_ms > 0).then_some(stats.elapsed_ms),
        phase_elapsed_ms: stats.phase_timings_ms.get(phase).copied(),
        files_per_sec: stats.files_per_sec,
        bytes_per_sec: stats.bytes_per_sec,
        active_workers: (phase == "scanning").then_some(state.cfg().scan_worker_count.max(1) as u64),
        dirty_folders: None,
        skipped_folders: Some(progress.series_skipped_unchanged),
        eta_ms: None,
    });

    let stats_json = match stats_json_with_progress(
        stats,
        Some((progress, phase, unit, current_label.as_deref())),
    ) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(scan_id = %scan_id, error = %e, "scanner: progress snapshot encode failed");
            return;
        }
    };

    if let Ok(Some(row)) = ScanRunEntity::find_by_id(scan_id).one(&state.db).await {
        let mut am: ScanRunAM = row.into();
        am.stats = Set(stats_json);
        if let Err(e) = am.update(&state.db).await {
            tracing::warn!(scan_id = %scan_id, error = %e, "scanner: progress snapshot persist failed");
        }
    }
}

async fn known_series_by_folder(
    state: &AppState,
    lib: &library::Model,
) -> anyhow::Result<HashMap<String, (Uuid, Option<chrono::DateTime<chrono::FixedOffset>>)>> {
    let rows = entity::series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib.id))
        .all(&state.db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|s| {
            s.folder_path
                .map(|folder| (folder, (s.id, s.last_scanned_at)))
        })
        .collect())
}

async fn list_archives_for_plan(
    folder: &std::path::Path,
    ignore: &crate::library::ignore::IgnoreRules,
) -> anyhow::Result<Vec<PathBuf>> {
    let folder_for_walk = folder.to_path_buf();
    let ignore_for_walk = ignore.clone();
    tokio::task::spawn_blocking(move || {
        enumerate::list_archives_with(&folder_for_walk, &ignore_for_walk)
    })
    .await
    .map_err(|e| anyhow::anyhow!("archive walk task failed: {e}"))
}

async fn build_library_scan_plan(
    state: &AppState,
    lib: &library::Model,
    layout: &enumerate::EnumerationResult,
    force: bool,
    ignore: &crate::library::ignore::IgnoreRules,
) -> anyhow::Result<ScanPlan> {
    let mut plan = ScanPlan {
        files_at_root: layout.files_at_root.len() as u64,
        empty_folders: layout.empty_folders.len() as u64,
        ..ScanPlan::default()
    };

    let known = known_series_by_folder(state, lib).await?;
    let concurrency = state.cfg().scan_worker_count.max(1);
    let mut planned = futures::stream::iter(layout.series_folders.iter().cloned())
        .map(|folder| {
            let ignore = ignore.clone();
            let known_entry = known.get(&folder.to_string_lossy().into_owned()).copied();
            async move {
                let mut known_series_id = known_entry.map(|(id, _)| id);
                let mut skipped_unchanged = false;
                let archives = if !force {
                    if let Some((_id, Some(last))) = known_entry {
                        let folder_for_walk = folder.clone();
                        let ignore_for_walk = ignore.clone();
                        let last_utc = last.to_utc();
                        let walk = tokio::task::spawn_blocking(move || {
                            enumerate::list_archives_changed_since(
                                &folder_for_walk,
                                &ignore_for_walk,
                                last_utc,
                            )
                        })
                        .await
                        .map_err(|e| anyhow::anyhow!("archive walk task failed: {e}"))?;
                        skipped_unchanged = !walk.changed_since;
                        walk.archives
                    } else {
                        let archives = list_archives_for_plan(&folder, &ignore).await?;
                        if let Some((id, _)) = known_entry {
                            known_series_id = Some(id);
                        }
                        archives
                    }
                } else {
                    list_archives_for_plan(&folder, &ignore).await?
                };
                Ok::<_, anyhow::Error>(PlannedFolder {
                    path: folder,
                    archives,
                    known_series_id,
                    skipped_unchanged,
                })
            }
        })
        .buffer_unordered(concurrency);

    while let Some(item) = planned.next().await {
        let folder = item?;
        if folder.skipped_unchanged {
            plan.skipped_unchanged += 1;
        }
        plan.total_archives += folder.archives.len() as u64;
        plan.folders.push(folder);
    }

    Ok(plan)
}

async fn build_series_scan_plan(
    folder: &std::path::Path,
    series_id: Uuid,
    ignore: &crate::library::ignore::IgnoreRules,
) -> anyhow::Result<ScanPlan> {
    let archives = list_archives_for_plan(folder, ignore).await?;
    let total_archives = archives.len() as u64;
    Ok(ScanPlan {
        folders: vec![PlannedFolder {
            path: folder.to_path_buf(),
            archives,
            known_series_id: Some(series_id),
            skipped_unchanged: false,
        }],
        total_archives,
        ..ScanPlan::default()
    })
}

/// Series-scoped equivalent of [`run_phases`]: process exactly one folder,
/// then run a series-only reconcile so siblings stay untouched. Thumbnail
/// catchup is scoped to the series; search/dictionary fanout remains
/// library-scoped until those jobs grow narrower invalidation APIs.
#[allow(clippy::too_many_arguments)]
async fn run_series_phases(
    state: &AppState,
    lib: &library::Model,
    scan_id: Uuid,
    series_id: Uuid,
    folder: &std::path::Path,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
    force: bool,
) -> anyhow::Result<()> {
    let ignore = crate::library::ignore::IgnoreRules::for_library(lib)
        .map_err(|e| anyhow::anyhow!("ignore_globs invalid: {e}"))?;

    // Narrow series scans run one folder, so parallel-phase totals equal wall.
    stats.set_parallel_workers(1);

    let mut progress = ProgressState::new(ScanKind::Series, 1, 1, 0);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning",
        "planning",
        Some("Planning series scan".to_owned()),
        stats,
    )
    .await;

    let plan_started = Instant::now();
    let plan = build_series_scan_plan(folder, series_id, &ignore).await?;
    stats.record_phase("plan", plan_started.elapsed());
    progress = ProgressState::new(ScanKind::Series, plan.total_work(), 1, plan.total_archives);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning_complete",
        "work",
        Some(format!("{} files", plan.total_archives)),
        stats,
    )
    .await;

    let planned = plan
        .folders
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("series scan plan did not include folder"))?;
    let work_units = 1 + planned.archives.len() as u64;
    let label = planned
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| planned.path.to_string_lossy().into_owned());
    let process_started = Instant::now();
    let outcome =
        process_planned_folder(state, lib, planned, stats, health, None, force, false).await?;
    stats.record_phase("process", process_started.elapsed());
    progress.series_scanned = 1;
    progress.completed = progress
        .completed
        .saturating_add(work_units)
        .min(progress.total);
    progress.health_issues = health.count() as u64;
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "scanning",
        "work",
        Some(label),
        stats,
    )
    .await;

    let seen_paths: HashSet<String> = outcome.seen_paths.into_iter().collect();
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "reconciling",
        "work",
        None,
        stats,
    )
    .await;
    let reconcile_started = Instant::now();
    crate::library::reconcile::reconcile_series_seen(&state.db, series_id, &seen_paths, stats)
        .await?;
    stats.record_phase("reconcile", reconcile_started.elapsed());
    progress.completed = progress.completed.saturating_add(1).min(progress.total);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "reconciled",
        "work",
        None,
        stats,
    )
    .await;

    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "enqueueing_thumbnails",
        "work",
        None,
        stats,
    )
    .await;
    let thumbnail_enqueue_started = Instant::now();
    let _ = crate::jobs::post_scan::enqueue_post_scan_for_series(state, lib.id, series_id).await;
    stats.record_phase("thumbnail_enqueue", thumbnail_enqueue_started.elapsed());
    // Saved-views M4: previously-missing CBL entries may now match
    // newly-scanned issues in this series.
    if stats.files_added > 0 || stats.issues_restored > 0 {
        spawn_cbl_rematch_all(state.clone());
    }
    progress.completed = progress.total;
    progress.health_issues = health.count() as u64;
    emit_progress(
        state, lib.id, scan_id, &progress, "complete", "work", None, stats,
    )
    .await;

    Ok(())
}

async fn run_issue_phase(
    state: &AppState,
    scan_id: Uuid,
    lib: &library::Model,
    row: &issue::Model,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
    force: bool,
) -> anyhow::Result<()> {
    // Narrow issue scan runs one archive, so parallel-phase totals equal wall.
    stats.set_parallel_workers(1);

    let mut progress = ProgressState::new(ScanKind::Issue, 1, 0, 1);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning",
        "planning",
        Some("Planning issue scan".to_owned()),
        stats,
    )
    .await;
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning_complete",
        "file",
        Some(row.file_path.clone()),
        stats,
    )
    .await;
    let path = PathBuf::from(&row.file_path);
    if !path.exists() {
        if row.removed_at.is_none() {
            let now = Utc::now().fixed_offset();
            let mut am: issue::ActiveModel = row.clone().into();
            am.removed_at = Set(Some(now));
            am.removal_confirmed_at = Set(None);
            am.update(&state.db).await?;
            stats.issues_removed += 1;
        }
        progress.completed = 1;
        progress.health_issues = health.count() as u64;
        emit_progress(
            state,
            lib.id,
            scan_id,
            &progress,
            "complete",
            "file",
            Some(row.file_path.clone()),
            stats,
        )
        .await;
        return Ok(());
    }

    let ignore = crate::library::ignore::IgnoreRules::for_library(lib)
        .map_err(|e| anyhow::anyhow!("ignore_globs invalid: {e}"))?;
    if ignore.should_skip(&path) {
        health.emit(crate::library::health::IssueKind::UnreadableFile {
            path,
            error: "issue path is ignored by library ignore rules".to_owned(),
        });
        stats.files_skipped += 1;
        progress.completed = 1;
        progress.health_issues = health.count() as u64;
        emit_progress(
            state,
            lib.id,
            scan_id,
            &progress,
            "complete",
            "file",
            Some(row.file_path.clone()),
            stats,
        )
        .await;
        return Ok(());
    }

    stats.files_seen += 1;
    let process_started = Instant::now();
    let manifest =
        process::IssueManifest::for_paths(&state.db, std::slice::from_ref(&path)).await?;
    let txn = state.db.begin().await?;
    let ingest = process::ingest_one(
        state,
        &txn,
        lib,
        &path,
        row.series_id,
        Some(&manifest),
        // Single-archive scan: no need to pre-fetch; the per-call DB
        // allocator's one COUNT query is fine.
        None,
        stats,
        health,
        force,
    )
    .await;
    match ingest {
        Ok(()) => txn.commit().await?,
        Err(e) => {
            if let Err(rollback) = txn.rollback().await {
                tracing::warn!(error = %rollback, "scanner: issue transaction rollback failed");
            }
            return Err(e);
        }
    }
    stats.record_phase("process", process_started.elapsed());

    if let Some(updated) = issue::Entity::find()
        .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(&state.db)
        .await?
    {
        if updated.removed_at.is_some() || updated.removal_confirmed_at.is_some() {
            let mut am: issue::ActiveModel = updated.clone().into();
            am.removed_at = Set(None);
            am.removal_confirmed_at = Set(None);
            am.update(&state.db).await?;
            stats.issues_restored += 1;
        }
        let _ = crate::jobs::post_scan::enqueue_thumb_job(
            state,
            crate::jobs::post_scan::ThumbsJob::cover(updated.id.clone()),
        )
        .await;
    }

    // Single-issue scan path: re-roll up the parent series so any change to
    // this issue's genre/tag/credit junctions propagates to series filters.
    let rollup_started = Instant::now();
    metadata_rollup::rollup_series_metadata_best_effort(&state.db, row.series_id).await;
    stats.record_phase("metadata_rollup", rollup_started.elapsed());
    let sidecar = match series::Entity::find_by_id(row.series_id)
        .one(&state.db)
        .await?
    {
        Some(parent) => parent
            .folder_path
            .as_deref()
            .map(std::path::Path::new)
            .and_then(process::read_series_json),
        None => None,
    };
    let reconcile_started = Instant::now();
    if let Err(e) =
        reconcile_status::reconcile_series_status(&state.db, row.series_id, sidecar.as_ref()).await
    {
        tracing::warn!(series_id = %row.series_id, error = %e, "scanner: issue status reconcile failed");
    }
    stats.record_phase("reconcile", reconcile_started.elapsed());

    progress.completed = 1;
    progress.health_issues = health.count() as u64;
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "complete",
        "file",
        Some(row.file_path.clone()),
        stats,
    )
    .await;

    Ok(())
}

async fn run_phases(
    state: &AppState,
    lib: &library::Model,
    scan_id: Uuid,
    scan_started_at: chrono::DateTime<chrono::FixedOffset>,
    force: bool,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
) -> anyhow::Result<()> {
    use crate::library::health::IssueKind;
    let root = PathBuf::from(&lib.root_path);

    // Compile ignore rules once per scan.
    let ignore = crate::library::ignore::IgnoreRules::for_library(lib)
        .map_err(|e| anyhow::anyhow!("ignore_globs invalid: {e}"))?;

    let mut progress = ProgressState::new(ScanKind::Library, 1, 0, 0);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning",
        "planning",
        Some("Planning scan".to_owned()),
        stats,
    )
    .await;

    // ───── Phase 2: enumerate (§4.3) ─────
    let enumerate_started = Instant::now();
    let root_for_walk = root.clone();
    let ignore_for_walk = ignore.clone();
    let layout = tokio::task::spawn_blocking(move || {
        enumerate::enumerate_with(&root_for_walk, &ignore_for_walk)
    })
    .await
    .map_err(|e| anyhow::anyhow!("enumerate task failed: {e}"))?
    .map_err(|e| anyhow::anyhow!("enumerate {}: {e}", root.display()))?;
    stats.record_phase("enumerate", enumerate_started.elapsed());

    for f in &layout.files_at_root {
        health.emit(IssueKind::FileAtRoot { path: f.clone() });
    }
    for f in &layout.empty_folders {
        health.emit(IssueKind::EmptyFolder { path: f.clone() });
    }

    let plan_started = Instant::now();
    let plan = build_library_scan_plan(state, lib, &layout, force, &ignore).await?;
    stats.record_phase("plan", plan_started.elapsed());
    progress = ProgressState::new(
        ScanKind::Library,
        plan.total_work(),
        plan.folders.len() as u64,
        plan.total_archives,
    );
    progress.series_skipped_unchanged = plan.skipped_unchanged;
    progress.root_files = plan.files_at_root;
    progress.empty_folders = plan.empty_folders;
    progress.health_issues = health.count() as u64;
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "planning_complete",
        "work",
        Some(format!(
            "{} series, {} files, {} root files, {} empty folders",
            plan.folders.len(),
            plan.total_archives,
            plan.files_at_root,
            plan.empty_folders,
        )),
        stats,
    )
    .await;

    // ───── Phase 3: per-folder processing (§4.4 + §6) ─────
    let present_folders: HashSet<String> = layout
        .series_folders
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut scanned_series = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut status_reconcile_entries = Vec::new();

    let concurrency = state.cfg().scan_worker_count.max(1);
    // Anchor the parallel-phase totals so doc readers can derive
    // wall ≈ summed/parallel_workers. Set once on the global stats;
    // per-worker local stats start at 0 and merge() takes max.
    stats.set_parallel_workers(concurrency as u32);
    let live_tracker = Arc::new(LiveProgressTracker::new());
    let process_started = Instant::now();
    let mut folder_results = futures::stream::iter(plan.folders.clone())
        .map(|planned| {
            let state = state.clone();
            let lib = lib.clone();
            let live_tracker = live_tracker.clone();
            async move {
                let mut local_stats = ScanStats::default();
                let mut local_health = HealthCollector::new(lib.id, scan_id, scan_started_at)
                    .with_events(state.events.clone());
                let label = planned
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| planned.path.to_string_lossy().into_owned());
                let work_units = 1 + planned.archives.len() as u64;
                let result = process_planned_folder(
                    &state,
                    &lib,
                    planned,
                    &mut local_stats,
                    &mut local_health,
                    Some(live_tracker),
                    force,
                    true,
                )
                .await;
                (label, work_units, result, local_stats, local_health)
            }
        })
        .buffer_unordered(concurrency);

    let mut heartbeat = tokio::time::interval(Duration::from_millis(750));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_live_completed = 0;
    let mut last_live_seen = 0;

    loop {
        tokio::select! {
            maybe_result = folder_results.next() => {
                let Some((label, work_units, result, local_stats, local_health)) = maybe_result else {
                    break;
                };
                let mut folder_processed = false;
        match result {
            Ok(outcome) => {
                folder_processed = outcome.processed;
                // Always reconcile series-level metadata when we know the
                // row, even if the folder was fast-path-skipped or empty.
                // The reconcile is idempotent (no-op when nothing
                // changed) and is the only path that can apply a
                // newly-added or freshly-edited series.json sidecar to
                // a folder whose mtime hasn't moved since the previous
                // scan — so without this push, sidecar edits would
                // never reach the DB unless the user also touched a
                // CBZ.
                if let Some(series_id) = outcome.series_id {
                    status_reconcile_entries.push((series_id, outcome.series_json));
                    if outcome.processed {
                        scanned_series.insert(series_id);
                        seen_paths.extend(outcome.seen_paths);
                    }
                }
            }
            Err(e) => {
                // Per-series failures are recoverable (§12.2).
                tracing::error!(folder = %label, error = %e, "scanner: folder failed");
            }
        }
        if !folder_processed {
            live_tracker.record_files_done(work_units.saturating_sub(1));
        }
        live_tracker.record_folder_done();
        stats.merge(local_stats);
        health.merge(local_health);
        progress = live_tracker.progress_snapshot(&progress);
        progress.health_issues = health.count() as u64;
        let live_stats = live_tracker.stats_snapshot();
        emit_progress(
            state,
            lib.id,
            scan_id,
            &progress,
            "scanning",
            "work",
            Some(label),
            &live_stats,
        )
        .await;
        last_live_completed = progress.completed;
        last_live_seen = live_stats.files_seen;
            }
            _ = heartbeat.tick() => {
                let mut live_progress = live_tracker.progress_snapshot(&progress);
                live_progress.health_issues = progress.health_issues;
                let live_stats = live_tracker.stats_snapshot();
                if live_progress.completed != last_live_completed || live_stats.files_seen != last_live_seen {
                    emit_progress(
                        state,
                        lib.id,
                        scan_id,
                        &live_progress,
                        "scanning",
                        "work",
                        Some("Scanning files".to_owned()),
                        &live_stats,
                    )
                    .await;
                    last_live_completed = live_progress.completed;
                    last_live_seen = live_stats.files_seen;
                }
            }
        }
    }
    stats.record_phase("process", process_started.elapsed());

    // ───── Phase 4: reconciliation (§4.7) ─────
    let reconcile_started = Instant::now();
    progress.health_issues = health.count() as u64;
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "reconciling",
        "work",
        None,
        stats,
    )
    .await;
    if let Err(e) = crate::library::reconcile::reconcile_library_seen(
        &state.db,
        lib.id,
        &scanned_series,
        &seen_paths,
        &present_folders,
        stats,
    )
    .await
    {
        tracing::error!(library_id = %lib.id, error = %e, "scanner: reconcile failed");
    }
    if let Err(e) =
        reconcile_status::reconcile_series_status_many(&state.db, &status_reconcile_entries).await
    {
        tracing::warn!(library_id = %lib.id, error = %e, "scanner: batched series status reconcile failed");
    }
    stats.record_phase("reconcile", reconcile_started.elapsed());
    progress.completed = progress.completed.saturating_add(1).min(progress.total);
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "reconciled",
        "work",
        None,
        stats,
    )
    .await;

    // ───── Phase 5: post-scan jobs (§4.8) ─────
    // Best-effort enqueue — failure here doesn't fail the scan; the scheduler
    // and the next scan will catch up.
    // Thumbs: one job per issue whose thumbs are missing or outdated. Issues
    // already at the current version are filtered out by
    // `enqueue_pending_for_library` so re-scans don't redo work.
    emit_progress(
        state,
        lib.id,
        scan_id,
        &progress,
        "enqueueing_thumbnails",
        "work",
        None,
        stats,
    )
    .await;
    let thumbnail_enqueue_started = Instant::now();
    let _ = crate::jobs::post_scan::enqueue_post_scan_for_library(state, lib.id).await;
    stats.record_phase("thumbnail_enqueue", thumbnail_enqueue_started.elapsed());
    // Saved-views M4: re-resolve CBL entries that were previously missing.
    // Best-effort, in a spawned task so the scan finalize path isn't blocked.
    if stats.files_added > 0 || stats.issues_restored > 0 {
        spawn_cbl_rematch_all(state.clone());
    }

    progress.completed = progress.total;
    progress.health_issues = health.count() as u64;
    emit_progress(
        state, lib.id, scan_id, &progress, "complete", "work", None, stats,
    )
    .await;

    Ok(())
}

/// Walk every `cbl_lists` row and re-run the matcher. Fire-and-forget;
/// individual list failures are logged and don't propagate. Called once
/// per scan completion (full-library or per-series) so previously-
/// missing entries can transition to `matched` without waiting for the
/// scheduled refresh window.
pub fn spawn_cbl_rematch_all(state: AppState) {
    tokio::spawn(async move {
        use entity::cbl_list;
        let lists = match cbl_list::Entity::find().all(&state.db).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "post_scan cbl_rematch: list query failed");
                return;
            }
        };
        for list in lists {
            match crate::cbl::import::rematch_existing(
                &state.db,
                list.id,
                crate::cbl::import::RefreshTrigger::PostScan,
            )
            .await
            {
                Ok(0) => {}
                Ok(n) => {
                    tracing::info!(
                        list_id = %list.id,
                        rematched = n,
                        "post_scan cbl_rematch: entries newly matched",
                    );
                }
                Err(e) => {
                    tracing::warn!(list_id = %list.id, error = %e, "post_scan cbl_rematch failed");
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn process_planned_folder(
    state: &AppState,
    lib: &library::Model,
    planned: PlannedFolder,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
    live_progress: Option<Arc<LiveProgressTracker>>,
    // When `true`, every archive in the folder is re-parsed even if its
    // size+mtime match the existing row. Used by manual "force" scans.
    force: bool,
    defer_status_reconcile: bool,
) -> anyhow::Result<ProcessFolderOutcome> {
    let folder = planned.path;
    let known_series_id = planned.known_series_id;
    let archives = planned.archives;

    // Read series.json up-front so the early-skip branches can still
    // surface it to the caller. The sidecar is the authoritative
    // source for series-level fields (status, summary, total_issues,
    // comicvine_id), and reconcile_series_status is idempotent — when
    // the row already matches the sidecar nothing is written. So
    // re-running it on every scan, including folder-fast-path-skipped
    // ones, is the cheap way to keep first-scan and re-scan paths
    // produce the same DB state. Without this, a sidecar that landed
    // (or shipped enriched) after the previous scan would never apply
    // to series rows whose folders haven't been touched since.
    let series_json = process::read_series_json(&folder);

    if planned.skipped_unchanged {
        stats.series_skipped_unchanged += 1;
        tracing::debug!(folder = %folder.display(), "skipped (mtime <= last_scanned_at)");
        // Same reason as the per-file fast-path in process.rs: nothing inside
        // this folder is being re-inspected, so any open health issue rooted
        // here must be touched or the auto-resolve sweep will close it.
        health.touch_folder(&folder);
        return Ok(ProcessFolderOutcome {
            series_id: known_series_id,
            processed: false,
            series_json,
            seen_paths: Vec::new(),
        });
    }

    if archives.is_empty() {
        return Ok(ProcessFolderOutcome {
            series_id: known_series_id,
            processed: false,
            series_json,
            seen_paths: Vec::new(),
        });
    }

    // Emit a SeriesUpdated event for the live-progress overlay. Throttled
    // per-library by the broadcaster (spec §14.3).
    let folder_label = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.to_string_lossy().into_owned());

    // ───── Series identity (§7) — resolved once per folder ─────
    // Build the identity hint from (in precedence order, lowest-first):
    //   1. series.json — folder-level metadata (Mylar3 sidecar)
    //   2. first archive's ComicInfo + filename inference
    // ComicInfo wins on overlapping fields per spec §6.7. The parsed
    // sidecar is also threaded into the post-scan reconcile step
    // (below) where it takes priority over the per-issue ComicInfo
    // `<Count>` MAX-reduction for status/total_issues/summary.
    let identity_started = Instant::now();
    let series_id = if let Some(series_id) = known_series_id {
        series_id
    } else {
        let mut hint = process::peek_identity_hint(&archives[0]);
        if let Some(meta) = series_json.as_ref() {
            // series.json fills gaps where ComicInfo is silent.
            if let Some(name) = meta.name.as_deref()
                && (hint.series_name == "Unknown Series" || hint.series_name.trim().is_empty())
            {
                hint.series_name = name.to_string();
            }
            if hint.year.is_none() {
                hint.year = meta.year_began;
            }
            if hint.publisher.is_none() {
                hint.publisher = meta.publisher.clone();
            }
            if hint.imprint.is_none() {
                hint.imprint = meta.imprint.clone();
            }
            if hint.age_rating.is_none() {
                hint.age_rating = meta.age_rating.clone();
            }
            if hint.total_issues.is_none() {
                hint.total_issues = meta.total_issues;
            }
            if hint.volume.is_none() {
                hint.volume = meta.volume;
            }
            if hint.comicvine_id.is_none() {
                hint.comicvine_id = meta.comicid;
            }
        }
        let resolved = crate::library::identity::resolve_or_create(
            &state.db,
            lib.id,
            &folder,
            &hint,
            &lib.default_language,
        )
        .await?;
        if resolved.was_created() {
            stats.series_created += 1;
        }
        resolved.id()
    };
    stats.record_phase_parallel("identity", identity_started.elapsed());

    state.events.emit(ScanEvent::SeriesUpdated {
        library_id: lib.id,
        series_id,
        name: folder_label,
    });

    // Spec §9: one transaction per series, batched at `scan_batch_size` so
    // very large series don't hold a single giant transaction. Per-batch
    // failures roll back that batch only — the next batches still commit
    // and the next scan re-tries the failed files.
    let batch_size = state.cfg().scan_batch_size.max(1);
    let manifest = process::IssueManifest::for_paths(&state.db, &archives).await?;
    // F-2: pre-fetch every existing slug for this series in one round-trip so
    // the per-archive INSERT path picks slugs against an in-memory HashSet
    // instead of issuing a `SELECT COUNT(*)` for every candidate.
    let mut slug_set = crate::slug::fetch_issue_slugs_for_series(&state.db, series_id).await?;
    let mut candidates = Vec::new();
    for path in &archives {
        stats.files_seen += 1;
        if let Some(tracker) = &live_progress {
            tracker.record_file_seen();
        }
        let path_str = path.to_string_lossy().into_owned();
        match process::file_fingerprint(path) {
            Ok((size, mtime)) => {
                if !force
                    && let Some(row) = manifest.by_path(&path_str)
                    && process::row_metadata_is_current(&row, size, mtime)
                {
                    let before_stats = stats.clone();
                    stats.files_unchanged += 1;
                    health.touch_file(path);
                    if let Some(tracker) = &live_progress {
                        tracker.record_file_done(&before_stats, stats);
                    }
                    continue;
                }
                candidates.push((path.clone(), Some((size, mtime))));
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "scan: metadata read failed; ingest will report");
                candidates.push((path.clone(), None));
            }
        }
    }

    let db_write_started = Instant::now();
    for chunk in candidates.chunks(batch_size) {
        let txn = state.db.begin().await?;
        // F-12: scan transactions can recover from a crash by re-running
        // (idempotent; `scan_runs.state` is the durable signal of completion).
        // Trade WAL fsync sync semantics for batch throughput — on crash we
        // lose at most ~200 ms of committed WAL, which a re-run replays. This
        // is safe because the writes here are scan-only data the next scan
        // will re-derive. Scoped to this txn via `LOCAL` — does NOT affect
        // any other concurrent connection or session.
        if let Err(e) = txn
            .execute_unprepared("SET LOCAL synchronous_commit = OFF")
            .await
        {
            tracing::warn!(error = %e, "scanner: SET LOCAL synchronous_commit=OFF failed; continuing with default");
        }
        let mut chunk_ok = true;
        for (path, fingerprint) in chunk {
            let before_stats = stats.clone();
            let ingest = if let Some((size, mtime)) = fingerprint {
                process::ingest_one_with_fingerprint(
                    state,
                    &txn,
                    lib,
                    path,
                    series_id,
                    Some(&manifest),
                    Some(&mut slug_set),
                    *size,
                    *mtime,
                    stats,
                    health,
                    force,
                )
                .await
            } else {
                process::ingest_one(
                    state,
                    &txn,
                    lib,
                    path,
                    series_id,
                    Some(&manifest),
                    Some(&mut slug_set),
                    stats,
                    health,
                    force,
                )
                .await
            };
            if let Some(tracker) = &live_progress {
                tracker.record_file_done(&before_stats, stats);
            }
            if let Err(e) = ingest {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "scan: ingest failed (batch will roll back)",
                );
                chunk_ok = false;
                break;
            }
        }
        if chunk_ok {
            txn.commit().await?;
        } else if let Err(e) = txn.rollback().await {
            tracing::warn!(error = %e, "scanner: transaction rollback failed");
        }
    }
    stats.record_phase_parallel("db_write", db_write_started.elapsed());

    // Stamp the series's last_scanned_at if we actually saw it.
    if let Some(s) = entity::series::Entity::find_by_id(series_id)
        .one(&state.db)
        .await?
    {
        let mut am: series::ActiveModel = s.into();
        am.last_scanned_at = Set(Some(Utc::now().fixed_offset()));
        am.update(&state.db).await?;
    }

    // Roll up genre/tag/credit junctions from this series's active issues.
    // Best-effort — a stale rollup is cosmetic and the next scan retries.
    let rollup_started = Instant::now();
    metadata_rollup::rollup_series_metadata_best_effort(&state.db, series_id).await;
    stats.record_phase_parallel("metadata_rollup", rollup_started.elapsed());

    // Refresh series.total_issues / status / summary using (in
    // priority order) the parsed series.json sidecar, then per-issue
    // ComicInfo Count MAX-reduction. The sidecar wins when present —
    // it's authoritative per-series metadata, vs. Count which is
    // per-issue inferred. Done after the commit + rollup so the
    // helper sees fully-flushed issue rows.
    // Best-effort: a missed reconcile just leaves the previous values
    // in place; the next scan retries.
    if !defer_status_reconcile
        && let Err(e) =
            reconcile_status::reconcile_series_status(&state.db, series_id, series_json.as_ref())
                .await
    {
        tracing::warn!(error = %e, "scanner: reconcile_series_status failed");
    }

    Ok(ProcessFolderOutcome {
        series_id: Some(series_id),
        processed: true,
        series_json,
        seen_paths: archives
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    })
}

#[derive(Debug, Default)]
struct ProcessFolderOutcome {
    series_id: Option<Uuid>,
    processed: bool,
    series_json: Option<parsers::series_json::SeriesMetadata>,
    seen_paths: Vec<String>,
}
