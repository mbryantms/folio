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
//! rename dance. Every step of it works relative to the archive's folder,
//! opened once through the no-follow walk from the library root
//! ([`crate::library::no_follow`]): the original is read through the
//! handle opened there, the staging file is created exclusively without
//! following a link, and the link, renames and fsyncs name entries in that
//! folder handle. A link at the archive (or on its folder path) is refused
//! ([`RewriteError::Refused`]). Both compete for the same per-issue Redis mutex (see
//! [`mutex`]). Boot-time cleanup of orphan staging files
//! (`<name>.<32 hex>.tmp`, see [`is_staging_name`]) lives in
//! [`startup_cleanup`].
//!
//! ## Atomic-swap contract
//!
//! On success, the original file at `target` is preserved as
//! `<target>.bak` and the new file lives at `target`. On failure mid-way
//! (write error, cap exceeded, fsync error, rename error), the original
//! file is *never* mutated, and the staging file is removed again (a
//! guard that also runs when a panic unwinds); only a crash that kills
//! the process leaves an orphan `.tmp` sibling, which [`startup_cleanup`]
//! reaps on the next boot. Once the new file is in place, a failed fsync
//! of the folder is logged, not returned (see [`RewriteStep::FinalSync`]).
//!
//! Conversions ([`convert_atomic`], [`rename_into_place`]) never replace
//! a file at their destination name ([`RewriteError::DestinationExists`],
//! a no-replace rename), and leave the original at its old name until the
//! caller has repointed its row ([`Converted`]).
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
//! each writeback-enabled library root for Folio's backups (regular files
//! named `<archive>.bak` / `<archive>.bak.N`, see [`is_backup_name`])
//! made more than `library.archive_backup_retain_days` ago (by the inode
//! change time, see [`backup_made_at`]) and removes them; `0` keeps
//! backups forever.

pub mod mutex;

use crate::library::no_follow::{EntryKind, LibraryDir, LibraryRoot, Refusal};
use archive::{ArchiveError, ArchiveFile};
use std::ffi::{OsStr, OsString};
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
    /// The message names the file only; [`RewriteError::no_parent`] logs
    /// the full path.
    #[error("target path has no parent directory: {}", file_label(.0))]
    NoParent(PathBuf),
    /// The freshly-written archive failed post-write validation (an
    /// expected entry went missing, a sidecar is absent, or the archive
    /// won't re-open). Returned from a `write_into` closure so
    /// [`rewrite_atomic`] aborts BEFORE swapping — the original stays
    /// intact even when `retain_count = 0`.
    #[error("rewrite validation failed: {0}")]
    ValidationFailed(String),
    /// The archive, a folder on its path, or a name the rewrite would
    /// write is a symbolic link, is not a regular file, or lies outside
    /// the library root (`library::no_follow`). Nothing was written. A
    /// refusal, not a fault: retrying gives the same answer.
    #[error("refused: {0}")]
    Refused(Refusal),
    /// A conversion's destination name is already taken (another archive,
    /// often another issue's file). Nothing was written over it. A refusal:
    /// retrying gives the same answer until someone resolves the clash.
    ///
    /// The message names the file only (it sits beside the original);
    /// [`RewriteError::destination_exists`] logs the full path.
    #[error(
        "{} already exists; nothing was written. If Folio wrote that file during a \
         conversion that did not finish, remove that file and retry: the original \
         and its .bak still hold the original bytes. Otherwise another file has \
         this name.",
        file_label(.0)
    )]
    DestinationExists(PathBuf),
}

/// The file name of `p`, for error text. An error message can end up
/// stored on a run or shown to a user, so it names the file and never
/// the directory it lives in; the full path goes to the log.
pub(crate) fn file_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(no file name)".to_owned())
}

impl RewriteError {
    /// True for [`RewriteError::Refused`] and
    /// [`RewriteError::DestinationExists`].
    pub fn is_refusal(&self) -> bool {
        matches!(self, Self::Refused(_) | Self::DestinationExists(_))
    }

    /// [`RewriteError::NoParent`] for `target`, logging the full path.
    fn no_parent(target: &Path) -> Self {
        tracing::warn!(path = %target.display(), "archive rewrite: target has no usable parent directory");
        Self::NoParent(target.to_path_buf())
    }

    /// [`RewriteError::DestinationExists`] for `dst`, logging the full
    /// path the message leaves out.
    fn destination_exists(dst: &Path) -> Self {
        tracing::warn!(path = %dst.display(), "archive conversion: destination already exists");
        Self::DestinationExists(dst.to_path_buf())
    }
}

impl From<Refusal> for RewriteError {
    /// A missing file or folder and plain I/O errors stay I/O errors (they
    /// may be transient); a link, a non-regular file or a path outside the
    /// root is a refusal.
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::Missing => Self::Io(io::Error::new(ErrorKind::NotFound, "file is missing")),
            Refusal::Io(e) => Self::Io(e),
            Refusal::Lookup(e) => Self::Io(io::Error::other(format!("library lookup failed: {e}"))),
            other => Self::Refused(other),
        }
    }
}

/// What a `write_into` closure works with: the original archive (opened
/// once, through the no-follow walk, before the closure runs) and the
/// staging file (created exclusively in the archive's folder). Both are
/// open handles; the closure never opens a path.
pub struct RewriteIo<'a> {
    source: Option<&'a ArchiveFile>,
    staging: &'a fs::File,
    staging_path: &'a Path,
}

impl RewriteIo<'_> {
    /// The original archive. An I/O `NotFound` error when the target did
    /// not exist (the additions path).
    pub fn source(&self) -> Result<ArchiveFile, RewriteError> {
        self.source.cloned().ok_or_else(|| {
            RewriteError::Io(io::Error::new(
                ErrorKind::NotFound,
                "archive to rewrite is missing",
            ))
        })
    }

    /// The staging file, for a writer to write into (a second handle to
    /// the same open file; it starts empty).
    pub fn staging_file(&self) -> Result<fs::File, RewriteError> {
        Ok(self.staging.try_clone()?)
    }

    /// The staging file as an archive to re-open for validation, read
    /// through its handle.
    pub fn staged_archive(&self) -> Result<ArchiveFile, RewriteError> {
        Ok(ArchiveFile::new(
            self.staging.try_clone()?,
            self.staging_path,
        ))
    }

    /// The staging file's path, for messages only.
    pub fn staging_path(&self) -> &Path {
        self.staging_path
    }
}

/// Atomically replace `target` (a file below `root`) with what `write_into`
/// writes into a staging file.
///
/// Everything happens relative to the target's folder, opened once through
/// the no-follow walk from the library root (`library::no_follow`): no
/// folder on the way may be a symbolic link, and every step below names an
/// entry relative to that folder handle (`openat`, `renameat`, `linkat`,
/// `unlinkat`), so the folder cannot be swapped for a link mid-way.
///
/// Steps:
///
///   1. Open the target through the folder handle: no link followed, a
///      regular file only (a link or anything else is refused,
///      [`RewriteError::Refused`]); a missing target is the additions path.
///   2. Create `<target>.<random>.tmp` in the same folder (same directory ⇒
///      rename is atomic on the same filesystem) with `O_CREAT | O_EXCL |
///      O_NOFOLLOW`, so anything already at that name, a link included,
///      fails the rewrite instead of being followed.
///   3. The caller reads the original through its handle and writes the new
///      bytes into the staging handle ([`RewriteIo`]).
///   4. fsync the staging file (through its handle) + the folder, so the new
///      bytes are durable before we touch the backup slots.
///   5. Confirm the target name still holds the file that was read (same
///      device and inode, not a link); otherwise stop with the original
///      untouched. If the target exists and `retain_count > 0`, shift any
///      existing `.bak.N` siblings forward (`.bak` → `.bak.1`, `.bak.1` →
///      `.bak.2`, …) up to the retain cap, then **hard-link** `target`
///      into the `<target>.bak` slot (falling back to a byte copy from the
///      original's handle when the filesystem refuses hard links — some
///      NAS / FAT / SMB mounts). The original inode is now reachable from
///      both names.
///   6. Confirm the target again, then rename `<target>.tmp` → `target`.
///      `rename(2)` replaces the destination atomically, so there is **no
///      instant at which `target` is missing** — a crash anywhere in this
///      sequence leaves either the old bytes or the new bytes at `target`
///      (WP-2.6 (a), audit OP-8). Pre-fix the order was
///      `rename(target, .bak)` then `rename(tmp, target)`, and a crash
///      between the two left only the `.bak` + a `.tmp` that
///      [`startup_cleanup`] would later delete.
///   7. fsync the folder.
///
/// `retain_count` is capped at 5. Pass `1` for the common case (one
/// rollback slot). Pass `0` to skip `.bak` entirely — the original file
/// is overwritten in-place via the final rename.
///
/// **Validate inside `write_into`.** The closure writes the new bytes to
/// the staging file and may then re-open + validate them
/// ([`RewriteIo::staged_archive`]), returning
/// [`RewriteError::ValidationFailed`] on any problem. Because the swap
/// only happens *after* `write_into` returns `Ok`, a validation failure
/// leaves the original untouched — which is what makes `retain_count = 0`
/// safe (a corrupt rewrite never replaces a good original, even with no
/// `.bak` rollback slot).
pub fn rewrite_atomic<F>(
    root: &LibraryRoot,
    target: &Path,
    retain_count: i32,
    write_into: F,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&RewriteIo<'_>) -> Result<(), RewriteError>,
{
    rewrite_atomic_with_faults(root, target, retain_count, write_into, |_| Ok(()))
}

/// Points between the steps of [`rewrite_atomic`] at which a crash can
/// happen. The fault hook receives each one in order; returning `Err`
/// simulates the process dying right there (nothing after the point
/// runs). Used by the crash-window tests to prove `target` is readable
/// at every point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteStep {
    /// New bytes written to the staging file and fsynced.
    Staged,
    /// Older `.bak.N` slots shifted forward (slot 0 is free).
    Rotated,
    /// `target` linked/copied into `<target>.bak`; `target` still holds
    /// the old bytes.
    BackedUp,
    /// `<target>.tmp` renamed over `target` (for a conversion: renamed to
    /// the new name); the parent dir is not yet fsynced.
    Swapped,
    /// The folder fsync after the swap. Unlike the other points, an `Err`
    /// from the hook here stands in for that fsync *failing* (the process
    /// lives on): it is logged and the call still returns `Ok`.
    FinalSync,
}

/// The target's folder and name, opened through the no-follow walk.
fn open_target(root: &LibraryRoot, target: &Path) -> Result<(LibraryDir, OsString), RewriteError> {
    if target.file_name().is_none() {
        return Err(RewriteError::no_parent(target));
    }
    Ok(root.open_parent(target)?)
}

/// Open `name` in `dir` as the archive being replaced: a regular file
/// (`Some`), or missing (`None`); a link or anything else is refused.
fn open_original(
    dir: &LibraryDir,
    name: &OsStr,
    shown: &Path,
) -> Result<Option<ArchiveFile>, RewriteError> {
    match dir.kind(name)? {
        EntryKind::Missing => Ok(None),
        EntryKind::Symlink => Err(RewriteError::Refused(Refusal::Symlink)),
        EntryKind::Other => Err(RewriteError::Refused(Refusal::NotRegular)),
        EntryKind::Regular => Ok(Some(ArchiveFile::new(dir.open_regular(name)?, shown))),
    }
}

