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
//! **Lifecycle.** Every run owns one `scan_runs` row (`kind =
//! deep_validate`), and the run closes it on its way out: `complete` with
//! the stats, `failed` with the error (a panic in the walk included), or
//! `cancelled`. The scan-run cancel (`POST …/scan-runs/{id}/cancel`, from
//! the Live scan page or Background work) flips the row; the walk polls it
//! every [`CANCEL_POLL_INTERVAL`] and stops, and the closing write only
//! lands `WHERE state = 'running'`, so it never clobbers a cancel. The
//! closing write is retried; a row it still cannot close, like one whose
//! process died, reads as a stalled task on Background work and is closed
//! `failed` by [`close_interrupted_runs`] at the next boot. A cancelled or
//! failed run still records the findings it made but resolves nothing: it
//! did not look at every issue.
//!
//! **No persistence.** Spawned via `tokio::spawn`; a server restart
//! abandons the in-flight run. Re-trigger if needed. While it runs it is
//! registered as a background task carrying its run id, which is how
//! `/admin/background-work` tells a live run from an abandoned row (one
//! server process per database, as the task registry already assumes).

use crate::library::health::{HealthCollector, IssueKind};
use crate::state::AppState;
use entity::scan_run::{self, ScanRunKind, ScanRunState};
use entity::{issue, library, library_health_issue};
use futures::FutureExt;
use futures::future::BoxFuture;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QuerySelect, Set,
};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Background-task kind (and `TaskWorkView.kind`) of a deep-validate run.
pub const TASK_KIND: &str = "deep_validate";

/// How often the walk re-reads its row for a cancel.
pub const CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Attempts at the closing write before the row is left to the boot sweep.
const CLOSE_ATTEMPTS: u32 = 3;

/// `scan_runs.error` of a run a server restart interrupted.
pub const INTERRUPTED_ERROR: &str = "Interrupted by a server restart";

/// Statistics for a deep-validate run. Returned to the caller (or
/// logged from the spawned task) so operators have a summary even
/// without per-page tracing.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeepValidateStats {
    /// Issues opened successfully and probed.
    pub issues_probed: u32,
    /// Issues that couldn't be opened (already surfaced as
    /// `MalformedArchive` etc. by the regular scan; we don't
    /// re-emit, just count).
    pub issues_unopenable: u32,
    /// Pages successfully decoded.
    pub pages_decoded: u32,
    /// Pages that failed to decode — one `UnreadablePage` health-
    /// issue emitted per such failure.
    pub pages_unreadable: u32,
    /// The run stopped early because its row was cancelled.
    pub cancelled: bool,
}

/// Called after each issue with the number walked so far; an `Err`
/// aborts the run as `failed` with that message. Test seam.
pub type AfterIssueHook = Arc<dyn Fn(u32) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

/// Knobs for [`run_with`]. [`run`] uses the defaults.
#[derive(Default, Clone)]
pub struct RunOptions {
    pub after_issue: Option<AfterIssueHook>,
    /// Overrides [`CANCEL_POLL_INTERVAL`] (`Duration::ZERO` polls before
    /// every issue).
    pub cancel_poll_interval: Option<Duration>,
}

/// Run a full deep-validate of `library_id`. Walks every active
/// issue, decodes every page, emits `UnreadablePage` for each
/// failure. Per-issue and per-page failures are tallied in the
/// returned `DeepValidateStats`; anything else (library not found, DB
/// unreachable) is an `Err`, and once the run row exists it is closed
/// `failed` with that error first.
pub async fn run(state: &AppState, library_id: Uuid) -> anyhow::Result<DeepValidateStats> {
    run_with(state, library_id, RunOptions::default()).await
}

/// `true` while a deep validation of `library_id` runs in this process.
pub fn is_running(state: &AppState, library_id: Uuid) -> bool {
    state
        .running_background_tasks()
        .iter()
        .any(|t| t.kind == TASK_KIND && t.library_id == Some(library_id))
}

