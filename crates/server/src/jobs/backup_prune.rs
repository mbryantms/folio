//! Daily retention sweep for archive-rewrite `.bak` backups
//! (archive-rewrite-1.0 M8 / roadmap WP-1.5).
//!
//! Every archive rewrite — page edit, sidecar writeback, scan-time
//! CBR→CBZ conversion — keeps the original behind as `<archive>.bak`
//! (plus rotated `.bak.N` slots, see [`crate::archive_rewrite`]). Those
//! are full-size copies, so without a reaper a library that gets edited
//! slowly doubles on disk. This sweep walks every library that has
//! `allow_archive_writeback = true` and a non-zero
//! `archive_backup_retain_days`, and removes backup files whose mtime is
//! older than that many days. `0` means "keep forever".
//!
//! Best-effort, like the thumbnail orphan sweep: a per-file delete error
//! is counted and logged, and the walk continues. Libraries without
//! writeback can't have produced backups through Folio, so nothing is
//! ever deleted under them — a stray `*.bak` an operator left on a
//! read-only mount is not ours to delete.
//!
//! The same walk is the source of the health page's backup-storage card
//! (audit 2026-11, N-L4): the files that survive the sweep are tallied
//! and stored in `library_backup_stats` via
//! [`crate::library::backup_stats`], for every library — the
//! non-writeback ones get a read-only walk so their card is a stored
//! value too, instead of a walk per request.

use crate::library::backup_stats::{self, Tally};
use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::state::AppState;
use entity::library;
use sea_orm::EntityTrait;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Roll-up of one sweep across all libraries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneStats {
    /// Backup files deleted.
    pub removed: usize,
    /// Bytes reclaimed by those deletes.
    pub bytes: u64,
    /// Files that matched the age cut-off but could not be deleted.
    pub errors: usize,
    /// Libraries that were pruned (writeback on, retain_days > 0). Every
    /// library is tallied; only these are candidates for deletion.
    pub libraries: usize,
}

impl PruneStats {
    fn absorb(&mut self, other: PruneStats) {
        self.removed += other.removed;
        self.bytes += other.bytes;
        self.errors += other.errors;
        self.libraries += other.libraries;
    }
}

/// Run one sweep over every library. Prunes the eligible ones, stores a
/// fresh backup tally for all of them, and emits one `archive.removed`
/// library event per library that had at least one backup pruned.
pub async fn run(state: &AppState) -> anyhow::Result<PruneStats> {
    let libs = library::Entity::find().all(&state.db).await?;

    let now = SystemTime::now();
    let mut total = PruneStats::default();
    for lib in libs {
        let root = std::path::PathBuf::from(&lib.root_path);
        // Only a writeback library is pruned; `0` on either side means
        // the walk is read-only and only refreshes the stored tally.
        let retain_days = if lib.allow_archive_writeback {
            lib.archive_backup_retain_days
        } else {
            0
        };
        // Walk off the async runtime — a large library has many dirents.
        let (stats, tally) =
            tokio::task::spawn_blocking(move || prune_library(&root, retain_days, now))
                .await
                .map_err(|e| anyhow::anyhow!("backup prune walk join failed: {e}"))?;
        total.absorb(stats);
        if let Some(tally) = tally
            && let Err(e) = backup_stats::store(&state.db, lib.id, &tally, chrono::Utc::now()).await
        {
            tracing::warn!(library_id = %lib.id, error = %e, "archive backup prune: tally store failed");
        }
        if stats.removed == 0 && stats.errors == 0 {
            continue;
        }
        tracing::info!(
            library_id = %lib.id,
            removed = stats.removed,
            bytes = stats.bytes,
            errors = stats.errors,
            retain_days,
            "archive backup prune: library swept"
        );
        if stats.removed > 0 {
            event_log::record(
                &state.db,
                NewEvent::new(
                    lib.id,
                    Category::Archive,
                    Action::Removed,
                    Severity::Info,
                    format!(
                        "Pruned {} archive backup{} older than {} day{}",
                        stats.removed,
                        if stats.removed == 1 { "" } else { "s" },
                        retain_days,
                        if retain_days == 1 { "" } else { "s" },
                    ),
                )
                .detail(serde_json::json!({
                    "removed": stats.removed,
                    "bytes": stats.bytes,
                    "errors": stats.errors,
                    "retain_days": retain_days,
                })),
            )
            .await;
        }
    }
    Ok(total)
}

