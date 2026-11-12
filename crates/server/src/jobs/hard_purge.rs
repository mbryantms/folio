//! Daily hard-purge of confirmed-removed rows (roadmap WP-3.5, audit §3.1).
//!
//! The removal lifecycle (see `docs/dev/library-scanner.md`) is:
//!
//! 1. **Soft-deleted** — reconcile stamps `removed_at` when an issue's file
//!    disappears. Restorable by the scanner (file comes back) or an admin.
//! 2. **Confirmed** — the 04:00 auto-confirm sweep
//!    ([`crate::library::reconcile::auto_confirm_sweep`]) stamps
//!    `removal_confirmed_at` once `removed_at` is older than
//!    `library.soft_delete_days` (or an admin confirms early).
//! 3. **Purged** — this sweep (04:15) hard-`DELETE`s rows whose
//!    `removal_confirmed_at` is older than
//!    `soft_delete_days × library.hard_purge_multiplier` days (global
//!    setting, default 2; `0` disables purging). The window has a floor of
//!    [`MIN_PURGE_WINDOW_DAYS`] so a library with `soft_delete_days = 0`
//!    never purges a row the same day it was confirmed.
//!
//! **Only confirmed-removed rows are ever touched.** Both the candidate
//! `SELECT` and the `DELETE` itself carry the full guard
//! (`removed_at IS NOT NULL AND removal_confirmed_at IS NOT NULL AND
//! removal_confirmed_at < cutoff`), so a row restored between the two
//! statements survives.
//!
//! **Cascades.** Every FK that references `issues(id)` / `series(id)` is
//! `ON DELETE CASCADE` (markers, reading sessions, collection entries,
//! metadata junctions, covers, …) or `ON DELETE SET NULL` (CBL matches,
//! reprint links, …); the database does that work. References **without** an
//! FK are cleaned by this module in the same transaction as the row delete:
//! polymorphic `(kind, text id)` pairs ([`POLYMORPHIC_REFS`] — ratings, rail
//! dismissals, external ids, field provenance), plain unconstrained issue-id
//! columns whose rows die with the issue ([`UNCONSTRAINED_ISSUE_DELETES`] —
//! `progress_records`, which has no FK), and pointers that are just cleared
//! ([`UNCONSTRAINED_ISSUE_NULLIFIES`]). History tables (`audit_log`,
//! `library_events`, `scan_runs`) keep their ids on purpose. The integration
//! tests `hard_purge.rs::fk_references_all_cascade_or_set_null` and
//! `::unconstrained_id_columns_are_accounted_for` pin both halves, so a new
//! `RESTRICT` FK or a new FK-less reference fails CI rather than this sweep.
//!
//! **Only what is really gone.** A library that fails the scanner's
//! `validate_library` gate (root missing, unreadable, empty: an unmounted
//! share) is skipped whole for the run. Every candidate issue's
//! `file_path` is stat-ed first (on the blocking pool, with the scanner's
//! own notion of an indexable archive,
//! [`crate::library::scanner::enumerate::archive_presence`]): a file
//! that is back on disk is **not** deleted, its `removal_confirmed_at` is
//! cleared so the confirm sweep re-evaluates it, and its series gets one
//! coalesced (throttled) rescan that restores it. A file the disk will not
//! answer for (permission denied, I/O error, stale mount) is *unknown*, not
//! gone: kept as it is, confirmation and all. A candidate series whose
//! folder holds an indexable archive again (or cannot be read) is kept the
//! same way. After every stat batch the root must still hold an entry, and
//! `validate_library` runs again before every DELETE chunk, so a share that
//! unmounts mid-run stops the deletes.
//! Duplicates-page removals (deliberate; the file is on disk by definition)
//! are purged without the stat. One mapping, [`decide`], turns each disk
//! answer into the action for issues and series alike.
//!
//! **Bounded, but never stuck.** Candidate issues are read oldest
//! confirmation first in keyset pages on `(removal_confirmed_at, id)`, and
//! at most [`MAX_ISSUES_PER_LIBRARY_PER_RUN`] are deleted per library per
//! run. Kept rows cost no budget, so unknown rows (which stay confirmed, at
//! the front of the order) never stop a run from reaching the purgeable
//! rows behind them.
//!
//! **Series** are purged only when confirmed-removed past the same window
//! **and** left with no issue rows at all (any state). `issues.series_id`
//! cascades, so deleting a series that still owned rows would take live or
//! merely soft-deleted issues with it; the no-remaining-issues guard is part
//! of the `DELETE` for the same race reason as above.
//!
//! **Export before delete.** For every row about to go, one structured
//! `tracing::warn!` line (`target = "folio::hard_purge"`) records the ids,
//! path, content hash, timestamps, and per-row counts of the user data the
//! cascade will take (markers, progress rows, reading sessions, collection
//! entries, ratings). After the delete, a `library_events` row per purged
//! entity (`category = issue|series`, `action = purged`) lands on the
//! Library stream.
//!
//! Trade-off: the purged row's `content_hash` is what lets a re-appearing
//! file de-dupe back into the same issue id, so once purged a returning file
//! comes back as a *new* issue with no read state.

