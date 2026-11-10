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
//! are purged without the stat.
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
    KeptRows, MAX_SERIES_RESCANS_PER_RUN, Presence, PresenceProbe, library_ready_for_sweep,
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

/// Max issue rows purged per library per run. A backlog (first run after an
/// upgrade on a high-churn library) drains over successive days instead of
/// one unbounded run.
pub const MAX_ISSUES_PER_LIBRARY_PER_RUN: i64 = 5_000;

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
        match purge_library(state, &lib, days, cutoff).await {
            Ok(lib_stats) => {
                if lib_stats.issues > 0 || lib_stats.series > 0 {
                    stats.libraries += 1;
                }
                stats.issues += lib_stats.issues;
                stats.series += lib_stats.series;
                stats.kept_present += lib_stats.kept_present;
                stats.kept_unknown += lib_stats.kept_unknown;
                stats.rescans_queued += lib_stats.rescans_queued;
                stats.library_scans_queued += lib_stats.library_scans_queued;
                stats.rescans_throttled += lib_stats.rescans_throttled;
                stats.libraries_aborted += lib_stats.libraries_aborted;
            }
            // One library failing must not stop the others being purged.
            Err(e) => {
                stats.libraries_aborted += 1;
                tracing::error!(
                    library_id = %lib.id,
                    error = %e,
                    "hard purge: library failed",
                );
            }
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
) -> anyhow::Result<PurgeStats> {
    let probe = PresenceProbe::for_library(lib);
    let mut kept = KeptRows::default();
    let result = purge_rows(state, lib, &probe, window_days, cutoff, &mut kept).await;
    // Rows kept before an abort are still rescanned and reported.
    let outcome = kept
        .finish(state, lib.id, "Hard purge", MAX_SERIES_RESCANS_PER_RUN)
        .await;
    let (issues, series, aborted) = result?;
    Ok(PurgeStats {
        issues,
        series,
        kept_present: kept.present(),
        kept_unknown: kept.unknown(),
        rescans_queued: outcome.series_scans,
        library_scans_queued: outcome.library_scans,
        rescans_throttled: outcome.throttled,
        libraries_aborted: u64::from(aborted),
        ..PurgeStats::default()
    })
}

/// `(issues purged, series purged, aborted)`. Aborted: the library root
/// stopped validating before a DELETE chunk, so nothing more was deleted.
async fn purge_rows(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    kept: &mut KeptRows,
) -> anyhow::Result<(u64, u64, bool)> {
    let (issues, aborted) = purge_issues(state, lib, probe, window_days, cutoff, kept).await?;
    if aborted {
        return Ok((issues, 0, true));
    }
    let (series, aborted) = purge_series(state, lib, probe, window_days, cutoff, kept).await?;
    Ok((issues, series, aborted))
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

async fn purge_issues(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    kept: &mut KeptRows,
) -> anyhow::Result<(u64, bool)> {
    let db = &state.db;
    let library_id = lib.id;
    let candidates = IssueCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT id, series_id, slug, file_path, content_hash, removed_at, \
                    removal_confirmed_at, \
                    EXISTS (SELECT 1 FROM issue_duplicate_decision d \
                            WHERE d.issue_id = issues.id AND d.decision = 'remove') \
                      AS duplicate_removed \
             FROM issues \
             WHERE library_id = $1 AND {CONFIRMED_PAST_CUTOFF} \
             ORDER BY removal_confirmed_at, id \
             LIMIT $3"
        ),
        [
            library_id.into(),
            cutoff.into(),
            MAX_ISSUES_PER_LIBRARY_PER_RUN.into(),
        ],
    ))
    .all(db)
    .await?;
    if candidates.is_empty() {
        return Ok((0, false));
    }

    // Stat guard: only a file that is really gone is deleted. Present: kept,
    // confirmation cleared, series rescanned. Unknown: kept as it is.
    let to_stat: Vec<((String, Uuid), String)> = candidates
        .iter()
        .filter(|c| !c.duplicate_removed)
        .map(|c| ((c.id.clone(), c.series_id), c.file_path.clone()))
        .collect();
    let mut present_ids: Vec<String> = Vec::new();
    let mut keep: HashSet<String> = HashSet::new();
    for ((id, series_id), presence) in probe.files(to_stat).await? {
        match presence {
            Presence::Absent => {}
            Presence::Present => {
                kept.rescan.insert(series_id);
                keep.insert(id.clone());
                present_ids.push(id);
            }
            Presence::Unknown => {
                keep.insert(id.clone());
                kept.unknown_issues.push(id);
            }
        }
    }
    if !present_ids.is_empty() {
        unconfirm(db, "issues", library_id, cutoff, &present_ids).await?;
        kept.present_issues.extend(present_ids);
    }
    let candidates: Vec<IssueCandidate> = candidates
        .into_iter()
        .filter(|c| !keep.contains(&c.id))
        .collect();
    if candidates.is_empty() {
        return Ok((0, false));
    }

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

    let mut deleted: Vec<String> = Vec::with_capacity(ids.len());
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
        deleted.extend(gone);
    }
    log_kept(library_id, "issue", ids.len(), deleted.len());

    let by_id: HashMap<&str, &IssueCandidate> =
        candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let events: Vec<NewEvent> = deleted
        .iter()
        .filter_map(|id| by_id.get(id.as_str()))
        .map(|c| {
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
        })
        .collect();
    event_log::record_many(db, events).await;

    Ok((deleted.len() as u64, aborted))
}

async fn purge_series(
    state: &AppState,
    lib: &library::Model,
    probe: &PresenceProbe,
    window_days: i64,
    cutoff: DateTime<FixedOffset>,
    kept: &mut KeptRows,
) -> anyhow::Result<(u64, bool)> {
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
        return Ok((0, false));
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
        match p {
            Presence::Absent => {}
            Presence::Present => present_ids.push(*id),
            Presence::Unknown => {
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
        .filter(|c| presence.get(&c.id).is_none_or(|p| *p == Presence::Absent))
        .collect();
    if candidates.is_empty() {
        return Ok((0, false));
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

    let mut deleted: Vec<String> = Vec::with_capacity(ids.len());
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
        deleted.extend(gone);
    }
    log_kept(library_id, "series", ids.len(), deleted.len());

    let by_id: HashMap<String, &SeriesCandidate> =
        candidates.iter().map(|c| (c.id.to_string(), c)).collect();
    let events: Vec<NewEvent> = deleted
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|c| {
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
        })
        .collect();
    event_log::record_many(db, events).await;

    Ok((deleted.len() as u64, aborted))
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
/// bound one parameter each (chunks stay ≤ [`DELETE_CHUNK`] /
/// [`MAX_ISSUES_PER_LIBRARY_PER_RUN`], far under the 65535 bind cap).
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
}
