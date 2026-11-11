//! Large-removal warning (audit 2026-11 R-01 step 4, decision D-5).
//!
//! When one library scan soft-removes an unusually large share of the
//! library, the scan raises a `LargeRemoval` health finding and one
//! `health` / `removed` library event. It is a warning only: the removal
//! proceeds exactly as before, there is no new state and no confirmation
//! step. The usual cause is a nested mount or network share that was
//! missing during the scan, or a folder moved aside while the library was
//! being reorganised. That is why it is worth a warning: the confirm and
//! purge sweeps only refuse a library whose *root* fails
//! `validate_library`, so the rows of a nested share that stays away are
//! confirmed after `soft_delete_days` and hard-purged
//! `soft_delete_days × library.hard_purge_multiplier` days later, taking
//! progress, markers, ratings and collection entries with them; a file that
//! returns after that is a new issue with no read state.
//!
//! **What is measured.** The library reconcile stamps one `removed_at` on
//! every issue and series it soft-removes in a pass
//! ([`ScanStats::removal_stamp`]), so "what this run removed" is exactly the
//! rows carrying that stamp: rows that were already removed are never
//! counted again. The denominator is the library as if those rows were back
//! (active rows now plus the rows removed by the run).
//!
//! **Which scans.** Only library scans raise it: full scans and
//! watcher-scoped library scans, both of which run the folder-missing sweep
//! where a mass removal happens, measured against the whole library. A
//! per-series or single-issue scan never raises it: it walks one folder the
//! admin pointed at, so the removal it reports is local and expected. A
//! scan cancelled or superseded before its reconcile, a scan whose
//! reconcile failed or stopped, and a scan refused by the unavailable-root
//! guard never reach the check. It runs as soon as the reconcile returns
//! Ok, before the scan's next lease / cancel check: the rows are removed
//! by then, and no later scan would report them.
//!
//! **Cost.** Nothing on an ordinary scan: the check queries only when the
//! run's own counters reach a floor ([`MIN_ISSUES`] / [`MIN_SERIES`]); then
//! it runs one counting statement and one grouped folder statement. The
//! keep-alive / resolve step in `HealthCollector::finalize` adds one indexed
//! lookup of the library's open `LargeRemoval` row per scan and, only while
//! such a row is open (or dismissed and not yet resolved), one statement
//! counting the rows that still carry the stamp ([`stamped_counts`]): two
//! range reads of the partial `(library_id, removed_at)` indexes, bounded
//! by the size of that removal. Active rows are never counted there: the
//! totals come from the payload.
//!
//! **Resolution.** The finding stays open while the rows the run removed
//! that still carry its stamp would cross the threshold against the totals
//! recorded when it was raised (same test, [`RemovalCounts::crosses`]; the
//! totals are fixed so that a library growing while the content is still
//! away does not dilute the share and close it). Each
//! scan that ends while it holds re-sights the row (`last_seen_at` moves,
//! `scan_id` stays on the run that removed the content), so the full-scan
//! auto-resolve sweep does not close it just because the next scan removed
//! nothing new. Once enough of the content is back (restored), or purged,
//! the row resolves. A later run that crosses the threshold again updates
//! the same row (one fingerprint per library) with its own numbers and
//! re-opens it, including clearing a dismissal: it is a new removal.

use chrono::{DateTime, DurationRound, FixedOffset, TimeDelta};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::health::IssueKind;

/// The share of the library, in percent, one scan has to remove (strictly
/// more than this) before it warns. With [`MIN_ISSUES`] / [`MIN_SERIES`]:
/// the warning needs a share AND an absolute count, so a small library
/// losing a few files never alarms and a huge one is judged by share.
///
/// Chosen without production data (none was available). 20% is far above
/// what an ordinary cleanup removes from a library of any real size (a
/// series or two), and well below what a missing mount or share removes
/// (usually the whole subtree it held). Making these tunable would need
/// the full settings chain (registry entry, `Config` field,
/// `apply_setting` branch, admin UI); they are constants until there is
/// data to tune against.
pub const SHARE_PERCENT: u64 = 20;

/// The fewest issues one scan has to remove for the issue share to count.
/// 50 keeps a deleted 30-issue series in a 100-issue library quiet.
pub const MIN_ISSUES: u64 = 50;

