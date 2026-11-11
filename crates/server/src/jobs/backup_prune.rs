//! Daily retention sweep for archive-rewrite `.bak` backups
//! (archive-rewrite-1.0 M8 / roadmap WP-1.5).
//!
//! Every archive rewrite — page edit, sidecar writeback, scan-time
//! CBR→CBZ conversion — keeps the original behind as `<archive>.bak`
//! (plus rotated `.bak.N` slots, see [`crate::archive_rewrite`]). Those
//! are full-size copies, so without a reaper a library that gets edited
//! slowly doubles on disk. This sweep walks every library that has
//! `allow_archive_writeback = true` and a non-zero
//! `archive_backup_retain_days`, and removes the backups made more than
//! that many days ago. `0` means "keep forever".
//!
//! **What is a backup:** a regular file (never a link or a directory)
//! named `<archive>.bak` or `<archive>.bak.<1..=5>`, where `<archive>` is a
//! recognised comic-archive name
//! ([`crate::archive_rewrite::is_backup_name`]). A user's `notes.bak` or
//! `photo.jpg.bak` is never touched.
//!
//! **How old a backup is:** counted from when Folio made it, which is the
//! inode change time ([`crate::archive_rewrite::backup_made_at`]), not the
//! mtime. A backup is usually a hard link to the original archive and
//! carries that archive's old mtime, so by mtime a backup made minutes
//! ago could be deleted at the next sweep. When the time cannot be known
//! the file is kept.
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
use crate::library::no_follow::{self, Refusal};
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
    /// Backups kept because their age cannot be known
    /// ([`crate::archive_rewrite::backup_made_at`] is `None`: no ctime, or
    /// a ctime before an mtime set into the future). Counted only in a
    /// library that is pruned; the sweep warns once per library.
    pub kept_unknown_age: usize,
    /// Backups that were old enough when the walk looked at them but were
    /// not deleted because the name changed before the delete: it holds
    /// another file now (a rewrite rotated a fresh backup in, a user copied
    /// a file over it), the file's age no longer qualifies, or it is gone.
    pub skipped_changed: usize,
}

impl PruneStats {
    fn absorb(&mut self, other: PruneStats) {
        self.removed += other.removed;
        self.bytes += other.bytes;
        self.errors += other.errors;
        self.libraries += other.libraries;
        self.kept_unknown_age += other.kept_unknown_age;
        self.skipped_changed += other.skipped_changed;
    }
}

/// Run one sweep over every library. Prunes the eligible ones, stores a
/// fresh backup tally for all of them, and emits one `archive.removed`
/// library event per library that had at least one backup pruned.
pub async fn run(state: &AppState) -> anyhow::Result<PruneStats> {
    run_at(state, SystemTime::now()).await
}

/// [`run`] with the clock fixed at `now`, so tests can move time forward
/// instead of back-dating a timestamp nothing in user space can set.
pub async fn run_at(state: &AppState, now: SystemTime) -> anyhow::Result<PruneStats> {
    let libs = library::Entity::find().all(&state.db).await?;

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
        if stats.kept_unknown_age > 0 {
            tracing::warn!(
                library_id = %lib.id,
                kept_unknown_age = stats.kept_unknown_age,
                retain_days,
                "archive backup prune: backups kept because their age cannot be told \
                 (a modification time in the future, or no change time); they are never \
                 pruned until that changes",
            );
        }
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
            skipped_changed = stats.skipped_changed,
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

/// Walk `root` once: delete every backup
/// ([`backup_stats::for_each_backup`]) made more than `retain_days` days
/// before `now` ([`crate::archive_rewrite::backup_made_at`]; a backup whose
/// making time is unknown is kept), and tally the ones that stay.
/// `retain_days <= 0` deletes nothing (an empty [`PruneStats`] with
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
    prune_library_with(root, retain_days, now, |_| {})
}

