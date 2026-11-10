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
    #[error("target path has no parent directory: {0}")]
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
}

impl RewriteError {
    /// True for [`RewriteError::Refused`].
    pub fn is_refusal(&self) -> bool {
        matches!(self, Self::Refused(_))
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
    /// `<target>.tmp` renamed over `target`; the parent dir is not yet
    /// fsynced.
    Swapped,
}

/// The target's folder and name, opened through the no-follow walk.
fn open_target(root: &LibraryRoot, target: &Path) -> Result<(LibraryDir, OsString), RewriteError> {
    if target.file_name().is_none() {
        return Err(RewriteError::NoParent(target.to_path_buf()));
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
    let staging = dir.create_new(&tmp_name)?;
    let staging_path = dir.display(&tmp_name);

    write_into(&RewriteIo {
        source: original.as_ref(),
        staging: &staging,
        staging_path: &staging_path,
    })?;
    staging.sync_all()?;
    drop(staging);
    dir.sync()?;
    fault(RewriteStep::Staged)?;

    confirm_unchanged(&dir, &name, original.as_ref())?;
    let backup = match (&original, retain_count > 0) {
        (Some(original), true) => {
            rotate_backups(&dir, &name, retain_count)?;
            fault(RewriteStep::Rotated)?;
            let slot0 = backup_slot_name(&name, 0);
            link_or_copy(&dir, &name, &slot0, original)?;
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
    fault(RewriteStep::Swapped)?;
    dir.sync()?;

    Ok(RewriteOutcome {
        target: target.to_path_buf(),
        backup,
    })
}

/// Make `dst` a second name for the original at `src` (both in `dir`)
/// without ever removing `src`: a hard link when the filesystem supports
/// one (same inode, zero extra bytes, instant; `linkat` without
/// `AT_SYMLINK_FOLLOW`), else a full byte copy read from the original's
/// handle into a file created exclusively at `dst` (cross-device layouts,
/// NAS / SMB / FAT mounts, or filesystems that refuse links on the file —
/// `EXDEV`, `EPERM`, `ENOTSUP`, `EMLINK`). Either way `src` is untouched,
/// which is what keeps the target readable through the whole swap. `dst`
/// must not exist (the rotation freed the slot).
fn link_or_copy(
    dir: &LibraryDir,
    src: &OsStr,
    dst: &OsStr,
    original: &ArchiveFile,
) -> Result<(), RewriteError> {
    match dir.hard_link(src, dst) {
        Ok(()) => Ok(()),
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
            let mut out = dir.create_new(dst)?;
            io::copy(&mut original.reader(), &mut out)?;
            out.sync_all()?;
            Ok(())
        }
    }
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

/// Atomically convert one archive into another under a *different* name in
/// the same folder — the CBR→CBZ edit path (`archive-rewrite-1.0` M6).
/// Unlike [`rewrite_atomic`], the original (`original`, e.g. `foo.cbr`)
/// and the destination (`dst`, e.g. `foo.cbz`) have different names. Same
/// folder-handle discipline as [`rewrite_atomic`] (no link followed below
/// `root`, every step relative to the folder handle, the original read
/// through its handle, a staging file created exclusively):
///
///   1. Caller reads the original through its handle and writes the new
///      archive into the staging file `<dst>.<hex>.tmp`.
///   2. fsync the staging file + the folder.
///   3. Rename `original` → `<original>.bak` (the `.cbr` is kept as the
///      single rollback slot; an existing `.bak` is overwritten).
///   4. Rename the staging file → `dst`.
///   5. fsync the folder.
///
/// The original or the destination being a link is refused. On failure
/// before step 3 the original is untouched (worst case an orphan
/// `<dst>.<hex>.tmp` staging file, reaped by [`startup_cleanup`]). Returns
/// the `.bak` path of the preserved original.
pub fn convert_atomic<F>(
    root: &LibraryRoot,
    original: &Path,
    dst: &Path,
    write_into: F,
) -> Result<RewriteOutcome, RewriteError>
where
    F: FnOnce(&RewriteIo<'_>) -> Result<(), RewriteError>,
{
    let (dir, orig_name) = open_target(root, original)?;
    let dst_name = match (dst.parent(), dst.file_name()) {
        (Some(p), Some(n)) if Some(p) == original.parent() => n.to_os_string(),
        _ => return Err(RewriteError::NoParent(dst.to_path_buf())),
    };
    let source = open_original(&dir, &orig_name, original)?;
    match dir.kind(&dst_name)? {
        EntryKind::Symlink => return Err(RewriteError::Refused(Refusal::Symlink)),
        EntryKind::Other => return Err(RewriteError::Refused(Refusal::NotRegular)),
        EntryKind::Missing | EntryKind::Regular => {}
    }
    let tmp_name = staging_name(&dst_name);
    let staging = dir.create_new(&tmp_name)?;
    let staging_path = dir.display(&tmp_name);

    write_into(&RewriteIo {
        source: source.as_ref(),
        staging: &staging,
        staging_path: &staging_path,
    })?;
    staging.sync_all()?;
    drop(staging);
    dir.sync()?;

    let backup = backup_slot_name(&orig_name, 0);
    if source.is_some() {
        confirm_unchanged(&dir, &orig_name, source.as_ref())?;
        // rename overwrites any prior .bak atomically.
        dir.rename(&orig_name, &backup)?;
    }
    if dir.kind(&dst_name)? == EntryKind::Symlink {
        return Err(RewriteError::Refused(Refusal::Symlink));
    }
    dir.rename(&tmp_name, &dst_name)?;
    dir.sync()?;

    Ok(RewriteOutcome {
        target: dst.to_path_buf(),
        backup: Some(dir.display(&backup)),
    })
}

/// Move `src` to `dst` (same folder) — a ZIP that only wore another
/// extension, moved into place byte-for-byte. Relative to the folder
/// handle from the no-follow walk; `src` must be a regular file and `dst`
/// must not exist (a link at either is refused). Returns `false` when
/// `dst` already exists. The rename is atomic, so a crash can't leave a
/// half-file.
pub fn rename_into_place(root: &LibraryRoot, src: &Path, dst: &Path) -> Result<bool, RewriteError> {
    let (dir, src_name) = open_target(root, src)?;
    let dst_name = match (dst.parent(), dst.file_name()) {
        (Some(p), Some(n)) if Some(p) == src.parent() => n.to_os_string(),
        _ => return Err(RewriteError::NoParent(dst.to_path_buf())),
    };
    if open_original(&dir, &src_name, src)?.is_none() {
        return Err(Refusal::Missing.into());
    }
    match dir.kind(&dst_name)? {
        EntryKind::Missing => {}
        EntryKind::Symlink => return Err(RewriteError::Refused(Refusal::Symlink)),
        _ => return Ok(false),
    }
    dir.rename(&src_name, &dst_name)?;
    dir.sync()?;
    Ok(true)
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
        link_or_copy(&folder, &name, OsStr::new("issue.cbz.bak"), &original).unwrap();
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

        let outcome = convert_atomic(&root(&dir), &original, &dst, |io| {
            let mut read = Vec::new();
            io::Read::read_to_end(&mut io.source()?.reader(), &mut read)?;
            assert_eq!(
                read, b"rar-bytes",
                "the closure reads the original's handle"
            );
            put(io, b"zip-bytes")
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
        assert!(
            !rename_into_place(&f.root, &src, &f.target).unwrap(),
            "issue.cbz exists"
        );
        assert!(rename_into_place(&f.root, &src, &f.series.join("zip.cbz")).unwrap());
        assert_eq!(fs::read(f.series.join("zip.cbz")).unwrap(), b"PK");
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
}