/// The fewest series one scan has to remove for the series share to count.
/// Series are measured too because one missing share of many small series
/// folders (one-shots, minis) can be a large part of the series while being
/// a small part of the issues next to a few long runs.
pub const MIN_SERIES: u64 = 10;

/// How many top-level folders the finding names (most removed issues
/// first). `folder_count` carries the full number.
pub const TOP_FOLDERS_LIMIT: usize = 5;

/// Rows one reconcile pass removed, against the library as if they were
/// back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemovalCounts {
    pub issues_removed: u64,
    pub issues_total: u64,
    pub series_removed: u64,
    pub series_total: u64,
}

impl RemovalCounts {
    /// More than [`SHARE_PERCENT`] of the issues and at least
    /// [`MIN_ISSUES`] of them, or the same for series with [`MIN_SERIES`].
    pub fn crosses(&self) -> bool {
        let over = |removed: u64, total: u64, floor: u64| {
            removed >= floor
                && u128::from(removed) * 100 > u128::from(total) * u128::from(SHARE_PERCENT)
        };
        over(self.issues_removed, self.issues_total, MIN_ISSUES)
            || over(self.series_removed, self.series_total, MIN_SERIES)
    }

    /// Could a run with these counters cross? The cheap gate before any
    /// query: below both floors it cannot.
    pub fn may_cross(issues_removed: u64, series_removed: u64) -> bool {
        issues_removed >= MIN_ISSUES || series_removed >= MIN_SERIES
    }
}

/// One top-level folder (relative to the library root) and how many of the
/// removed issues lived under it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovedFolder {
    pub folder: String,
    pub issues: u64,
}

/// `removed_at` values as stored: Postgres keeps microseconds.
pub fn normalize_stamp(stamp: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    stamp
        .duration_trunc(TimeDelta::microseconds(1))
        .unwrap_or(stamp)
}

#[derive(Debug, FromQueryResult)]
struct CountsRow {
    issues_removed: i64,
    issues_active: i64,
    series_removed: i64,
    series_active: i64,
}

fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// [`RemovalCounts`] for the rows of `library_id` soft-removed at `stamp`,
/// in one statement.
pub async fn episode_counts(
    db: &DatabaseConnection,
    library_id: Uuid,
    stamp: DateTime<FixedOffset>,
) -> anyhow::Result<RemovalCounts> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT
             (SELECT count(*) FROM issues WHERE library_id = $1 AND removed_at = $2) AS issues_removed,
             (SELECT count(*) FROM issues WHERE library_id = $1 AND removed_at IS NULL) AS issues_active,
             (SELECT count(*) FROM series WHERE library_id = $1 AND removed_at = $2) AS series_removed,
             (SELECT count(*) FROM series WHERE library_id = $1 AND removed_at IS NULL) AS series_active"#,
        [library_id.into(), normalize_stamp(stamp).into()],
    );
    let row = CountsRow::find_by_statement(stmt)
        .one(db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("large-removal counts returned no row"))?;
    let issues_removed = to_u64(row.issues_removed);
    let series_removed = to_u64(row.series_removed);
    Ok(RemovalCounts {
        issues_removed,
        issues_total: to_u64(row.issues_active) + issues_removed,
        series_removed,
        series_total: to_u64(row.series_active) + series_removed,
    })
}

#[derive(Debug, FromQueryResult)]
struct StampedRow {
    issues_removed: i64,
    series_removed: i64,
}

/// How many issues and series of `library_id` still carry `stamp`: the
/// rows one removal run removed that are neither restored nor purged (a
/// row restored and removed again carries the later run's stamp). Reads only the partial
/// `(library_id, removed_at)` indexes, so its cost is bounded by the size
/// of that removal, not of the library.
pub async fn stamped_counts(
    db: &DatabaseConnection,
    library_id: Uuid,
    stamp: DateTime<FixedOffset>,
) -> anyhow::Result<(u64, u64)> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT
             (SELECT count(*) FROM issues WHERE library_id = $1 AND removed_at = $2) AS issues_removed,
             (SELECT count(*) FROM series WHERE library_id = $1 AND removed_at = $2) AS series_removed"#,
        [library_id.into(), normalize_stamp(stamp).into()],
    );
    let row = StampedRow::find_by_statement(stmt)
        .one(db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("large-removal stamped counts returned no row"))?;
    Ok((to_u64(row.issues_removed), to_u64(row.series_removed)))
}

