//! Shared archive rewrite orchestration (M0 of
//! [`metadata-sidecar-writeback-1.0`](../../../../../.claude/plans/metadata-sidecar-writeback-1.0.md)).
//!
//! Two consumers, one foundation:
//!
//! - **Sidecar writeback** (`metadata-sidecar-writeback-1.0` M3+) — provider
//!   apply jobs swap `ComicInfo.xml` + `MetronInfo.xml` entries inside the
//!   archive without touching page bytes.
//! - **Page-byte edits** (`archive-rewrite-1.0` M2+) — operator-initiated
//!   `<PageEditor>` modal: remove / rotate / replace / reorder pages.
//!
//! Both go through [`rewrite_atomic`], which owns the temp→fsync→`.bak`→
//! rename dance. Both compete for the same per-issue Redis mutex (see
//! [`mutex`]). Boot-time cleanup of orphan `.tmp` files lives in
//! [`startup_cleanup`].
//!
//! ## Atomic-swap contract
//!
//! On success, the original file at `target` is preserved as
//! `<target>.bak` and the new file lives at `target`. On failure mid-way
//! (write error, cap exceeded, fsync error, rename error), the original
//! file is *never* mutated — the worst case is an orphan `.tmp` sibling,
//! which [`startup_cleanup`] will reap on the next boot.
//!
//! ## Backup retention
//!
//! v1 keeps a single `.bak` per archive (overwritten on each rewrite).
//! Per-library `archive_backup_retain_count` controls how many older
//! slots (`.bak.1`, `.bak.2`, …) are also retained — capped at 5. The
//! daily prune cron (sister plan M8) walks `.bak*` files older than
//! `library.archive_backup_retain_days` and removes them.

pub mod mutex;

use archive::ArchiveError;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Result returned by [`rewrite_atomic`].
#[derive(Debug, Clone)]
pub struct RewriteOutcome {
    /// Path of the archive that was rewritten. Same as the `target`
    /// argument — returned here so callers don't have to retain it.
    pub target: PathBuf,
    /// Path of the `.bak` left in place. None when the caller asked for
    /// `retain_count = 0` or when the original file didn't exist (the
    /// "additions" path — sidecar adding ComicInfo.xml to an archive
    /// that never had one, etc.; the orchestrator still goes through
    /// the same atomic-rename so failure semantics are uniform).
    pub backup: Option<PathBuf>,
}

/// Errors specific to the rewrite orchestrator. Most paths fold into
/// `Io`; `ArchiveErr` surfaces the underlying writer failure (cap
/// exceeded, malformed zip, etc.) without losing detail.
#[derive(Debug, thiserror::Error)]
pub enum RewriteError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("archive writer: {0}")]
    ArchiveErr(#[from] ArchiveError),
    #[error("retain_count {0} out of allowed range 0..=5")]
    InvalidRetainCount(i32),
    #[error("target path has no parent directory: {0}")]
    NoParent(PathBuf),
}

/// Atomically replace `target` with the result of `write_into(temp_path)`.
///
/// Steps:
///
///   1. Pick `<target>.tmp` as the staging path (same directory ⇒ rename
///      is atomic on the same filesystem).
///   2. Caller writes the new bytes into the tmp path via the closure.
///   3. fsync the tmp file + the parent dir so the new bytes are durable
///      before we rotate the `.bak`.
///   4. If `target` exists and `retain_count > 0`, shift any existing
///      `.bak.N` siblings forward (`.bak` → `.bak.1`, `.bak.1` → `.bak.2`,
///      …) up to the retain cap, then rename `target` → `<target>.bak`.
///   5. Rename `<target>.tmp` → `target`.
///   6. fsync the parent dir.
///
/// `retain_count` is capped at 5. Pass `1` for the common case (one
/// rollback slot). Pass `0` to skip `.bak` entirely (the original file
/// is overwritten in-place via rename, no rollback possible).
pub fn rewrite_atomic<F>(
    target: &Path,
    retain_count: i32,
    write_into: F,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&Path) -> Result<(), RewriteError>,
{
    if !(0..=5).contains(&retain_count) {
        return Err(RewriteError::InvalidRetainCount(retain_count));
    }
    let parent = target
        .parent()
        .ok_or_else(|| RewriteError::NoParent(target.to_path_buf()))?;
    let tmp = temp_sibling(target);

    // Always remove any orphan tmp before writing; defensive against the
    // crash-mid-rename window (boot cleanup is the canonical reaper but
    // doing it here too costs nothing).
    let _ = fs::remove_file(&tmp);

    write_into(&tmp)?;
    fsync_file(&tmp)?;
    fsync_dir(parent)?;

    let backup = if retain_count > 0 && target.exists() {
        Some(rotate_backups(target, retain_count)?)
    } else {
        None
    };
    // If retain=0 and target exists, rename will overwrite atomically.
    fs::rename(&tmp, target)?;
    fsync_dir(parent)?;

    Ok(RewriteOutcome {
        target: target.to_path_buf(),
        backup,
    })
}

