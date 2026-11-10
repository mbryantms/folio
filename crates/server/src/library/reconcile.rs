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
//! defined here as [`auto_confirm_sweep`]. Milestone 9 wires the cron.

use chrono::{Duration, Utc};
use entity::{
    issue::{self, Entity as IssueEntity},
    library, series,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter,
    QuerySelect, Set, sea_query::Expr,
};
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

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

use super::event_log::{Action, Category, EventCollector, Severity};
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
    let removed: Vec<(Uuid, String)> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::Name)
        .filter(series::Column::Id.is_in(ids))
        .filter(series::Column::RemovedAt.is_not_null())
        .into_tuple()
        .all(db)
        .await?;
    restore_removed_series_with_live_issues(db, removed, events).await
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
    restore_removed_series_with_live_issues(db, removed, events).await
}

/// The shared body: `removed` are `(id, name)` of soft-removed series.
async fn restore_removed_series_with_live_issues(
    db: &DatabaseConnection,
    removed: Vec<(Uuid, String)>,
    events: &mut EventCollector,
) -> anyhow::Result<u64> {
    if removed.is_empty() {
        return Ok(0);
    }
    let has_live: HashSet<Uuid> = IssueEntity::find()
        .select_only()
        .column(issue::Column::SeriesId)
        .filter(issue::Column::SeriesId.is_in(removed.iter().map(|(id, _)| *id)))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(not_duplicate_removed())
        .distinct()
        .into_tuple::<Uuid>()
        .all(db)
        .await?
        .into_iter()
        .collect();
    let to_restore: Vec<(Uuid, String)> = removed
        .into_iter()
        .filter(|(id, _)| has_live.contains(id))
        .collect();
    if to_restore.is_empty() {
        return Ok(0);
    }
    let res = series::Entity::update_many()
        .col_expr(
            series::Column::RemovedAt,
            Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .col_expr(
            series::Column::RemovalConfirmedAt,
            Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .filter(series::Column::Id.is_in(to_restore.iter().map(|(id, _)| *id)))
        .filter(series::Column::RemovedAt.is_not_null())
        .exec(db)
        .await?;
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
    Ok(res.rows_affected)
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
    restore_library_series_with_live_issues(db, library_id, &mut events).await?;
    events.flush(db).await;

    Ok(())
}

/// Returns the number of soft-removed series it restored (the caller drops
/// the caches that depend on `series.removed_at` when it is non-zero).
#[expect(clippy::too_many_arguments)]
pub async fn reconcile_library_seen(
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
) -> anyhow::Result<u64> {
    let now = Utc::now().fixed_offset();

    if !scanned_series_ids.is_empty() {
        let issues = IssueEntity::find()
            .filter(not_duplicate_removed())
            .filter(issue::Column::LibraryId.eq(library_id))
            .filter(
                issue::Column::SeriesId
                    .is_in(scanned_series_ids.iter().copied().collect::<Vec<_>>()),
            )
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
        )
        .await?;
        mark_empty_series_removed(db, scanned_series_ids.iter().copied(), now, stats, events)
            .await?;
    }

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
    restore_library_series_with_live_issues(db, library_id, events).await
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
    )
    .await?;
    mark_empty_series_removed(db, std::iter::once(series_id), now, stats, events).await?;
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

async fn reconcile_issue_rows_seen(
    db: &DatabaseConnection,
    issues: Vec<ReconcileIssue>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
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
    series_ids: impl IntoIterator<Item = Uuid>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
    events: &mut EventCollector,
) -> anyhow::Result<()> {
    let all_ids: Vec<Uuid> = series_ids.into_iter().collect();
    if all_ids.is_empty() {
        return Ok(());
    }

    // PERF-7: one grouped probe for "which of these series still have an active
    // issue?" instead of a per-series `.one()` (N+1 by series count).
    let has_active: HashSet<Uuid> = IssueEntity::find()
        .select_only()
        .column(issue::Column::SeriesId)
        .filter(issue::Column::SeriesId.is_in(all_ids.clone()))
        .filter(issue::Column::RemovedAt.is_null())
        .distinct()
        .into_tuple::<Uuid>()
        .all(db)
        .await?
        .into_iter()
        .collect();

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
    let to_flip: Vec<Uuid> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .filter(series::Column::Id.is_in(empty))
        .filter(series::Column::RemovedAt.is_null())
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if to_flip.is_empty() {
        return Ok(());
    }

    // One batched soft-delete for every newly-empty series.
    series::Entity::update_many()
        .col_expr(series::Column::RemovedAt, Expr::value(now))
        .col_expr(
            series::Column::RemovalConfirmedAt,
            Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .filter(series::Column::Id.is_in(to_flip.clone()))
        .exec(db)
        .await?;
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

/// Cron-driven sweep: confirm removals once they've sat past the per-library
/// `soft_delete_days` window. Runs without any scanner involvement.
pub async fn auto_confirm_sweep(db: &DatabaseConnection) -> anyhow::Result<u64> {
    let libs = library::Entity::find().all(db).await?;
    let mut total = 0u64;
    let now = Utc::now().fixed_offset();

    for lib in libs {
        let cutoff = now - Duration::days(lib.soft_delete_days as i64);

        // Issues
        let res = IssueEntity::update_many()
            .col_expr(issue::Column::RemovalConfirmedAt, Expr::value(now))
            .filter(issue::Column::LibraryId.eq(lib.id))
            .filter(issue::Column::RemovedAt.is_not_null())
            .filter(issue::Column::RemovalConfirmedAt.is_null())
            .filter(issue::Column::RemovedAt.lt(cutoff))
            .exec(db)
            .await?;
        total += res.rows_affected;

        // Series
        let res = series::Entity::update_many()
            .col_expr(series::Column::RemovalConfirmedAt, Expr::value(now))
            .filter(series::Column::LibraryId.eq(lib.id))
            .filter(series::Column::RemovedAt.is_not_null())
            .filter(series::Column::RemovalConfirmedAt.is_null())
            .filter(series::Column::RemovedAt.lt(cutoff))
            .exec(db)
            .await?;
        total += res.rows_affected;
    }

    Ok(total)
}