use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::library::reconcile::{
    KeptRows, MAX_SERIES_RESCANS_PER_RUN, PRESENCE_BATCH, Presence, PresenceProbe,
    library_ready_for_sweep,
};
use crate::state::AppState;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::library;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, Statement,
    TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Lower bound on the purge window, in days, regardless of
/// `soft_delete_days × multiplier`.
pub const MIN_PURGE_WINDOW_DAYS: i64 = 1;

/// Max issue rows **deleted** per library per run. A backlog (first run
/// after an upgrade on a high-churn library) drains over successive days
/// instead of one unbounded run of deletes, exports and cascades.
///
/// It counts rows actually deleted, not candidates examined: the candidates
/// are read in keyset pages of [`PRESENCE_BATCH`] on
/// `(removal_confirmed_at, id)`, and a row the run keeps (file present, or a
/// disk that will not answer) costs one stat and no budget, as does a row
/// the `DELETE`'s own guard rejects (restored or un-confirmed since the
/// read). So a run stops either after this many deletes or at the end of
/// the library's candidates, and kept rows at the front of the order can no
/// longer hold the rows behind them back: every run reaches the oldest
/// purgeable rows, and each run that stops early has deleted this many of
/// them.
pub const MAX_ISSUES_PER_LIBRARY_PER_RUN: usize = 5_000;

/// Rows per `DELETE … WHERE id IN (…)` statement (one transaction each).
const DELETE_CHUNK: usize = 500;

/// Tables that reference issues / series **polymorphically** (a
/// `(kind, text id)` pair, no FK) and so don't cascade. Each entry is
/// `(table, kind column, id column)`. Left behind they'd dangle forever and,
/// for `external_ids`, hold the `UNIQUE (source, external_id, entity_type)`
/// slot a re-imported copy of the same book needs.
pub const POLYMORPHIC_REFS: &[(&str, &str, &str)] = &[
    ("user_ratings", "target_type", "target_id"),
    ("rail_dismissals", "target_kind", "target_id"),
    ("external_ids", "entity_type", "entity_id"),
    ("field_provenance", "entity_type", "entity_id"),
];

/// Plain `issue_id`-style columns that point at `issues(id)` **without** an
/// FK, whose rows are user data owned by the issue and must go with it.
/// `(table, column)`. `progress_records` predates the FK convention (it has
/// no FK to `issues` or `users`), so the database won't cascade it.
pub const UNCONSTRAINED_ISSUE_DELETES: &[(&str, &str)] = &[("progress_records", "issue_id")];

/// Unconstrained columns that merely *mention* an issue id; the referencing
/// row stays, the dangling pointer is cleared. `(table, column)`.
pub const UNCONSTRAINED_ISSUE_NULLIFIES: &[(&str, &str)] = &[
    // In-place-modified file → the replacement issue.
    ("issues", "superseded_by"),
    // Provider-named first appearance we happened to have locally.
    ("character", "first_appearance_issue_id"),
];

/// The confirmed-removed-past-cutoff guard, shared by every SELECT and DELETE
/// so the two can never disagree about what's eligible. `$2` is the cutoff.
const CONFIRMED_PAST_CUTOFF: &str = "removed_at IS NOT NULL \
     AND removal_confirmed_at IS NOT NULL \
     AND removal_confirmed_at < $2";

/// Series-side guard: no issue rows of any state remain.
const SERIES_HAS_NO_ISSUES: &str =
    "NOT EXISTS (SELECT 1 FROM issues i WHERE i.series_id = series.id)";

/// Paging bounds for the issue purge. [`Default`] is the production values;
/// tests pass small ones through [`run_with_limits`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurgeLimits {
    /// Candidate issue rows read (and stat-ed) per keyset page.
    pub page: usize,
    /// Most issue rows deleted per library per run.
    pub max_issue_deletes: usize,
}

impl Default for PurgeLimits {
    fn default() -> Self {
        Self {
            page: PRESENCE_BATCH,
            max_issue_deletes: MAX_ISSUES_PER_LIBRARY_PER_RUN,
        }
    }
}

/// What the purge does with one candidate, given what the disk said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// Really gone: delete the row.
    Purge,
    /// Back on disk: keep the row, clear its confirmation, rescan its series.
    KeepUnconfirm,
    /// The disk would not say: keep the row exactly as it is, confirmed.
    KeepConfirmed,
}

/// The one mapping from a probe answer to the purge's action, shared by the
/// issue and series purges. Only [`Presence::Absent`] (the not-found class,
/// see [`Presence`]) ever leads to a delete.
fn decide(presence: Presence) -> Decision {
    match presence {
        Presence::Absent => Decision::Purge,
        Presence::Present => Decision::KeepUnconfirm,
        Presence::Unknown => Decision::KeepConfirmed,
    }
}

