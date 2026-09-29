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
//! **`target` is never missing.** The backup slot is filled by
//! hard-linking (or, when the filesystem refuses links, copying) the
//! original into `<target>.bak` *while it still lives at `target`*, and
//! the staging file is then `rename(2)`d over it in one atomic replace.
//! A crash at any point leaves the old or the new bytes at `target`;
//! there is no window where only the `.bak` + a `.tmp` survive (WP-2.6
//! (a), audit OP-8). [`rewrite_atomic_with_faults`] exposes the step
//! boundaries so tests can prove it.
//!
//! ## Backup retention
//!
//! v1 keeps a single `.bak` per archive (overwritten on each rewrite).
//! Per-library `archive_backup_retain_count` controls how many older
//! slots (`.bak.1`, `.bak.2`, …) are also retained — capped at 5. The
//! daily backup sweep ([`crate::jobs::backup_prune`], 04:45 UTC) walks
//! each writeback-enabled library root for `.bak` / `.bak.N` files
//! (see [`is_backup_name`]) whose mtime is older than
//! `library.archive_backup_retain_days` and removes them; `0` keeps
//! backups forever.

pub mod mutex;

use archive::ArchiveError;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

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
    /// The freshly-written archive failed post-write validation (an
    /// expected entry went missing, a sidecar is absent, or the archive
    /// won't re-open). Returned from a `write_into` closure so
    /// [`rewrite_atomic`] aborts BEFORE swapping — the original stays
    /// intact even when `retain_count = 0`.
    #[error("rewrite validation failed: {0}")]
    ValidationFailed(String),
}

/// Atomically replace `target` with the result of `write_into(temp_path)`.
///
/// Steps:
///
///   1. Pick `<target>.<random>.tmp` as the staging path (same directory ⇒
///      rename is atomic on the same filesystem).
///   2. Caller writes the new bytes into the tmp path via the closure.
///   3. fsync the tmp file + the parent dir so the new bytes are durable
///      before we touch the backup slots.
///   4. If `target` exists and `retain_count > 0`, shift any existing
///      `.bak.N` siblings forward (`.bak` → `.bak.1`, `.bak.1` → `.bak.2`,
///      …) up to the retain cap, then **hard-link** `target` into the
///      `<target>.bak` slot (falling back to a byte copy when the
///      filesystem refuses hard links — some NAS / FAT / SMB mounts). The
///      original inode is now reachable from both names.
///   5. Rename `<target>.tmp` → `target`. `rename(2)` replaces the
///      destination atomically, so there is **no instant at which
///      `target` is missing** — a crash anywhere in this sequence leaves
///      either the old bytes or the new bytes at `target` (WP-2.6 (a),
///      audit OP-8). Pre-fix the order was `rename(target, .bak)` then
///      `rename(tmp, target)`, and a crash between the two left only the
///      `.bak` + a `.tmp` that [`startup_cleanup`] would later delete.
///   6. fsync the parent dir.
///
/// `retain_count` is capped at 5. Pass `1` for the common case (one
/// rollback slot). Pass `0` to skip `.bak` entirely — the original file
/// is overwritten in-place via the final rename.
///
/// **Validate inside `write_into`.** The closure writes the new bytes to
/// the temp path and may then re-open + validate them, returning
/// [`RewriteError::ValidationFailed`] on any problem. Because the swap
/// only happens *after* `write_into` returns `Ok`, a validation failure
/// leaves the original untouched — which is what makes `retain_count = 0`
/// safe (a corrupt rewrite never replaces a good original, even with no
/// `.bak` rollback slot).
pub fn rewrite_atomic<F>(
    target: &Path,
    retain_count: i32,
    write_into: F,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&Path) -> Result<(), RewriteError>,
{
    rewrite_atomic_with_faults(target, retain_count, write_into, |_| Ok(()))
}

/// Points between the steps of [`rewrite_atomic`] at which a crash can
/// happen. The fault hook receives each one in order; returning `Err`
/// simulates the process dying right there (nothing after the point
/// runs). Used by the crash-window tests to prove `target` is readable
/// at every point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteStep {
    /// New bytes written to the staging file and fsynced.
    AfterStage,
    /// Older `.bak.N` slots shifted forward (slot 0 is free).
    AfterRotate,
    /// `target` linked/copied into `<target>.bak`; `target` still holds
    /// the old bytes.
    AfterBackup,
    /// `<target>.tmp` renamed over `target`; the parent dir is not yet
    /// fsynced.
    AfterSwap,
}

