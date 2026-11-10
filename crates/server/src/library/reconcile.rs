//! Reconciliation pass (spec §4.7).
//!
//! Library Scanner v1, Milestone 7.
//!
//! After the per-folder processing phase finishes, this module sweeps every
//! non-removed issue belonging to a scanned series and checks whether the
//! file is still on disk:
//!
//!   - Missing → set `removed_at = now()`. The issue stays in the DB so user
//!     progress, bookmarks, and reviews aren't lost (spec §4.7 rationale).
//!   - Returning (a soft-deleted issue's file reappears with the same hash)
//!     → clear `removed_at` + `removal_confirmed_at`.
//!   - A series whose every issue is removed gets `series.removed_at` set
//!     too.
//!
//! The follow-up auto-confirm sweep (the one that flips `removal_confirmed_at`
//! after `library.soft_delete_days` have elapsed) runs as a daily cron job
//! defined here as [`auto_confirm_sweep`]. Milestone 9 wires the cron. It
//! confirms only what is really gone: a library failing `validate_library`
//! is skipped whole, and a row whose file is on disk again gets a series
//! rescan instead of a confirmation. `jobs::hard_purge` applies the same
//! guards (via [`PresenceProbe`] and [`queue_series_rescans`]) before it
//! deletes anything.

use chrono::{Duration, Utc};
use entity::{
    issue::{self, Entity as IssueEntity},
    library, series,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, QuerySelect, Set, Statement, sea_query::Expr,
};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

use super::ignore::IgnoreRules;
use super::scanner::enumerate;
use crate::state::AppState;