/// Totals for one sweep.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PurgeStats {
    pub issues: u64,
    pub series: u64,
    /// Libraries that had at least one row purged.
    pub libraries: u64,
    /// Candidate rows kept because their file (or series folder) is on disk
    /// again; their confirmation was cleared and a rescan queued.
    pub kept_present: u64,
    /// Candidate rows kept because the disk would not say (unreadable
    /// folder, I/O error); left confirmed, not rescanned.
    pub kept_unknown: u64,
    /// Scoped series rescans queued (or coalesced into one in flight).
    pub rescans_queued: u64,
    /// Library scans queued instead of series scans.
    pub library_scans_queued: u64,
    /// Series not rescanned because they were scanned recently.
    pub rescans_throttled: u64,
    /// Libraries skipped because `validate_library` refused them.
    pub libraries_skipped: u64,
    /// Libraries whose root became unavailable mid-run (deletes stopped).
    pub libraries_aborted: u64,
}

impl PurgeStats {
    /// Add one library's totals to the run's.
    fn absorb(&mut self, lib: &PurgeStats) {
        if lib.issues > 0 || lib.series > 0 {
            self.libraries += 1;
        }
        self.issues += lib.issues;
        self.series += lib.series;
        self.kept_present += lib.kept_present;
        self.kept_unknown += lib.kept_unknown;
        self.rescans_queued += lib.rescans_queued;
        self.library_scans_queued += lib.library_scans_queued;
        self.rescans_throttled += lib.rescans_throttled;
        self.libraries_skipped += lib.libraries_skipped;
        self.libraries_aborted += lib.libraries_aborted;
    }
}

/// Rows deleted so far in one library's run, counted as each `DELETE`
/// transaction commits so a later failure cannot lose them.
#[derive(Debug, Default, Clone, Copy)]
struct Purged {
    issues: u64,
    series: u64,
}

/// Cron entry point: reads the multiplier from the live config.
pub async fn run(state: &AppState) -> anyhow::Result<PurgeStats> {
    let multiplier = state.cfg().library_hard_purge_multiplier;
    run_with(state, multiplier, Utc::now().fixed_offset()).await
}

/// Purge window in days for a library, or `None` when purging is disabled
/// (`multiplier == 0`).
pub fn window_days(soft_delete_days: i32, multiplier: u32) -> Option<i64> {
    if multiplier == 0 {
        return None;
    }
    let base = i64::from(soft_delete_days.max(0));
    Some((base * i64::from(multiplier)).max(MIN_PURGE_WINDOW_DAYS))
}

/// Run one sweep against `db` as of `now`. Split from [`run`] so tests can
/// drive the window deterministically.
pub async fn run_with(
    state: &AppState,
    multiplier: u32,
    now: DateTime<FixedOffset>,
) -> anyhow::Result<PurgeStats> {
    run_with_limits(state, multiplier, now, PurgeLimits::default()).await
}

/// [`run_with`] with explicit paging bounds (tests).
#[doc(hidden)]
pub async fn run_with_limits(
    state: &AppState,
    multiplier: u32,
    now: DateTime<FixedOffset>,
    limits: PurgeLimits,
) -> anyhow::Result<PurgeStats> {
    let mut stats = PurgeStats::default();
    if multiplier == 0 {
        return Ok(stats);
    }
    let libs = library::Entity::find().all(&state.db).await?;
    for lib in libs {
        let Some(days) = window_days(lib.soft_delete_days, multiplier) else {
            continue;
        };
        if !library_ready_for_sweep(state, &lib, "Hard purge").await {
            stats.libraries_skipped += 1;
            continue;
        }
        let cutoff = now - Duration::days(days);
        let (lib_stats, failed) = purge_library(state, &lib, days, cutoff, limits).await;
        // Rows committed before a failure are gone: they are counted (and
        // already have their library events) either way.
        stats.absorb(&lib_stats);
        // One library failing must not stop the others being purged.
        if let Some(e) = failed {
            stats.libraries_aborted += 1;
            tracing::error!(
                library_id = %lib.id,
                issues_purged = lib_stats.issues,
                series_purged = lib_stats.series,
                error = %e,
                "hard purge: library failed",
            );
        }
    }
    if stats.issues > 0 {
        metrics::counter!("folio_library_hard_purged_total", "kind" => "issue")
            .increment(stats.issues);
    }
    if stats.series > 0 {
        metrics::counter!("folio_library_hard_purged_total", "kind" => "series")
            .increment(stats.series);
    }
    Ok(stats)
}

#[derive(Debug, FromQueryResult)]
struct IssueCandidate {
    id: String,
    series_id: Uuid,
    slug: String,
    file_path: String,
    content_hash: String,
    removed_at: DateTime<FixedOffset>,
    removal_confirmed_at: DateTime<FixedOffset>,
    duplicate_removed: bool,
}

#[derive(Debug, FromQueryResult)]
struct SeriesCandidate {
    id: Uuid,
    slug: String,
    name: String,
    folder_path: Option<String>,
    removed_at: DateTime<FixedOffset>,
    removal_confirmed_at: DateTime<FixedOffset>,
}

#[derive(Debug, FromQueryResult)]
struct IdCount {
    id: String,
    n: i64,
}

#[derive(Debug, FromQueryResult)]
struct DeletedId {
    id: String,
}