/// [`rewrite_atomic`] with a fault-injection hook between every step.
/// Production callers use [`rewrite_atomic`] (no-op hook); this is
/// `pub` only so integration tests can simulate a crash mid-sequence.
pub fn rewrite_atomic_with_faults<F, H>(
    target: &Path,
    retain_count: i32,
    write_into: F,
    mut fault: H,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&Path) -> Result<(), RewriteError>,
    H: FnMut(RewriteStep) -> Result<(), RewriteError>,
{
    if !(0..=5).contains(&retain_count) {
        return Err(RewriteError::InvalidRetainCount(retain_count));
    }
    let parent = target
        .parent()
        .ok_or_else(|| RewriteError::NoParent(target.to_path_buf()))?;
    let tmp = temp_staging(target);
    claim_staging_path(&tmp)?;

    write_into(&tmp)?;
    fsync_file(&tmp)?;
    fsync_dir(parent)?;
    fault(RewriteStep::AfterStage)?;

    let backup = if retain_count > 0 && target.exists() {
        rotate_backups(target, retain_count)?;
        fault(RewriteStep::AfterRotate)?;
        let slot0 = backup_slot_path(target, 0);
        link_or_copy(target, &slot0)?;
        fault(RewriteStep::AfterBackup)?;
        Some(slot0)
    } else {
        None
    };
    // `rename(2)` replaces `target` atomically — the old inode stays
    // reachable through `.bak` (hard link) and the new one appears under
    // `target` in the same instant. If retain=0 and target exists, the
    // rename overwrites it in place.
    fs::rename(&tmp, target)?;
    fault(RewriteStep::AfterSwap)?;
    fsync_dir(parent)?;

    Ok(RewriteOutcome {
        target: target.to_path_buf(),
        backup,
    })
}

/// Make `dst` a second name for `src`'s bytes without ever removing
/// `src`: a hard link when the filesystem supports one (same inode,
/// zero extra bytes, instant), else a full byte copy (cross-device
/// layouts, NAS / SMB / FAT mounts, or filesystems that refuse links on
/// the file — `EXDEV`, `EPERM`, `ENOTSUP`, `EMLINK`). Either way `src`
/// is untouched, which is what keeps the target readable through the
/// whole swap. `dst` must not exist (the rotation freed the slot).
fn link_or_copy(src: &Path, dst: &Path) -> Result<(), RewriteError> {
    match fs::hard_link(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::debug!(
                src = %src.display(),
                dst = %dst.display(),
                error = %e,
                "archive_rewrite: hard_link refused; copying the backup instead",
            );
            // A failed link attempt can't leave a half-file, but be
            // defensive: clear anything at `dst` before copying so a
            // stale slot never masquerades as the backup.
            let _ = fs::remove_file(dst);
            fs::copy(src, dst)?;
            fsync_file(dst)?;
            Ok(())
        }
    }
}

/// A fresh, unpredictable staging path for `target`: `<target>.<random>.tmp`
/// in the same directory (so the final rename stays on one filesystem). The
/// random component means an attacker who can write to the media directory
/// can't pre-plant a symlink at the staging path to redirect the write
/// out of the library root (SEC-8). The `.tmp` suffix is preserved so
/// [`startup_cleanup`] still recognizes and reaps orphans.
fn temp_staging(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(".");
    s.push(Uuid::now_v7().simple().to_string());
    s.push(".tmp");
    PathBuf::from(s)
}

/// Atomically create the staging file with `O_EXCL` (`create_new`), so the
/// write fails closed if anything already exists at the path — a pre-planted
/// regular file or symlink can never be followed/truncated (SEC-8). The
/// randomized name from [`temp_staging`] makes a pre-plant infeasible in the
/// first place; this is the belt-and-suspenders that proves nothing was there.
fn claim_staging_path(tmp: &Path) -> Result<(), RewriteError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    Ok(())
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

