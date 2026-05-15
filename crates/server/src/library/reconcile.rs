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
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    sea_query::Expr,
};
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

use super::scanner::stats::ScanStats;

/// Walk all non-removed issues for `library_id`. Soft-delete the ones whose
/// file is missing; restore the ones whose file came back.
pub async fn reconcile_library(
    db: &DatabaseConnection,
    library_id: Uuid,
    stats: &mut ScanStats,
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();

    let issues = IssueEntity::find()
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

    Ok(())
}

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
    stats: &mut ScanStats,
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();

    if !scanned_series_ids.is_empty() {
        let issues = IssueEntity::find()
            .filter(issue::Column::LibraryId.eq(library_id))
            .filter(
                issue::Column::SeriesId
                    .is_in(scanned_series_ids.iter().copied().collect::<Vec<_>>()),
            )
            .all(db)
            .await?;
        reconcile_issue_rows_seen(db, issues, seen_paths, scanned_folder_paths, now, stats).await?;
        mark_empty_series_removed(db, scanned_series_ids.iter().copied(), now, stats).await?;
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
        missing_series.push(srow.id);
        let res = IssueEntity::update_many()
            .col_expr(issue::Column::RemovedAt, Expr::value(now))
            .col_expr(
                issue::Column::RemovalConfirmedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .filter(issue::Column::SeriesId.eq(srow.id))
            .filter(issue::Column::RemovedAt.is_null())
            .exec(db)
            .await?;
        stats.issues_removed += res.rows_affected;
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

    Ok(())
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

    let issues = IssueEntity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .all(db)
        .await?;

    for row in issues {
        let path = Path::new(&row.file_path);
        let exists = path.exists();
        match (row.removed_at.is_some(), exists) {
            (false, false) => {
                let mut am: issue::ActiveModel = row.into();
                am.removed_at = Set(Some(now));
                am.removal_confirmed_at = Set(None);
                am.update(db).await?;
                stats.issues_removed += 1;
            }
            (true, true) => {
                let mut am: issue::ActiveModel = row.into();
                am.removed_at = Set(None);
                am.removal_confirmed_at = Set(None);
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
) -> anyhow::Result<()> {
    let now = Utc::now().fixed_offset();

    let issues = IssueEntity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .all(db)
        .await?;
    reconcile_issue_rows_seen(db, issues, seen_paths, scanned_folder_paths, now, stats).await?;
    mark_empty_series_removed(db, std::iter::once(series_id), now, stats).await?;

    Ok(())
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
    issues: Vec<issue::Model>,
    seen_paths: &HashSet<String>,
    scanned_folder_paths: &HashSet<String>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
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
                    remove_ids.push(row.id);
                }
            }
            (true, true) => restore_ids.push(row.id),
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

    Ok(())
}

async fn mark_empty_series_removed(
    db: &DatabaseConnection,
    series_ids: impl IntoIterator<Item = Uuid>,
    now: chrono::DateTime<chrono::FixedOffset>,
    stats: &mut ScanStats,
) -> anyhow::Result<()> {
    for series_id in series_ids {
        let any_active = IssueEntity::find()
            .filter(issue::Column::SeriesId.eq(series_id))
            .filter(issue::Column::RemovedAt.is_null())
            .one(db)
            .await?
            .is_some();
        if !any_active {
            let res = series::Entity::update_many()
                .col_expr(series::Column::RemovedAt, Expr::value(now))
                .col_expr(
                    series::Column::RemovalConfirmedAt,
                    Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
                )
                .filter(series::Column::Id.eq(series_id))
                .filter(series::Column::RemovedAt.is_null())
                .exec(db)
                .await?;
            stats.series_removed += res.rows_affected;
        }
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