/// `<target>.tmp` in the same directory as `target`. The shared suffix
/// is intentional so [`startup_cleanup`] can identify and remove orphans.
pub fn temp_sibling(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Backup-slot name for `target`: `<target>.bak` for slot 0,
/// `<target>.bak.N` for slot N >= 1.
fn backup_slot_path(target: &Path, slot: i32) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    if slot == 0 {
        s.push(".bak");
    } else {
        s.push(format!(".bak.{slot}"));
    }
    PathBuf::from(s)
}

/// Shift existing `.bak.{N-1..0}` slots forward by one, then move
/// `target` into slot 0. Slots past `retain_count - 1` are dropped on
/// the floor. Returns the path of the freshly-written slot 0.
fn rotate_backups(target: &Path, retain_count: i32) -> Result<PathBuf, RewriteError> {
    // Walk high→low so we never clobber a higher-numbered slot that
    // hasn't been moved yet.
    for slot in (0..retain_count).rev() {
        let from = backup_slot_path(target, slot);
        let to = backup_slot_path(target, slot + 1);
        if from.exists() {
            if slot + 1 >= retain_count {
                // This slot would fall off the end of the retention
                // window — remove the source rather than rename. (We
                // never write a slot >= retain_count.)
                fs::remove_file(&from)?;
            } else {
                fs::rename(&from, &to)?;
            }
        }
    }
    let slot0 = backup_slot_path(target, 0);
    fs::rename(target, &slot0)?;
    Ok(slot0)
}

fn fsync_file(path: &Path) -> Result<(), RewriteError> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn fsync_dir(path: &Path) -> Result<(), RewriteError> {
    let f = fs::OpenOptions::new().read(true).open(path)?;
    // sync_all on a directory handle issues a fdatasync(dirfd) on Linux,
    // which is what makes the just-completed rename durable. macOS
    // tolerates it (no-op on some FSes) — acceptable for v1 since we
    // target Linux-first deploys.
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn fsync_dir(_path: &Path) -> Result<(), RewriteError> {
    // Windows doesn't expose a directory-fsync primitive; the rename
    // already follows an fsync of the destination file, which is the
    // best durability story we can offer without raw NT APIs. Defer to
    // the platform's rename atomicity.
    Ok(())
}

/// Walk every library root and remove `.tmp` siblings older than `ttl`
/// — leftovers from a crashed rewrite. Called once at server boot from
/// [`crate::state::AppState::new`] (M0.5). Safe to call repeatedly; no-op
/// when no orphans are present.
///
/// Returns the count of removed files (mostly for log scraping).
pub fn startup_cleanup(roots: impl IntoIterator<Item = PathBuf>, ttl: std::time::Duration) -> u64 {
    let cutoff = SystemTime::now()
        .checked_sub(ttl)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0u64;
    for root in roots {
        removed += walk_and_remove(&root, cutoff);
    }
    removed
}

fn walk_and_remove(dir: &Path, cutoff: SystemTime) -> u64 {
    let mut removed = 0u64;
    let entries = match fs::read_dir(dir) {
        Ok(d) => d,
        Err(e) => {
            if e.kind() != ErrorKind::NotFound {
                tracing::warn!(path = %dir.display(), error = %e, "archive_rewrite startup: read_dir failed");
            }
            return 0;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            removed += walk_and_remove(&path, cutoff);
            continue;
        }
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|s| s.ends_with(".tmp"))
        {
            continue;
        }
        let Ok(mtime) = meta.modified() else { continue };
        if mtime > cutoff {
            // Younger than the cutoff — could be a live rewrite in
            // progress on another worker. Skip.
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                removed += 1;
                tracing::info!(path = %path.display(), "archive_rewrite startup: removed orphan .tmp");
            }
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "archive_rewrite startup: failed to remove orphan .tmp",
            ),
        }
    }
    removed
}