/// Shift existing `.bak.{N-1..0}` slots forward by one so slot 0 is
/// free for the caller to fill. Slots past `retain_count - 1` are
/// dropped on the floor. Never touches `target` itself — the caller
/// links/copies it into slot 0 *without* removing it (see
/// [`rewrite_atomic`] step 4).
fn rotate_backups(target: &Path, retain_count: i32) -> Result<(), RewriteError> {
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
    Ok(())
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

/// Atomically convert one archive into another at a *different* path —
/// the CBR→CBZ edit path (`archive-rewrite-1.0` M6). Unlike
/// [`rewrite_atomic`], the original (`original`, e.g. `foo.cbr`) and the
/// destination (`dst`, e.g. `foo.cbz`) have different names:
///
///   1. Caller writes the new archive into `<dst>.tmp`.
///   2. fsync the tmp file + parent dir.
///   3. Rename `original` → `<original>.bak` (the `.cbr` is kept as the
///      single rollback slot; an existing `.bak` is overwritten).
///   4. Rename `<dst>.tmp` → `dst`.
///   5. fsync the parent dir.
///
/// On failure before step 3 the original is untouched (worst case an
/// orphan `<dst>.tmp`, reaped by [`startup_cleanup`]). Returns the `.bak`
/// path of the preserved original.
pub fn convert_atomic<F>(
    original: &Path,
    dst: &Path,
    write_into: F,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&Path) -> Result<(), RewriteError>,
{
    let parent = dst
        .parent()
        .ok_or_else(|| RewriteError::NoParent(dst.to_path_buf()))?;
    let tmp = temp_staging(dst);
    claim_staging_path(&tmp)?;

    write_into(&tmp)?;
    fsync_file(&tmp)?;
    fsync_dir(parent)?;

    let backup = backup_slot_path(original, 0);
    if original.exists() {
        // rename overwrites any prior .bak atomically.
        fs::rename(original, &backup)?;
    }
    fs::rename(&tmp, dst)?;
    fsync_dir(parent)?;

    Ok(RewriteOutcome {
        target: dst.to_path_buf(),
        backup: Some(backup),
    })
}

/// One backup-slot path for `target` — `<target>.bak` for slot 0,
/// `<target>.bak.N` for N >= 1. Exposed so callers (the backups-listing
/// endpoint, restore) can enumerate the retention slots without
/// re-deriving the naming scheme.
pub fn backup_slot(target: &Path, slot: i32) -> PathBuf {
    backup_slot_path(target, slot)
}

/// Whether `name` is a backup file this module produces: `<original>.bak`
/// (slot 0) or `<original>.bak.<n>` (slots 1+, numeric suffix only). The
/// inverse of [`backup_slot`] — shared by the backup-storage rollup card
/// and the retention sweep so both agree on what counts as a backup.
pub fn is_backup_name(name: &str) -> bool {
    if name.ends_with(".bak") {
        return true;
    }
    if let Some(idx) = name.rfind(".bak.") {
        let suffix = &name[idx + 5..];
        return !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// Restore the most recent backup (`<target>.bak`) over `target` — the
/// one-click undo of the last rewrite. The rename is atomic on the same
/// filesystem, so a crash can't leave `target` half-written. Returns
/// `Err(NotFound)` when no `.bak` exists. Higher slots (`.bak.1`, …) are
/// left untouched (older history stays available).
pub fn restore_latest_backup(target: &Path) -> Result<(), RewriteError> {
    let bak = backup_slot_path(target, 0);
    if !bak.exists() {
        return Err(RewriteError::Io(io::Error::new(
            ErrorKind::NotFound,
            "no .bak to restore",
        )));
    }
    let parent = target
        .parent()
        .ok_or_else(|| RewriteError::NoParent(target.to_path_buf()))?;
    fs::rename(&bak, target)?;
    fsync_dir(parent)?;
    Ok(())
}

/// Whether `dir` lives on a writable mount — the precondition for any
/// archive rewrite (page edits or sidecar writeback). Probed with
/// `statvfs(2)`: a read-only mount sets `ST_RDONLY` in `f_flag`. We fail
/// closed — a path that can't be stat'd (missing root, permission error)
/// reports `false` so the admin toggle / PATCH path refuses to enable
/// writeback against a library we can't write to.
///
/// Side-effect-free (no probe file), so it's safe to call on every admin
/// library list/detail render.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs(2) FFI: no safe std wrapper exists for the read-only mount flag"
)]
pub fn mount_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(cpath) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `cpath` is a valid NUL-terminated C string for the lifetime
    // of the call; `stat` is zero-initialized and only read after a `0`
    // (success) return from `statvfs`.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut stat) != 0 {
            return false;
        }
        (stat.f_flag & libc::ST_RDONLY) == 0
    }
}