/// The name must still hold `original` (same device and inode, not a
/// link), or still be missing when there was none. A change means another
/// writer got there first: the rewrite stops with nothing replaced.
fn confirm_unchanged(
    dir: &LibraryDir,
    name: &OsStr,
    original: Option<&ArchiveFile>,
) -> Result<(), RewriteError> {
    let kind = dir.kind(name)?;
    match (original, kind) {
        (_, EntryKind::Symlink) => Err(RewriteError::Refused(Refusal::Symlink)),
        (None, EntryKind::Missing) => Ok(()),
        (Some(f), EntryKind::Regular) if dir.identity(name)? == f.identity()? => Ok(()),
        _ => Err(RewriteError::Io(io::Error::other(
            "the archive changed on disk during the rewrite; nothing was replaced",
        ))),
    }
}

/// [`rewrite_atomic`] with a fault-injection hook between every step.
/// Production callers use [`rewrite_atomic`] (no-op hook); this is
/// `pub` only so integration tests can simulate a crash mid-sequence.
pub fn rewrite_atomic_with_faults<F, H>(
    root: &LibraryRoot,
    target: &Path,
    retain_count: i32,
    write_into: F,
    mut fault: H,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&RewriteIo<'_>) -> Result<(), RewriteError>,
    H: FnMut(RewriteStep) -> Result<(), RewriteError>,
{
    if !(0..=5).contains(&retain_count) {
        return Err(RewriteError::InvalidRetainCount(retain_count));
    }
    let (dir, name) = open_target(root, target)?;
    let original = open_original(&dir, &name, target)?;
    let tmp_name = staging_name(&name);
    let (staging, mut guard) = Staged::create(&dir, &tmp_name)?;
    let staging_path = dir.display(&tmp_name);

    write_into(&RewriteIo {
        source: original.as_ref(),
        staging: &staging,
        staging_path: &staging_path,
    })?;
    staging.sync_all()?;
    let written_at = written_at(&staging);
    drop(staging);
    dir.sync()?;
    fault(RewriteStep::Staged)?;

    confirm_unchanged(&dir, &name, original.as_ref())?;
    let backup = match (&original, retain_count > 0) {
        (Some(original), true) => {
            rotate_backups(&dir, &name, retain_count)?;
            fault(RewriteStep::Rotated)?;
            let slot0 = backup_slot_name(&name, 0);
            link_or_copy(&dir, &name, &slot0, original, written_at)?;
            fault(RewriteStep::BackedUp)?;
            Some(dir.display(&slot0))
        }
        _ => None,
    };
    // `rename(2)` replaces `target` atomically — the old inode stays
    // reachable through `.bak` (hard link) and the new one appears under
    // `target` in the same instant. If retain=0 and target exists, the
    // rename overwrites it in place.
    confirm_unchanged(&dir, &name, original.as_ref())?;
    dir.rename(&tmp_name, &name)?;
    guard.disarm();
    fault(RewriteStep::Swapped)?;
    sync_after_swap(&dir, &name, &mut fault);

    Ok(RewriteOutcome {
        target: target.to_path_buf(),
        backup,
    })
}

/// fsync the folder after the new file took its name. The work is done at
/// that point: the new file is in place and the original sits in `.bak`.
/// A failure is logged at ERROR and NOT returned, because the caller's
/// answer to an error is to retry, and a retry would rotate the backup
/// slots again, which with one slot deletes `.bak` (the only copy of the
/// original) and links the already-rewritten file in its place.
fn sync_after_swap<H>(dir: &LibraryDir, name: &OsStr, fault: &mut H)
where
    H: FnMut(RewriteStep) -> Result<(), RewriteError>,
{
    let synced = fault(RewriteStep::FinalSync).and_then(|()| Ok(dir.sync()?));
    if let Err(e) = synced {
        tracing::error!(
            path = %dir.display(name).display(),
            error = %e,
            "archive_rewrite: the new file is in place but syncing its folder failed; \
             not retried, so the backup is kept",
        );
    }
}

/// A file this module created in a library folder (a staging file, or a
/// backup copy being filled) that must not outlive a failed rewrite. Until
/// [`Staged::disarm`], dropping it (an early `?` return, or a panic
/// unwinding through `spawn_blocking`) removes the name again, and only if
/// it still names the file that was created (same device and inode, not a
/// link): something else that took the name since is left alone. A crash
/// that skips the drop leaves the file for [`startup_cleanup`].
struct Staged<'d> {
    dir: &'d LibraryDir,
    name: OsString,
    identity: Option<(u64, u64)>,
}

impl<'d> Staged<'d> {
    /// Create `name` exclusively in `dir` and guard it.
    fn create(dir: &'d LibraryDir, name: &OsStr) -> Result<(fs::File, Self), RewriteError> {
        let file = dir.create_new(name)?;
        let identity = file_identity(&file)?;
        Ok((
            file,
            Self {
                dir,
                name: name.to_os_string(),
                identity: Some(identity),
            },
        ))
    }

    /// The name now holds the finished file (renamed into place, or a
    /// complete backup): keep it.
    fn disarm(&mut self) {
        self.identity = None;
    }
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        let Some(identity) = self.identity else {
            return;
        };
        let still_ours = self.dir.kind(&self.name).ok() == Some(EntryKind::Regular)
            && self.dir.identity(&self.name).ok() == Some(identity);
        if !still_ours {
            return;
        }
        if let Err(e) = self.dir.remove_file(&self.name) {
            tracing::warn!(
                path = %self.dir.display(&self.name).display(),
                error = %e,
                "archive_rewrite: could not remove a leftover file after a failed rewrite",
            );
        }
    }
}

/// `(device, inode)` of an open file.
fn file_identity(f: &fs::File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = f.metadata()?;
    Ok((m.dev(), m.ino()))
}

/// When the staging file's bytes were last written, by the filesystem's
/// own clock: the reference [`link_or_copy`] checks a fresh link's age
/// against. `None` when the filesystem reports no mtime.
fn written_at(staging: &fs::File) -> Option<SystemTime> {
    staging.metadata().ok().and_then(|m| m.modified().ok())
}

/// How far a fresh hard link's [`backup_made_at`] may lie before the
/// reference time and still count as "made now". Covers timestamp
/// granularity (whole seconds on sshfs and some NAS filesystems, two on
/// FAT), not clock skew: the reference is the staging file's mtime, read
/// from the same filesystem clock as the link's ctime.
const LINK_AGE_SLACK: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether a hard link whose [`backup_made_at`] reads `made_at` will age
/// from the moment it was made, given `reference`: a time no later than the
/// link, by the same clock. A filesystem that does not set ctime on
/// `link(2)` (sshfs: SFTP v3 has no ctime and reports the mtime, so the
/// link reads as old as the original archive) or whose ctime cannot be
/// read fails, and so does a link whose age is unknown (an original with an
/// mtime in the future): the caller then copies, and a copy is a new file
/// whose times are now.
fn link_ages_from_now(made_at: Option<SystemTime>, reference: SystemTime) -> bool {
    made_at.is_some_and(|m| m + LINK_AGE_SLACK >= reference)
}

/// Make `dst` a second name for the original at `src` (both in `dir`)
/// without ever removing `src`: a hard link when the filesystem supports
/// one (same inode, zero extra bytes, instant; `linkat` without
/// `AT_SYMLINK_FOLLOW`), else a full copy read from the original's handle
/// into a file created exclusively at `dst` (cross-device layouts, NAS /
/// SMB / FAT mounts, or filesystems that refuse links on the file —
/// `EXDEV`, `EPERM`, `ENOTSUP`, `EMLINK`). The copy carries the original's
/// permission bits, as `fs::copy` did; a hard link shares them anyway.
/// Either way `src` is untouched, which is what keeps the target readable
/// through the whole swap. `dst` must not exist (the rotation freed the
/// slot). A copy that fails part-way is removed again.
///
/// **A link must age from now.** The retention sweep ages a backup by its
/// ctime ([`backup_made_at`]). After a link is made, the new name is
/// looked at once (no link followed): when it is still the original's
/// file and [`link_ages_from_now`] fails against `written_at` (the staging
/// file's mtime, the filesystem's own clock; our clock taken just before
/// the link when that is unknown), the backup name alone is unlinked (the
/// original keeps its own name, so it is never touched) and the copy path
/// runs. A failure part-way leaves a valid backup (the link, when the
/// unlink fails: the error stops the rewrite before the swap) or none
/// (the copy failed and was removed), and the archive at `src` either way.
fn link_or_copy(
    dir: &LibraryDir,
    src: &OsStr,
    dst: &OsStr,
    original: &ArchiveFile,
    written_at: Option<SystemTime>,
) -> Result<(), RewriteError> {
    let before = SystemTime::now();
    match dir.hard_link(src, dst) {
        Ok(()) => {
            let reference = written_at.unwrap_or(before);
            if linked_backup_ages_from_now(dir, dst, original, reference)? {
                return Ok(());
            }
            warn_links_become_copies(dir, original);
            tracing::debug!(
                dst = %dir.display(dst).display(),
                "archive_rewrite: the hard link does not read as made now; copying the backup instead",
            );
            dir.remove_file(dst)?;
        }
        Err(e) => {
            tracing::debug!(
                src = %dir.display(src).display(),
                dst = %dir.display(dst).display(),
                error = %e,
                "archive_rewrite: hard_link refused; copying the backup instead",
            );
            // A failed link attempt can't leave a half-file, but be
            // defensive: clear anything at `dst` before copying so a
            // stale slot never masquerades as the backup.
            let _ = dir.remove_file(dst);
        }
    }
    let (out, mut guard) = Staged::create(dir, dst)?;
    copy_from_handle(original.file(), &out)?;
    out.set_permissions(original.file().metadata()?.permissions())?;
    out.sync_all()?;
    guard.disarm();
    Ok(())
}

/// Look at the freshly made link `dst` (a no-follow `stat`, the file is
/// not opened): an error unless it is still the original's file (a regular
/// file with the original's device and inode), then [`link_ages_from_now`].
fn linked_backup_ages_from_now(
    dir: &LibraryDir,
    dst: &OsStr,
    original: &ArchiveFile,
    reference: SystemTime,
) -> Result<bool, RewriteError> {
    let st = dir.stat(dst)?;
    match st {
        Some(st) if st.kind == EntryKind::Regular && st.identity == original.identity()? => {
            Ok(link_ages_from_now(backup_made_at_stat(&st), reference))
        }
        _ => Err(RewriteError::Io(io::Error::other(
            "the backup name changed while it was being made; nothing was replaced",
        ))),
    }
}

/// Say once per process per filesystem (device) that backups there are
/// made by copy: a hard link was made and then replaced because its age
/// did not read as "now" (the filesystem does not set ctime on a link, or
/// the archive's mtime lies in the future). Per file it stays at debug.
fn warn_links_become_copies(dir: &LibraryDir, original: &ArchiveFile) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static WARNED: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    let Ok((dev, _)) = original.identity() else {
        return;
    };
    let first = WARNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut seen| seen.insert(dev))
        .unwrap_or(false);
    if first {
        tracing::warn!(
            dir = %dir.path().display(),
            device = dev,
            "archive_rewrite: backups on this filesystem are made by copy, not hard link: \
             a fresh hard link here does not read as made now (the filesystem does not \
             set the change time on a link, as sshfs does not, or the archive's \
             modification time lies in the future), and the retention sweep ages a \
             backup by that time. Copies take disk space and time; said once per \
             filesystem per process",
        );
    }
}