// ───────── tests ─────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn atomic_swap_preserves_bak() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1-contents").unwrap();

        let outcome = rewrite_atomic(&target, 1, |tmp| {
            fs::write(tmp, b"v2-contents")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"v2-contents");
        let bak = outcome.backup.unwrap();
        assert_eq!(fs::read(bak).unwrap(), b"v1-contents");
    }

    #[test]
    fn write_failure_leaves_original_untouched() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1-contents").unwrap();

        let res = rewrite_atomic(&target, 1, |_tmp| {
            Err(RewriteError::Io(io::Error::other("synthetic")))
        });
        assert!(res.is_err());

        // Original untouched, no .bak created, tmp cleaned up.
        assert_eq!(fs::read(&target).unwrap(), b"v1-contents");
        let bak = target.with_extension("cbz.bak");
        assert!(!bak.exists());
        let tmp = temp_sibling(&target);
        // Tmp may exist if the closure wrote partial bytes before
        // returning — that's the orphan boot-cleanup will reap.
        let _ = tmp;
    }

    #[test]
    fn retain_count_zero_skips_bak() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();

        let outcome = rewrite_atomic(&target, 0, |tmp| {
            fs::write(tmp, b"v2")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"v2");
        assert!(outcome.backup.is_none());
        let bak = target.with_extension("cbz.bak");
        assert!(!bak.exists());
    }

    #[test]
    fn retain_count_three_keeps_history() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        rewrite_atomic(&target, 3, |tmp| Ok(fs::write(tmp, b"v2")?)).unwrap();
        rewrite_atomic(&target, 3, |tmp| Ok(fs::write(tmp, b"v3")?)).unwrap();
        rewrite_atomic(&target, 3, |tmp| Ok(fs::write(tmp, b"v4")?)).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"v4");
        let bak0 = target.with_extension("cbz.bak");
        let bak1 = target.with_extension("cbz.bak.1");
        let bak2 = target.with_extension("cbz.bak.2");
        assert_eq!(fs::read(bak0).unwrap(), b"v3");
        assert_eq!(fs::read(bak1).unwrap(), b"v2");
        assert_eq!(fs::read(bak2).unwrap(), b"v1");
    }

    #[test]
    fn invalid_retain_count_rejected() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let res = rewrite_atomic(&target, 6, |_| Ok(()));
        assert!(matches!(res, Err(RewriteError::InvalidRetainCount(6))));
    }

    #[test]
    fn startup_cleanup_removes_old_tmp() {
        let dir = TempDir::new().unwrap();
        let stale = dir.path().join("issue.cbz.tmp");
        let mut f = fs::File::create(&stale).unwrap();
        f.write_all(b"partial").unwrap();
        drop(f);
        // Backdate mtime well past the cutoff via filetime crate? Avoid
        // pulling in another dep — sleep + tiny ttl is fine in test.
        std::thread::sleep(std::time::Duration::from_millis(50));

        let removed = startup_cleanup(
            [dir.path().to_path_buf()],
            std::time::Duration::from_millis(10),
        );
        assert_eq!(removed, 1);
        assert!(!stale.exists());
    }

    #[test]
    fn startup_cleanup_skips_recent_tmp() {
        let dir = TempDir::new().unwrap();
        let recent = dir.path().join("issue.cbz.tmp");
        fs::write(&recent, b"in-flight").unwrap();

        // TTL much longer than file age — should be skipped.
        let removed = startup_cleanup(
            [dir.path().to_path_buf()],
            std::time::Duration::from_secs(3600),
        );
        assert_eq!(removed, 0);
        assert!(recent.exists());
    }

    #[test]
    fn startup_cleanup_recurses_subdirs() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("series");
        fs::create_dir(&sub).unwrap();
        let stale = sub.join("issue.cbz.tmp");
        fs::write(&stale, b"partial").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        let removed = startup_cleanup(
            [dir.path().to_path_buf()],
            std::time::Duration::from_millis(10),
        );
        assert_eq!(removed, 1);
        assert!(!stale.exists());
    }
}