/// The only `issue` columns the seen-reconcile pass reads. Selecting just these
/// avoids materializing the heavy `pages` / `comic_info_raw` JSON for every
/// issue of every scanned series — on a large library that was hundreds of MB
/// of transient model data per scan (PERF-9).
#[derive(Debug, FromQueryResult)]
struct ReconcileIssue {
    id: String,
    file_path: String,
    series_id: Uuid,
    slug: String,
    removed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

/// Filter out issues an admin soft-removed from the Duplicates page
/// (WP-3.3). Their file is still on disk, so the presence-driven
/// restore rule (`(removed, present) → restore`) would undo the removal
/// on the next scan; the `issue_duplicate_decision` row pins it. The
/// decision row is deleted when the admin restores the issue.
pub(crate) fn not_duplicate_removed() -> sea_orm::sea_query::SimpleExpr {
    Expr::cust(
        "NOT EXISTS (SELECT 1 FROM issue_duplicate_decision d \
         WHERE d.issue_id = issues.id AND d.decision = 'remove')",
    )
}

use super::event_log::{self, Action, Category, EventCollector, NewEvent, Severity};
use super::scanner::stats::ScanStats;

/// What the scan planner needs to know about one series' soft-removed
/// state to decide whether an unchanged folder may still be skipped.
#[derive(Debug, Default)]
pub(crate) struct SeriesRemovalPaths {
    /// `file_path`s of the series' soft-removed issues that are not
    /// Duplicates-page removals: a returning file at one of these paths
    /// restores the issue, so the folder must be processed.
    pub restorable: HashSet<String>,
    /// The `file_path` of every row the query returned for the series.
    /// Complete (every row, any state) only for a soft-removed series, the
    /// one case the planner reads it: an archive in such a folder that is
    /// not in this set is unknown to the database (for example its rows
    /// were hard-purged) and has to be ingested.
    pub known: HashSet<String>,
}

/// Per-series removal paths for the scan planner of `library_id`, in one
/// query projecting `(series_id, file_path, restorable)`. Rows come from
/// every soft-removed issue of the library, plus every issue of a
/// soft-removed series. Series absent from the map have nothing to restore.
pub(crate) async fn series_removal_paths(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> anyhow::Result<std::collections::HashMap<Uuid, SeriesRemovalPaths>> {
    let removed_series = sea_orm::sea_query::Query::select()
        .column(series::Column::Id)
        .from(series::Entity)
        .and_where(series::Column::LibraryId.eq(library_id))
        .and_where(series::Column::RemovedAt.is_not_null())
        .to_owned();
    let rows: Vec<(Uuid, String, bool)> = IssueEntity::find()
        .select_only()
        .column(issue::Column::SeriesId)
        .column(issue::Column::FilePath)
        .column_as(
            sea_orm::ExprTrait::and(
                issue::Column::RemovedAt.is_not_null(),
                not_duplicate_removed(),
            ),
            "restorable",
        )
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(
            sea_orm::Condition::any()
                .add(issue::Column::RemovedAt.is_not_null())
                .add(issue::Column::SeriesId.in_subquery(removed_series)),
        )
        .into_tuple()
        .all(db)
        .await?;
    let mut out: std::collections::HashMap<Uuid, SeriesRemovalPaths> =
        std::collections::HashMap::new();
    for (series_id, path, restorable) in rows {
        let entry = out.entry(series_id).or_default();
        entry.known.insert(path.clone());
        if restorable {
            entry.restorable.insert(path);
        }
    }
    Ok(out)
}

/// Lift the removal off every soft-removed series among `series_ids` that
/// holds at least one active issue (Duplicates-page removals never count;
/// they are sticky). Clears `removed_at` and `removal_confirmed_at` and
/// queues one `series restored` event per series, the automatic twin of
/// the admin `POST /series/{slug}/restore`.
///
/// `series.removed_at` is only ever set by the scanner, and only for a
/// series with no live issue, so a removed series holding a live issue is
/// always an inconsistent state and safe to repair. The scan reconciles
/// call [`restore_library_series_with_live_issues`] as their last step;
/// the single-issue scan, the admin per-issue restore and the Duplicates
/// clear-decision undo call this for the one series they touched.
///
/// Cache invalidation stays with the caller: scans invalidate when the
/// reconcile reports a restored series, the admin handlers invalidate the
/// Duplicates memo themselves (as the admin series restore does).
///
/// Returns the number of series restored.
pub(crate) async fn restore_series_with_live_issues(
    db: &DatabaseConnection,
    series_ids: impl IntoIterator<Item = Uuid>,
    events: &mut EventCollector,
) -> anyhow::Result<u64> {
    let ids: Vec<Uuid> = series_ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let mut removed: Vec<(Uuid, String)> = Vec::new();
    for chunk in ids.chunks(RECONCILE_ID_CHUNK) {
        removed.extend(
            series::Entity::find()
                .select_only()
                .column(series::Column::Id)
                .column(series::Column::Name)
                .filter(series::Column::Id.is_in(chunk.to_vec()))
                .filter(series::Column::RemovedAt.is_not_null())
                .into_tuple::<(Uuid, String)>()
                .all(db)
                .await?,
        );
    }
    restore_removed_series_with_live_issues(db, removed, events, RECONCILE_ID_CHUNK).await
}

/// Library-wide [`restore_series_with_live_issues`]: every soft-removed
/// series of `library_id` that holds a live issue. One query for the
/// removed series of the library (usually few), then the live-issue check.
/// Runs at the very end of a library scan reconcile, after the
/// folder-missing sweep, so a series that sweep just removed (its issues
/// went with it) is never restored and removed again in the same scan.
pub(crate) async fn restore_library_series_with_live_issues(
    db: &DatabaseConnection,
    library_id: Uuid,
    events: &mut EventCollector,
    chunk: usize,
) -> anyhow::Result<u64> {
    let removed: Vec<(Uuid, String)> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::Name)
        .filter(series::Column::LibraryId.eq(library_id))
        .filter(series::Column::RemovedAt.is_not_null())
        .into_tuple()
        .all(db)
        .await?;
    restore_removed_series_with_live_issues(db, removed, events, chunk).await
}

/// The shared body: `removed` are `(id, name)` of soft-removed series. Every
/// id list is bound `chunk` ids per statement (see [`RECONCILE_ID_CHUNK`]).
async fn restore_removed_series_with_live_issues(
    db: &DatabaseConnection,
    removed: Vec<(Uuid, String)>,
    events: &mut EventCollector,
    chunk: usize,
) -> anyhow::Result<u64> {
    if removed.is_empty() {
        return Ok(0);
    }
    let removed_ids: Vec<Uuid> = removed.iter().map(|(id, _)| *id).collect();
    let mut has_live: HashSet<Uuid> = HashSet::new();
    for ids in removed_ids.chunks(chunk.max(1)) {
        has_live.extend(
            IssueEntity::find()
                .select_only()
                .column(issue::Column::SeriesId)
                .filter(issue::Column::SeriesId.is_in(ids.to_vec()))
                .filter(issue::Column::RemovedAt.is_null())
                .filter(not_duplicate_removed())
                .distinct()
                .into_tuple::<Uuid>()
                .all(db)
                .await?,
        );
    }
    let to_restore: Vec<(Uuid, String)> = removed
        .into_iter()
        .filter(|(id, _)| has_live.contains(id))
        .collect();
    if to_restore.is_empty() {
        return Ok(0);
    }
    let restore_ids: Vec<Uuid> = to_restore.iter().map(|(id, _)| *id).collect();
    let mut restored = 0u64;
    for ids in restore_ids.chunks(chunk.max(1)) {
        let res = series::Entity::update_many()
            .col_expr(
                series::Column::RemovedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .col_expr(
                series::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(series::Column::Id.is_in(ids.to_vec()))
            .filter(series::Column::RemovedAt.is_not_null())
            .exec(db)
            .await?;
        restored += res.rows_affected;
    }
    for (series_id, name) in to_restore {
        tracing::info!(%series_id, series = %name, "reconcile: series restored (issues returned)");
        let ev = events
            .build(
                Category::Series,
                Action::Restored,
                Severity::Info,
                format!("Restored series {name}"),
            )
            .entity("series", series_id.to_string(), Some(name));
        events.push(ev);
    }
    Ok(restored)
}

/// Walk all non-removed issues for `library_id`. Soft-delete the ones whose
/// file is missing; restore the ones whose file came back.
pub async fn reconcile_library(
    db: &DatabaseConnection,
    library_id: Uuid,
    stats: &mut ScanStats,
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();

    let issues = IssueEntity::find()
        .filter(not_duplicate_removed())
        .filter(issue::Column::LibraryId.eq(library_id))
        .all(db)
        .await?;

    for row in issues {
        let path = Path::new(&row.file_path);
        let exists = path.exists();
        match (row.removed_at.is_some(), exists) {
            // Soft-delete: present in DB, missing on disk.
            (false, false) => {
                let mut am: issue::ActiveModel = row.into();
                am.removed_at = Set(Some(now));
                am.removal_confirmed_at = Set(None);
                am.update(db).await?;
                stats.issues_removed += 1;
            }
            // Restore: was soft-deleted, file is back.
            (true, true) => {
                let mut am: issue::ActiveModel = row.into();
                am.removed_at = Set(None);
                am.removal_confirmed_at = Set(None);
                am.update(db).await?;
                stats.issues_restored += 1;
            }
            // Steady states: present + present, or removed + still missing.
            _ => {}
        }
    }
    // Series with all issues removed → soft-delete the series itself.
    let series_rows = series::Entity::find()
        .filter(series::Column::LibraryId.eq(library_id))
        .filter(series::Column::RemovedAt.is_null())
        .all(db)
        .await?;
    for srow in series_rows {
        let any_active = IssueEntity::find()
            .filter(issue::Column::SeriesId.eq(srow.id))
            .filter(issue::Column::RemovedAt.is_null())
            .one(db)
            .await?
            .is_some();
        if !any_active {
            let mut am: series::ActiveModel = srow.into();
            am.removed_at = Set(Some(now));
            am.removal_confirmed_at = Set(None);
            am.update(db).await?;
            stats.series_removed += 1;
        }
    }

    // Last: a removed series holding a live issue comes back.
    let mut events = EventCollector::new(library_id, None);
    restore_library_series_with_live_issues(db, library_id, &mut events, RECONCILE_ID_CHUNK)
        .await?;
    events.flush(db).await;

    Ok(())
}

/// Returns the number of soft-removed series it restored (the caller drops
/// the caches that depend on `series.removed_at` when it is non-zero).
///
/// Every series-id list is bound [`RECONCILE_ID_CHUNK`] ids per statement,
/// so a library with more series than Postgres' 65,535 bind parameters
/// still reconciles.
#[expect(clippy::too_many_arguments)]
pub async fn reconcile_library_seen(
    db: &DatabaseConnection,
    library_id: Uuid,
    scanned_series_ids: &HashSet<Uuid>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    present_folders: &HashSet<String>,
    missing_scope: Option<&[std::path::PathBuf]>,
    unreadable_folders: &HashSet<String>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
) -> anyhow::Result<u64> {
    reconcile_library_seen_with_chunk(
        db,
        library_id,
        scanned_series_ids,
        seen_paths,
        scanned_folder_paths,
        present_folders,
        missing_scope,
        unreadable_folders,
        stats,
        events,
        RECONCILE_ID_CHUNK,
    )
    .await
}

/// [`reconcile_library_seen`] that stops between its units of work once
/// `stop` is set: the scan's coalescing-key lease was lost (R-03), so
/// another run owns the library and this one must stop removing rows.
/// Returns an error when it stopped early.
#[expect(clippy::too_many_arguments)]
pub async fn reconcile_library_seen_until(
    db: &DatabaseConnection,
    library_id: Uuid,
    scanned_series_ids: &HashSet<Uuid>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    present_folders: &HashSet<String>,
    missing_scope: Option<&[std::path::PathBuf]>,
    unreadable_folders: &HashSet<String>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
    stop: Option<&AtomicBool>,
) -> anyhow::Result<u64> {
    reconcile_library_seen_inner(
        db,
        library_id,
        scanned_series_ids,
        seen_paths,
        scanned_folder_paths,
        present_folders,
        missing_scope,
        unreadable_folders,
        stats,
        events,
        RECONCILE_ID_CHUNK,
        stop,
    )
    .await
}

/// Error out when `stop` is set; see [`reconcile_library_seen_until`].
fn check_stop(stop: Option<&AtomicBool>) -> anyhow::Result<()> {
    if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
        anyhow::bail!("reconcile stopped: another scan of this library took over");
    }
    Ok(())
}

/// [`reconcile_library_seen`] with an explicit id-list chunk size. Public so
/// the integration suite can cross a chunk boundary with a handful of
/// series; production always goes through [`reconcile_library_seen`].
#[doc(hidden)]
#[expect(clippy::too_many_arguments)]
pub async fn reconcile_library_seen_with_chunk(
    db: &DatabaseConnection,
    library_id: Uuid,
    scanned_series_ids: &HashSet<Uuid>,
    seen_paths: &HashSet<String>,
    // Folders that were actually walked this scan. An issue belonging to a
    // scanned series but whose `file_path` lives **outside** every scanned
    // folder must be left alone — it's a sibling folder we didn't visit.
    // Without this filter, two distinct on-disk folders that collapsed to
    // one series row would cycle: each scan soft-deletes whichever
    // folder's issues weren't this run's `seen_paths`, then the next scan
    // restores them and removes the others (folder-collapse bug, dev DB
    // 2026-05-14).
    scanned_folder_paths: &HashSet<String>,
    present_folders: &HashSet<String>,
    // Watcher-scoped scans (WP-3.1) enumerate only part of the root. When
    // `Some`, only series whose folder lives under one of these paths are
    // judged against `present_folders`; everything else was simply not
    // looked at and is left alone. `None` = full scan, whole library.
    missing_scope: Option<&[std::path::PathBuf]>,
    // Folders the enumeration could not read (post-audit T1.1). A series
    // whose folder is at or under one of these is *unknown*, not gone:
    // it is never judged against `present_folders`. Only successfully
    // enumerated scopes authorise absence.
    unreadable_folders: &HashSet<String>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
    chunk: usize,
) -> anyhow::Result<u64> {
    reconcile_library_seen_inner(
        db,
        library_id,
        scanned_series_ids,
        seen_paths,
        scanned_folder_paths,
        present_folders,
        missing_scope,
        unreadable_folders,
        stats,
        events,
        chunk,
        None,
    )
    .await
}

#[expect(clippy::too_many_arguments)]
async fn reconcile_library_seen_inner(
    db: &DatabaseConnection,
    library_id: Uuid,
    scanned_series_ids: &HashSet<Uuid>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    present_folders: &HashSet<String>,
    missing_scope: Option<&[std::path::PathBuf]>,
    unreadable_folders: &HashSet<String>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
    chunk: usize,
    stop: Option<&AtomicBool>,
) -> anyhow::Result<u64> {
    let now = Utc::now().fixed_offset();
    let chunk = chunk.max(1);
    check_stop(stop)?;

    if !scanned_series_ids.is_empty() {
        let scanned: Vec<Uuid> = scanned_series_ids.iter().copied().collect();
        let mut issues = Vec::new();
        for ids in scanned.chunks(chunk) {
            issues.extend(
                IssueEntity::find()
                    .filter(not_duplicate_removed())
                    .filter(issue::Column::LibraryId.eq(library_id))
                    .filter(issue::Column::SeriesId.is_in(ids.to_vec()))
                    .select_only()
                    .columns([
                        issue::Column::Id,
                        issue::Column::FilePath,
                        issue::Column::SeriesId,
                        issue::Column::Slug,
                        issue::Column::RemovedAt,
                    ])
                    .into_model::<ReconcileIssue>()
                    .all(db)
                    .await?,
            );
        }
        check_stop(stop)?;
        reconcile_issue_rows_seen(
            db,
            issues,
            seen_paths,
            scanned_folder_paths,
            now,
            stats,
            events,
            stop,
        )
        .await?;
        check_stop(stop)?;
        mark_empty_series_removed(db, scanned, now, stats, events, chunk).await?;
    }
    check_stop(stop)?;

    let series_rows = series::Entity::find()
        .filter(series::Column::LibraryId.eq(library_id))
        .all(db)
        .await?;
    let mut missing_series = Vec::new();
    for srow in series_rows {
        let Some(folder) = srow.folder_path.as_deref() else {
            continue;
        };
        if present_folders.contains(folder) {
            continue;
        }
        if let Some(scope) = missing_scope
            && !scope.iter().any(|top| Path::new(folder).starts_with(top))
        {
            continue;
        }
        if path_under_any(folder, unreadable_folders) {
            tracing::warn!(
                series_id = %srow.id,
                folder,
                "reconcile: series folder is under an unreadable path — not marking removed"
            );
            continue;
        }
        check_stop(stop)?;
        let series_label = srow.name.clone();
        let series_id = srow.id;
        missing_series.push(series_id);
        let res = IssueEntity::update_many()
            .col_expr(issue::Column::RemovedAt, Expr::value(now))
            .col_expr(
                issue::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(issue::Column::SeriesId.eq(series_id))
            .filter(issue::Column::RemovedAt.is_null())
            .exec(db)
            .await?;
        stats.issues_removed += res.rows_affected;
        // Whole folder gone: one series-scoped removed event carrying the
        // issue count, rather than N per-issue rows (the issues went via a
        // bulk `update_many`, so individual ids aren't materialized here).
        let ev = events
            .build(
                Category::Series,
                Action::Removed,
                Severity::Warning,
                format!("Removed series {series_label} (folder missing)"),
            )
            .entity("series", series_id.to_string(), Some(series_label))
            .detail(serde_json::json!({ "issues_removed": res.rows_affected }));
        events.push(ev);
    }
    if !missing_series.is_empty() {
        for chunk in missing_series.chunks(500) {
            check_stop(stop)?;
            let res = series::Entity::update_many()
                .col_expr(series::Column::RemovedAt, Expr::value(now))
                .col_expr(
                    series::Column::RemovalConfirmedAt,
                    Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
                )
                .filter(series::Column::Id.is_in(chunk.to_vec()))
                .filter(series::Column::RemovedAt.is_null())
                .exec(db)
                .await?;
            stats.series_removed += res.rows_affected;
        }
    }

    // Last, after the folder-missing sweep: lift the removal off every
    // removed series of the library that holds a live issue. That covers a
    // folder that came back this scan (its issues were just restored above)
    // and a series that was already hidden with live issues (left by an
    // older scanner, a forced or issue scan, or a failure between the issue
    // and series writes). A series the sweep removed has no live issue left,
    // so it stays removed: restoring it any earlier made it flap.
    check_stop(stop)?;
    restore_library_series_with_live_issues(db, library_id, events, chunk).await
}

/// Per-series variant of [`reconcile_library`]. Scoped to one series so a
/// per-series scan (`POST /series/{id}/scan`, `POST /issues/{id}/scan`,
/// file-watch event on a single folder) doesn't pay the cost of walking
/// every issue in the library, and doesn't touch siblings whose folder
/// wasn't actually rescanned.
///
/// Behavior parity with `reconcile_library` for the in-scope series: missing
/// files are soft-deleted; returning files are restored; the series itself
/// is soft-deleted when it has no remaining active issues.
pub async fn reconcile_series(
    db: &DatabaseConnection,
    series_id: Uuid,
    stats: &mut ScanStats,
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();

    // PERF-9: project to the 5 columns this reconcile actually reads instead of
    // loading full issue rows (with the large `comic_info_raw` blob) just to
    // check file existence — this runs on the file-watch / per-issue-scan hot
    // path. The sibling `reconcile_series_seen` already uses this projection.
    let issues = IssueEntity::find()
        .filter(not_duplicate_removed())
        .filter(issue::Column::SeriesId.eq(series_id))
        .select_only()
        .columns([
            issue::Column::Id,
            issue::Column::FilePath,
            issue::Column::SeriesId,
            issue::Column::Slug,
            issue::Column::RemovedAt,
        ])
        .into_model::<ReconcileIssue>()
        .all(db)
        .await?;

    for row in issues {
        let path = Path::new(&row.file_path);
        let exists = path.exists();
        // Build a targeted ActiveModel from the id so only the two flipped
        // columns are written (a projected row can't `.into()` a full model).
        match (row.removed_at.is_some(), exists) {
            (false, false) => {
                let am = issue::ActiveModel {
                    id: Set(row.id.clone()),
                    removed_at: Set(Some(now)),
                    removal_confirmed_at: Set(None),
                    ..Default::default()
                };
                am.update(db).await?;
                stats.issues_removed += 1;
            }
            (true, true) => {
                let am = issue::ActiveModel {
                    id: Set(row.id.clone()),
                    removed_at: Set(None),
                    removal_confirmed_at: Set(None),
                    ..Default::default()
                };
                am.update(db).await?;
                stats.issues_restored += 1;
            }
            _ => {}
        }
    }

    // Soft-delete the series itself if every issue is now removed.
    let srow = series::Entity::find_by_id(series_id)
        .filter(series::Column::RemovedAt.is_null())
        .one(db)
        .await?;
    if let Some(srow) = srow {
        let any_active = IssueEntity::find()
            .filter(issue::Column::SeriesId.eq(srow.id))
            .filter(issue::Column::RemovedAt.is_null())
            .one(db)
            .await?
            .is_some();
        if !any_active {
            let mut am: series::ActiveModel = srow.into();
            am.removed_at = Set(Some(now));
            am.removal_confirmed_at = Set(None);
            am.update(db).await?;
            stats.series_removed += 1;
        }
    }

    // Last: a removed series holding a live issue comes back.
    if let Some(library_id) = series::Entity::find_by_id(series_id)
        .select_only()
        .column(series::Column::LibraryId)
        .into_tuple::<Uuid>()
        .one(db)
        .await?
    {
        let mut events = EventCollector::new(library_id, None);
        restore_series_with_live_issues(db, [series_id], &mut events).await?;
        events.flush(db).await;
    }

    // Issues just flipped removed/restored — let the status helper
    // recompute total_issues + status. A once-Complete series with a
    // file removed from disk should show Incomplete on the next read.
    if let Err(e) =
        crate::library::scanner::reconcile_status::reconcile_series_status(db, series_id, None)
            .await
    {
        tracing::warn!(error = %e, "reconcile: status recompute failed");
    }

    Ok(())
}

pub async fn reconcile_series_seen(
    db: &DatabaseConnection,
    series_id: Uuid,
    seen_paths: &HashSet<String>,
    // The folder(s) actually walked this scan. See
    // [`reconcile_library_seen`] for the rationale.
    scanned_folder_paths: &HashSet<String>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
) -> anyhow::Result<u64> {
    let now = Utc::now().fixed_offset();

    let issues = IssueEntity::find()
        .filter(not_duplicate_removed())
        .filter(issue::Column::SeriesId.eq(series_id))
        .select_only()
        .columns([
            issue::Column::Id,
            issue::Column::FilePath,
            issue::Column::SeriesId,
            issue::Column::Slug,
            issue::Column::RemovedAt,
        ])
        .into_model::<ReconcileIssue>()
        .all(db)
        .await?;
    reconcile_issue_rows_seen(
        db,
        issues,
        seen_paths,
        scanned_folder_paths,
        now,
        stats,
        events,
        None,
    )
    .await?;
    mark_empty_series_removed(db, vec![series_id], now, stats, events, RECONCILE_ID_CHUNK).await?;
    // Last: the series comes back if it is removed but holds a live issue
    // (returned this scan, or already live before it). Returns the count.
    restore_series_with_live_issues(db, [series_id], events).await
}

/// Is `path` equal to, or nested under, any folder in `folders`? Uses a
/// trailing-slash check so `/foo` does **not** match `/foo-bar/baz.cbz`.
fn path_under_any(path: &str, folders: &HashSet<String>) -> bool {
    folders.iter().any(|folder| {
        path == folder
            || (path.len() > folder.len()
                && path.starts_with(folder)
                && path.as_bytes()[folder.len()] == b'/')
    })
}

#[expect(clippy::too_many_arguments)]
async fn reconcile_issue_rows_seen(
    db: &DatabaseConnection,
    issues: Vec<ReconcileIssue>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
    stop: Option<&AtomicBool>,
) -> anyhow::Result<()> {
    let mut remove_ids = Vec::new();
    let mut restore_ids = Vec::new();
    for row in issues {
        let seen = seen_paths.contains(&row.file_path);
        match (row.removed_at.is_some(), seen) {
            (false, false) => {
                // Only soft-delete when this scan actually walked the
                // folder the file claims to live in. An issue whose
                // file_path is outside every scanned folder belongs to a
                // sibling folder we didn't visit — leave it alone for
                // that folder's own scan.
                if path_under_any(&row.file_path, scanned_folder_paths) {
                    let ev = events
                        .build(
                            Category::Issue,
                            Action::Removed,
                            Severity::Warning,
                            format!("Removed issue {} (file missing)", row.slug),
                        )
                        .entity("issue", row.id.clone(), Some(row.slug.clone()))
                        .detail(serde_json::json!({
                            "series_id": row.series_id,
                            "file_path": row.file_path,
                        }));
                    events.push(ev);
                    remove_ids.push(row.id);
                }
            }
            (true, true) => {
                let ev = events
                    .build(
                        Category::Issue,
                        Action::Restored,
                        Severity::Info,
                        format!("Restored issue {}", row.slug),
                    )
                    .entity("issue", row.id.clone(), Some(row.slug.clone()));
                events.push(ev);
                restore_ids.push(row.id);
            }
            _ => {}
        }
    }

    for chunk in remove_ids.chunks(500) {
        check_stop(stop)?;
        let res = IssueEntity::update_many()
            .col_expr(issue::Column::RemovedAt, Expr::value(now))
            .col_expr(
                issue::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(issue::Column::Id.is_in(chunk.to_vec()))
            .exec(db)
            .await?;
        stats.issues_removed += res.rows_affected;
    }
    for chunk in restore_ids.chunks(500) {
        let res = IssueEntity::update_many()
            .col_expr(
                issue::Column::RemovedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .col_expr(
                issue::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(issue::Column::Id.is_in(chunk.to_vec()))
            .exec(db)
            .await?;
        stats.issues_restored += res.rows_affected;
    }
    // The series of restored issues come back in the caller's last step
    // (`restore_*series_with_live_issues`), after every removal sweep.

    Ok(())
}

async fn mark_empty_series_removed(
    db: &DatabaseConnection,
    all_ids: Vec<Uuid>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
    chunk: usize,
) -> anyhow::Result<()> {
    if all_ids.is_empty() {
        return Ok(());
    }
    let chunk = chunk.max(1);

    // PERF-7: one grouped probe per id chunk for "which of these series
    // still have an active issue?" instead of a per-series `.one()` (N+1 by
    // series count).
    let mut has_active: HashSet<Uuid> = HashSet::new();
    for ids in all_ids.chunks(chunk) {
        has_active.extend(
            IssueEntity::find()
                .select_only()
                .column(issue::Column::SeriesId)
                .filter(issue::Column::SeriesId.is_in(ids.to_vec()))
                .filter(issue::Column::RemovedAt.is_null())
                .distinct()
                .into_tuple::<Uuid>()
                .all(db)
                .await?,
        );
    }

    // The now-empty series among the input. Narrow to the ones still active
    // (not already soft-deleted) so `stats`/events count only real flips — the
    // same "don't fire when already removed" semantics the per-row path had.
    let empty: Vec<Uuid> = all_ids
        .into_iter()
        .filter(|id| !has_active.contains(id))
        .collect();
    if empty.is_empty() {
        return Ok(());
    }
    let mut to_flip: Vec<Uuid> = Vec::new();
    for ids in empty.chunks(chunk) {
        to_flip.extend(
            series::Entity::find()
                .select_only()
                .column(series::Column::Id)
                .filter(series::Column::Id.is_in(ids.to_vec()))
                .filter(series::Column::RemovedAt.is_null())
                .into_tuple::<Uuid>()
                .all(db)
                .await?,
        );
    }
    if to_flip.is_empty() {
        return Ok(());
    }

    // One batched soft-delete per chunk of newly-empty series.
    for ids in to_flip.chunks(chunk) {
        series::Entity::update_many()
            .col_expr(series::Column::RemovedAt, Expr::value(now))
            .col_expr(
                series::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(series::Column::Id.is_in(ids.to_vec()))
            .exec(db)
            .await?;
    }
    stats.series_removed += to_flip.len() as u64;
    for series_id in to_flip {
        events.record(
            Category::Series,
            Action::Removed,
            Severity::Warning,
            format!("Removed empty series {series_id}"),
        );
    }
    Ok(())
}

/// Max soft-removed issue rows the auto-confirm sweep examines per library
/// per run, read oldest removal first in keyset pages. Rows kept because
/// they are present or unknown are re-examined every run, so the bound is
/// generous; past it the sweep makes no further progress that run (the
/// safe direction) and the next daily run continues.
pub const CONFIRM_MAX_ISSUE_ROWS_PER_LIBRARY_PER_RUN: usize = 20_000;

/// The same bound for soft-removed series rows.
pub const CONFIRM_MAX_SERIES_ROWS_PER_LIBRARY_PER_RUN: usize = 20_000;

/// Paths stat-ed per `spawn_blocking` call by the removal sweeps, and rows
/// per candidate page.
pub const PRESENCE_BATCH: usize = 500;

/// Ids bound per `IN (...)` statement by the library reconcile and the
/// removal sweeps. Postgres caps a statement at 65,535 bind parameters;
/// 5,000 (the `jobs::orphan_sweep::ALIVE_QUERY_CHUNK` size) keeps every
/// statement far below it.
pub const RECONCILE_ID_CHUNK: usize = 5_000;

/// A sweep never queues a rescan for a series scanned within this many
/// days. A file that is present but that a recent scan did not restore will
/// not be restored by another one; the row is still kept, never confirmed
/// or purged.
pub const RESCAN_THROTTLE_DAYS: i64 = 6;

/// Above this many series to rescan in one library and run, a sweep queues
/// one (coalesced) library scan instead of per-series scans.
pub const MAX_SERIES_RESCANS_PER_RUN: usize = 50;

/// Paging bounds for [`auto_confirm_sweep_with`]. Production uses
/// [`SweepLimits::default`]; tests shrink them to cross page boundaries
/// with a handful of rows.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct SweepLimits {
    pub page: usize,
    pub max_issue_rows: usize,
    pub max_series_rows: usize,
    pub max_series_rescans: usize,
}

impl Default for SweepLimits {
    fn default() -> Self {
        Self {
            page: PRESENCE_BATCH,
            max_issue_rows: CONFIRM_MAX_ISSUE_ROWS_PER_LIBRARY_PER_RUN,
            max_series_rows: CONFIRM_MAX_SERIES_ROWS_PER_LIBRARY_PER_RUN,
            max_series_rescans: MAX_SERIES_RESCANS_PER_RUN,
        }
    }
}

pub use super::scanner::enumerate::Presence;

/// The library root emptied or vanished while a sweep was stat-ing it (a
/// share unmounted mid-run leaves an empty mount point). The batch's
/// "absent" answers are worthless; the library is aborted for the run.
#[derive(Debug, thiserror::Error)]
#[error("library root {0} is empty or unavailable mid-sweep; aborting the library for this run")]
pub struct RootVanished(pub String);

/// The canonical root must still resolve and hold at least one entry.
fn root_still_populated(root: &Path) -> anyhow::Result<PathBuf> {
    let vanished = || RootVanished(root.display().to_string());
    let canon = std::fs::canonicalize(root).map_err(|_| vanished())?;
    let mut entries = std::fs::read_dir(&canon).map_err(|_| vanished())?;
    if entries.next().is_none() {
        return Err(vanished().into());
    }
    Ok(canon)
}

/// Blocking batch: the [`Presence`] of each path, then a check that the
/// root still holds an entry. If it does not, the whole batch is an error
/// ([`RootVanished`]) and nothing from it may be confirmed or deleted.
#[doc(hidden)]
pub fn probe_files_blocking(
    root: &Path,
    ignore: &IgnoreRules,
    paths: &[String],
) -> anyhow::Result<Vec<Presence>> {
    let root_canon = root_still_populated(root)?;
    let out = paths
        .iter()
        .map(|p| enumerate::archive_presence(root, &root_canon, Path::new(p), ignore))
        .collect();
    root_still_populated(root)?;
    Ok(out)
}

/// Blocking batch: the [`Presence`] of each series folder (archives in
/// `exclude` do not count), with the same root check as
/// [`probe_files_blocking`].
#[doc(hidden)]
pub fn probe_folders_blocking(
    root: &Path,
    ignore: &IgnoreRules,
    folders: &[String],
    exclude: &HashSet<String>,
) -> anyhow::Result<Vec<Presence>> {
    let root_canon = root_still_populated(root)?;
    let out = folders
        .iter()
        .map(|f| {
            enumerate::folder_presence(root, &root_canon, Path::new(f), ignore, |a| {
                exclude.contains(a.to_string_lossy().as_ref())
            })
        })
        .collect();
    root_still_populated(root)?;
    Ok(out)
}

/// Is `lib` safe to judge from disk right now? Runs the scanner's own
/// [`validate_library`](super::scanner::validate::validate_library) gate
/// (root missing, unreadable, empty, …). On failure the removal sweep
/// `sweep` skips the whole library: one WARN line and one `issue` /
/// `skipped` library event, so an unmounted share never marches its rows
/// towards deletion.
pub(crate) async fn library_ready_for_sweep(
    state: &AppState,
    lib: &library::Model,
    sweep: &'static str,
) -> bool {
    let Err(e) = super::scanner::validate::validate_library(state, lib).await else {
        return true;
    };
    tracing::warn!(
        library_id = %lib.id,
        sweep,
        error = %e,
        "removal sweep: library root unavailable, skipping the library this run",
    );
    event_log::record(
        &state.db,
        NewEvent::new(
            lib.id,
            Category::Issue,
            Action::Skipped,
            Severity::Warning,
            format!("{sweep} skipped: library root unavailable ({e})"),
        )
        .detail(serde_json::json!({ "sweep": sweep, "error": e.to_string() })),
    )
    .await;
    false
}

/// What the removal sweeps need to stat a library's files: its root (as
/// stored) and its ignore rules, so "present" means exactly what a scan
/// would index ([`enumerate::archive_presence`]).
#[derive(Clone)]
pub(crate) struct PresenceProbe {
    root: PathBuf,
    ignore: IgnoreRules,
}

impl PresenceProbe {
    pub(crate) fn for_library(lib: &library::Model) -> Self {
        // Invalid globs make a scan refuse to run; fall back to the built-in
        // rules, which only ever count *more* files as present (the safe
        // direction for a sweep that deletes what is absent).
        let ignore = IgnoreRules::for_library(lib).unwrap_or_default();
        Self {
            root: PathBuf::from(&lib.root_path),
            ignore,
        }
    }

    /// The [`Presence`] of each `(key, file_path)`, [`PRESENCE_BATCH`]
    /// paths per blocking call. Errors (the root vanished mid-run) abort.
    pub(crate) async fn files<K>(
        &self,
        items: Vec<(K, String)>,
    ) -> anyhow::Result<Vec<(K, Presence)>>
    where
        K: Send + 'static,
    {
        let mut out = Vec::with_capacity(items.len());
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let (keys, paths): (Vec<K>, Vec<String>) = items.by_ref().take(PRESENCE_BATCH).unzip();
            let probe = self.clone();
            let presence = tokio::task::spawn_blocking(move || {
                probe_files_blocking(&probe.root, &probe.ignore, &paths)
            })
            .await??;
            out.extend(keys.into_iter().zip(presence));
        }
        Ok(out)
    }

    /// The [`Presence`] of each `(series id, folder_path)`, excluding the
    /// archives in `exclude`.
    pub(crate) async fn folders(
        &self,
        folders: Vec<(Uuid, String)>,
        exclude: HashSet<String>,
    ) -> anyhow::Result<std::collections::HashMap<Uuid, Presence>> {
        let exclude = std::sync::Arc::new(exclude);
        let mut out = std::collections::HashMap::new();
        for batch in folders.chunks(PRESENCE_BATCH) {
            let (ids, paths): (Vec<Uuid>, Vec<String>) = batch.iter().cloned().unzip();
            let probe = self.clone();
            let exclude = exclude.clone();
            let presence = tokio::task::spawn_blocking(move || {
                probe_folders_blocking(&probe.root, &probe.ignore, &paths, &exclude)
            })
            .await??;
            out.extend(ids.into_iter().zip(presence));
        }
        Ok(out)
    }
}

/// What [`queue_series_rescans`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RescanOutcome {
    /// Scoped series scans queued (or coalesced into one in flight).
    pub series_scans: u64,
    /// Library scans queued instead (0 or 1).
    pub library_scans: u64,
    /// Series not rescanned because they were scanned within
    /// [`RESCAN_THROTTLE_DAYS`].
    pub throttled: u64,
}

/// Queue rescans for `series_ids` so the normal restore path brings their
/// returning files back. A series scanned within [`RESCAN_THROTTLE_DAYS`]
/// is skipped (throttled). Up to `max_series` series get one scoped scan
/// each through the admin "Scan series" gate
/// (`JobRuntime::coalesce_scoped_scan`, not forced; a series with a scoped
/// scan in flight is coalesced into it); above that, one library scan goes
/// through `JobRuntime::coalesce_scan` instead. Enqueue failures are logged,
/// never fatal.
pub(crate) async fn queue_series_rescans(
    state: &AppState,
    library_id: Uuid,
    series_ids: &BTreeSet<Uuid>,
    max_series: usize,
) -> RescanOutcome {
    type Row = (
        Uuid,
        Option<String>,
        Option<chrono::DateTime<chrono::FixedOffset>>,
    );
    let mut outcome = RescanOutcome::default();
    let ids: Vec<Uuid> = series_ids.iter().copied().collect();
    let mut rows: Vec<Row> = Vec::new();
    for chunk in ids.chunks(RECONCILE_ID_CHUNK) {
        match series::Entity::find()
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::FolderPath)
            .column(series::Column::LastScannedAt)
            .filter(series::Column::Id.is_in(chunk.to_vec()))
            .into_tuple::<Row>()
            .all(&state.db)
            .await
        {
            Ok(r) => rows.extend(r),
            Err(e) => {
                tracing::error!(library_id = %library_id, error = %e, "removal sweep: series lookup failed");
                return outcome;
            }
        }
    }
    let recent = Utc::now().fixed_offset() - Duration::days(RESCAN_THROTTLE_DAYS);
    let (eligible, throttled): (Vec<Row>, Vec<Row>) = rows
        .into_iter()
        .partition(|(_, _, last)| last.is_none_or(|t| t < recent));
    outcome.throttled = throttled.len() as u64;
    if eligible.is_empty() {
        return outcome;
    }
    if eligible.len() > max_series {
        match state.jobs.coalesce_scan(library_id, false).await {
            Ok(_) => outcome.library_scans = 1,
            Err(e) => tracing::error!(
                library_id = %library_id,
                error = %e,
                "removal sweep: library rescan enqueue failed",
            ),
        }
        return outcome;
    }
    for (series_id, folder_path, _) in eligible {
        match state
            .jobs
            .coalesce_scoped_scan(
                library_id,
                series_id,
                folder_path,
                crate::jobs::scan_series::JobKind::Series,
                None,
                false,
            )
            .await
        {
            Ok(_) => outcome.series_scans += 1,
            Err(e) => tracing::error!(
                library_id = %library_id,
                series_id = %series_id,
                error = %e,
                "removal sweep: series rescan enqueue failed",
            ),
        }
    }
    outcome
}

/// `file_path`s of the Duplicates-page removals among `series_ids`'
/// issues. Those removals are deliberate and their files are on disk by
/// definition, so a removal sweep never counts them as "present again".
pub(crate) async fn duplicate_removed_paths(
    db: &DatabaseConnection,
    series_ids: &[Uuid],
) -> anyhow::Result<HashSet<String>> {
    let mut out = HashSet::new();
    for chunk in series_ids.chunks(RECONCILE_ID_CHUNK) {
        out.extend(
            IssueEntity::find()
                .select_only()
                .column(issue::Column::FilePath)
                .filter(issue::Column::SeriesId.is_in(chunk.to_vec()))
                .filter(Expr::cust(
                    "EXISTS (SELECT 1 FROM issue_duplicate_decision d \
                     WHERE d.issue_id = issues.id AND d.decision = 'remove')",
                ))
                .into_tuple::<String>()
                .all(db)
                .await?,
        );
    }
    Ok(out)
}

/// Rows a removal sweep left alone in one library run.
#[derive(Debug, Default)]
pub(crate) struct KeptRows {
    /// Present again: kept, and their series rescanned.
    pub present_issues: Vec<String>,
    pub present_series: HashSet<Uuid>,
    /// The disk would not say: kept, not rescanned.
    pub unknown_issues: Vec<String>,
    pub unknown_series: HashSet<Uuid>,
    /// Series to rescan (those of present issues plus present series).
    pub rescan: BTreeSet<Uuid>,
    /// Series holding an issue whose presence is unknown: never confirmed
    /// in this run, never rescanned.
    pub held_series: HashSet<Uuid>,
}

impl KeptRows {
    pub(crate) fn present(&self) -> u64 {
        (self.present_issues.len() + self.present_series.len()) as u64
    }
    pub(crate) fn unknown(&self) -> u64 {
        (self.unknown_issues.len() + self.unknown_series.len()) as u64
    }

    /// Queue the rescans, then log and record what was kept: one WARN each
    /// for present and unknown rows, and one `issue` / `skipped` summary
    /// event per library and sweep.
    pub(crate) async fn finish(
        &self,
        state: &AppState,
        library_id: Uuid,
        sweep: &'static str,
        max_series_rescans: usize,
    ) -> RescanOutcome {
        let outcome = if self.rescan.is_empty() {
            RescanOutcome::default()
        } else {
            queue_series_rescans(state, library_id, &self.rescan, max_series_rescans).await
        };
        if self.present() > 0 {
            tracing::warn!(
                library_id = %library_id,
                sweep,
                kept_issues = self.present_issues.len(),
                kept_series = self.present_series.len(),
                series_scans = outcome.series_scans,
                library_scans = outcome.library_scans,
                throttled = outcome.throttled,
                "removal sweep: removed rows are on disk again; kept and rescanned",
            );
        }
        if self.unknown() > 0 {
            tracing::warn!(
                library_id = %library_id,
                sweep,
                unknown_issues = self.unknown_issues.len(),
                unknown_series = self.unknown_series.len(),
                "removal sweep: rows whose files could not be read (unknown, not gone); kept",
            );
        }
        if self.present() > 0 || self.unknown() > 0 {
            /// Ids listed in the event detail; the counts are always complete.
            const SAMPLE: usize = 50;
            event_log::record(
                &state.db,
                NewEvent::new(
                    library_id,
                    Category::Issue,
                    Action::Skipped,
                    Severity::Warning,
                    format!(
                        "{sweep} kept {} row(s) whose files are on disk again and {} it could \
                         not read; {} series scan(s) and {} library scan(s) queued",
                        self.present(),
                        self.unknown(),
                        outcome.series_scans,
                        outcome.library_scans,
                    ),
                )
                .detail(serde_json::json!({
                    "sweep": sweep,
                    "present_issues": self.present_issues.len(),
                    "present_series": self.present_series.len(),
                    "unknown_issues": self.unknown_issues.len(),
                    "unknown_series": self.unknown_series.len(),
                    "series_scans": outcome.series_scans,
                    "library_scans": outcome.library_scans,
                    "rescans_throttled": outcome.throttled,
                    "present_issue_ids": self.present_issues.iter().take(SAMPLE).collect::<Vec<_>>(),
                    "present_series_ids": self.present_series.iter().take(SAMPLE).collect::<Vec<_>>(),
                    "unknown_issue_ids": self.unknown_issues.iter().take(SAMPLE).collect::<Vec<_>>(),
                    "unknown_series_ids": self.unknown_series.iter().take(SAMPLE).collect::<Vec<_>>(),
                })),
            )
            .await;
        }
        outcome
    }
}

#[derive(Debug, FromQueryResult)]
struct ConfirmIssueCandidate {
    id: String,
    series_id: Uuid,
    file_path: String,
    removed_at: chrono::DateTime<chrono::FixedOffset>,
    duplicate_removed: bool,
}

#[derive(Debug, FromQueryResult)]
struct ConfirmSeriesCandidate {
    id: Uuid,
    folder_path: Option<String>,
    removed_at: chrono::DateTime<chrono::FixedOffset>,
}

/// What one [`auto_confirm_sweep`] run did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmStats {
    /// Issue + series rows stamped `removal_confirmed_at`.
    pub confirmed: u64,
    /// Rows left unconfirmed because their file (or series folder) is on
    /// disk again.
    pub kept_present: u64,
    /// Rows left unconfirmed because the disk would not say (unreadable
    /// folder, I/O error): unknown is never gone.
    pub kept_unknown: u64,
    /// Scoped series scans queued (or coalesced into one in flight).
    pub rescans_queued: u64,
    /// Library scans queued instead of series scans.
    pub library_scans_queued: u64,
    /// Series not rescanned because they were scanned recently.
    pub rescans_throttled: u64,
    /// Libraries skipped because `validate_library` refused them.
    pub libraries_skipped: u64,
    /// Libraries aborted mid-run (the root emptied, a query failed).
    pub libraries_aborted: u64,
}

/// Cron-driven sweep: confirm removals once they've sat past the per-library
/// `soft_delete_days` window, but only for what is really gone.
///
/// Per library: the scanner's `validate_library` gate runs first and a
/// library that fails it (root missing, unreadable, empty) is skipped whole.
/// Then the candidates (soft-removed past the window, unconfirmed) are read
/// oldest first in keyset pages, at most
/// [`CONFIRM_MAX_ISSUE_ROWS_PER_LIBRARY_PER_RUN`] issue rows and
/// [`CONFIRM_MAX_SERIES_ROWS_PER_LIBRARY_PER_RUN`] series rows per run, and
/// stat-ed on the blocking pool ([`Presence`]):
///
/// - `Absent` (not-found class only): confirmed as before;
/// - `Present`: not confirmed; its series is rescanned (throttled, see
///   [`queue_series_rescans`]), which restores it;
/// - `Unknown` (any other I/O error, an unreadable folder): not confirmed,
///   not rescanned;
/// - a Duplicates-page removal (deliberate, file present by definition) is
///   confirmed without a stat;
/// - a series is not confirmed while one of its issues was just kept, or
///   while its folder holds (or may hold) an archive a scan would index.
///
/// After every stat batch the root must still hold an entry; if it emptied
/// mid-run the library is aborted and nothing from that batch is confirmed.
pub async fn auto_confirm_sweep(state: &AppState) -> anyhow::Result<ConfirmStats> {
    auto_confirm_sweep_with(state, SweepLimits::default()).await
}

/// [`auto_confirm_sweep`] with explicit paging bounds (tests).
#[doc(hidden)]
pub async fn auto_confirm_sweep_with(
    state: &AppState,
    limits: SweepLimits,
) -> anyhow::Result<ConfirmStats> {
    let libs = library::Entity::find().all(&state.db).await?;
    let mut stats = ConfirmStats::default();
    let now = Utc::now().fixed_offset();

    for lib in libs {
        if !library_ready_for_sweep(state, &lib, "Removal confirmation").await {
            stats.libraries_skipped += 1;
            continue;
        }
        let cutoff = now - Duration::days(lib.soft_delete_days as i64);
        let mut kept = KeptRows::default();
        let result = confirm_library(state, &lib, now, cutoff, limits, &mut kept).await;
        // Rows kept before an abort are still reported and rescanned.
        let outcome = kept
            .finish(
                state,
                lib.id,
                "Removal confirmation",
                limits.max_series_rescans,
            )
            .await;
        stats.kept_present += kept.present();
        stats.kept_unknown += kept.unknown();
        stats.rescans_queued += outcome.series_scans;
        stats.library_scans_queued += outcome.library_scans;
        stats.rescans_throttled += outcome.throttled;
        match result {
            Ok(n) => stats.confirmed += n,
            // One library failing must not stop the others being swept.
            Err((n, e)) => {
                stats.confirmed += n;
                stats.libraries_aborted += 1;
                tracing::error!(
                    library_id = %lib.id,
                    error = %e,
                    "reconcile sweep: library aborted",
                );
            }
        }
    }

    Ok(stats)
}

/// Returns the rows confirmed, or on an abort the rows confirmed before it
/// together with the error.
async fn confirm_library(
    state: &AppState,
    lib: &library::Model,
    now: chrono::DateTime<chrono::FixedOffset>,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
    limits: SweepLimits,
    kept: &mut KeptRows,
) -> Result<u64, (u64, anyhow::Error)> {
    let mut confirmed = 0u64;
    match confirm_issues(state, lib, now, cutoff, limits, kept, &mut confirmed).await {
        Ok(()) => {}
        Err(e) => return Err((confirmed, e)),
    }
    match confirm_series(state, lib, now, cutoff, limits, kept, &mut confirmed).await {
        Ok(()) => Ok(confirmed),
        Err(e) => Err((confirmed, e)),
    }
}

async fn confirm_issues(
    state: &AppState,
    lib: &library::Model,
    now: chrono::DateTime<chrono::FixedOffset>,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
    limits: SweepLimits,
    kept: &mut KeptRows,
    confirmed: &mut u64,
) -> anyhow::Result<()> {
    let db = &state.db;
    let probe = PresenceProbe::for_library(lib);
    let page_size = limits.page.max(1);
    let mut after: Option<(chrono::DateTime<chrono::FixedOffset>, String)> = None;
    let mut examined = 0usize;
    while examined < limits.max_issue_rows {
        let limit = page_size.min(limits.max_issue_rows - examined);
        let page = confirm_issue_page(db, lib.id, cutoff, after.as_ref(), limit).await?;
        if page.is_empty() {
            break;
        }
        examined += page.len();
        let full_page = page.len() == limit;
        after = page.last().map(|c| (c.removed_at, c.id.clone()));

        let mut confirm: Vec<String> = Vec::new();
        let mut to_stat: Vec<((String, Uuid), String)> = Vec::new();
        for c in page {
            if c.duplicate_removed {
                confirm.push(c.id);
            } else {
                to_stat.push(((c.id, c.series_id), c.file_path));
            }
        }
        for ((id, series_id), presence) in probe.files(to_stat).await? {
            match presence {
                Presence::Absent => confirm.push(id),
                Presence::Present => {
                    kept.rescan.insert(series_id);
                    kept.present_issues.push(id);
                }
                Presence::Unknown => {
                    kept.held_series.insert(series_id);
                    kept.unknown_issues.push(id);
                }
            }
        }
        for ids in confirm.chunks(RECONCILE_ID_CHUNK) {
            let res = IssueEntity::update_many()
                .col_expr(issue::Column::RemovalConfirmedAt, Expr::value(now))
                .filter(issue::Column::LibraryId.eq(lib.id))
                .filter(issue::Column::Id.is_in(ids.to_vec()))
                .filter(issue::Column::RemovedAt.is_not_null())
                .filter(issue::Column::RemovalConfirmedAt.is_null())
                .filter(issue::Column::RemovedAt.lt(cutoff))
                .exec(db)
                .await?;
            *confirmed += res.rows_affected;
        }
        if !full_page {
            break;
        }
    }
    Ok(())
}

async fn confirm_series(
    state: &AppState,
    lib: &library::Model,
    now: chrono::DateTime<chrono::FixedOffset>,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
    limits: SweepLimits,
    kept: &mut KeptRows,
    confirmed: &mut u64,
) -> anyhow::Result<()> {
    let db = &state.db;
    let probe = PresenceProbe::for_library(lib);
    let page_size = limits.page.max(1);
    let mut after: Option<(chrono::DateTime<chrono::FixedOffset>, Uuid)> = None;
    let mut examined = 0usize;
    while examined < limits.max_series_rows {
        let limit = page_size.min(limits.max_series_rows - examined);
        let page = confirm_series_page(db, lib.id, cutoff, after.as_ref(), limit).await?;
        if page.is_empty() {
            break;
        }
        examined += page.len();
        let full_page = page.len() == limit;
        after = page.last().map(|c| (c.removed_at, c.id));

        // Never confirm a series one of whose issues was just kept.
        let candidates: Vec<ConfirmSeriesCandidate> = page
            .into_iter()
            .filter(|c| !kept.rescan.contains(&c.id) && !kept.held_series.contains(&c.id))
            .collect();
        let with_folder: Vec<(Uuid, String)> = candidates
            .iter()
            .filter_map(|c| c.folder_path.clone().map(|f| (c.id, f)))
            .collect();
        let folder_ids: Vec<Uuid> = with_folder.iter().map(|(id, _)| *id).collect();
        let exclude = duplicate_removed_paths(db, &folder_ids).await?;
        let presence = probe.folders(with_folder, exclude).await?;
        let mut confirm: Vec<Uuid> = Vec::new();
        for c in &candidates {
            match presence.get(&c.id).copied().unwrap_or(Presence::Absent) {
                Presence::Absent => confirm.push(c.id),
                Presence::Present => {
                    kept.present_series.insert(c.id);
                    kept.rescan.insert(c.id);
                }
                Presence::Unknown => {
                    kept.unknown_series.insert(c.id);
                }
            }
        }
        for ids in confirm.chunks(RECONCILE_ID_CHUNK) {
            let res = series::Entity::update_many()
                .col_expr(series::Column::RemovalConfirmedAt, Expr::value(now))
                .filter(series::Column::LibraryId.eq(lib.id))
                .filter(series::Column::Id.is_in(ids.to_vec()))
                .filter(series::Column::RemovedAt.is_not_null())
                .filter(series::Column::RemovalConfirmedAt.is_null())
                .filter(series::Column::RemovedAt.lt(cutoff))
                .exec(db)
                .await?;
            *confirmed += res.rows_affected;
        }
        if !full_page {
            break;
        }
    }
    Ok(())
}

async fn confirm_issue_page(
    db: &DatabaseConnection,
    library_id: Uuid,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
    after: Option<&(chrono::DateTime<chrono::FixedOffset>, String)>,
    limit: usize,
) -> anyhow::Result<Vec<ConfirmIssueCandidate>> {
    const SELECT: &str = "SELECT id, series_id, file_path, removed_at, \
            EXISTS (SELECT 1 FROM issue_duplicate_decision d \
                    WHERE d.issue_id = issues.id AND d.decision = 'remove') \
              AS duplicate_removed \
         FROM issues \
         WHERE library_id = $1 AND removed_at IS NOT NULL \
           AND removal_confirmed_at IS NULL AND removed_at < $2";
    let stmt = match after {
        None => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("{SELECT} ORDER BY removed_at, id LIMIT $3"),
            [library_id.into(), cutoff.into(), (limit as i64).into()],
        ),
        Some((at, id)) => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("{SELECT} AND (removed_at, id) > ($4, $5) ORDER BY removed_at, id LIMIT $3"),
            [
                library_id.into(),
                cutoff.into(),
                (limit as i64).into(),
                (*at).into(),
                id.clone().into(),
            ],
        ),
    };
    Ok(ConfirmIssueCandidate::find_by_statement(stmt)
        .all(db)
        .await?)
}

async fn confirm_series_page(
    db: &DatabaseConnection,
    library_id: Uuid,
    cutoff: chrono::DateTime<chrono::FixedOffset>,
    after: Option<&(chrono::DateTime<chrono::FixedOffset>, Uuid)>,
    limit: usize,
) -> anyhow::Result<Vec<ConfirmSeriesCandidate>> {
    const SELECT: &str = "SELECT id, folder_path, removed_at FROM series \
         WHERE library_id = $1 AND removed_at IS NOT NULL \
           AND removal_confirmed_at IS NULL AND removed_at < $2";
    let stmt = match after {
        None => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("{SELECT} ORDER BY removed_at, id LIMIT $3"),
            [library_id.into(), cutoff.into(), (limit as i64).into()],
        ),
        Some((at, id)) => Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("{SELECT} AND (removed_at, id) > ($4, $5) ORDER BY removed_at, id LIMIT $3"),
            [
                library_id.into(),
                cutoff.into(),
                (limit as i64).into(),
                (*at).into(),
                (*id).into(),
            ],
        ),
    };
    Ok(ConfirmSeriesCandidate::find_by_statement(stmt)
        .all(db)
        .await?)
}