/// Copy all of `src` into the empty `dst`, reading `src` at explicit
/// offsets so the shared descriptor's position (and any other reader of
/// it) is never disturbed. `copy_file_range(2)` on Linux (in-kernel, and
/// a reflink or server-side copy where the filesystem offers one), else
/// positioned reads and writes with a 1 MiB buffer.
fn copy_from_handle(src: &fs::File, dst: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    let len = src.metadata()?.len();
    let mut off: u64 = 0;
    #[cfg(target_os = "linux")]
    {
        use rustix::io::Errno;
        while off < len {
            let want = usize::try_from(len - off)
                .unwrap_or(usize::MAX)
                .min(1 << 30);
            let mut off_out = off;
            match rustix::fs::copy_file_range(src, Some(&mut off), dst, Some(&mut off_out), want) {
                Ok(0) => break,
                Ok(_) => {}
                Err(Errno::INTR) => {}
                // Not offered for this pair of files: copy by hand from
                // wherever the kernel got to.
                Err(Errno::XDEV | Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => break,
                Err(e) => return Err(e.into()),
            }
        }
    }
    let mut buf = vec![0u8; 1 << 20];
    while off < len {
        let n = src.read_at(&mut buf, off)?;
        if n == 0 {
            break;
        }
        dst.write_all_at(&buf[..n], off)?;
        off += n as u64;
    }
    if off != len {
        return Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "the original ended before the backup copy was complete",
        ));
    }
    Ok(())
}

/// Suffix of every staging file [`temp_staging`] names.
const STAGING_SUFFIX: &str = ".tmp";
/// Length of the random token in a staging name: a v7 UUID in `simple`
/// form, 32 lowercase hex characters.
const STAGING_TOKEN_LEN: usize = 32;

/// A fresh, unpredictable staging name for the file name `target`:
/// `<target>.<32 lowercase hex>.tmp`, created in the same folder (so the
/// final rename stays on one filesystem). The random component means
/// nobody can predict the name to plant something there in advance, and
/// the exclusive, no-follow create ([`LibraryDir::create_new`]) refuses
/// anything that is there anyway (SEC-8). [`is_staging_name`] is the exact
/// inverse: it is what [`startup_cleanup`] matches, so only names built
/// here are ever reaped.
fn staging_name(target: &OsStr) -> OsString {
    let token = Uuid::now_v7().simple().to_string();
    debug_assert_eq!(token.len(), STAGING_TOKEN_LEN);
    let mut s = target.to_os_string();
    s.push(".");
    s.push(token);
    s.push(STAGING_SUFFIX);
    s
}

/// [`staging_name`] as a full path next to `target`.
#[cfg(test)]
fn temp_staging(target: &Path) -> PathBuf {
    target.with_file_name(staging_name(target.file_name().unwrap_or_default()))
}

/// Whether `name` (a bare file name) is a staging file [`staging_name`]
/// produces: a recognized comic-archive name (`.cbz` / `.cbt` / `.cbr` /
/// `.cb7`, any case, non-empty stem), a dot, exactly
/// [`STAGING_TOKEN_LEN`] lowercase hex characters, then `.tmp`. Anything
/// else — a user's `notes.tmp`, another tool's `x.cbz.tmp`, an uppercase
/// token — is not Folio's and is never touched.
pub fn is_staging_name(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(STAGING_SUFFIX) else {
        return false;
    };
    let Some((base, token)) = rest.rsplit_once('.') else {
        return false;
    };
    if token.len() != STAGING_TOKEN_LEN
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return false;
    }
    let base = Path::new(base);
    base.file_stem().is_some_and(|s| !s.is_empty())
        && base
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(crate::library::ignore::is_recognized_archive_ext)
}

/// Backup-slot name for the file name `target`: `<target>.bak` for slot 0,
/// `<target>.bak.N` for slot N >= 1.
fn backup_slot_name(target: &OsStr, slot: i32) -> OsString {
    let mut s = target.to_os_string();
    if slot == 0 {
        s.push(".bak");
    } else {
        s.push(format!(".bak.{slot}"));
    }
    s
}

/// Backup-slot path for `target`: `<target>.bak` for slot 0,
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
/// [`rewrite_atomic`] step 5). Every rename and removal names entries in
/// `dir` and follows no link.
fn rotate_backups(dir: &LibraryDir, target: &OsStr, retain_count: i32) -> Result<(), RewriteError> {
    // Walk high→low so we never clobber a higher-numbered slot that
    // hasn't been moved yet.
    for slot in (0..retain_count).rev() {
        let from = backup_slot_name(target, slot);
        let to = backup_slot_name(target, slot + 1);
        if dir.kind(&from)? != EntryKind::Missing {
            if slot + 1 >= retain_count {
                // This slot would fall off the end of the retention
                // window — remove the source rather than rename. (We
                // never write a slot >= retain_count.)
                dir.remove_file(&from)?;
            } else {
                dir.rename(&from, &to)?;
            }
        }
    }
    Ok(())
}

/// A conversion whose new file is in place under its new name while the
/// original still sits at its old name (and, for a repack, also at
/// `<original>.bak`). The caller repoints whatever refers to the old name
/// (the issue row) and then calls [`Converted::finish`], which removes the
/// old name; or, when it could not repoint, [`Converted::abandon`], which
/// removes the new file again. Until one of them runs, both names exist,
/// so whichever name the row holds at a crash, the file is there.
#[must_use = "finish() removes the original name once the row is repointed; abandon() undoes"]
#[derive(Debug)]
pub struct Converted {
    /// `target` is the new file; `backup` the `.bak` holding the original,
    /// when one was made.
    pub outcome: RewriteOutcome,
    pending: Option<Pending>,
}

/// The two names a [`Converted`] still has to settle.
#[derive(Debug)]
struct Pending {
    dir: LibraryDir,
    original: OsString,
    original_identity: (u64, u64),
    placed: OsString,
    placed_identity: (u64, u64),
}

impl Converted {
    /// A conversion with nothing left to settle (an in-place conversion).
    fn done(outcome: RewriteOutcome) -> Self {
        Self {
            outcome,
            pending: None,
        }
    }

    /// Nothing was converted: `target` already was what the caller wanted
    /// (a ZIP named `.cbz`).
    pub fn unchanged(target: PathBuf) -> Self {
        Self::done(RewriteOutcome {
            target,
            backup: None,
        })
    }

    /// The row now names the new file: remove the original's old name
    /// (the original stays in `.bak` when the conversion made one), then
    /// fsync the folder. Only removed while the name still holds the
    /// original (same device and inode, not a link); anything else there
    /// is left alone. A failure here is logged, not returned: the
    /// conversion is complete, and the leftover name holds the original's
    /// bytes. Blocking.
    pub fn finish(mut self) -> RewriteOutcome {
        if let Some(p) = self.pending.take() {
            let same = p.dir.kind(&p.original).ok() == Some(EntryKind::Regular)
                && p.dir.identity(&p.original).ok() == Some(p.original_identity);
            if same {
                if let Err(e) = p.dir.remove_file(&p.original) {
                    tracing::warn!(
                        path = %p.dir.display(&p.original).display(),
                        error = %e,
                        "archive_rewrite: converted, but the original's old name could not be removed",
                    );
                } else if let Err(e) = p.dir.sync() {
                    tracing::error!(
                        path = %p.dir.display(&p.original).display(),
                        error = %e,
                        "archive_rewrite: converted, but syncing the folder after removing the original's old name failed",
                    );
                }
            } else {
                tracing::warn!(
                    path = %p.dir.display(&p.original).display(),
                    "archive_rewrite: converted; the original's old name changed meanwhile and was left alone",
                );
            }
        }
        self.outcome.clone()
    }

    /// The row could not be repointed: remove the new file (only while its
    /// name still holds it) so the original, still at its name, is again
    /// the only copy, and a later retry starts from the same state. The
    /// original and its `.bak` are left as they are. Blocking.
    pub fn abandon(mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let same = p.dir.kind(&p.placed).ok() == Some(EntryKind::Regular)
            && p.dir.identity(&p.placed).ok() == Some(p.placed_identity);
        if !same {
            return;
        }
        match p.dir.remove_file(&p.placed).and_then(|()| p.dir.sync()) {
            Ok(()) => tracing::info!(
                path = %p.dir.display(&p.placed).display(),
                "archive_rewrite: conversion undone; the original is unchanged",
            ),
            Err(e) => tracing::warn!(
                path = %p.dir.display(&p.placed).display(),
                error = %e,
                "archive_rewrite: could not remove the converted file while undoing",
            ),
        }
    }
}

impl Converted {
    /// The original's path while it still has its old name.
    pub fn original_path(&self) -> Option<PathBuf> {
        self.pending.as_ref().map(|p| p.dir.display(&p.original))
    }

    /// Settle nothing and say so: both names stay, for the operator.
    fn leave(mut self, why: &str) {
        if let Some(p) = self.pending.take() {
            tracing::error!(
                original = %p.dir.display(&p.original).display(),
                converted = %p.dir.display(&p.placed).display(),
                reason = why,
                "archive_rewrite: conversion left unsettled; both files kept",
            );
        }
    }
}

impl Drop for Converted {
    /// Neither `finish` nor `abandon` ran (an early return, a dropped
    /// future): both files stay, which is recoverable. Nothing is deleted
    /// here; the names are logged for the operator.
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            tracing::warn!(
                original = %p.dir.display(&p.original).display(),
                converted = %p.dir.display(&p.placed).display(),
                "archive_rewrite: conversion dropped before it was settled; both files kept",
            );
        }
    }
}

/// What to do with a conversion whose row update reported an error. An
/// update (or its commit) can fail on the client after the database
/// applied it, so the row is read again on a fresh statement first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// The row names the new file: remove the original's old name.
    Finish,
    /// The row still names the original: remove the new file.
    Abandon,
    /// The re-read failed, or the row names neither (or is gone): keep both
    /// files. A wrong guess would strand the row; both files are
    /// recoverable.
    Leave,
}

/// [`Settle`] for the row's current `file_path` (`Err` when the re-read
/// failed).
pub fn settle_for<E>(row_path: Result<Option<String>, E>, original: &Path, dst: &Path) -> Settle {
    match row_path {
        Ok(Some(p)) if Path::new(&p) == dst => Settle::Finish,
        Ok(Some(p)) if Path::new(&p) == original => Settle::Abandon,
        _ => Settle::Leave,
    }
}

/// Settle `converted` after the caller's row update for `issue_id`
/// returned an error: re-read the row's `file_path` (outside any failed
/// transaction) and finish, abandon or leave per [`settle_for`]. Returns
/// what it did.
pub async fn settle_unclear_repoint(
    db: &sea_orm::DatabaseConnection,
    issue_id: &str,
    converted: Converted,
) -> Settle {
    use sea_orm::{EntityTrait, QuerySelect};
    let Some(original) = converted.original_path() else {
        return Settle::Leave;
    };
    let dst = converted.outcome.target.clone();
    let row_path = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .select_only()
        .column(entity::issue::Column::FilePath)
        .into_tuple::<String>()
        .one(db)
        .await;
    let decision = settle_for(row_path.clone(), &original, &dst);
    match decision {
        Settle::Finish => {
            let _ = tokio::task::spawn_blocking(move || converted.finish()).await;
        }
        Settle::Abandon => {
            let _ = tokio::task::spawn_blocking(move || converted.abandon()).await;
        }
        Settle::Leave => {
            let why = match &row_path {
                Err(e) => format!("re-reading the issue row failed: {e}"),
                Ok(Some(p)) => format!("the issue row names neither file: {p}"),
                Ok(None) => "the issue row is gone".to_owned(),
            };
            converted.leave(&why);
        }
    }
    decision
}