async fn purge_library(
    state: &AppState,
    lib: &library::Model,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    limits: PurgeLimits,
) -> (PurgeStats, Option<anyhow::Error>) {
    let probe = PresenceProbe::for_library(lib);
    let mut kept = KeptRows::default();
    let mut purged = Purged::default();
    let result = purge_rows(
        state,
        lib,
        &probe,
        window_days,
        cutoff,
        limits,
        &mut kept,
        &mut purged,
    )
    .await;
    // Rows kept before an abort are still rescanned and reported.
    let outcome = kept
        .finish(state, lib.id, "Hard purge", MAX_SERIES_RESCANS_PER_RUN)
        .await;
    let (aborted, failed) = match result {
        Ok(aborted) => (aborted, None),
        Err(e) => (false, Some(e)),
    };
    let stats = PurgeStats {
        issues: purged.issues,
        series: purged.series,
        kept_present: kept.present(),
        kept_unknown: kept.unknown(),
        rescans_queued: outcome.series_scans,
        library_scans_queued: outcome.library_scans,
        rescans_throttled: outcome.throttled,
        libraries_aborted: u64::from(aborted),
        ..PurgeStats::default()
    };
    (stats, failed)
}

/// Purge one library's issues, then its series, adding each committed
/// delete to `purged`. `Ok(true)`: the library root stopped validating
/// before a DELETE chunk, so nothing more was deleted.
#[allow(clippy::too_many_arguments)]
async fn purge_rows(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    limits: PurgeLimits,
    kept: &mut KeptRows,
    purged: &mut Purged,
) -> anyhow::Result<bool> {
    let aborted = purge_issues(
        state,
        lib,
        probe,
        window_days,
        cutoff,
        limits,
        kept,
        &mut purged.issues,
    )
    .await?;
    if aborted {
        return Ok(true);
    }
    purge_series(
        state,
        lib,
        probe,
        window_days,
        cutoff,
        kept,
        &mut purged.series,
    )
    .await
}

/// Clear `removal_confirmed_at` on the still-eligible `ids` of `table`
/// (`issues` / `series`) so the confirm sweep re-evaluates them.
async fn unconfirm<T>(
    db: &DatabaseConnection,
    table: &str,
    library_id: Uuid,
    cutoff: DateTime<FixedOffset>,
    ids: &[T],
) -> anyhow::Result<()>
where
    T: Clone + Into<sea_orm::Value>,
{
    for chunk in ids.chunks(DELETE_CHUNK) {
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "UPDATE {table} SET removal_confirmed_at = NULL \
                 WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF} AND id IN ({})",
                placeholders(3, chunk.len()),
            ),
            with_ids([library_id.into(), cutoff.into()], chunk),
        ))
        .await?;
    }
    Ok(())
}

/// Walk the library's candidates in keyset pages on
/// `(removal_confirmed_at, id)`, oldest confirmation first, deleting what is
/// really gone until [`PurgeLimits::max_issue_deletes`] rows were deleted or
/// the candidates run out. Kept rows, and rows the in-transaction guard
/// rejects (restored or un-confirmed since the read), cost no budget, so
/// neither stops the walk from reaching the purgeable rows behind them.
/// Every committed delete is added to `purged` as it commits. `Ok(true)`:
/// aborted (the root stopped validating).
#[allow(clippy::too_many_arguments)]
async fn purge_issues(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    limits: PurgeLimits,
    kept: &mut KeptRows,
    purged: &mut u64,
) -> anyhow::Result<bool> {
    let db = &state.db;
    let page_size = limits.page.max(1);
    let mut budget = limits.max_issue_deletes;
    let mut after: Option<(DateTime<FixedOffset>, String)> = None;
    while budget > 0 {
        let page = issue_candidate_page(db, lib.id, cutoff, after.as_ref(), page_size).await?;
        let Some(last) = page.last() else {
            break;
        };
        let full_page = page.len() == page_size;
        // The cursor advances past the whole page whatever happens to its
        // rows, so the walk always terminates.
        after = Some((last.removal_confirmed_at, last.id.clone()));

        let mut to_purge = settle_issue_page(db, lib.id, cutoff, probe, page, kept).await?;
        // At most `budget` rows per DELETE batch; a batch the guard partly
        // rejects leaves budget for the page's next purgeable rows. Each
        // batch takes at least one row, so this ends. Rows still left when
        // the budget runs out stay confirmed for the next run, which reaches
        // them first among the purgeable ones (oldest first).
        while budget > 0 && !to_purge.is_empty() {
            let rest = to_purge.split_off(budget.min(to_purge.len()));
            let before = *purged;
            let aborted = delete_issues(state, lib, window_days, cutoff, to_purge, purged).await?;
            let deleted = usize::try_from(*purged - before).unwrap_or(usize::MAX);
            budget = budget.saturating_sub(deleted);
            if aborted {
                return Ok(true);
            }
            to_purge = rest;
        }
        if !full_page {
            break;
        }
    }
    Ok(false)
}