/// Walk `root` once: delete every `.bak` / `.bak.N` file whose mtime is
/// more than `retain_days` days before `now`, and tally the ones that
/// stay. `retain_days <= 0` deletes nothing (an empty [`PruneStats`] with
/// `libraries = 0`) but still tallies. An unreadable root (unmounted
/// share) prunes nothing and yields no tally, so the stored count is left
/// alone rather than zeroed.
///
/// Pure filesystem — no DB — so tests and the sweep share it.
pub fn prune_library(
    root: &Path,
    retain_days: i32,
    now: SystemTime,
) -> (PruneStats, Option<Tally>) {
    let mut stats = PruneStats::default();
    if let Err(e) = std::fs::read_dir(root) {
        tracing::warn!(root = %root.display(), error = %e, "backup prune: root unreadable, skipped");
        return (stats, None);
    }
    let mut tally = Tally::default();
    let cutoff = (retain_days > 0).then(|| {
        stats.libraries = 1;
        now - Duration::from_secs(u64::from(retain_days.unsigned_abs()) * 86_400)
    });

    backup_stats::for_each_backup(root, |entry, meta| {
        let expired = match (cutoff, meta.modified()) {
            (Some(cutoff), Ok(modified)) => modified < cutoff,
            _ => false,
        };
        if !expired {
            tally.add(meta);
            return;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                stats.removed += 1;
                stats.bytes += meta.len();
                tracing::debug!(path = %entry.path().display(), "backup prune: removed");
            }
            Err(e) => {
                stats.errors += 1;
                tally.add(meta);
                tracing::warn!(path = %entry.path().display(), error = %e, "backup prune: remove failed");
            }
        }
    });
    (stats, Some(tally))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, age: Duration, now: SystemTime) {
        fs::write(path, b"x").unwrap();
        let t = filetime::FileTime::from_system_time(now - age);
        filetime::set_file_mtime(path, t).unwrap();
    }

    #[test]
    fn prunes_only_old_backups_and_tallies_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let day = Duration::from_secs(86_400);
        let old = dir.path().join("a.cbz.bak");
        let old_slot = dir.path().join("a.cbz.bak.2");
        let young = dir.path().join("b.cbz.bak");
        let not_backup = dir.path().join("c.cbz");
        touch(&old, 40 * day, now);
        touch(&old_slot, 40 * day, now);
        touch(&young, 5 * day, now);
        touch(&not_backup, 400 * day, now);

        let (stats, tally) = prune_library(dir.path(), 30, now);
        let tally = tally.expect("readable root");
        assert_eq!(stats.removed, 2);
        assert_eq!(stats.errors, 0);
        assert!(!old.exists());
        assert!(!old_slot.exists());
        assert!(young.exists());
        assert!(not_backup.exists(), "non-backup files are never touched");
        assert_eq!(
            tally.file_count, 1,
            "the survivor is what the health card shows"
        );
        assert_eq!(tally.total_bytes, 1);
    }

    #[test]
    fn zero_retain_days_is_a_no_op_that_still_tallies() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = dir.path().join("a.cbz.bak");
        touch(&old, Duration::from_secs(400 * 86_400), now);
        let (stats, tally) = prune_library(dir.path(), 0, now);
        assert_eq!(stats, PruneStats::default());
        assert!(old.exists());
        assert_eq!(tally.expect("readable root").file_count, 1);
    }

    #[test]
    fn unreadable_root_yields_no_tally() {
        let dir = tempfile::tempdir().unwrap();
        let (stats, tally) = prune_library(&dir.path().join("unmounted"), 30, SystemTime::now());
        assert_eq!(stats, PruneStats::default());
        assert_eq!(tally, None, "a missing root must not zero the stored count");
    }
}