/// The destination name in the original's folder; any other folder is
/// refused (`NoParent`).
fn same_folder_name(original: &Path, dst: &Path) -> Result<OsString, RewriteError> {
    match (dst.parent(), dst.file_name()) {
        (Some(p), Some(n)) if Some(p) == original.parent() => Ok(n.to_os_string()),
        _ => Err(RewriteError::no_parent(dst)),
    }
}

/// The destination name must be free: a link is refused as a link, any
/// other entry as [`RewriteError::DestinationExists`].
fn require_free(dir: &LibraryDir, name: &OsStr, dst: &Path) -> Result<(), RewriteError> {
    match dir.kind(name)? {
        EntryKind::Missing => Ok(()),
        EntryKind::Symlink => Err(RewriteError::Refused(Refusal::Symlink)),
        EntryKind::Regular | EntryKind::Other => Err(RewriteError::destination_exists(dst)),
    }
}

/// Map a no-replace rename or link that found its name taken.
fn taken(e: io::Error, dst: &Path) -> RewriteError {
    if e.kind() == ErrorKind::AlreadyExists {
        RewriteError::destination_exists(dst)
    } else {
        RewriteError::Io(e)
    }
}

/// Convert one archive into another — the CBR/CB7→CBZ path of the page
/// editor, the sidecar writeback and the scanner. Same folder-handle
/// discipline as [`rewrite_atomic`] (no link followed below `root`, every
/// step relative to the folder handle, the original read through its
/// handle, a staging file created exclusively and removed again on any
/// failure).
///
/// **In place** (`dst == original`, a RAR or 7z that wore `.cbz`): exactly
/// the [`rewrite_atomic`] sequence with one backup slot, so the original
/// ends up in `<name>.bak` and the name never goes missing.
///
/// **To a new name** (`foo.cbr` → `foo.cbz`):
///
///   1. Refuse a destination that exists ([`RewriteError::DestinationExists`],
///      a link as [`RewriteError::Refused`]): a conversion never writes over
///      another file.
///   2. Write the new archive into `<dst>.<hex>.tmp`; fsync it and the
///      folder.
///   3. Confirm the original is unchanged, then hard-link (or copy) it to
///      a staging name and rename that over `<original>.bak` (an older
///      `.bak` is replaced in that one step, never removed first): the
///      original now has two names.
///   4. Rename the staging file to `dst` without replacing anything
///      ([`LibraryDir::rename_noreplace`]); fsync the folder (a failure of
///      that fsync is logged, not returned).
///   5. Return a [`Converted`]: the original is still at its name. The
///      caller repoints its row at `dst` and calls [`Converted::finish`],
///      which removes the original's old name and fsyncs the folder.
///
/// There is no moment without a file at the name the row holds: the
/// original's name is removed only after the caller has moved the row.
pub fn convert_atomic<F>(
    root: &LibraryRoot,
    original: &Path,
    dst: &Path,
    write_into: F,
) -> Result<Converted, RewriteError>
where
    F: FnOnce(&RewriteIo<'_>) -> Result<(), RewriteError>,
{
    convert_atomic_with_faults(root, original, dst, write_into, |_| Ok(()))
}

/// [`convert_atomic`] with the fault hook of [`rewrite_atomic_with_faults`]
/// at the same points (for a new name, `Rotated` is the new backup made
/// under its staging name, `BackedUp` that renamed over `.bak`, `Swapped`
/// the staging file renamed to `dst`). Tests only.
pub fn convert_atomic_with_faults<F, H>(
    root: &LibraryRoot,
    original: &Path,
    dst: &Path,
    write_into: F,
    mut fault: H,
) -> Result<Converted, RewriteError>
where
    F: FnOnce(&RewriteIo<'_>) -> Result<(), RewriteError>,
    H: FnMut(RewriteStep) -> Result<(), RewriteError>,
{
    if dst == original {
        return rewrite_atomic_with_faults(root, original, 1, write_into, fault)
            .map(Converted::done);
    }
    let (dir, orig_name) = open_target(root, original)?;
    let dst_name = same_folder_name(original, dst)?;
    let Some(source) = open_original(&dir, &orig_name, original)? else {
        return Err(Refusal::Missing.into());
    };
    require_free(&dir, &dst_name, dst)?;
    let tmp_name = staging_name(&dst_name);
    let (staging, mut guard) = Staged::create(&dir, &tmp_name)?;
    let staging_path = dir.display(&tmp_name);
    let placed_identity = file_identity(&staging)?;

    write_into(&RewriteIo {
        source: Some(&source),
        staging: &staging,
        staging_path: &staging_path,
    })?;
    staging.sync_all()?;
    let written_at = written_at(&staging);
    drop(staging);
    dir.sync()?;
    fault(RewriteStep::Staged)?;

    confirm_unchanged(&dir, &orig_name, Some(&source))?;
    // One rollback slot. The new backup is made under a staging name first
    // and then renamed over `.bak` in one step, so there is never a moment
    // with neither the older backup nor the new one.
    let backup = backup_slot_name(&orig_name, 0);
    let backup_tmp = staging_name(&orig_name);
    link_or_copy(&dir, &orig_name, &backup_tmp, &source, written_at)?;
    let mut backup_guard = Staged {
        dir: &dir,
        identity: dir.identity(&backup_tmp).ok(),
        name: backup_tmp.clone(),
    };
    fault(RewriteStep::Rotated)?;
    dir.rename(&backup_tmp, &backup)?;
    backup_guard.disarm();
    drop(backup_guard);
    fault(RewriteStep::BackedUp)?;

    // Taken before the rename, so nothing can fail between the rename and
    // handing the caller a `Converted` to settle.
    let original_identity = source.identity()?;
    dir.rename_noreplace(&tmp_name, &dst_name)
        .map_err(|e| taken(e, dst))?;
    guard.disarm();
    drop(guard);
    fault(RewriteStep::Swapped)?;
    sync_after_swap(&dir, &dst_name, &mut fault);

    Ok(Converted {
        outcome: RewriteOutcome {
            target: dst.to_path_buf(),
            backup: Some(dir.display(&backup)),
        },
        pending: Some(Pending {
            original_identity,
            dir,
            original: orig_name,
            placed: dst_name,
            placed_identity,
        }),
    })
}

/// Give `src`'s file (a ZIP that only wore another extension) the name
/// `dst` in the same folder, byte-for-byte, without replacing anything: a
/// destination that exists is [`RewriteError::DestinationExists`], a link
/// at either name is refused. The file gets the new name as a hard link
/// (which itself refuses an existing name), so the old name still holds it
/// until the caller repoints its row and calls [`Converted::finish`]. Where
/// the filesystem refuses hard links it is renamed instead
/// ([`LibraryDir::rename_noreplace`]); then the old name is gone at once and
/// a row still holding it points at nothing until the caller repoints it.
pub fn rename_into_place(
    root: &LibraryRoot,
    src: &Path,
    dst: &Path,
) -> Result<Converted, RewriteError> {
    let (dir, src_name) = open_target(root, src)?;
    let dst_name = same_folder_name(src, dst)?;
    let Some(source) = open_original(&dir, &src_name, src)? else {
        return Err(Refusal::Missing.into());
    };
    require_free(&dir, &dst_name, dst)?;
    let identity = source.identity()?;
    let outcome = RewriteOutcome {
        target: dst.to_path_buf(),
        backup: None,
    };
    match dir.hard_link(&src_name, &dst_name) {
        Ok(()) => {
            sync_after_swap(&dir, &dst_name, &mut |_| Ok(()));
            Ok(Converted {
                outcome,
                pending: Some(Pending {
                    dir,
                    original: src_name,
                    original_identity: identity,
                    placed: dst_name,
                    placed_identity: identity,
                }),
            })
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            Err(RewriteError::destination_exists(dst))
        }
        Err(e) => {
            tracing::debug!(
                src = %dir.display(&src_name).display(),
                error = %e,
                "archive_rewrite: hard link refused; renaming the archive into place instead",
            );
            dir.rename_noreplace(&src_name, &dst_name)
                .map_err(|e| taken(e, dst))?;
            sync_after_swap(&dir, &dst_name, &mut |_| Ok(()));
            Ok(Converted::done(outcome))
        }
    }
}

/// One backup-slot path for `target` — `<target>.bak` for slot 0,
/// `<target>.bak.N` for N >= 1. Exposed so callers (the backups-listing
/// endpoint, restore) can enumerate the retention slots without
/// re-deriving the naming scheme.
pub fn backup_slot(target: &Path, slot: i32) -> PathBuf {
    backup_slot_path(target, slot)
}

/// Highest backup slot any Folio version names: `retain_count` is capped at
/// 5, so the rotation writes slots `0..=4`; the backups listing looks at
/// `0..=5`, and so does [`is_backup_name`].
pub const MAX_BACKUP_SLOT: i32 = 5;

/// Whether `name` (a bare file name) is a backup this module produces:
/// `<archive>.bak` (slot 0) or `<archive>.bak.<n>` with `n` in
/// `1..=`[`MAX_BACKUP_SLOT`] written without leading zeros, where
/// `<archive>` is a recognised comic-archive file name (`.cbz` / `.cbt` /
/// `.cbr` / `.cb7`, any case, non-empty stem). Every archive Folio rewrites
/// or converts has such a name, so this covers the pre-conversion backups
/// too (`X.cbr.bak` beside the converted `X.cbz`). The exact inverse of
/// [`backup_slot`] over an archive path.
///
/// Anything else is not a backup: a user's `notes.bak`, `photo.jpg.bak`,
/// `X.cbz.bak.txt`, `X.cbz.bak.01`. The retention sweep, the backup-storage
/// tally and the series folder's backup count all go through this, so a
/// file that fails it is never deleted, counted or offered. Callers also
/// require a regular file (no link, no directory); the name alone is not
/// enough.
pub fn is_backup_name(name: &str) -> bool {
    let base = if let Some(base) = name.strip_suffix(".bak") {
        base
    } else if let Some((base, slot)) = name.rsplit_once(".bak.") {
        let valid_slot = !slot.starts_with('0')
            && slot.bytes().all(|b| b.is_ascii_digit())
            && slot
                .parse::<i32>()
                .is_ok_and(|n| (1..=MAX_BACKUP_SLOT).contains(&n));
        if !valid_slot {
            return false;
        }
        base
    } else {
        return false;
    };
    let base = Path::new(base);
    base.file_stem().is_some_and(|s| !s.is_empty())
        && base
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(crate::library::ignore::is_recognized_archive_ext)
}

/// The backup slot of `candidate` (a bare file name in the same folder) when
/// it is a backup of the archive `archive` (a bare file name, the issue's
/// current file) was converted from: `X.cbr.bak[.n]` or `X.cb7.bak[.n]`
/// (extension in any case) beside `X.cbz`. A CBR / CB7 converted to CBZ
/// (scan, page editor, sidecar writeback) keeps its original under its old
/// name, so the backups built from the current name ([`backup_slot`]) never
/// include it. `None` for anything else, including the archive's own
/// backups and a candidate that fails [`is_backup_name`].
pub fn pre_conversion_backup_slot(archive: &str, candidate: &str) -> Option<i32> {
    if !is_backup_name(candidate) {
        return None;
    }
    let (base, slot) = match candidate.strip_suffix(".bak") {
        Some(base) => (base, 0),
        None => {
            let (base, n) = candidate.rsplit_once(".bak.")?;
            (base, n.parse::<i32>().ok()?)
        }
    };
    let base = Path::new(base);
    let ext = base.extension()?.to_str()?;
    if !(ext.eq_ignore_ascii_case("cbr") || ext.eq_ignore_ascii_case("cb7")) {
        return None;
    }
    let archive = Path::new(archive);
    let converted_to_cbz = archive
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cbz"));
    (converted_to_cbz && base.file_stem() == archive.file_stem()).then_some(slot)
}

/// When Folio made the backup described by `meta` (the `lstat` of a file
/// [`is_backup_name`] accepts), for the retention sweep and the
/// backup-storage dates. `None` when that cannot be known: the sweep then
/// keeps the file.
///
/// The inode change time (`ctime`), not the modification time. A backup is
/// usually a hard link to the original archive, which keeps the
/// original's (often years old) mtime, so by mtime a backup made a minute
/// ago could already be "older than the retention". Making the link sets
/// the inode's ctime to the moment of the link (POSIX `link(2)`: ext4,
/// btrfs, XFS, ZFS, tmpfs, NFS servers), and the copy fallback creates a
/// new file, so ctime is never earlier than the backup. Nothing in user
/// space can set ctime back. Later changes (the rewrite unlinking the
/// archive's old name, a slot rotation's rename, a `chmod` / `chown`)
/// only move it forward, which keeps a backup longer, never shorter.
/// A filesystem that does NOT set ctime on a link (sshfs reports the mtime
/// as ctime) is caught when the backup is made, not here: a link that
/// does not read as made now is replaced by a copy ([`link_or_copy`]).
/// Such a filesystem reads ctime == mtime, which this function cannot tell
/// from a file that simply was not changed since it was written (every
/// file on FAT / exFAT), so equal times are trusted.
///
/// Unknown (`None`): no ctime (zero or before the epoch), or a ctime
/// earlier than the mtime. A filesystem that keeps ctime produces that
/// only for a file whose mtime was set into the future (every mtime change
/// also sets ctime), for example an archive tagged by a machine whose
/// clock ran ahead; such a backup's age cannot be told, so the sweep keeps
/// it, counts it (`PruneStats::kept_unknown_age`) and warns once per
/// library.
pub fn backup_made_at(meta: &fs::Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let ctime = u64::try_from(meta.ctime())
        .ok()
        .filter(|&s| s > 0)
        .map(|s| {
            let nanos = u32::try_from(meta.ctime_nsec()).unwrap_or(0);
            SystemTime::UNIX_EPOCH + std::time::Duration::new(s, nanos)
        });
    made_at(ctime, meta.modified().ok())
}

/// [`backup_made_at`] from a no-follow `stat` taken without opening the
/// file ([`crate::library::no_follow::LibraryDir::stat`]).
pub fn backup_made_at_stat(st: &crate::library::no_follow::EntryStat) -> Option<SystemTime> {
    made_at(st.ctime, st.mtime)
}

/// [`backup_made_at`]'s rule over the two timestamps.
fn made_at(ctime: Option<SystemTime>, mtime: Option<SystemTime>) -> Option<SystemTime> {
    let ctime = ctime?;
    match mtime {
        Some(m) if ctime < m => None,
        _ => Some(ctime),
    }
}

/// Restore the most recent backup (`<target>.bak`) over `target` — the
/// one-click undo of the last rewrite. The rename is atomic on the same
/// filesystem, so a crash can't leave `target` half-written. Relative to
/// the folder handle from the no-follow walk: the `.bak` must be a regular
/// file and `target` a regular file or missing (a link at either is
/// refused). Returns `Err(NotFound)` when no `.bak` exists. Higher slots
/// (`.bak.1`, …) are left untouched (older history stays available).
pub fn restore_latest_backup(root: &LibraryRoot, target: &Path) -> Result<(), RewriteError> {
    let (dir, name) = open_target(root, target)?;
    let bak = backup_slot_name(&name, 0);
    match dir.kind(&bak)? {
        EntryKind::Regular => {}
        EntryKind::Missing => {
            return Err(RewriteError::Io(io::Error::new(
                ErrorKind::NotFound,
                "no .bak to restore",
            )));
        }
        EntryKind::Symlink => return Err(RewriteError::Refused(Refusal::Symlink)),
        EntryKind::Other => return Err(RewriteError::Refused(Refusal::NotRegular)),
    }
    match dir.kind(&name)? {
        EntryKind::Regular | EntryKind::Missing => {}
        EntryKind::Symlink => return Err(RewriteError::Refused(Refusal::Symlink)),
        EntryKind::Other => return Err(RewriteError::Refused(Refusal::NotRegular)),
    }
    dir.rename(&bak, &name)?;
    dir.sync()?;
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

/// Age floor for [`startup_cleanup`]: a staging file younger than this may
/// belong to a rewrite in progress and is left alone.
pub const STARTUP_CLEANUP_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Walk `roots` and remove Folio's own staging files ([`is_staging_name`])
/// older than `ttl` — leftovers from a crashed rewrite. Nothing else is
/// touched: other `.tmp` files and `.bak` backups stay. The caller passes
/// only roots of libraries with `allow_archive_writeback = true`
/// ([`spawn_startup_cleanup`]); a library with writeback off is never
/// walked. Blocking (synchronous filesystem walk); safe to call repeatedly.
///
/// The cutoff is fixed once, when the walk starts, so a rewrite that
/// begins while a slow walk is still running is always newer than it.
///
/// Returns the count of removed files (mostly for log scraping).
pub fn startup_cleanup(roots: impl IntoIterator<Item = PathBuf>, ttl: std::time::Duration) -> u64 {
    let cutoff = SystemTime::now()
        .checked_sub(ttl)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0u64;
    for root in roots {
        removed += walk_and_remove(&root, &root, cutoff);
    }
    removed
}

/// Run the boot-time staging sweep in the background: load the roots of
/// writeback-enabled libraries, then walk them on the blocking pool with
/// [`STARTUP_CLEANUP_TTL`]. Called after the HTTP listener is bound, so a
/// large or slow library never delays the server coming up. The returned
/// handle resolves to the removed count (tests await it; boot drops it).
pub fn spawn_startup_cleanup(db: sea_orm::DatabaseConnection) -> tokio::task::JoinHandle<u64> {
    tokio::spawn(async move {
        use entity::library;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
        let roots: Vec<String> = match library::Entity::find()
            .select_only()
            .column(library::Column::RootPath)
            .filter(library::Column::AllowArchiveWriteback.eq(true))
            .into_tuple()
            .all(&db)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "archive_rewrite startup: library lookup failed; sweep skipped");
                return 0;
            }
        };
        if roots.is_empty() {
            return 0;
        }
        let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        match tokio::task::spawn_blocking(move || startup_cleanup(roots, STARTUP_CLEANUP_TTL)).await
        {
            Ok(removed) => {
                if removed > 0 {
                    tracing::info!(
                        removed,
                        "archive_rewrite startup: removed orphan staging files"
                    );
                }
                removed
            }
            Err(e) => {
                tracing::warn!(error = %e, "archive_rewrite startup: sweep task failed");
                0
            }
        }
    })
}