/// One keyset page of confirmed-past-cutoff issue candidates after `after`.
async fn issue_candidate_page(
    db: &DatabaseConnection,
    library_id: Uuid,
    cutoff: DateTime<FixedOffset>,
    after: Option<&(DateTime<FixedOffset>, String)>,
    limit: usize,
) -> anyhow::Result<Vec<IssueCandidate>> {
    let select = format!(
        "SELECT id, series_id, slug, file_path, content_hash, removed_at, \
                removal_confirmed_at, \
                EXISTS (SELECT 1 FROM issue_duplicate_decision d \
                        WHERE d.issue_id = issues.id AND d.decision = 'remove') \
                  AS duplicate_removed \
         FROM issues \
         WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF}"
    );
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let stmt = match after {
        None => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("{select} ORDER BY removal_confirmed_at, id LIMIT $3"),
            [library_id.into(), cutoff.into(), limit.into()],
        ),
        Some((at, id)) => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "{select} AND (removal_confirmed_at, id) > ($4, $5) \
                 ORDER BY removal_confirmed_at, id LIMIT $3"
            ),
            [
                library_id.into(),
                cutoff.into(),
                limit.into(),
                (*at).into(),
                id.clone().into(),
            ],
        ),
    };
    Ok(IssueCandidate::find_by_statement(stmt).all(db).await?)
}

/// Stat one page and settle its kept rows ([`decide`]); returns the rows to
/// delete, in page order. Present: kept, confirmation cleared, series
/// rescanned. Unknown: kept as it is. Duplicates-page removals skip the stat.
async fn settle_issue_page(
    db: &DatabaseConnection,
    library_id: Uuid,
    cutoff: DateTime<FixedOffset>,
    probe: &PresenceProbe,
    page: Vec<IssueCandidate>,
    kept: &mut KeptRows,
) -> anyhow::Result<Vec<IssueCandidate>> {
    let to_stat: Vec<((String, Uuid), String)> = page
        .iter()
        .filter(|c| !c.duplicate_removed)
        .map(|c| ((c.id.clone(), c.series_id), c.file_path.clone()))
        .collect();
    let mut present_ids: Vec<String> = Vec::new();
    let mut keep: HashSet<String> = HashSet::new();
    for ((id, series_id), presence) in probe.files(to_stat).await? {
        match decide(presence) {
            Decision::Purge => {}
            Decision::KeepUnconfirm => {
                kept.rescan.insert(series_id);
                keep.insert(id.clone());
                present_ids.push(id);
            }
            Decision::KeepConfirmed => {
                keep.insert(id.clone());
                kept.unknown_issues.push(id);
            }
        }
    }
    if !present_ids.is_empty() {
        unconfirm(db, "issues", library_id, cutoff, &present_ids).await?;
        kept.present_issues.extend(present_ids);
    }
    Ok(page.into_iter().filter(|c| !keep.contains(&c.id)).collect())
}

/// Export, delete (re-checking the guard inside each transaction) and log
/// `candidates`. Each committed chunk is added to `purged` and gets its
/// library events at once, so a later failure cannot lose either.
/// `Ok(true)`: aborted (the root stopped validating).
async fn delete_issues(
    state: &AppState,
    lib: &library::Model,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    candidates: Vec<IssueCandidate>,
    purged: &mut u64,
) -> anyhow::Result<bool> {
    if candidates.is_empty() {
        return Ok(false);
    }
    let db = &state.db;
    let library_id = lib.id;
    let ids: Vec<String> = candidates.iter().map(|c| c.id.clone()).collect();
    let markers = count_by(db, "markers", "issue_id", &ids).await?;
    let progress = count_by(db, "progress_records", "issue_id", &ids).await?;
    let sessions = count_by(db, "reading_sessions", "issue_id", &ids).await?;
    let collection_entries = count_by(db, "collection_entries", "issue_id", &ids).await?;
    let ratings = count_ratings(db, "issue", &ids).await?;
    let get = |m: &HashMap<String, i64>, id: &str| m.get(id).copied().unwrap_or(0);

    // Export-to-log BEFORE the delete: once the cascade runs these counts
    // are unrecoverable.
    for c in &candidates {
        tracing::warn!(
            target: "folio::hard_purge",
            library_id = %library_id,
            issue_id = %c.id,
            series_id = %c.series_id,
            slug = %c.slug,
            file_path = %c.file_path,
            content_hash = %c.content_hash,
            removed_at = %c.removed_at.to_rfc3339(),
            removal_confirmed_at = %c.removal_confirmed_at.to_rfc3339(),
            window_days,
            markers = get(&markers, &c.id),
            progress_records = get(&progress, &c.id),
            reading_sessions = get(&sessions, &c.id),
            collection_entries = get(&collection_entries, &c.id),
            ratings = get(&ratings, &c.id),
            "hard purge: deleting confirmed-removed issue",
        );
    }

    let by_id: HashMap<&str, &IssueCandidate> =
        candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let event = |c: &IssueCandidate| {
        NewEvent::new(
            library_id,
            Category::Issue,
            Action::Purged,
            Severity::Warning,
            format!("Purged issue {} (removal confirmed)", c.slug),
        )
        .entity("issue", c.id.clone(), Some(c.slug.clone()))
        .detail(serde_json::json!({
            "series_id": c.series_id,
            "file_path": c.file_path,
            "content_hash": c.content_hash,
            "removed_at": c.removed_at.to_rfc3339(),
            "removal_confirmed_at": c.removal_confirmed_at.to_rfc3339(),
            "window_days": window_days,
            "markers": get(&markers, &c.id),
            "progress_records": get(&progress, &c.id),
            "reading_sessions": get(&sessions, &c.id),
            "collection_entries": get(&collection_entries, &c.id),
            "ratings": get(&ratings, &c.id),
        }))
    };

    let mut deleted = 0usize;
    let mut aborted = false;
    for chunk in ids.chunks(DELETE_CHUNK) {
        // The root must still validate before every delete (a share can
        // unmount mid-run).
        if !library_ready_for_sweep(state, lib, "Hard purge").await {
            aborted = true;
            break;
        }
        let txn = db.begin().await?;
        let rows = DeletedId::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "DELETE FROM issues \
                 WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF} AND id IN ({}) \
                 RETURNING id",
                placeholders(3, chunk.len()),
            ),
            with_ids([library_id.into(), cutoff.into()], chunk),
        ))
        .all(&txn)
        .await?;
        let gone: Vec<String> = rows.into_iter().map(|r| r.id).collect();
        clean_unconstrained_refs(&txn, "issue", &gone).await?;
        txn.commit().await?;
        *purged += gone.len() as u64;
        deleted += gone.len();
        let events: Vec<NewEvent> = gone
            .iter()
            .filter_map(|id| by_id.get(id.as_str()))
            .map(|c| event(c))
            .collect();
        event_log::record_many(db, events).await;
    }
    log_kept(library_id, "issue", ids.len(), deleted);
    Ok(aborted)
}