/// [`run`] with [`RunOptions`].
pub async fn run_with(
    state: &AppState,
    library_id: Uuid,
    opts: RunOptions,
) -> anyhow::Result<DeepValidateStats> {
    let _lib = library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("library not found: {library_id}"))?;

    // Open a real `scan_runs` row so the FK from `library_health_issues.scan_id`
    // resolves and the History tab can show this run.
    let scan_id = Uuid::now_v7();
    let started_at = chrono::Utc::now().fixed_offset();
    let started_instant = Instant::now();
    scan_run::ActiveModel {
        id: Set(scan_id),
        library_id: Set(library_id),
        state: Set(ScanRunState::Running),
        started_at: Set(started_at),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set(ScanRunKind::DeepValidate),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&state.db)
    .await?;
    let _task = state.track_background_run(TASK_KIND, Some(library_id), scan_id);

    // Scoped: the collector upserts what this run found but never runs the
    // library-wide auto-resolve sweep, which would close every other kind
    // of finding (a deep validate re-emits only `UnreadablePage`).
    let mut health = HealthCollector::new_scoped(library_id, scan_id, started_at);
    let mut stats = DeepValidateStats::default();
    let walked = AssertUnwindSafe(walk(
        state,
        library_id,
        scan_id,
        &opts,
        &mut health,
        &mut stats,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|panic| {
        Err(anyhow::anyhow!(
            "deep-validate panicked: {}",
            panic_message(panic.as_ref())
        ))
    });

    let mut ending = match walked {
        Ok(Walk::Finished) => Ending::Complete,
        Ok(Walk::Stopped) => Ending::Cancelled,
        Err(e) => Ending::Failed(format!("{e:#}")),
    };

    // Record the findings whatever the ending: a page that failed to
    // decode is broken even if the run stopped early.
    if let Err(e) = health.finalize(&state.db).await {
        tracing::warn!(error = %e, library_id = %library_id, "deep-validate finalize failed");
        if matches!(ending, Ending::Complete) {
            ending = Ending::Failed(format!("recording findings failed: {e:#}"));
        }
    }
    // Only a run that probed every issue may resolve an `UnreadablePage`
    // it did not re-emit (the file was repacked, the page fixed).
    if matches!(ending, Ending::Complete)
        && let Err(e) = resolve_stale_unreadable_pages(&state.db, library_id, started_at).await
    {
        tracing::warn!(error = %e, library_id = %library_id, "deep-validate resolve failed");
    }

    let elapsed_ms = started_instant.elapsed().as_millis() as i64;
    let stats_json = serde_json::json!({
        "issues_probed": stats.issues_probed,
        "issues_unopenable": stats.issues_unopenable,
        "pages_decoded": stats.pages_decoded,
        "pages_unreadable": stats.pages_unreadable,
        "elapsed_ms": elapsed_ms,
    });
    let mut attempt = 1;
    let closed_as = loop {
        match close_run(&state.db, scan_id, &ending, stats_json.clone()).await {
            Ok(st) => break st,
            Err(e) if attempt < CLOSE_ATTEMPTS => {
                tracing::warn!(error = %e, scan_id = %scan_id, attempt, "deep-validate close failed; retrying");
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                attempt += 1;
            }
            Err(e) => return Err(e.context("deep-validate: closing the run row")),
        }
    };
    stats.cancelled = closed_as == Some(ScanRunState::Cancelled);

    tracing::info!(
        library_id = %library_id,
        scan_id = %scan_id,
        ?stats,
        elapsed_ms,
        state = closed_as.map_or("missing", ScanRunState::as_str),
        "deep-validate finished",
    );
    match ending {
        Ending::Failed(e) => Err(anyhow::anyhow!(e)),
        Ending::Complete | Ending::Cancelled => Ok(stats),
    }
}

/// How the walk ended, before the row is closed.
enum Ending {
    Complete,
    Cancelled,
    Failed(String),
}

/// Whether the walk reached the last issue or stopped on a cancel.
enum Walk {
    Finished,
    Stopped,
}

/// Probe every active issue of the library, stopping when the run row
/// leaves `running` (cancelled through the scan-run cancel endpoint).
async fn walk(
    state: &AppState,
    library_id: Uuid,
    scan_id: Uuid,
    opts: &RunOptions,
    health: &mut HealthCollector,
    stats: &mut DeepValidateStats,
) -> anyhow::Result<Walk> {
    // Only the path is needed; `comic_info_raw` makes full rows heavy.
    let paths: Vec<String> = issue::Entity::find()
        .select_only()
        .column(issue::Column::FilePath)
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .into_tuple()
        .all(&state.db)
        .await?;

    let archive_limits = state.cfg().archive_limits();
    let poll_every = opts.cancel_poll_interval.unwrap_or(CANCEL_POLL_INTERVAL);
    let mut last_poll: Option<Instant> = None;
    let mut walked: u32 = 0;
    for file_path in paths {
        if last_poll.is_none_or(|t| t.elapsed() >= poll_every) {
            last_poll = Some(Instant::now());
            match run_state(&state.db, scan_id).await {
                Ok(Some(ScanRunState::Running)) => {}
                // Cancelled, or the row is gone (its library was deleted).
                Ok(_) => return Ok(Walk::Stopped),
                // A blip must not end an hours-long run; the next poll retries.
                Err(e) => {
                    tracing::warn!(error = %e, scan_id = %scan_id, "deep-validate: cancel poll failed")
                }
            }
        }
        let path = PathBuf::from(&file_path);
        let outcome = probe_one(state, &path, archive_limits, health, stats).await;
        if outcome.is_err() {
            stats.issues_unopenable += 1;
            // Its pages were not decoded this run, so its open findings
            // must survive the stale-`UnreadablePage` sweep.
            health.touch_file(&path);
        }
        walked += 1;
        if let Some(hook) = &opts.after_issue {
            hook(walked).await?;
        }
        // Yield between issues so the spawned task is preemptible by
        // other tokio work (HTTP requests, scan-event broadcasts).
        tokio::task::yield_now().await;
    }
    Ok(Walk::Finished)
}