/// [`prune_library`] with a hook that runs between the walk's decision to
/// delete a file and the delete, so tests can change the file in that
/// window.
pub(crate) fn prune_library_with(
    root: &Path,
    retain_days: i32,
    now: SystemTime,
    mut before_delete: impl FnMut(&Path),
) -> (PruneStats, Option<Tally>) {
    use std::os::unix::fs::MetadataExt;
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
        let Some(cutoff) = cutoff else {
            tally.add(meta);
            return;
        };
        let Some(made) = crate::archive_rewrite::backup_made_at(meta) else {
            stats.kept_unknown_age += 1;
            tally.add(meta);
            return;
        };
        if made >= cutoff {
            tally.add(meta);
            return;
        }
        before_delete(entry.path());
        match remove_if_still_expired(root, entry.path(), (meta.dev(), meta.ino()), cutoff) {
            Ok(Removal::Removed) => {
                stats.removed += 1;
                stats.bytes += meta.len();
                tracing::debug!(path = %entry.path().display(), "backup prune: removed");
            }
            Ok(Removal::Changed { regular_file_there }) => {
                stats.skipped_changed += 1;
                // Count what holds the name now. The tally is a display
                // value, so a plain `lstat` by path is enough here.
                if regular_file_there
                    && let Ok(m) = std::fs::symlink_metadata(entry.path())
                    && m.file_type().is_file()
                {
                    tally.add(&m);
                }
                tracing::debug!(
                    path = %entry.path().display(),
                    "backup prune: the file changed after the walk looked at it; kept",
                );
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

/// What [`remove_if_still_expired`] did.
#[derive(Debug, PartialEq, Eq)]
enum Removal {
    Removed,
    /// Not deleted: the name holds another file or no regular file, or the
    /// file's age no longer qualifies. `regular_file_there` says whether a
    /// regular file holds the name now (to tally).
    Changed {
        regular_file_there: bool,
    },
}

/// Delete the backup at `path` only if the name still holds the file the
/// walk decided on (`walked` = device and inode), still a regular file,
/// whose age still qualifies (made before `cutoff`). The name is looked at
/// again with a no-follow `stat` through the folder handle the no-follow
/// walk from the root opens (a folder on the way that has become a link is
/// refused), just before the unlink in that same handle. The file is never
/// opened: no read permission is needed (a mode-000 backup is deleted like
/// any other) and no file data is touched (an open can recall the whole
/// file on tiered storage). What is left is the window between that look
/// and the unlink itself, which no portable call closes.
fn remove_if_still_expired(
    root: &Path,
    path: &Path,
    walked: (u64, u64),
    cutoff: SystemTime,
) -> Result<Removal, Refusal> {
    let (dir, name) = no_follow::open_library_parent(root, path)?;
    let Some(st) = dir.stat(&name).map_err(Refusal::Io)? else {
        return Ok(Removal::Changed {
            regular_file_there: false,
        });
    };
    let regular = st.kind == no_follow::EntryKind::Regular;
    let still_expired =
        crate::archive_rewrite::backup_made_at_stat(&st).is_some_and(|made| made < cutoff);
    if !regular || st.identity != walked || !still_expired {
        return Ok(Removal::Changed {
            regular_file_there: regular,
        });
    }
    match dir.remove_file(&name) {
        Ok(()) => Ok(Removal::Removed),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Removal::Changed {
            regular_file_there: false,
        }),
        Err(e) => Err(Refusal::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::no_follow::LibraryRoot;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    const DAY: Duration = Duration::from_secs(86_400);

    fn write(path: &Path) {
        fs::write(path, b"x").unwrap();
    }

    /// Set `path`'s mtime to `age` before now. Its ctime stays "now":
    /// nothing in user space can move a ctime back, which is the point.
    fn backdate_mtime(path: &Path, age: Duration) {
        let t = filetime::FileTime::from_system_time(SystemTime::now() - age);
        filetime::set_file_mtime(path, t).unwrap();
    }

    /// Two moments around which files are made, far enough apart that the
    /// filesystem's (coarse) ctime tells them apart.
    fn pause() {
        std::thread::sleep(Duration::from_millis(60));
    }

    #[test]
    fn prunes_only_backups_made_before_the_cutoff_and_tallies_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("a.cbz.bak");
        let old_slot = dir.path().join("a.cbz.bak.2");
        let not_backup = dir.path().join("c.cbz");
        write(&old);
        write(&old_slot);
        write(&not_backup);
        backdate_mtime(&not_backup, 400 * DAY);
        pause();
        let mid = SystemTime::now();
        pause();
        let young = dir.path().join("b.cbz.bak");
        write(&young);

        // 29 days after `mid`: nothing is old enough yet.
        let (stats, _) = prune_library(dir.path(), 30, mid + 29 * DAY);
        assert_eq!(stats.removed, 0);

        let (stats, tally) = prune_library(dir.path(), 30, mid + 30 * DAY);
        let tally = tally.expect("readable root");
        assert_eq!(stats.removed, 2, "{stats:?}");
        assert_eq!(stats.errors, 0);
        assert!(!old.exists());
        assert!(!old_slot.exists());
        assert!(young.exists(), "made after the cutoff");
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
        let old = dir.path().join("a.cbz.bak");
        write(&old);
        let (stats, tally) = prune_library(dir.path(), 0, SystemTime::now() + 4_000 * DAY);
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

    /// ARC-BAK: only `<archive>.bak[.N]` regular files are Folio's. A user's
    /// own `.bak` files, a directory or a link with a backup's name are never
    /// deleted or counted, however old. `real.cbz.bak` is the control: the
    /// same sweep deletes it.
    #[test]
    fn files_folio_did_not_write_are_never_deleted_or_counted() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        let foreign = [
            "notes.bak",
            "photo.jpg.bak",
            "X.cbz.bak.txt",
            "X.cbz.bak.01",
            "X.cbz.bak.6",
            "X.cbz.bak.",
            ".cbz.bak",
            "X.zip.bak",
            "X.cbz.BAK",
        ];
        for name in foreign {
            write(&root.join(name));
            backdate_mtime(&root.join(name), 4_000 * DAY);
        }
        // A directory named like a backup, with a file inside that is not.
        fs::create_dir(root.join("something.cbz.bak")).unwrap();
        write(&root.join("something.cbz.bak").join("inner.txt"));
        fs::create_dir(root.join("something.bak")).unwrap();
        // A link named like a backup, pointing out of the library.
        let victim = outside.path().join("victim.cbz.bak");
        write(&victim);
        std::os::unix::fs::symlink(&victim, root.join("link.cbz.bak")).unwrap();
        // The control.
        let real = root.join("real.cbz.bak");
        write(&real);

        let (_, tally) = prune_library(root, 0, SystemTime::now());
        assert_eq!(tally.unwrap().file_count, 1, "only real.cbz.bak is counted");

        let (stats, tally) = prune_library(root, 30, SystemTime::now() + 10_000 * DAY);
        assert_eq!(stats.removed, 1, "{stats:?}");
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert!(!real.exists(), "the control is pruned");
        assert_eq!(tally.unwrap().file_count, 0);
        for name in foreign {
            assert!(root.join(name).exists(), "{name} must survive");
        }
        assert!(root.join("something.cbz.bak").join("inner.txt").exists());
        assert!(root.join("something.bak").is_dir());
        assert!(root.join("link.cbz.bak").symlink_metadata().is_ok());
        assert!(victim.exists(), "a link is never followed");
    }

    /// ARC-WRITE a: a rewrite hard-links the original into `.bak`, so the
    /// backup carries the ORIGINAL's mtime. A backup of a 400-day-old archive
    /// made just now must survive a 30-day retention until 30 days have
    /// really passed.
    #[test]
    fn hard_linked_backup_of_an_old_archive_ages_from_when_it_was_made() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        backdate_mtime(&target, 400 * DAY);
        let old_ino = fs::metadata(&target).unwrap().ino();
        pause();
        let before = SystemTime::now();
        pause();

        let root = LibraryRoot::resolve(dir.path());
        let outcome = crate::archive_rewrite::rewrite_atomic(&root, &target, 1, |io| {
            use std::io::Write;
            io.staging_file()?.write_all(b"v2")?;
            Ok(())
        })
        .unwrap();
        let bak = outcome.backup.expect("a backup was made");
        let meta = fs::symlink_metadata(&bak).unwrap();
        assert_eq!(meta.ino(), old_ino, "the backup is a hard link");
        assert!(
            meta.modified().unwrap() < SystemTime::now() - 399 * DAY,
            "the backup carries the original's old mtime"
        );

        // A sweep tomorrow keeps it.
        let (stats, _) = prune_library(dir.path(), 30, SystemTime::now() + DAY);
        assert_eq!(stats.removed, 0, "{stats:?}");
        assert!(
            bak.exists(),
            "a backup made today survives a 30-day retention"
        );
        // 30 days after a moment before the backup was made: still kept.
        let (stats, _) = prune_library(dir.path(), 30, before + 30 * DAY);
        assert_eq!(stats.removed, 0, "{stats:?}");
        assert!(bak.exists());

        // Once it really is older than the retention, it goes.
        let (stats, _) = prune_library(dir.path(), 30, SystemTime::now() + 31 * DAY);
        assert_eq!(stats.removed, 1, "{stats:?}");
        assert!(!bak.exists());
        assert_eq!(
            fs::read(&target).unwrap(),
            b"v2",
            "the archive is untouched"
        );
    }

    /// A backup whose making time cannot be known is kept. Here the mtime is
    /// set into the future, so the ctime (the moment of that change) is
    /// earlier than the mtime, which a filesystem that keeps ctime cannot
    /// produce. `known.cbz.bak` is the control.
    #[test]
    fn a_backup_of_unknown_age_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let unknown = dir.path().join("unknown.cbz.bak");
        let known = dir.path().join("known.cbz.bak");
        write(&unknown);
        write(&known);
        let future = filetime::FileTime::from_system_time(SystemTime::now() + 365 * DAY);
        filetime::set_file_mtime(&unknown, future).unwrap();
        assert_eq!(
            crate::archive_rewrite::backup_made_at(&fs::symlink_metadata(&unknown).unwrap()),
            None
        );

        let (stats, tally) = prune_library(dir.path(), 30, SystemTime::now() + 10_000 * DAY);
        assert_eq!(stats.removed, 1, "{stats:?}");
        assert_eq!(stats.kept_unknown_age, 1, "{stats:?}");
        assert!(!known.exists(), "the control is pruned");
        assert!(unknown.exists(), "unknown age: kept");
        let tally = tally.unwrap();
        assert_eq!(tally.file_count, 1, "kept and still counted");
        assert_eq!(
            (tally.oldest_modified, tally.newest_modified),
            (None, None),
            "no date to show for a backup of unknown age"
        );
        // Not counted as "kept" where nothing is pruned.
        let (stats, _) = prune_library(dir.path(), 0, SystemTime::now());
        assert_eq!(stats.kept_unknown_age, 0);
    }

    /// ARC-WRITE: the walk's look and the delete are two looks at the name.
    /// A file that took the name in between (a rewrite rotating a fresh
    /// backup in, a user copying over it) is not deleted, and neither is
    /// the same file once its age no longer qualifies. The control: the
    /// same sweep with a hook that changes nothing deletes the file.
    #[test]
    fn a_file_that_changes_between_the_walk_and_the_delete_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let bak = dir.path().join("a.cbz.bak");

        // Control.
        write(&bak);
        let mid = {
            pause();
            let t = SystemTime::now();
            pause();
            t
        };
        let (stats, _) = prune_library_with(dir.path(), 30, mid + 30 * DAY, |_| {});
        assert_eq!((stats.removed, stats.skipped_changed), (1, 0), "{stats:?}");
        assert!(!bak.exists());

        // Another file takes the name (a new inode; its own age would still
        // qualify against this cutoff, so only the identity check saves it).
        write(&bak);
        let later = SystemTime::now() + 31 * DAY;
        let (stats, tally) = prune_library_with(dir.path(), 30, later, |p| {
            fs::remove_file(p).unwrap();
            fs::write(p, b"someone else's bytes").unwrap();
        });
        assert_eq!((stats.removed, stats.skipped_changed), (0, 1), "{stats:?}");
        assert_eq!(fs::read(&bak).unwrap(), b"someone else's bytes");
        assert_eq!(tally.unwrap().file_count, 1, "what is there now is counted");

        // The same file, but changed so its age no longer qualifies.
        let mid = {
            pause();
            let t = SystemTime::now();
            pause();
            t
        };
        let (stats, _) = prune_library_with(dir.path(), 30, mid + 30 * DAY, |p| {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        });
        assert_eq!((stats.removed, stats.skipped_changed), (0, 1), "{stats:?}");
        assert!(bak.exists());

        // Gone in between: skipped, not an error.
        let (stats, _) = prune_library_with(dir.path(), 30, SystemTime::now() + 31 * DAY, |p| {
            fs::remove_file(p).unwrap();
        });
        assert_eq!(
            (stats.removed, stats.skipped_changed, stats.errors),
            (0, 1, 0),
            "{stats:?}"
        );

        // Replaced by a link (to a backup-named file outside): not a regular
        // file any more, so kept, and the link's target is untouched.
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("victim.cbz.bak");
        write(&target);
        write(&bak);
        let (stats, _) = prune_library_with(dir.path(), 30, SystemTime::now() + 31 * DAY, |p| {
            fs::remove_file(p).unwrap();
            std::os::unix::fs::symlink(&target, p).unwrap();
        });
        assert_eq!(
            (stats.removed, stats.skipped_changed, stats.errors),
            (0, 1, 0),
            "{stats:?}"
        );
        assert!(bak.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(target.exists());
    }

    /// The re-check before the delete is a `stat`, not an open: a backup
    /// Folio may not read (mode 000) is deleted when old, as before the
    /// re-check existed. Control: the same backup is kept while young.
    #[test]
    #[allow(
        clippy::print_stderr,
        reason = "a skipped test must say so loudly when it runs as root"
    )]
    fn an_unreadable_backup_is_still_pruned_when_old() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bak = dir.path().join("locked.cbz.bak");
        write(&bak);
        fs::set_permissions(&bak, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&bak).is_ok() {
            eprintln!(
                "SKIPPED an_unreadable_backup_is_still_pruned_when_old: running with \
                 permission to read a mode-000 file (root or CAP_DAC_OVERRIDE), so the \
                 unreadable case cannot be exercised"
            );
            return;
        }
        let (stats, _) = prune_library(dir.path(), 30, SystemTime::now() + DAY);
        assert_eq!((stats.removed, stats.errors), (0, 0), "{stats:?}");
        assert!(bak.symlink_metadata().is_ok(), "young: kept");

        let (stats, _) = prune_library(dir.path(), 30, SystemTime::now() + 31 * DAY);
        assert_eq!(
            (stats.removed, stats.errors, stats.skipped_changed),
            (1, 0, 0),
            "{stats:?}"
        );
        assert!(bak.symlink_metadata().is_err(), "old: deleted");
    }
}