#[derive(Debug, FromQueryResult)]
struct FolderRow {
    folder: String,
    n: i64,
}

/// Removed issues per top-level folder under `root`, most first. Names are
/// relative to the root; a file outside it (never expected) is left out.
async fn top_folders(
    db: &DatabaseConnection,
    library_id: Uuid,
    root: &str,
    stamp: DateTime<FixedOffset>,
) -> anyhow::Result<(Vec<RemovedFolder>, u32)> {
    let prefix = format!("{}/", root.trim_end_matches('/'));
    // `substr` counts characters, 1-based.
    let start = i32::try_from(prefix.chars().count())
        .unwrap_or(i32::MAX - 1)
        .saturating_add(1);
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT split_part(substr(file_path, $3::int4), '/', 1) AS folder, count(*) AS n
             FROM issues
            WHERE library_id = $1 AND removed_at = $2 AND starts_with(file_path, $4)
            GROUP BY 1
            ORDER BY n DESC, folder ASC"#,
        [
            library_id.into(),
            normalize_stamp(stamp).into(),
            start.into(),
            prefix.into(),
        ],
    );
    let rows = FolderRow::find_by_statement(stmt).all(db).await?;
    let folder_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let top = rows
        .into_iter()
        .take(TOP_FOLDERS_LIMIT)
        .map(|r| RemovedFolder {
            folder: r.folder,
            issues: to_u64(r.n),
        })
        .collect();
    Ok((top, folder_count))
}

/// The finding for this run, if its removal crosses the threshold.
/// `issues_removed` / `series_removed` are the run's own counters for the
/// reconcile pass that wrote `stamp`; below both floors no query runs.
pub async fn assess(
    db: &DatabaseConnection,
    library_id: Uuid,
    root: &str,
    scan_id: Uuid,
    stamp: DateTime<FixedOffset>,
    issues_removed: u64,
    series_removed: u64,
) -> anyhow::Result<Option<IssueKind>> {
    if !RemovalCounts::may_cross(issues_removed, series_removed) {
        return Ok(None);
    }
    let counts = episode_counts(db, library_id, stamp).await?;
    if !counts.crosses() {
        return Ok(None);
    }
    let (top_folders, folder_count) = top_folders(db, library_id, root, stamp).await?;
    Ok(Some(IssueKind::LargeRemoval {
        scan_id,
        removed_at: normalize_stamp(stamp),
        issues_removed: counts.issues_removed,
        issues_total: counts.issues_total,
        series_removed: counts.series_removed,
        series_total: counts.series_total,
        top_folders,
        folder_count,
    }))
}

/// `part` of `whole` as a whole percent, for messages.
pub fn percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    (u128::from(part) * 100 / u128::from(whole)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(ir: u64, it: u64, sr: u64, st: u64) -> RemovalCounts {
        RemovalCounts {
            issues_removed: ir,
            issues_total: it,
            series_removed: sr,
            series_total: st,
        }
    }

    #[test]
    fn needs_share_and_floor() {
        // A 10-issue library losing all of it: below both floors.
        assert!(!c(10, 10, 1, 1).crosses());
        // Exactly 20% is not "more than".
        assert!(!c(200, 1000, 1, 50).crosses());
        assert!(c(201, 1000, 1, 50).crosses());
        // Share without the floor.
        assert!(!c(49, 60, 1, 5).crosses());
        assert!(c(50, 60, 1, 5).crosses());
        // A large library, small share.
        assert!(!c(500, 10_000, 5, 400).crosses());
    }

    #[test]
    fn series_share_trips_alone() {
        // Ten one-shot folders out of 30 next to a long run.
        assert!(c(10, 1_000, 10, 30).crosses());
        assert!(!c(9, 1_000, 9, 30).crosses());
        assert!(!c(10, 1_000, 10, 50).crosses());
    }

    #[test]
    fn gate_matches_floors() {
        assert!(!RemovalCounts::may_cross(MIN_ISSUES - 1, MIN_SERIES - 1));
        assert!(RemovalCounts::may_cross(MIN_ISSUES, 0));
        assert!(RemovalCounts::may_cross(0, MIN_SERIES));
    }

    #[test]
    fn percent_rounds_down() {
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(5, 0), 0);
    }
}