/// The run row's state, `None` once the row is gone.
async fn run_state<C: ConnectionTrait>(
    db: &C,
    scan_id: Uuid,
) -> Result<Option<ScanRunState>, sea_orm::DbErr> {
    scan_run::Entity::find_by_id(scan_id)
        .select_only()
        .column(scan_run::Column::State)
        .into_tuple()
        .one(db)
        .await
}

/// Resolve open `UnreadablePage` rows of the library this run did not
/// re-emit. Every other kind belongs to the regular scan.
async fn resolve_stale_unreadable_pages(
    db: &DatabaseConnection,
    library_id: Uuid,
    started_at: chrono::DateTime<chrono::FixedOffset>,
) -> anyhow::Result<()> {
    library_health_issue::Entity::update_many()
        .col_expr(
            library_health_issue::Column::ResolvedAt,
            Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(library_health_issue::Column::LibraryId.eq(library_id))
        .filter(library_health_issue::Column::Kind.eq("UnreadablePage"))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .filter(library_health_issue::Column::LastSeenAt.lt(started_at))
        .exec(db)
        .await?;
    Ok(())
}

/// Close the run row. The terminal write only lands on a row still
/// `running`, so a cancel that arrived meanwhile stands; such a row (or
/// any already-terminal one) gets the partial stats and keeps its state.
/// Returns the state the row ended in (`None` if it is gone).
async fn close_run<C: ConnectionTrait>(
    db: &C,
    scan_id: Uuid,
    ending: &Ending,
    stats: serde_json::Value,
) -> anyhow::Result<Option<ScanRunState>> {
    let terminal = match ending {
        Ending::Complete => Some((ScanRunState::Complete, None)),
        // The cancel endpoint already set state / ended_at / error.
        Ending::Cancelled => None,
        Ending::Failed(e) => Some((ScanRunState::Failed, Some(e.clone()))),
    };
    if let Some((state, error)) = terminal {
        let res = scan_run::Entity::update_many()
            .col_expr(scan_run::Column::State, Expr::value(state))
            .col_expr(
                scan_run::Column::EndedAt,
                Expr::value(Some(chrono::Utc::now().fixed_offset())),
            )
            .col_expr(scan_run::Column::Error, Expr::value(error))
            .col_expr(scan_run::Column::Stats, Expr::value(stats.clone()))
            .filter(scan_run::Column::Id.eq(scan_id))
            .filter(scan_run::Column::State.eq(ScanRunState::Running))
            .exec(db)
            .await?;
        if res.rows_affected > 0 {
            return Ok(Some(state));
        }
    }
    scan_run::Entity::update_many()
        .col_expr(scan_run::Column::Stats, Expr::value(stats))
        .filter(scan_run::Column::Id.eq(scan_id))
        .exec(db)
        .await?;
    Ok(run_state(db, scan_id).await?)
}

/// Boot sweep: a deep-validate row still `running` belongs to a task the
/// previous process took down with it (runs are not persisted). Mark each
/// `failed` so nothing reads it as in progress. Returns the rows closed.
pub async fn close_interrupted_runs<C: ConnectionTrait>(db: &C) -> anyhow::Result<u64> {
    let res = scan_run::Entity::update_many()
        .col_expr(scan_run::Column::State, Expr::value(ScanRunState::Failed))
        .col_expr(
            scan_run::Column::EndedAt,
            Expr::value(Some(chrono::Utc::now().fixed_offset())),
        )
        .col_expr(
            scan_run::Column::Error,
            Expr::value(Some(INTERRUPTED_ERROR.to_owned())),
        )
        .filter(scan_run::Column::Kind.eq(ScanRunKind::DeepValidate))
        .filter(scan_run::Column::State.is_in([ScanRunState::Queued, ScanRunState::Running]))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Open one archive and decode every page entry. Per-page failures
/// are emitted into `health`; the function returns `Err` only if the
/// archive itself can't be opened (which the regular scan already
/// flags as `MalformedArchive` etc., so we don't re-emit here).
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
                Ok(bytes) => match crate::util::image_decode::decode_limited(&bytes) {
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
