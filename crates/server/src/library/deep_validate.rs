//! Tranche C of recovery-visibility — admin-triggered, per-library
//! deep page-decode validation.
//!
//! The regular scan deliberately does NOT decode page bytes; it only
//! reads headers + metadata for speed (a single scan of a 20K-issue
//! library takes minutes). But that means a CBZ whose LFH/CDFH is
//! intact while its compressed page bytes are corrupt past the image
//! header passes scan cleanly — the failure surfaces only when the
//! reader hits the bad page at read time.
//!
//! This module fills that gap. Operators trigger
//! `run(state, library_id)` via `POST /libraries/{slug}/validate-deeply`;
//! the worker walks every active issue in the library, reads every
//! page entry, and runs it through `image::load_from_memory`. Decode
//! failures land as `IssueKind::UnreadablePage` health-issues so the
//! admin Health tab + per-issue badge surfaces start flagging them.
//!
//! **Cost.** Each page is decompressed and decoded — typical pages
//! are 0.5-3 MB compressed, decoding takes 10-100ms each. A 20K-issue
//! library with 22 pages per issue averages ~440K decode operations,
//! ~1-2 hours wall clock on a single core. Operator-only opt-in;
//! never automatic.
//!
//! **Concurrency.** Honors the existing `archive_work_semaphore` so
//! deep-validate runs at the same parallelism as normal scans.
//!
//! **No persistence.** Spawned via `tokio::spawn`; a server restart
//! abandons the in-flight run. Re-trigger if needed. A full apalis
//! job (queued, restart-safe) would be the obvious follow-up but
//! out of scope for v1.

use crate::library::health::{HealthCollector, IssueKind};
use crate::state::AppState;
use entity::{issue, library, scan_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::path::PathBuf;
use std::time::Instant;
use uuid::Uuid;

/// Statistics for a deep-validate run. Returned to the caller (or
/// logged from the spawned task) so operators have a summary even
/// without per-page tracing.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeepValidateStats {
    /// Issues opened successfully and probed.
    pub issues_probed: u32,
    /// Issues that couldn't be opened (already surfaced as
    /// `MalformedComicInfo` etc. by the regular scan; we don't
    /// re-emit, just count).
    pub issues_unopenable: u32,
    /// Pages successfully decoded.
    pub pages_decoded: u32,
    /// Pages that failed to decode — one `UnreadablePage` health-
    /// issue emitted per such failure.
    pub pages_unreadable: u32,
}

/// Run a full deep-validate of `library_id`. Walks every active
/// issue, decodes every page, emits `UnreadablePage` for each
/// failure. Errors that occur outside of per-page decoding (e.g.,
/// library not found, DB unreachable) bubble up; per-issue and
/// per-page failures are tallied in the returned `DeepValidateStats`.
pub async fn run(state: &AppState, library_id: Uuid) -> anyhow::Result<DeepValidateStats> {
    let _lib = library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("library not found: {library_id}"))?;

    // Open a real `scan_runs` row so the FK from `library_health_issues.scan_id`
    // resolves and the History tab can show this run. `kind="deep_validate"`
    // is a new kind value — the existing History filter chips (`library`,
    // `series`, `issue`) just won't show it; a future enhancement can add a
    // chip for it.
    let scan_id = Uuid::now_v7();
    let started_at = chrono::Utc::now().fixed_offset();
    let started_instant = Instant::now();
    scan_run::ActiveModel {
        id: Set(scan_id),
        library_id: Set(library_id),
        state: Set("running".into()),
        started_at: Set(started_at),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("deep_validate".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&state.db)
    .await?;
    let mut health = HealthCollector::new(library_id, scan_id, started_at);

    let issues = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await?;

    let mut stats = DeepValidateStats::default();
    let archive_limits = state.cfg().archive_limits();

    for issue_row in issues {
        let path = PathBuf::from(&issue_row.file_path);
        let outcome = probe_one(state, &path, archive_limits, &mut health, &mut stats).await;
        if outcome.is_err() {
            stats.issues_unopenable += 1;
        }
        // Yield between issues so the spawned task is preemptible by
        // other tokio work (HTTP requests, scan-event broadcasts).
        tokio::task::yield_now().await;
    }

    // Finalize: upsert all emitted rows in one batch, auto-resolve
    // any prior `UnreadablePage` rows from a previous run that
    // didn't re-emit this time (the file was repacked, the bad page
    // was fixed, etc.). HealthCollector::finalize already handles
    // this.
    if let Err(e) = health.finalize(&state.db).await {
        tracing::warn!(error = %e, library_id = %library_id, "deep-validate finalize failed");
    }

    // Close the `scan_runs` row so the History tab shows the run as
    // complete with its stats payload.
    let elapsed_ms = started_instant.elapsed().as_millis() as i64;
    if let Some(existing) = scan_run::Entity::find_by_id(scan_id).one(&state.db).await? {
        let mut am: scan_run::ActiveModel = existing.into();
        am.state = Set("complete".into());
        am.ended_at = Set(Some(chrono::Utc::now().fixed_offset()));
        am.stats = Set(serde_json::json!({
            "issues_probed": stats.issues_probed,
            "issues_unopenable": stats.issues_unopenable,
            "pages_decoded": stats.pages_decoded,
            "pages_unreadable": stats.pages_unreadable,
            "elapsed_ms": elapsed_ms,
        }));
        am.update(&state.db).await?;
    }

    tracing::info!(
        library_id = %library_id,
        ?stats,
        elapsed_ms,
        "deep-validate complete",
    );
    Ok(stats)
}