async fn purge_series(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    kept: &mut KeptRows,
    purged: &mut u64,
) -> anyhow::Result<bool> {
    let db = &state.db;
    let library_id = lib.id;
    let candidates = SeriesCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT id, slug, name, folder_path, removed_at, removal_confirmed_at \
             FROM series \
             WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF} AND {SERIES_HAS_NO_ISSUES} \
             ORDER BY removal_confirmed_at, id"
        ),
        [library_id.into(), cutoff.into()],
    ))
    .all(db)
    .await?;
    if candidates.is_empty() {
        return Ok(false);
    }

    // Stat guard: a series whose folder holds an indexable archive again is
    // kept (unconfirmed) for the rescan that will re-ingest it; one whose
    // folder cannot be read is kept as it is. It owns no issue rows, so
    // nothing is excluded from the folder check.
    let folders: Vec<(Uuid, String)> = candidates
        .iter()
        .filter_map(|c| c.folder_path.clone().map(|f| (c.id, f)))
        .collect();
    let presence = probe.folders(folders, HashSet::new()).await?;
    let mut present_ids: Vec<Uuid> = Vec::new();
    for (id, p) in &presence {
        match decide(*p) {
            Decision::Purge => {}
            Decision::KeepUnconfirm => present_ids.push(*id),
            Decision::KeepConfirmed => {
                kept.unknown_series.insert(*id);
            }
        }
    }
    if !present_ids.is_empty() {
        unconfirm(db, "series", library_id, cutoff, &present_ids).await?;
        kept.rescan.extend(present_ids.iter().copied());
        kept.present_series.extend(present_ids.iter().copied());
    }
    let candidates: Vec<SeriesCandidate> = candidates
        .into_iter()
        .filter(|c| {
            presence
                .get(&c.id)
                .is_none_or(|p| decide(*p) == Decision::Purge)
        })
        .collect();
    if candidates.is_empty() {
        return Ok(false);
    }

    let ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();
    let id_strs: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    // Markers / progress / sessions all hang off issues, which are already
    // gone by the time a series qualifies; what's left is series-scoped.
    let collection_entries = count_by(db, "collection_entries", "series_id", &ids).await?;
    let ratings = count_ratings(db, "series", &id_strs).await?;
    let get = |m: &HashMap<String, i64>, id: &str| m.get(id).copied().unwrap_or(0);

    for c in &candidates {
        let key = c.id.to_string();
        tracing::warn!(
            target: "folio::hard_purge",
            library_id = %library_id,
            series_id = %c.id,
            slug = %c.slug,
            name = %c.name,
            folder_path = c.folder_path.as_deref().unwrap_or(""),
            removed_at = %c.removed_at.to_rfc3339(),
            removal_confirmed_at = %c.removal_confirmed_at.to_rfc3339(),
            window_days,
            collection_entries = get(&collection_entries, &key),
            ratings = get(&ratings, &key),
            "hard purge: deleting confirmed-removed empty series",
        );
    }

    let by_id: HashMap<String, &SeriesCandidate> =
        candidates.iter().map(|c| (c.id.to_string(), c)).collect();
    let event = |c: &SeriesCandidate| {
        let key = c.id.to_string();
        NewEvent::new(
            library_id,
            Category::Series,
            Action::Purged,
            Severity::Warning,
            format!("Purged series {} (removal confirmed)", c.name),
        )
        .entity("series", key.clone(), Some(c.name.clone()))
        .detail(serde_json::json!({
            "slug": c.slug,
            "folder_path": c.folder_path,
            "removed_at": c.removed_at.to_rfc3339(),
            "removal_confirmed_at": c.removal_confirmed_at.to_rfc3339(),
            "window_days": window_days,
            "collection_entries": get(&collection_entries, &key),
            "ratings": get(&ratings, &key),
        }))
    };

    let mut deleted = 0usize;
    let mut aborted = false;
    for chunk in ids.chunks(DELETE_CHUNK) {
        if !library_ready_for_sweep(state, lib, "Hard purge").await {
            aborted = true;
            break;
        }
        let txn = db.begin().await?;
        let rows = DeletedId::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "DELETE FROM series \
                 WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF} AND {SERIES_HAS_NO_ISSUES} \
                   AND id IN ({}) \
                 RETURNING id::text AS id",
                placeholders(3, chunk.len()),
            ),
            with_ids([library_id.into(), cutoff.into()], chunk),
        ))
        .all(&txn)
        .await?;
        let gone: Vec<String> = rows.into_iter().map(|r| r.id).collect();
        clean_unconstrained_refs(&txn, "series", &gone).await?;
        txn.commit().await?;
        *purged += gone.len() as u64;
        deleted += gone.len();
        let events: Vec<NewEvent> = gone
            .iter()
            .filter_map(|id| by_id.get(id))
            .map(|c| event(c))
            .collect();
        event_log::record_many(db, events).await;
    }
    log_kept(library_id, "series", ids.len(), deleted);
    Ok(aborted)
}