/// Find staging files under `dir` and remove the old ones. The walk only
/// finds candidates (it never descends into a link: `DirEntry::metadata`
/// does not follow one); each removal goes through the no-follow walk from
/// `root` ([`crate::library::no_follow::remove_library_file`]), so a folder
/// swapped for a link after it was listed is refused, and a link at a
/// staging name is left alone.
fn walk_and_remove(root: &Path, dir: &Path, cutoff: SystemTime) -> u64 {
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
            removed += walk_and_remove(root, &path, cutoff);
            continue;
        }
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_staging_name)
        {
            continue;
        }
        let Ok(mtime) = meta.modified() else { continue };
        if mtime > cutoff {
            // Younger than the cutoff — could be a live rewrite in
            // progress on another worker. Skip.
            continue;
        }
        match crate::library::no_follow::remove_library_file(root, &path) {
            Ok(()) => {
                removed += 1;
                tracing::info!(path = %path.display(), "archive_rewrite startup: removed orphan staging file");
            }
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "archive_rewrite startup: failed to remove orphan staging file",
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

    fn root(dir: &TempDir) -> LibraryRoot {
        LibraryRoot::resolve(dir.path())
    }

    /// A `write_into` body that writes `bytes` into the staging file.
    fn put(io: &RewriteIo<'_>, bytes: &[u8]) -> Result<(), RewriteError> {
        io.staging_file()?.write_all(bytes)?;
        Ok(())
    }

    #[test]
    fn atomic_swap_preserves_bak() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1-contents").unwrap();

        let outcome =
            rewrite_atomic(&root(&dir), &target, 1, |io| put(io, b"v2-contents")).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"v2-contents");
        let bak = outcome.backup.unwrap();
        assert_eq!(fs::read(bak).unwrap(), b"v1-contents");
    }

    #[test]
    fn write_failure_leaves_original_untouched() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1-contents").unwrap();

        let res = rewrite_atomic(&root(&dir), &target, 1, |_io| {
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

        let outcome = rewrite_atomic(&root(&dir), &target, 0, |io| put(io, b"v2")).unwrap();

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

        let res = rewrite_atomic(&root(&dir), &target, 0, |io| {
            put(io, b"corrupt-rewrite")?;
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
        rewrite_atomic(&root(&dir), &target, 3, |io| put(io, b"v2")).unwrap();
        rewrite_atomic(&root(&dir), &target, 3, |io| put(io, b"v3")).unwrap();
        rewrite_atomic(&root(&dir), &target, 3, |io| put(io, b"v4")).unwrap();

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
            RewriteStep::Staged,
            RewriteStep::Rotated,
            RewriteStep::BackedUp,
            RewriteStep::Swapped,
        ] {
            let dir = TempDir::new().unwrap();
            let target = dir.path().join("issue.cbz");
            fs::write(&target, b"old-bytes").unwrap();
            // A pre-existing `.bak` so the rotation step has real work.
            fs::write(target.with_extension("cbz.bak"), b"older-bytes").unwrap();

            let res = rewrite_atomic_with_faults(
                &root(&dir),
                &target,
                2,
                |io| put(io, b"new-bytes"),
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
            let expected: &[u8] = if step == RewriteStep::Swapped {
                b"new-bytes"
            } else {
                b"old-bytes"
            };
            assert_eq!(bytes, expected, "target bytes after crash at {step:?}");

            // Once the backup slot has been filled, the old bytes are also
            // reachable through `.bak` — a crash never strands the only copy.
            if matches!(step, RewriteStep::BackedUp | RewriteStep::Swapped) {
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
            &root(&dir),
            &target,
            1,
            |io| put(io, b"v2"),
            |at| {
                if at == RewriteStep::BackedUp {
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
        let (folder, name) = root(&dir).open_parent(&src).unwrap();
        let original = ArchiveFile::new(folder.open_regular(&name).unwrap(), &src);
        link_or_copy(&folder, &name, OsStr::new("issue.cbz.bak"), &original, None).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"payload", "source untouched");
        assert_eq!(fs::read(&dst).unwrap(), b"payload", "slot holds the copy");
    }

    #[test]
    fn invalid_retain_count_rejected() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let res = rewrite_atomic(&root(&dir), &target, 6, |_| Ok(()));
        assert!(matches!(res, Err(RewriteError::InvalidRetainCount(6))));
    }

    #[test]
    fn temp_staging_is_randomized_and_tmp_suffixed() {
        let target = Path::new("/lib/series/issue.cbz");
        let a = temp_staging(target);
        let b = temp_staging(target);
        // Distinct each call (random component), still under the same dir,
        // and named exactly what startup_cleanup matches.
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert_eq!(p.parent(), target.parent());
            let name = p.file_name().unwrap().to_str().unwrap();
            assert!(is_staging_name(name), "{name}");
        }
        // Every target name the writers stage under matches.
        for t in ["a.cbz", "b.CBT", "c.cbr", "d.cb7", "Issue #1 (2020).cbz"] {
            let p = temp_staging(&Path::new("/lib").join(t));
            assert!(is_staging_name(p.file_name().unwrap().to_str().unwrap()));
        }
    }

    #[test]
    fn is_staging_name_matches_only_folio_staging_files() {
        let hex = "0192f3a4b5c6d7e8f90a1b2c3d4e5f60";
        assert_eq!(hex.len(), 32);
        assert!(is_staging_name(&format!("issue.cbz.{hex}.tmp")));
        assert!(is_staging_name(&format!("Issue 1.CBZ.{hex}.tmp")));
        assert!(is_staging_name(&format!("x.cbt.{hex}.tmp")));
        assert!(is_staging_name(&format!("x.cbr.{hex}.tmp")));
        assert!(is_staging_name(&format!("x.cb7.{hex}.tmp")));

        for name in [
            "notes.tmp".to_owned(),
            "x.cbz.tmp".to_owned(),
            ".tmp".to_owned(),
            format!("x.cbz.{}.tmp", &hex[..31]),
            format!("x.cbz.{hex}0.tmp"),
            format!("x.cbz.{}.tmp", hex.to_ascii_uppercase()),
            format!("x.cbz.{}g.tmp", &hex[..31]),
            format!("notes.txt.{hex}.tmp"),
            format!("notes.{hex}.tmp"),
            format!("{hex}.tmp"),
            format!(".cbz.{hex}.tmp"),
            format!("x.cbz.{hex}.TMP"),
            format!("x.cbz.{hex}.tmp.bak"),
            format!("x.cbz.{hex}"),
        ] {
            assert!(!is_staging_name(&name), "{name}");
        }
    }

    #[test]
    fn staging_create_fails_closed_on_existing_file_or_link() {
        let dir = TempDir::new().unwrap();
        let (folder, _) = root(&dir)
            .open_parent(&dir.path().join("issue.cbz"))
            .unwrap();
        // Fresh name: the exclusive create succeeds.
        let fresh = staging_name(OsStr::new("issue.cbz"));
        folder.create_new(&fresh).unwrap();
        assert!(dir.path().join(&fresh).exists());
        // Something already there (a file, or a link to a file outside):
        // O_EXCL | O_NOFOLLOW fails rather than open, follow or truncate it.
        assert!(folder.create_new(&fresh).is_err());
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let planted = staging_name(OsStr::new("issue.cbz"));
        std::os::unix::fs::symlink(&victim, dir.path().join(&planted)).unwrap();
        assert!(folder.create_new(&planted).is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
    }

    /// A staging name as [`temp_staging`] would produce for `target`.
    fn staged(dir: &Path, target: &str) -> PathBuf {
        temp_staging(&dir.join(target))
    }

    #[test]
    fn startup_cleanup_removes_old_staging_file() {
        let dir = TempDir::new().unwrap();
        let stale = staged(dir.path(), "issue.cbz");
        let mut f = fs::File::create(&stale).unwrap();
        f.write_all(b"partial").unwrap();
        drop(f);
        // Sleep + tiny ttl stands in for an old mtime.
        std::thread::sleep(std::time::Duration::from_millis(50));

        let removed = startup_cleanup(
            [dir.path().to_path_buf()],
            std::time::Duration::from_millis(10),
        );
        assert_eq!(removed, 1);
        assert!(!stale.exists());
    }

    #[test]
    fn startup_cleanup_leaves_other_tmp_files() {
        let dir = TempDir::new().unwrap();
        let others = [
            dir.path().join("notes.tmp"),
            dir.path().join("x.cbz.tmp"),
            dir.path().join("x.cbz.bak"),
        ];
        for p in &others {
            fs::write(p, b"not ours").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));

        let removed = startup_cleanup(
            [dir.path().to_path_buf()],
            std::time::Duration::from_millis(10),
        );
        assert_eq!(removed, 0);
        for p in &others {
            assert!(p.exists(), "{}", p.display());
        }
    }

    #[test]
    fn startup_cleanup_skips_recent_staging_file() {
        let dir = TempDir::new().unwrap();
        let recent = staged(dir.path(), "issue.cbz");
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
        let stale = staged(&sub, "issue.cbz");
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
        rewrite_atomic(&root(&dir), &target, 1, |io| put(io, b"v2")).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2");

        restore_latest_backup(&root(&dir), &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v1");
        // The .bak was consumed by the restore.
        assert!(!target.with_extension("cbz.bak").exists());
    }

    #[test]
    fn restore_latest_backup_errors_without_bak() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let err = restore_latest_backup(&root(&dir), &target).unwrap_err();
        assert!(matches!(err, RewriteError::Io(_)));
    }

    #[test]
    fn convert_atomic_moves_original_to_bak_and_writes_dst() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("issue.cbr");
        let dst = dir.path().join("issue.cbz");
        fs::write(&original, b"rar-bytes").unwrap();

        let converted = convert_atomic(&root(&dir), &original, &dst, |io| {
            let mut read = Vec::new();
            io::Read::read_to_end(&mut io.source()?.reader(), &mut read)?;
            assert_eq!(
                read, b"rar-bytes",
                "the closure reads the original's handle"
            );
            put(io, b"zip-bytes")
        })
        .unwrap();
        // Both names hold a file until the caller has repointed its row.
        assert_eq!(fs::read(&original).unwrap(), b"rar-bytes");
        assert_eq!(fs::read(&dst).unwrap(), b"zip-bytes");
        let outcome = converted.finish();

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

    // ── links: every write is relative to a checked folder handle ──

    /// A library with `Series/issue.cbz` and, outside it, a folder holding
    /// a file that must never change.
    struct LinkFixture {
        _tmp: TempDir,
        root: LibraryRoot,
        series: PathBuf,
        target: PathBuf,
        outside: PathBuf,
        victim: PathBuf,
    }

    fn link_fixture() -> LinkFixture {
        let tmp = TempDir::new().unwrap();
        let lib = tmp.path().join("library");
        let series = lib.join("Series");
        fs::create_dir_all(&series).unwrap();
        let target = series.join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let victim = outside.join("issue.cbz");
        fs::write(&victim, b"outside-bytes").unwrap();
        LinkFixture {
            root: LibraryRoot::resolve(&lib),
            _tmp: tmp,
            series,
            target,
            outside,
            victim,
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn rewrite_refuses_a_target_that_is_a_link_and_leaves_its_target_alone() {
        let f = link_fixture();
        fs::remove_file(&f.target).unwrap();
        std::os::unix::fs::symlink(&f.victim, &f.target).unwrap();
        let mut ran = false;
        let res = rewrite_atomic(&f.root, &f.target, 1, |io| {
            ran = true;
            put(io, b"new")
        });
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert!(!ran, "nothing is written for a refused target");
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert!(
            fs::symlink_metadata(&f.target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            names_in(&f.series),
            ["issue.cbz"],
            "no staging file, no .bak"
        );
        assert_eq!(names_in(&f.outside), ["issue.cbz"]);
    }

    #[test]
    fn rewrite_refuses_a_folder_replaced_by_a_link_out_of_the_root() {
        let f = link_fixture();
        fs::rename(&f.series, f.series.with_file_name("Series.real")).unwrap();
        std::os::unix::fs::symlink(&f.outside, &f.series).unwrap();
        let res = rewrite_atomic(&f.root, &f.target, 1, |io| put(io, b"new"));
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert_eq!(
            names_in(&f.outside),
            ["issue.cbz"],
            "nothing created outside the root"
        );
    }

    /// The folder is swapped for a link while the closure runs: every later
    /// step is relative to the folder handle opened first, so the staging
    /// file, the backup and the swap all stay in the real folder.
    #[test]
    fn rewrite_stays_in_the_checked_folder_when_it_is_swapped_mid_way() {
        let f = link_fixture();
        let real = f.series.with_file_name("Series.real");
        let res = rewrite_atomic(&f.root, &f.target, 1, |io| {
            fs::rename(&f.series, &real).unwrap();
            std::os::unix::fs::symlink(&f.outside, &f.series).unwrap();
            put(io, b"new")
        });
        res.unwrap();
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert_eq!(
            names_in(&f.outside),
            ["issue.cbz"],
            "nothing created outside the root"
        );
        assert_eq!(fs::read(real.join("issue.cbz")).unwrap(), b"new");
        assert_eq!(fs::read(real.join("issue.cbz.bak")).unwrap(), b"v1");
    }

    /// The target is replaced by a link after it was read: the swap is
    /// refused and the link's target is untouched.
    #[test]
    fn rewrite_refuses_when_the_target_becomes_a_link_before_the_swap() {
        let f = link_fixture();
        let res = rewrite_atomic(&f.root, &f.target, 1, |io| {
            fs::remove_file(&f.target).unwrap();
            std::os::unix::fs::symlink(&f.victim, &f.target).unwrap();
            put(io, b"new")
        });
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert!(
            fs::symlink_metadata(&f.target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!f.series.join("issue.cbz.bak").exists());
    }

    /// The target is replaced by a different regular file after it was
    /// read: the rewrite stops (it would otherwise back up and replace a
    /// file it never read) and leaves that file in place.
    #[test]
    fn rewrite_stops_when_the_target_is_swapped_for_another_file() {
        let f = link_fixture();
        let res = rewrite_atomic(&f.root, &f.target, 1, |io| {
            let other = f.series.join("other");
            fs::write(&other, b"someone else's").unwrap();
            fs::rename(&other, &f.target).unwrap();
            put(io, b"new")
        });
        assert!(matches!(res, Err(RewriteError::Io(_))), "{res:?}");
        assert_eq!(fs::read(&f.target).unwrap(), b"someone else's");
        assert!(!f.series.join("issue.cbz.bak").exists());
    }

    #[test]
    fn rewrite_reads_the_original_through_its_handle() {
        let f = link_fixture();
        rewrite_atomic(&f.root, &f.target, 1, |io| {
            // The name is gone by now; the handle still reads the original.
            fs::rename(&f.target, f.series.join("moved")).unwrap();
            let mut read = Vec::new();
            io::Read::read_to_end(&mut io.source()?.reader(), &mut read)?;
            assert_eq!(read, b"v1");
            fs::rename(f.series.join("moved"), &f.target).unwrap();
            put(io, b"v2")
        })
        .unwrap();
        assert_eq!(fs::read(&f.target).unwrap(), b"v2");
    }

    #[test]
    fn restore_refuses_a_backup_or_target_that_is_a_link() {
        let f = link_fixture();
        let bak = f.series.join("issue.cbz.bak");
        std::os::unix::fs::symlink(&f.victim, &bak).unwrap();
        let res = restore_latest_backup(&f.root, &f.target);
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert_eq!(fs::read(&f.target).unwrap(), b"v1");
        fs::remove_file(&bak).unwrap();
        fs::write(&bak, b"v0").unwrap();
        fs::remove_file(&f.target).unwrap();
        std::os::unix::fs::symlink(&f.victim, &f.target).unwrap();
        let res = restore_latest_backup(&f.root, &f.target);
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert_eq!(fs::read(&bak).unwrap(), b"v0");
    }

    #[test]
    fn convert_refuses_an_original_or_destination_that_is_a_link() {
        let f = link_fixture();
        let cbr = f.series.join("other.cbr");
        std::os::unix::fs::symlink(&f.victim, &cbr).unwrap();
        let res = convert_atomic(&f.root, &cbr, &f.series.join("other.cbz"), |io| {
            put(io, b"z")
        });
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        let real = f.series.join("real.cbr");
        fs::write(&real, b"rar").unwrap();
        let dst = f.series.join("real.cbz");
        std::os::unix::fs::symlink(&f.victim, &dst).unwrap();
        let res = convert_atomic(&f.root, &real, &dst, |io| put(io, b"z"));
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        assert_eq!(fs::read(&real).unwrap(), b"rar");
        assert_eq!(fs::read(&f.victim).unwrap(), b"outside-bytes");
        assert_eq!(names_in(&f.outside), ["issue.cbz"]);
    }

    #[test]
    fn rename_into_place_refuses_links_and_keeps_an_existing_destination() {
        let f = link_fixture();
        let src = f.series.join("zip.cbr");
        std::os::unix::fs::symlink(&f.victim, &src).unwrap();
        let res = rename_into_place(&f.root, &src, &f.series.join("zip.cbz"));
        assert!(
            matches!(res, Err(RewriteError::Refused(Refusal::Symlink))),
            "{res:?}"
        );
        fs::remove_file(&src).unwrap();
        fs::write(&src, b"PK").unwrap();
        // Onto an existing name: refused, both files exactly as they were.
        let res = rename_into_place(&f.root, &src, &f.target);
        assert!(
            matches!(res, Err(RewriteError::DestinationExists(_))),
            "{res:?}"
        );
        assert!(res.unwrap_err().is_refusal());
        assert_eq!(fs::read(&src).unwrap(), b"PK");
        assert_eq!(fs::read(&f.target).unwrap(), b"v1");
        // Onto a free name: the file has both names until `finish`.
        let zip = f.series.join("zip.cbz");
        let converted = rename_into_place(&f.root, &src, &zip).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"PK", "old name kept until finish");
        assert_eq!(fs::read(&zip).unwrap(), b"PK");
        converted.finish();
        assert!(!src.exists());
        assert_eq!(fs::read(&zip).unwrap(), b"PK");
    }

    // ── conversion: never over another file, never without a file ──

    /// Every name in `dir` that is a staging file.
    fn staging_files(dir: &Path) -> Vec<String> {
        names_in(dir)
            .into_iter()
            .filter(|n| is_staging_name(n))
            .collect()
    }

    #[test]
    fn convert_refuses_an_existing_destination_and_changes_neither_file() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("issue.cbr");
        let other = dir.path().join("issue.cbz");
        fs::write(&original, b"rar-bytes").unwrap();
        fs::write(&other, b"another issue's cbz").unwrap();
        let mut ran = false;
        let res = convert_atomic(&root(&dir), &original, &other, |io| {
            ran = true;
            put(io, b"zip-bytes")
        });
        assert!(
            matches!(res, Err(RewriteError::DestinationExists(_))),
            "{res:?}"
        );
        assert!(!ran, "nothing is written when the name is taken");
        assert_eq!(fs::read(&original).unwrap(), b"rar-bytes");
        assert_eq!(fs::read(&other).unwrap(), b"another issue's cbz");
        assert_eq!(names_in(dir.path()), ["issue.cbr", "issue.cbz"]);
    }

    /// The destination appears while the new archive is being written (the
    /// check before writing passed): the final no-replace rename refuses,
    /// both files stay as they are, and the staging file is removed.
    #[test]
    fn convert_never_replaces_a_destination_that_appears_mid_way() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("issue.cbr");
        let dst = dir.path().join("issue.cbz");
        fs::write(&original, b"rar-bytes").unwrap();
        let res = convert_atomic(&root(&dir), &original, &dst, |io| {
            fs::write(&dst, b"arrived meanwhile").unwrap();
            put(io, b"zip-bytes")
        });
        assert!(
            matches!(res, Err(RewriteError::DestinationExists(_))),
            "{res:?}"
        );
        assert_eq!(fs::read(&dst).unwrap(), b"arrived meanwhile");
        assert_eq!(fs::read(&original).unwrap(), b"rar-bytes");
        assert!(staging_files(dir.path()).is_empty());
    }

    /// A crash (or error) after each step of a conversion to a new name:
    /// the original's bytes are always at its own name (and at `.bak` once
    /// the backup was made), the new name holds either nothing or the whole
    /// new file, and nothing else is left over. The guard removes the
    /// staging file on the way out; a real crash would leave only that.
    #[test]
    fn convert_crash_at_every_step_keeps_the_original_at_its_name() {
        for step in [
            RewriteStep::Staged,
            RewriteStep::Rotated,
            RewriteStep::BackedUp,
            RewriteStep::Swapped,
        ] {
            let dir = TempDir::new().unwrap();
            let original = dir.path().join("issue.cbr");
            let dst = dir.path().join("issue.cbz");
            fs::write(&original, b"rar-bytes").unwrap();
            fs::write(dir.path().join("issue.cbr.bak"), b"older-bytes").unwrap();
            let res = convert_atomic_with_faults(
                &root(&dir),
                &original,
                &dst,
                |io| put(io, b"zip-bytes"),
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
            assert_eq!(
                fs::read(&original).unwrap(),
                b"rar-bytes",
                "original at its name after {step:?}"
            );
            let bak = fs::read(dir.path().join("issue.cbr.bak")).ok();
            // The older backup stays until the new one replaces it in one
            // rename: never neither.
            match step {
                RewriteStep::Staged | RewriteStep::Rotated => {
                    assert_eq!(bak.as_deref(), Some(&b"older-bytes"[..]), "{step:?}");
                }
                _ => assert_eq!(bak.as_deref(), Some(&b"rar-bytes"[..]), "{step:?}"),
            }
            if step == RewriteStep::Swapped {
                assert_eq!(fs::read(&dst).unwrap(), b"zip-bytes");
            } else {
                assert!(!dst.exists(), "no half file at the new name after {step:?}");
            }
            assert!(
                staging_files(dir.path()).is_empty(),
                "staging removed after {step:?}"
            );
        }
    }

    /// Between the conversion and `finish` (the window in which the caller
    /// repoints its row) both names hold a file; `abandon` undoes the
    /// conversion and leaves the original as the only copy at its name.
    #[test]
    fn convert_abandon_leaves_the_original_as_it_was() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("issue.cbr");
        let dst = dir.path().join("issue.cbz");
        fs::write(&original, b"rar-bytes").unwrap();
        let converted =
            convert_atomic(&root(&dir), &original, &dst, |io| put(io, b"zip-bytes")).unwrap();
        converted.abandon();
        assert_eq!(fs::read(&original).unwrap(), b"rar-bytes");
        assert!(!dst.exists());
        // A retry starts from the same state and succeeds.
        convert_atomic(&root(&dir), &original, &dst, |io| put(io, b"zip-bytes"))
            .unwrap()
            .finish();
        assert_eq!(fs::read(&dst).unwrap(), b"zip-bytes");
        assert_eq!(
            fs::read(dir.path().join("issue.cbr.bak")).unwrap(),
            b"rar-bytes"
        );
        assert!(!original.exists());
    }

    /// In place (a RAR named `.cbz`): the rewrite sequence, so the name is
    /// never missing and the original lands in `.bak`.
    #[test]
    fn convert_in_place_keeps_the_name_and_backs_up_the_original() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"rar-bytes").unwrap();
        let outcome = convert_atomic(&root(&dir), &target, &target, |io| put(io, b"zip-bytes"))
            .unwrap()
            .finish();
        assert_eq!(fs::read(&target).unwrap(), b"zip-bytes");
        assert_eq!(fs::read(outcome.backup.unwrap()).unwrap(), b"rar-bytes");
    }

    #[test]
    fn settle_follows_what_the_row_names_and_leaves_both_when_unsure() {
        let orig = Path::new("/lib/S/x.cbr");
        let dst = Path::new("/lib/S/x.cbz");
        let ok = |p: &str| Ok::<_, ()>(Some(p.to_owned()));
        assert_eq!(settle_for(ok("/lib/S/x.cbz"), orig, dst), Settle::Finish);
        assert_eq!(settle_for(ok("/lib/S/x.cbr"), orig, dst), Settle::Abandon);
        assert_eq!(settle_for(ok("/lib/S/y.cbz"), orig, dst), Settle::Leave);
        assert_eq!(settle_for(Ok::<_, ()>(None), orig, dst), Settle::Leave);
        assert_eq!(
            settle_for(Err::<Option<String>, _>(()), orig, dst),
            Settle::Leave
        );
    }

    // ── staging files never outlive a failure ──

    #[test]
    fn a_failed_rewrite_leaves_no_staging_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        // The writer fails after writing.
        let res = rewrite_atomic(&root(&dir), &target, 1, |io| {
            put(io, b"partial")?;
            Err(RewriteError::ValidationFailed("synthetic".into()))
        });
        assert!(res.is_err());
        assert!(staging_files(dir.path()).is_empty());
        // A fault after the staging file is complete and synced.
        let res = rewrite_atomic_with_faults(
            &root(&dir),
            &target,
            1,
            |io| put(io, b"v2"),
            |at| {
                if at == RewriteStep::BackedUp {
                    Err(RewriteError::Io(io::Error::other("injected")))
                } else {
                    Ok(())
                }
            },
        );
        assert!(res.is_err());
        assert!(staging_files(dir.path()).is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"v1");
        // A panic inside the writer (as in a `spawn_blocking` task).
        let r = root(&dir);
        let t = target.clone();
        let caught = std::panic::catch_unwind(move || {
            let _ = rewrite_atomic(&r, &t, 1, |io| {
                put(io, b"half").unwrap();
                panic!("writer panicked");
            });
        });
        assert!(caught.is_err());
        assert!(staging_files(dir.path()).is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"v1");
    }

    // ── the folder fsync after the swap ──

    /// The new file is in place when the folder fsync after the swap
    /// fails: the call returns `Ok` (a retry would rotate the backup slots
    /// again and, with one slot, delete the only copy of the original), and
    /// `.bak` still holds the original.
    #[test]
    fn a_failed_sync_after_the_swap_is_ok_and_keeps_the_backup() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("issue.cbz");
        fs::write(&target, b"v1").unwrap();
        let failing_sync = |at| {
            if at == RewriteStep::FinalSync {
                Err(RewriteError::Io(io::Error::other("fsync failed")))
            } else {
                Ok(())
            }
        };
        let outcome =
            rewrite_atomic_with_faults(&root(&dir), &target, 1, |io| put(io, b"v2"), failing_sync)
                .expect("the work is done; not an error");
        assert_eq!(fs::read(&target).unwrap(), b"v2");
        assert_eq!(fs::read(outcome.backup.unwrap()).unwrap(), b"v1");

        let original = dir.path().join("other.cbr");
        let dst = dir.path().join("other.cbz");
        fs::write(&original, b"rar").unwrap();
        let converted = convert_atomic_with_faults(
            &root(&dir),
            &original,
            &dst,
            |io| put(io, b"zip"),
            failing_sync,
        )
        .expect("the conversion is done; not an error");
        assert_eq!(fs::read(&dst).unwrap(), b"zip");
        assert_eq!(fs::read(dir.path().join("other.cbr.bak")).unwrap(), b"rar");
        converted.finish();
    }

    // ── file modes ──

    /// A backup made by copying (hard links refused) keeps the original's
    /// permission bits, as `fs::copy` did; the rewritten file gets the mode
    /// any newly created file gets (0o666 less the umask), as before
    /// handles were used.
    #[cfg(unix)]
    #[test]
    fn copied_backup_keeps_the_original_mode_and_new_files_get_the_default() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("issue.cbz");
        fs::write(&src, b"payload").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(dir.path().join("issue.cbz.bak"), b"stale").unwrap();
        let (folder, name) = root(&dir).open_parent(&src).unwrap();
        let original = ArchiveFile::new(folder.open_regular(&name).unwrap(), &src);
        link_or_copy(&folder, &name, OsStr::new("issue.cbz.bak"), &original, None).unwrap();
        let bak = dir.path().join("issue.cbz.bak");
        assert_eq!(fs::read(&bak).unwrap(), b"payload");
        assert_eq!(
            fs::metadata(&bak).unwrap().permissions().mode() & 0o7777,
            0o640
        );

        let probe = dir.path().join("probe");
        fs::File::create(&probe).unwrap();
        let default_mode = fs::metadata(&probe).unwrap().permissions().mode() & 0o7777;
        rewrite_atomic(&root(&dir), &src, 1, |io| put(io, b"v2")).unwrap();
        assert_eq!(
            fs::metadata(&src).unwrap().permissions().mode() & 0o7777,
            default_mode
        );
    }

    #[test]
    fn startup_cleanup_never_removes_through_a_link() {
        let f = link_fixture();
        let hex = "0192f3a4b5c6d7e8f90a1b2c3d4e5f60";
        let staged_name = format!("issue.cbz.{hex}.tmp");
        // Outside the root, a file with a staging name; inside, a link to it
        // under the same name and a linked folder leading to it.
        let outside_staged = f.outside.join(&staged_name);
        fs::write(&outside_staged, b"not ours").unwrap();
        std::os::unix::fs::symlink(&outside_staged, f.series.join(&staged_name)).unwrap();
        std::os::unix::fs::symlink(&f.outside, f.series.join("Linked")).unwrap();
        // Both the file and the link itself look old enough to reap.
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&outside_staged)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let secs = old
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let ts = rustix::fs::Timespec {
            tv_sec: secs,
            tv_nsec: 0,
        };
        rustix::fs::utimensat(
            rustix::fs::CWD,
            f.series.join(&staged_name),
            &rustix::fs::Timestamps {
                last_access: ts,
                last_modification: ts,
            },
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
        let removed = startup_cleanup(
            [f.root.configured.clone()],
            std::time::Duration::from_secs(60),
        );
        assert_eq!(removed, 0, "a link at a staging name is not removed");
        assert!(outside_staged.exists());
        assert!(
            fs::symlink_metadata(f.series.join(&staged_name)).is_ok(),
            "the link is left for the operator"
        );
    }

    // ── what counts as a backup, and how old it is ──

    #[test]
    fn is_backup_name_accepts_every_backup_folio_writes() {
        for ext in ["cbz", "cbt", "cbr", "cb7", "CBZ", "Cbr"] {
            let archive = format!("Saga (2012) 001.{ext}");
            for slot in 0..=MAX_BACKUP_SLOT {
                let p = backup_slot(Path::new(&archive), slot);
                let name = p.to_str().unwrap();
                assert!(is_backup_name(name), "{name}");
            }
        }
        // Pre-conversion names: the converted `.cbz` sits beside them.
        for name in ["issue.cbr.bak", "issue.cb7.bak", "issue.cbz.bak"] {
            assert!(is_backup_name(name), "{name}");
        }
    }

    #[test]
    fn is_backup_name_rejects_what_folio_never_writes() {
        for name in [
            "notes.bak",
            "photo.jpg.bak",
            "X.cbz.bak.txt",
            "X.cbz.bak.01",
            "X.cbz.bak.0",
            "X.cbz.bak.6",
            "X.cbz.bak.",
            "X.cbz.bak.1x",
            ".cbz.bak",
            "X.zip.bak",
            "X.cbz.BAK",
            "X.bak.1",
            "bak",
            ".bak",
            "X.cbz",
            "X.cbz.bak.bak",
        ] {
            assert!(!is_backup_name(name), "{name}");
        }
    }

    #[test]
    fn made_at_is_the_ctime_unless_it_cannot_be_trusted() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let earlier = t - std::time::Duration::from_secs(400 * 86_400);
        // A hard link of an old file: old mtime, fresh ctime.
        assert_eq!(made_at(Some(t), Some(earlier)), Some(t));
        assert_eq!(made_at(Some(t), Some(t)), Some(t));
        assert_eq!(made_at(Some(t), None), Some(t));
        // No ctime, or a ctime before the mtime: unknown.
        assert_eq!(made_at(None, Some(earlier)), None);
        assert_eq!(made_at(Some(earlier), Some(t)), None);
    }

    /// The copy fallback (hard links refused) makes a new file: its age, too,
    /// counts from when the backup was made, not from the original's old
    /// mtime, and it is pruned once that is past the retention.
    #[test]
    fn copied_backup_ages_from_when_it_was_made() {
        use std::os::unix::fs::MetadataExt;
        let day = std::time::Duration::from_secs(86_400);
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("issue.cbz");
        fs::write(&src, b"payload").unwrap();
        let old = filetime::FileTime::from_system_time(SystemTime::now() - 400 * day);
        filetime::set_file_mtime(&src, old).unwrap();
        // A stale slot makes `hard_link` fail with EEXIST → copy path.
        let bak = dir.path().join("issue.cbz.bak");
        fs::write(&bak, b"stale").unwrap();
        let (folder, name) = root(&dir).open_parent(&src).unwrap();
        let original = ArchiveFile::new(folder.open_regular(&name).unwrap(), &src);
        link_or_copy(&folder, &name, OsStr::new("issue.cbz.bak"), &original, None).unwrap();
        assert_ne!(
            fs::metadata(&bak).unwrap().ino(),
            fs::metadata(&src).unwrap().ino(),
            "the backup is a copy"
        );
        assert_eq!(fs::read(&bak).unwrap(), b"payload");

        let (stats, _) =
            crate::jobs::backup_prune::prune_library(dir.path(), 30, SystemTime::now() + day);
        assert_eq!(stats.removed, 0, "{stats:?}");
        assert!(
            bak.exists(),
            "a copy made today survives a 30-day retention"
        );
        let (stats, _) =
            crate::jobs::backup_prune::prune_library(dir.path(), 30, SystemTime::now() + 31 * day);
        assert_eq!(stats.removed, 1, "{stats:?}");
        assert!(!bak.exists());
        assert!(src.exists(), "the archive is never touched");
    }

    #[test]
    fn a_link_must_read_as_made_now_against_the_reference() {
        let r = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let secs = std::time::Duration::from_secs;
        // A link on a filesystem that sets ctime: at or after the reference.
        assert!(link_ages_from_now(Some(r), r));
        assert!(link_ages_from_now(Some(r + secs(30)), r));
        // Timestamp granularity: up to the slack before the reference.
        assert!(link_ages_from_now(Some(r - secs(1)), r));
        assert!(link_ages_from_now(Some(r - LINK_AGE_SLACK), r));
        // sshfs-like: the link reads the original's (old) time.
        assert!(!link_ages_from_now(Some(r - LINK_AGE_SLACK - secs(1)), r));
        assert!(!link_ages_from_now(Some(r - secs(400 * 86_400)), r));
        // Unknown age (no ctime, or a ctime before a future mtime).
        assert!(!link_ages_from_now(None, r));
    }

    /// The fallback itself, driven by a reference the link cannot meet
    /// (the real filesystem here sets ctime correctly, so a filesystem that
    /// does not cannot be reproduced; this is the closest stand-in): the
    /// backup name alone is unlinked, the copy path runs, and the original
    /// keeps its name, bytes and link count.
    #[test]
    fn a_link_that_does_not_age_from_now_is_replaced_by_a_copy() {
        use std::os::unix::fs::MetadataExt;
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("issue.cbz");
        fs::write(&src, b"payload").unwrap();
        let src_ino = fs::metadata(&src).unwrap().ino();
        let (folder, name) = root(&dir).open_parent(&src).unwrap();
        let original = ArchiveFile::new(folder.open_regular(&name).unwrap(), &src);
        let unreachable = SystemTime::now() + std::time::Duration::from_secs(86_400);
        link_or_copy(
            &folder,
            &name,
            OsStr::new("issue.cbz.bak"),
            &original,
            Some(unreachable),
        )
        .unwrap();
        let bak = dir.path().join("issue.cbz.bak");
        assert_ne!(fs::metadata(&bak).unwrap().ino(), src_ino, "a copy");
        assert_eq!(fs::read(&bak).unwrap(), b"payload");
        let src_meta = fs::metadata(&src).unwrap();
        assert_eq!(src_meta.ino(), src_ino, "the original keeps its file");
        assert_eq!(src_meta.nlink(), 1, "the link was removed");
        assert_eq!(fs::read(&src).unwrap(), b"payload");

        // Control: a reachable reference keeps the hard link.
        fs::remove_file(&bak).unwrap();
        link_or_copy(
            &folder,
            &name,
            OsStr::new("issue.cbz.bak"),
            &original,
            Some(SystemTime::now()),
        )
        .unwrap();
        assert_eq!(fs::metadata(&bak).unwrap().ino(), src_ino, "a hard link");
    }

    /// The "not the original's file" branch: a different file, or a link,
    /// at the backup name when the fresh link is looked at is an error (the
    /// rewrite stops before the swap); the real link is the control.
    #[test]
    fn a_backup_name_that_is_not_the_originals_file_is_an_error() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("issue.cbz");
        fs::write(&src, b"payload").unwrap();
        let (folder, name) = root(&dir).open_parent(&src).unwrap();
        let original = ArchiveFile::new(folder.open_regular(&name).unwrap(), &src);
        let now = SystemTime::now();

        // Control: the link itself.
        folder.hard_link(&name, OsStr::new("a.cbz.bak")).unwrap();
        assert!(
            linked_backup_ages_from_now(&folder, OsStr::new("a.cbz.bak"), &original, now).unwrap()
        );

        // Another file at the name.
        fs::write(dir.path().join("b.cbz.bak"), b"payload").unwrap();
        assert!(matches!(
            linked_backup_ages_from_now(&folder, OsStr::new("b.cbz.bak"), &original, now),
            Err(RewriteError::Io(_))
        ));
        // A link to the original at the name: not followed, not the file.
        std::os::unix::fs::symlink(&src, dir.path().join("c.cbz.bak")).unwrap();
        assert!(matches!(
            linked_backup_ages_from_now(&folder, OsStr::new("c.cbz.bak"), &original, now),
            Err(RewriteError::Io(_))
        ));
        // Nothing at the name.
        assert!(matches!(
            linked_backup_ages_from_now(&folder, OsStr::new("d.cbz.bak"), &original, now),
            Err(RewriteError::Io(_))
        ));
        assert_eq!(fs::read(&src).unwrap(), b"payload");
    }
}