/// Non-Unix fallback: we don't ship a Windows rewrite path in v1, so
/// optimistically report writable and let the rewrite itself surface any
/// failure.
#[cfg(not(unix))]
pub fn mount_writable(_dir: &Path) -> bool {
    true
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

        // Original untouched, no .bak created. Any staging orphan has a
        // randomized name (temp_staging) and is reaped by startup_cleanup; we
        // don't reconstruct it here.
        assert_eq!(fs::read(&target).unwrap(), b"v1-contents");
        let bak = target.with_extension("cbz.bak");
        assert!(!bak.exists());
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
    fn retain_zero_validation_failure_keeps_original() {
        // With retain_count = 0 (no .bak) the original is the ONLY copy.
        // A closure that writes the new bytes then rejects them (the
        // validate-before-swap contract) must leave the original intact —
        // proving retain=0 is safe against a corrupt rewrite.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"original-bytes").unwrap();

        let res = rewrite_atomic(&target, 0, |tmp| {
            fs::write(tmp, b"corrupt-rewrite")?;
            Err(RewriteError::ValidationFailed("synthetic".into()))
        });
        assert!(matches!(res, Err(RewriteError::ValidationFailed(_))));
        // Original survives byte-for-byte; no .bak was ever made.
        assert_eq!(fs::read(&target).unwrap(), b"original-bytes");
        assert!(!target.with_extension("cbz.bak").exists());
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

    /// WP-2.6 (a) / audit OP-8: simulate a crash at every step boundary
    /// of the swap and prove `target` is readable — holding either the
    /// old or the new bytes — at each one. The pre-fix order
    /// (`rename(target, .bak)` then `rename(tmp, target)`) had a window
    /// after the first rename where `target` did not exist at all.
    #[test]
    fn crash_at_every_step_leaves_target_readable() {
        for step in [
            RewriteStep::AfterStage,
            RewriteStep::AfterRotate,
            RewriteStep::AfterBackup,
            RewriteStep::AfterSwap,
        ] {
            let dir = TempDir::new().unwrap();
            let target = dir.path().join("issue.cbz");
            fs::write(&target, b"old-bytes").unwrap();
            // A pre-existing `.bak` so the rotation step has real work.
            fs::write(target.with_extension("cbz.bak"), b"older-bytes").unwrap();

            let res = rewrite_atomic_with_faults(
                &target,
                2,
                |tmp| Ok(fs::write(tmp, b"new-bytes")?),
                |at| {
                    if at == step {
                        Err(RewriteError::Io(io::Error::other(format!(
                            "crash at {at:?}"
                        ))))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(res.is_err(), "fault at {step:?} must surface");

            let bytes = fs::read(&target)
                .unwrap_or_else(|e| panic!("target missing after crash at {step:?}: {e}"));
            let expected: &[u8] = if step == RewriteStep::AfterSwap {
                b"new-bytes"
            } else {
                b"old-bytes"
            };
            assert_eq!(bytes, expected, "target bytes after crash at {step:?}");

            // Once the backup slot has been filled, the old bytes are also
            // reachable through `.bak` — a crash never strands the only copy.
            if matches!(step, RewriteStep::AfterBackup | RewriteStep::AfterSwap) {
                assert_eq!(
                    fs::read(target.with_extension("cbz.bak")).unwrap(),
                    b"old-bytes",
                    "slot 0 after crash at {step:?}",
                );
                assert_eq!(
                    fs::read(target.with_extension("cbz.bak.1")).unwrap(),
                    b"older-bytes",
                    "rotated slot 1 after crash at {step:?}",
                );
            }
        }
    }

    /// The backup slot is a hard link of the original (same inode) on
    /// filesystems that support it, so filling it never removes
    /// `target`. After the swap the two names point at different inodes.
    #[cfg(unix)]
    #[test]
    fn backup_slot_is_hard_linked_then_swapped() {
        use std::os::unix::fs::MetadataExt;
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let old_ino = fs::metadata(&target).unwrap().ino();

        let mut linked_ino = None;
        rewrite_atomic_with_faults(
            &target,
            1,
            |tmp| Ok(fs::write(tmp, b"v2")?),
            |at| {
                if at == RewriteStep::AfterBackup {
                    // Both names resolve to the original inode right now.
                    let bak = target.with_extension("cbz.bak");
                    linked_ino = Some(fs::metadata(&bak).unwrap().ino());
                    assert_eq!(fs::metadata(&target).unwrap().ino(), old_ino);
                }
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(linked_ino, Some(old_ino), "slot 0 was a hard link");
        assert_eq!(fs::read(&target).unwrap(), b"v2");
        assert_eq!(fs::read(target.with_extension("cbz.bak")).unwrap(), b"v1");
        assert_ne!(
            fs::metadata(&target).unwrap().ino(),
            old_ino,
            "rename replaced the target inode"
        );
    }

    /// When hard links are refused the slot is filled by a byte copy —
    /// same contract, slower. Simulated by pointing the copy at a
    /// directory-crossing layout isn't portable, so exercise the helper
    /// directly against a pre-existing stale slot.
    #[test]
    fn link_or_copy_falls_back_and_clears_stale_slot() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("issue.cbz");
        fs::write(&src, b"payload").unwrap();
        let dst = dir.path().join("issue.cbz.bak");
        // A stale slot makes `hard_link` fail with EEXIST → copy path.
        fs::write(&dst, b"stale").unwrap();
        link_or_copy(&src, &dst).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"payload", "source untouched");
        assert_eq!(fs::read(&dst).unwrap(), b"payload", "slot holds the copy");
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
    fn temp_staging_is_randomized_and_tmp_suffixed() {
        let target = Path::new("/lib/series/issue.cbz");
        let a = temp_staging(target);
        let b = temp_staging(target);
        // Distinct each call (random component) and still under the same dir
        // with a `.tmp` suffix so startup_cleanup reaps orphans.
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert!(p.to_str().unwrap().ends_with(".tmp"));
            assert_eq!(p.parent(), target.parent());
        }
    }

    #[test]
    fn claim_staging_path_fails_closed_on_existing_file() {
        let dir = TempDir::new().unwrap();
        // Fresh path: claim succeeds and creates the file.
        let fresh = dir.path().join("issue.cbz.fresh.tmp");
        claim_staging_path(&fresh).unwrap();
        assert!(fresh.exists());
        // Pre-existing path (stand-in for a pre-planted file/symlink): O_EXCL
        // makes the second claim fail rather than open/truncate it.
        assert!(claim_staging_path(&fresh).is_err());
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

    #[test]
    fn restore_latest_backup_reverts_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        rewrite_atomic(&target, 1, |tmp| Ok(fs::write(tmp, b"v2")?)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2");

        restore_latest_backup(&target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v1");
        // The .bak was consumed by the restore.
        assert!(!target.with_extension("cbz.bak").exists());
    }

    #[test]
    fn restore_latest_backup_errors_without_bak() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let err = restore_latest_backup(&target).unwrap_err();
        assert!(matches!(err, RewriteError::Io(_)));
    }

    #[test]
    fn convert_atomic_moves_original_to_bak_and_writes_dst() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("issue.cbr");
        let dst = dir.path().join("issue.cbz");
        fs::write(&original, b"rar-bytes").unwrap();

        let outcome = convert_atomic(&original, &dst, |tmp| {
            fs::write(tmp, b"zip-bytes")?;
            Ok(())
        })
        .unwrap();

        assert!(!original.exists(), "original .cbr moved away");
        assert_eq!(fs::read(&dst).unwrap(), b"zip-bytes");
        let bak = outcome.backup.unwrap();
        assert_eq!(fs::read(&bak).unwrap(), b"rar-bytes");
        assert_eq!(bak, dir.path().join("issue.cbr.bak"));
    }

    #[test]
    fn mount_writable_true_for_normal_dir() {
        let dir = TempDir::new().unwrap();
        assert!(mount_writable(dir.path()));
    }

    #[test]
    fn mount_writable_false_for_missing_path() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does/not/exist");
        assert!(!mount_writable(&missing));
    }
}