/// A candidate restored (or un-confirmed) between the SELECT and the DELETE
/// is kept by the guard; say so, since the export lines over-reported it.
fn log_kept(library_id: Uuid, kind: &str, candidates: usize, deleted: usize) {
    if deleted < candidates {
        tracing::info!(
            target: "folio::hard_purge",
            library_id = %library_id,
            kind,
            kept = candidates - deleted,
            "hard purge: candidates no longer eligible at delete time; kept",
        );
    }
}

/// `$start, $start+1, …` — `n` positional placeholders for an `IN (…)` list.
/// The workspace's sea-orm build has no `postgres-array` feature, so ids are
/// bound one parameter each (chunks stay ≤ [`DELETE_CHUNK`] / one candidate
/// page, [`PRESENCE_BATCH`] in production, far under the 65535 bind cap).
fn placeholders(start: usize, n: usize) -> String {
    (start..start + n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Leading bind values followed by one value per id.
fn with_ids<const N: usize, T>(lead: [sea_orm::Value; N], ids: &[T]) -> Vec<sea_orm::Value>
where
    T: Clone + Into<sea_orm::Value>,
{
    lead.into_iter()
        .chain(ids.iter().cloned().map(Into::into))
        .collect()
}

/// Clean every non-FK reference to the just-deleted `ids` of `kind`
/// (`"issue"` / `"series"`), inside the caller's transaction:
/// [`POLYMORPHIC_REFS`] rows are deleted, and for issues the
/// [`UNCONSTRAINED_ISSUE_DELETES`] rows are deleted and the
/// [`UNCONSTRAINED_ISSUE_NULLIFIES`] pointers cleared.
async fn clean_unconstrained_refs<C: ConnectionTrait>(
    conn: &C,
    kind: &str,
    ids: &[String],
) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    for (table, kind_col, id_col) in POLYMORPHIC_REFS {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "DELETE FROM \"{table}\" WHERE {kind_col} = $1 AND {id_col} IN ({})",
                placeholders(2, ids.len()),
            ),
            with_ids([kind.into()], ids),
        ))
        .await?;
    }
    if kind != "issue" {
        return Ok(());
    }
    for (table, col) in UNCONSTRAINED_ISSUE_DELETES {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "DELETE FROM \"{table}\" WHERE {col} IN ({})",
                placeholders(1, ids.len()),
            ),
            with_ids([], ids),
        ))
        .await?;
    }
    for (table, col) in UNCONSTRAINED_ISSUE_NULLIFIES {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "UPDATE \"{table}\" SET {col} = NULL WHERE {col} IN ({})",
                placeholders(1, ids.len()),
            ),
            with_ids([], ids),
        ))
        .await?;
    }
    Ok(())
}

/// Per-target `user_ratings` count for `kind` targets.
async fn count_ratings(
    db: &DatabaseConnection,
    kind: &str,
    ids: &[String],
) -> anyhow::Result<HashMap<String, i64>> {
    let rows = IdCount::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT target_id AS id, count(*)::bigint AS n FROM user_ratings \
             WHERE target_type = $1 AND target_id IN ({}) GROUP BY target_id",
            placeholders(2, ids.len()),
        ),
        with_ids([kind.into()], ids),
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.n)).collect())
}