/// Open one archive and decode every page entry. Per-page failures
/// are emitted into `health`; the function returns `Err` only if the
/// archive itself can't be opened (which the regular scan already
/// flags as `MalformedComicInfo` etc., so we don't re-emit here).
async fn probe_one(
    state: &AppState,
    path: &std::path::Path,
    archive_limits: archive::ArchiveLimits,
    health: &mut HealthCollector,
    stats: &mut DeepValidateStats,
) -> anyhow::Result<()> {
    // Defer to blocking work — decode is CPU-heavy and we don't want
    // to block the runtime. Honor the existing archive permit pool.
    let _permit = state
        .archive_work_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| anyhow::anyhow!("archive permit closed: {e}"))?;
    let path_owned = path.to_path_buf();
    let path_for_blocking = path_owned.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Vec<DecodeResult> {
        let Ok(mut archive) = archive::open(&path_for_blocking, archive_limits) else {
            return Vec::new();
        };
        let page_names: Vec<String> = archive.pages().iter().map(|e| e.name.clone()).collect();
        let mut results: Vec<DecodeResult> = Vec::with_capacity(page_names.len());
        for (idx, name) in page_names.iter().enumerate() {
            match archive.read_entry_bytes(name) {
                Ok(bytes) => match image::load_from_memory(&bytes) {
                    Ok(_) => results.push(DecodeResult::Ok),
                    Err(e) => results.push(DecodeResult::DecodeError {
                        page_index: idx as u32,
                        error: e.to_string(),
                    }),
                },
                Err(e) => results.push(DecodeResult::ReadError {
                    page_index: idx as u32,
                    error: e.to_string(),
                }),
            }
        }
        results
    })
    .await
    .map_err(|e| anyhow::anyhow!("decode task panicked: {e}"))?;

    if outcome.is_empty() {
        return Err(anyhow::anyhow!("archive open failed"));
    }
    stats.issues_probed += 1;
    for r in outcome {
        match r {
            DecodeResult::Ok => stats.pages_decoded += 1,
            DecodeResult::DecodeError { page_index, error }
            | DecodeResult::ReadError { page_index, error } => {
                stats.pages_unreadable += 1;
                health.emit(IssueKind::UnreadablePage {
                    path: path_owned.clone(),
                    page_index,
                    error,
                });
            }
        }
    }
    Ok(())
}

enum DecodeResult {
    Ok,
    DecodeError { page_index: u32, error: String },
    ReadError { page_index: u32, error: String },
}
