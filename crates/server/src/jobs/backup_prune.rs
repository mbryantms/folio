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
//! writeback can't have produced backups through Folio, so they are
//! skipped rather than walked — a stray `*.bak` an operator left on a
//! read-only mount is not ours to delete.

use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::state::AppState;
use entity::library;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
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
    /// Libraries that were walked (writeback on, retain_days > 0).
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

/// Run one sweep over every eligible library. Emits one `archive.removed`
/// library event per library that had at least one backup pruned.
pub async fn run(state: &AppState) -> anyhow::Result<PruneStats> {
    let libs = library::Entity::find()
        .filter(library::Column::AllowArchiveWriteback.eq(true))
        .filter(library::Column::ArchiveBackupRetainDays.gt(0))
        .all(&state.db)
        .await?;

    let now = SystemTime::now();
    let mut total = PruneStats::default();
    for lib in libs {
        let root = std::path::PathBuf::from(&lib.root_path);
        let retain_days = lib.archive_backup_retain_days;
        // Walk off the async runtime — a large library has many dirents.
        let stats = tokio::task::spawn_blocking(move || prune_library(&root, retain_days, now))
            .await
            .map_err(|e| anyhow::anyhow!("backup prune walk join failed: {e}"))?;
        total.absorb(stats);
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

/// Walk `root` and delete every `.bak` / `.bak.N` file whose mtime is
/// more than `retain_days` days before `now`. `retain_days <= 0` is a
/// no-op (returns an empty [`PruneStats`] with `libraries = 0`).
///
/// Pure filesystem — no DB — so tests and the sweep share it.
pub fn prune_library(root: &Path, retain_days: i32, now: SystemTime) -> PruneStats {
    let mut stats = PruneStats::default();
    if retain_days <= 0 {
        return stats;
    }
    stats.libraries = 1;
    let cutoff = now - Duration::from_secs(u64::from(retain_days.unsigned_abs()) * 86_400);

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| match e {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!(root = %root.display(), error = %err, "backup prune: walk error");
                None
            }
        })
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !crate::archive_rewrite::is_backup_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                stats.removed += 1;
                stats.bytes += meta.len();
                tracing::debug!(path = %entry.path().display(), "backup prune: removed");
            }
            Err(e) => {
                stats.errors += 1;
                tracing::warn!(path = %entry.path().display(), error = %e, "backup prune: remove failed");
            }
        }
    }
    stats
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
    fn prunes_only_old_backups() {
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

        let stats = prune_library(dir.path(), 30, now);
        assert_eq!(stats.removed, 2);
        assert_eq!(stats.errors, 0);
        assert!(!old.exists());
        assert!(!old_slot.exists());
        assert!(young.exists());
        assert!(not_backup.exists(), "non-backup files are never touched");
    }

    #[test]
    fn zero_retain_days_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = dir.path().join("a.cbz.bak");
        touch(&old, Duration::from_secs(400 * 86_400), now);
        let stats = prune_library(dir.path(), 0, now);
        assert_eq!(stats, PruneStats::default());
        assert!(old.exists());
    }
}