/// `SELECT col, count(*) FROM table WHERE col IN (…) GROUP BY col`, keyed by
/// the id's text form. `table` / `col` are compile-time constants from this
/// module, never user input.
async fn count_by<T>(
    db: &DatabaseConnection,
    table: &str,
    col: &str,
    ids: &[T],
) -> anyhow::Result<HashMap<String, i64>>
where
    T: Clone + Into<sea_orm::Value>,
{
    let rows = IdCount::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {col}::text AS id, count(*)::bigint AS n FROM {table} \
             WHERE {col} IN ({}) GROUP BY {col}",
            placeholders(1, ids.len()),
        ),
        with_ids([], ids),
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.n)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_soft_delete_days_times_multiplier() {
        assert_eq!(window_days(30, 2), Some(60));
        assert_eq!(window_days(10, 3), Some(30));
    }

    #[test]
    fn zero_multiplier_disables_purge() {
        assert_eq!(window_days(30, 0), None);
    }

    #[test]
    fn window_has_a_floor() {
        assert_eq!(window_days(0, 2), Some(MIN_PURGE_WINDOW_DAYS));
        assert_eq!(window_days(-5, 2), Some(MIN_PURGE_WINDOW_DAYS));
    }

    #[test]
    fn default_limits_are_the_production_bounds() {
        let l = PurgeLimits::default();
        assert_eq!(l.page, PRESENCE_BATCH);
        assert_eq!(l.max_issue_deletes, MAX_ISSUES_PER_LIBRARY_PER_RUN);
    }

    #[test]
    fn only_absent_is_purged() {
        assert_eq!(decide(Presence::Absent), Decision::Purge);
        assert_eq!(decide(Presence::Present), Decision::KeepUnconfirm);
        assert_eq!(decide(Presence::Unknown), Decision::KeepConfirmed);
    }

    /// A failed stat, through the probe's error mapping, to the decision.
    fn decide_error(e: std::io::Error) -> Decision {
        decide(Presence::from_error(&e))
    }

    #[test]
    fn missing_file_errors_are_purged() {
        use std::io::ErrorKind;
        assert_eq!(decide_error(ErrorKind::NotFound.into()), Decision::Purge);
        assert_eq!(
            decide_error(ErrorKind::NotADirectory.into()),
            Decision::Purge
        );
        // ENOENT / ENOTDIR as the OS reports them.
        assert_eq!(
            decide_error(std::io::Error::from_raw_os_error(2)),
            Decision::Purge
        );
        assert_eq!(
            decide_error(std::io::Error::from_raw_os_error(20)),
            Decision::Purge
        );
    }

    #[test]
    fn permission_denied_is_kept_confirmed() {
        assert_eq!(
            decide_error(std::io::ErrorKind::PermissionDenied.into()),
            Decision::KeepConfirmed
        );
        // EACCES / EPERM as the OS reports them.
        assert_eq!(
            decide_error(std::io::Error::from_raw_os_error(13)),
            Decision::KeepConfirmed
        );
        assert_eq!(
            decide_error(std::io::Error::from_raw_os_error(1)),
            Decision::KeepConfirmed
        );
    }

    #[test]
    fn other_io_errors_are_kept_confirmed() {
        use std::io::ErrorKind;
        for kind in [
            ErrorKind::Other,
            ErrorKind::TimedOut,
            ErrorKind::Interrupted,
            ErrorKind::InvalidInput,
            ErrorKind::Unsupported,
        ] {
            assert_eq!(
                decide_error(kind.into()),
                Decision::KeepConfirmed,
                "{kind:?}"
            );
        }
        // EIO, ELOOP, ENAMETOOLONG, ESTALE, ENOTCONN, EHOSTDOWN (Linux).
        for errno in [5, 40, 36, 116, 107, 112] {
            assert_eq!(
                decide_error(std::io::Error::from_raw_os_error(errno)),
                Decision::KeepConfirmed,
                "errno {errno}"
            );
        }
    }

    /// The real probe on a real directory, for the answers a test can
    /// produce without privileges (no chmod, so it holds under root too).
    #[cfg(unix)]
    #[test]
    fn archive_probe_answers_map_to_decisions() {
        use crate::library::ignore::IgnoreRules;
        use crate::library::scanner::enumerate::archive_presence;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let canon = std::fs::canonicalize(root).unwrap();
        let series = root.join("Series (2020)");
        std::fs::create_dir_all(&series).unwrap();
        std::fs::write(series.join("Here 001.cbz"), b"x").unwrap();
        std::fs::write(root.join("plain-file"), b"x").unwrap();
        // A folder link that points at itself: every path through it fails
        // with ELOOP, an error that is not the not-found class.
        std::os::unix::fs::symlink("Loop", root.join("Loop")).unwrap();
        let ignore = IgnoreRules::default();
        let probe = |p: std::path::PathBuf| decide(archive_presence(root, &canon, &p, &ignore));

        assert_eq!(
            probe(series.join("Here 001.cbz")),
            Decision::KeepUnconfirm,
            "exists"
        );
        assert_eq!(
            probe(series.join("Gone 002.cbz")),
            Decision::Purge,
            "missing"
        );
        assert_eq!(
            probe(root.join("No Such Folder").join("Gone 003.cbz")),
            Decision::Purge,
            "missing folder"
        );
        assert_eq!(
            probe(root.join("plain-file").join("Gone 004.cbz")),
            Decision::Purge,
            "a path component is a file (ENOTDIR)"
        );
        assert_eq!(
            probe(root.join("Loop").join("Kept 005.cbz")),
            Decision::KeepConfirmed,
            "the disk would not say (ELOOP)"
        );
    }
}
