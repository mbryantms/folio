//! Opening library files without following symbolic links.
//!
//! Folio never follows a symbolic link below a library root: the scanner
//! skips them (`scanner::enumerate`, reported as
//! [`IssueKind::SymlinkSkipped`](crate::library::health::IssueKind::SymlinkSkipped)),
//! and every open of a file under a library root goes through this module.
//! The library root's own path may contain links (a mount point or
//! `/data/comics -> /mnt/x` is a deployment choice): it is resolved once,
//! exactly as `validate_library` canonicalizes it, and nothing below it is
//! followed.
//!
//! [`open_parent_below_roots`] walks from the root directory one path
//! component at a time with `openat(…, O_NOFOLLOW | O_DIRECTORY)`, so a
//! link anywhere below the root (a linked series folder, a linked publisher
//! folder) is refused, and a link swapped in during the walk is refused
//! too: each step opens a name relative to the directory handle the
//! previous step proved, never a path the kernel resolves again. It returns
//! that folder as a [`LibraryDir`], and every later operation names an
//! entry relative to it:
//!
//! - **Reads** ([`open_below_roots`], [`LibraryDir::open_regular`]): the
//!   final open adds `O_NOFOLLOW | O_NONBLOCK` (a FIFO cannot hang it) and
//!   the handle is `fstat`-ed, so only a regular file is returned. Callers
//!   read through that handle and never reopen the path; the archive crate
//!   takes it as an [`archive::ArchiveFile`] ([`open_issue_archive`],
//!   [`LibraryRoot::open_archive`]).
//! - **Writes** ([`LibraryDir::create_new`], [`LibraryDir::rename`],
//!   [`LibraryDir::hard_link`], [`LibraryDir::remove_file`],
//!   [`LibraryDir::sync`]): `openat` with `O_CREAT | O_EXCL | O_NOFOLLOW`,
//!   `renameat`, `linkat` without `AT_SYMLINK_FOLLOW`, `unlinkat`, `fsync`
//!   of the folder handle. `crate::archive_rewrite` builds the archive
//!   write sequence on these.
//!
//! Hard links are not refused: they cannot be told apart from ordinary
//! files.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::state::AppState;

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

/// Why a stored path was not opened.
#[derive(Debug)]
pub enum Refusal {
    /// The path does not lie below the library root (or names the root).
    OutsideRoot,
    /// The file or a folder on its path does not exist.
    Missing,
    /// The file, or a folder between the root and it, is a symbolic link.
    Symlink,
    /// The entry exists but is not a regular file (directory, FIFO,
    /// socket, device).
    NotRegular,
    /// Any other error from the filesystem.
    Io(std::io::Error),
    /// The library's root could not be looked up (a database error). Not an
    /// answer about the file: a request answers 5xx, a job retries.
    Lookup(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideRoot => f.write_str("path is not below the library root"),
            Self::Missing => f.write_str("file is missing"),
            Self::Symlink => f.write_str("path goes through a symbolic link"),
            Self::NotRegular => f.write_str("not a regular file"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::Lookup(e) => write!(f, "library lookup failed: {e}"),
        }
    }
}

/// Classify a failed `openat` of `name` in `dir`: a link (ELOOP from
/// `O_NOFOLLOW`, or ENOTDIR when `O_DIRECTORY` met a link first) is
/// reported as such; not-found is missing.
fn refusal_for(dir: &OwnedFd, name: &OsStr, err: Errno) -> Refusal {
    if let Ok(st) = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
        && FileType::from_raw_mode(st.st_mode).is_symlink()
    {
        return Refusal::Symlink;
    }
    match err {
        Errno::LOOP => Refusal::Symlink,
        Errno::NOENT | Errno::NOTDIR => Refusal::Missing,
        e => Refusal::Io(e.into()),
    }
}

/// Open the regular file at `path`, which must lie below `root`, without
/// following a symbolic link at or below the root. Blocking: run it on the
/// blocking pool.
pub fn open_below_root(root: &Path, path: &Path) -> Result<File, Refusal> {
    open_below_roots(root, None, path)
}

/// [`open_below_root`] that also accepts `path` spelled under the root's
/// canonical path (`root_canon`, the root with its own links resolved).
/// Rows written by an older single-series rescan of a library whose root is
/// a link carry that spelling. Either way the walk starts at the opened
/// root and refuses every link below it.
pub fn open_below_roots(
    root: &Path,
    root_canon: Option<&Path>,
    path: &Path,
) -> Result<File, Refusal> {
    let (dir, file_name) = open_parent_below_roots(root, root_canon, path)?;
    dir.open_regular(&file_name)
}

/// [`open_below_roots`] for a caller that has only the configured root:
/// the canonical spelling is resolved only when `path` is not spelled under
/// the configured root.
pub fn open_library_file(root: &Path, path: &Path) -> Result<File, Refusal> {
    match open_below_roots(root, None, path) {
        Err(Refusal::OutsideRoot) => {
            let canon = std::fs::canonicalize(root).map_err(|_| Refusal::OutsideRoot)?;
            open_below_roots(root, Some(&canon), path)
        }
        other => other,
    }
}

/// The relative components of `path` below `root` (or `root_canon`), all
/// plain names, at least one.
fn names_below<'a>(
    root: &Path,
    root_canon: Option<&Path>,
    path: &'a Path,
) -> Result<Vec<&'a OsStr>, Refusal> {
    let rel = path
        .strip_prefix(root)
        .ok()
        .or_else(|| root_canon.and_then(|c| path.strip_prefix(c).ok()))
        .ok_or(Refusal::OutsideRoot)?;
    let mut names = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => names.push(n),
            _ => return Err(Refusal::OutsideRoot),
        }
    }
    if names.is_empty() {
        return Err(Refusal::OutsideRoot);
    }
    Ok(names)
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);

/// Open the library root directory itself. Its own links are followed
/// (the deployment rule).
fn open_root_dir(root: &Path) -> Result<OwnedFd, Refusal> {
    rustix::fs::openat(rustix::fs::CWD, root, DIR_FLAGS, Mode::empty()).map_err(|e| match e {
        Errno::NOENT | Errno::NOTDIR => Refusal::Missing,
        e => Refusal::Io(e.into()),
    })
}

/// Walk `folders` from `dir`, one `O_NOFOLLOW` directory open at a time.
fn walk_folders(mut dir: OwnedFd, folders: &[&OsStr]) -> Result<OwnedFd, Refusal> {
    for name in folders {
        dir = rustix::fs::openat(&dir, *name, DIR_FLAGS | OFlags::NOFOLLOW, Mode::empty())
            .map_err(|e| refusal_for(&dir, name, e))?;
    }
    Ok(dir)
}

/// Open the directory that holds `path` (which must lie below `root`, or
/// below `root_canon`) through the same no-follow walk, and return it with
/// the final name. The final name itself is not looked at: the returned
/// [`LibraryDir`] checks it per operation. The directory may be the root
/// itself. Blocking.
pub fn open_parent_below_roots(
    root: &Path,
    root_canon: Option<&Path>,
    path: &Path,
) -> Result<(LibraryDir, OsString), Refusal> {
    let names = names_below(root, root_canon, path)?;
    let Some((file_name, folders)) = names.split_last() else {
        return Err(Refusal::OutsideRoot);
    };
    let dir = walk_folders(open_root_dir(root)?, folders)?;
    let shown = path.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok((
        LibraryDir {
            fd: dir,
            path: shown,
        },
        file_name.to_os_string(),
    ))
}

/// [`open_parent_below_roots`] for a caller that has only the configured
/// root (see [`open_library_file`]).
pub fn open_library_parent(root: &Path, path: &Path) -> Result<(LibraryDir, OsString), Refusal> {
    match open_parent_below_roots(root, None, path) {
        Err(Refusal::OutsideRoot) => {
            let canon = std::fs::canonicalize(root).map_err(|_| Refusal::OutsideRoot)?;
            open_parent_below_roots(root, Some(&canon), path)
        }
        other => other,
    }
}

/// A directory below (or at) a library root, opened by the no-follow walk.
/// Every operation names an entry relative to this handle (`openat`,
/// `renameat`, `linkat`, `unlinkat`), so a folder swapped for a link after
/// the walk cannot redirect it, and no operation follows a link in the
/// final name. Blocking.
#[derive(Debug)]
pub struct LibraryDir {
    fd: OwnedFd,
    /// The directory's path, for messages only; never opened.
    path: PathBuf,
}

/// What an entry of a [`LibraryDir`] is, looked at without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Missing,
    Regular,
    Symlink,
    Other,
}

/// What a no-follow `stat` of an existing entry says ([`LibraryDir::stat`]).
/// Taken without opening the entry: no read permission is needed, and no
/// file data is touched (an open can recall a file on tiered storage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    /// Never [`EntryKind::Missing`].
    pub kind: EntryKind,
    /// `(device, inode)`.
    pub identity: (u64, u64),
    /// Modification time; `None` at or before the epoch.
    pub mtime: Option<std::time::SystemTime>,
    /// Inode change time; `None` at or before the epoch.
    pub ctime: Option<std::time::SystemTime>,
}

/// A `stat` timestamp as a [`std::time::SystemTime`]; `None` at or before
/// the epoch.
fn stat_time(secs: i64, nsec: u32) -> Option<std::time::SystemTime> {
    u64::try_from(secs)
        .ok()
        .filter(|&s| s > 0)
        .map(|s| std::time::UNIX_EPOCH + Duration::new(s, nsec.min(999_999_999)))
}

impl LibraryDir {
    /// The directory's path, for messages.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `name`'s full path, for messages.
    pub fn display(&self, name: &OsStr) -> PathBuf {
        self.path.join(name)
    }

    /// What `name` is, without following it.
    pub fn kind(&self, name: &OsStr) -> std::io::Result<EntryKind> {
        match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(match FileType::from_raw_mode(st.st_mode) {
                FileType::RegularFile => EntryKind::Regular,
                FileType::Symlink => EntryKind::Symlink,
                _ => EntryKind::Other,
            }),
            Err(Errno::NOENT) => Ok(EntryKind::Missing),
            Err(e) => Err(e.into()),
        }
    }

    /// A no-follow `stat` of `name` (`fstatat` with `AT_SYMLINK_NOFOLLOW`
    /// through this directory's handle; the entry is never opened). `None`
    /// when nothing is at the name.
    pub fn stat(&self, name: &OsStr) -> std::io::Result<Option<EntryStat>> {
        let st = match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => st,
            Err(Errno::NOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let kind = match FileType::from_raw_mode(st.st_mode) {
            FileType::RegularFile => EntryKind::Regular,
            FileType::Symlink => EntryKind::Symlink,
            _ => EntryKind::Other,
        };
        Ok(Some(EntryStat {
            kind,
            identity: (st.st_dev, st.st_ino),
            mtime: stat_time(st.st_mtime, u32::try_from(st.st_mtime_nsec).unwrap_or(0)),
            ctime: stat_time(st.st_ctime, u32::try_from(st.st_ctime_nsec).unwrap_or(0)),
        }))
    }

    /// `(device, inode)` of `name`, without following it.
    pub fn identity(&self, name: &OsStr) -> std::io::Result<(u64, u64)> {
        let st = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        Ok((st.st_dev, st.st_ino))
    }

    /// Open `name` for reading: no link followed, regular files only.
    pub fn open_regular(&self, name: &OsStr) -> Result<File, Refusal> {
        Ok(self.open_regular_identified(name)?.0)
    }

    /// [`LibraryDir::open_regular`], plus the opened file's
    /// [`archive::FileStamp`] (device, inode, size, modification and
    /// status-change times) from the `fstat` the check already makes.
    pub fn open_regular_identified(
        &self,
        name: &OsStr,
    ) -> Result<(File, archive::FileStamp), Refusal> {
        // Stat before opening so a device or FIFO is never opened at all.
        let pre = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| refusal_for(&self.fd, name, e))?;
        match FileType::from_raw_mode(pre.st_mode) {
            FileType::RegularFile => {}
            FileType::Symlink => return Err(Refusal::Symlink),
            _ => return Err(Refusal::NotRegular),
        }
        // Swapped between the stat and the open: `O_NOFOLLOW` refuses a
        // link, `O_NONBLOCK` keeps a FIFO from blocking, the `fstat` below
        // refuses anything that is not a regular file.
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| refusal_for(&self.fd, name, e))?;
        let file = File::from(fd);
        let meta = file.metadata().map_err(Refusal::Io)?;
        if !meta.file_type().is_file() {
            return Err(Refusal::NotRegular);
        }
        Ok((file, archive::FileStamp::from_metadata(&meta)))
    }

    /// Create `name` as a new file, read-write. Exclusive (`O_EXCL`) and
    /// `O_NOFOLLOW`: anything already at the name, a link included, makes
    /// it fail instead of being opened or followed.
    pub fn create_new(&self, name: &OsStr) -> std::io::Result<File> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )?;
        Ok(File::from(fd))
    }

    /// Rename `from` to `to`, both in this directory. Neither name is
    /// followed; an entry at `to` is replaced atomically.
    pub fn rename(&self, from: &OsStr, to: &OsStr) -> std::io::Result<()> {
        rustix::fs::renameat(&self.fd, from, &self.fd, to)?;
        Ok(())
    }

    /// Rename `from` to `to` (both in this directory) only if nothing is at
    /// `to`; otherwise fail with `ErrorKind::AlreadyExists` and change
    /// nothing. On Linux this is `renameat2(RENAME_NOREPLACE)`, one atomic
    /// step. Where that is not available (another platform, or a
    /// filesystem that answers `EINVAL` / `ENOSYS` / `EOPNOTSUPP`, which
    /// some network filesystems do) it falls back to looking at `to`
    /// without following it and then renaming: an entry created at `to`
    /// between the two would be replaced.
    pub fn rename_noreplace(&self, from: &OsStr, to: &OsStr) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        match rustix::fs::renameat_with(
            &self.fd,
            from,
            &self.fd,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(()),
            Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
                tracing::debug!(
                    dir = %self.path.display(),
                    "RENAME_NOREPLACE not supported here; checking the destination before renaming",
                );
            }
            Err(e) => return Err(e.into()),
        }
        if self.kind(to)? != EntryKind::Missing {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        self.rename(from, to)
    }

    /// Swap the entries `a` and `b` (both in this directory, both must
    /// exist) in one atomic step: `renameat2(RENAME_EXCHANGE)` on Linux.
    /// Neither name is followed. `Ok(false)` when the swap is not available
    /// here (another platform, or a filesystem that answers `EINVAL` /
    /// `ENOSYS` / `EOPNOTSUPP`, as some network filesystems do); nothing was
    /// changed then, and the caller falls back to separate steps.
    pub fn exchange(&self, a: &OsStr, b: &OsStr) -> std::io::Result<bool> {
        #[cfg(target_os = "linux")]
        match rustix::fs::renameat_with(&self.fd, a, &self.fd, b, rustix::fs::RenameFlags::EXCHANGE)
        {
            Ok(()) => return Ok(true),
            Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
                tracing::debug!(
                    dir = %self.path.display(),
                    "RENAME_EXCHANGE not supported here; swapping in separate steps",
                );
            }
            Err(e) => return Err(e.into()),
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (a, b);
        Ok(false)
    }

    /// Make `to` a second name for the file at `from`, both in this
    /// directory. Without `AT_SYMLINK_FOLLOW`, so a link at `from` is
    /// never followed.
    pub fn hard_link(&self, from: &OsStr, to: &OsStr) -> std::io::Result<()> {
        rustix::fs::linkat(&self.fd, from, &self.fd, to, AtFlags::empty())?;
        Ok(())
    }

    /// Remove the entry `name` (never a directory, never through a link).
    pub fn remove_file(&self, name: &OsStr) -> std::io::Result<()> {
        rustix::fs::unlinkat(&self.fd, name, AtFlags::empty())?;
        Ok(())
    }

    /// fsync the directory, making renames and links in it durable.
    pub fn sync(&self) -> std::io::Result<()> {
        rustix::fs::fsync(&self.fd)?;
        Ok(())
    }
}

/// Remove the file at `path` below `root`, through the no-follow walk: the
/// parent folders may not be links and the entry must be a regular file
/// (a link at the name is refused, not removed). Blocking.
pub fn remove_library_file(root: &Path, path: &Path) -> Result<(), Refusal> {
    let (dir, name) = open_library_parent(root, path)?;
    match dir.kind(&name).map_err(Refusal::Io)? {
        EntryKind::Regular => {}
        EntryKind::Missing => return Err(Refusal::Missing),
        EntryKind::Symlink => return Err(Refusal::Symlink),
        EntryKind::Other => return Err(Refusal::NotRegular),
    }
    dir.remove_file(&name).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Refusal::Missing
        } else {
            Refusal::Io(e)
        }
    })
}

/// A library's root as configured and as resolved (its own links followed).
#[derive(Debug, Clone)]
pub struct LibraryRoot {
    pub configured: PathBuf,
    /// `None` while the root cannot be resolved (missing, unmounted).
    pub canonical: Option<PathBuf>,
    loaded_at: Instant,
}

/// How long a cached root is trusted before it is read again. Explicit
/// invalidation (library settings saved, library deleted) covers the API;
/// the TTL covers an edit made outside it and a remounted root.
const ROOT_TTL: Duration = Duration::from_secs(60);

/// In-memory map of library id → [`LibraryRoot`], on
/// [`AppState`](crate::state::AppState) as `library_roots`, so the serving
/// check costs no database query per page request.
#[derive(Debug, Default)]
pub struct LibraryRoots {
    map: std::sync::Mutex<HashMap<Uuid, Arc<LibraryRoot>>>,
}

impl LibraryRoots {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget one library (its row was edited or deleted).
    pub fn invalidate(&self, library_id: Uuid) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(&library_id);
        }
    }

    /// The library's root, from the map while fresh, else from the database
    /// (one projected query) with the canonical path resolved on the
    /// blocking pool. `Ok(None)` when the library does not exist; `Err`
    /// when the lookup itself failed (never reported as not-found).
    pub async fn get(
        &self,
        db: &sea_orm::DatabaseConnection,
        library_id: Uuid,
    ) -> Result<Option<Arc<LibraryRoot>>, sea_orm::DbErr> {
        if let Some(r) = self.fresh(library_id, None) {
            return Ok(Some(r));
        }
        use sea_orm::{EntityTrait, QuerySelect};
        let Some(configured) = entity::library::Entity::find_by_id(library_id)
            .select_only()
            .column(entity::library::Column::RootPath)
            .into_tuple::<String>()
            .one(db)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.resolve_and_store(library_id, configured).await))
    }

    /// The root of a library row the caller already loaded: no database
    /// query, so nothing here can fail. Reuses the cached entry while it is
    /// fresh and names the same configured path.
    pub async fn of(&self, lib: &entity::library::Model) -> Arc<LibraryRoot> {
        if let Some(r) = self.fresh(lib.id, Some(&lib.root_path)) {
            return r;
        }
        self.resolve_and_store(lib.id, lib.root_path.clone()).await
    }

    /// The cached entry while fresh (and, with `configured`, only when it
    /// names that path).
    fn fresh(&self, library_id: Uuid, configured: Option<&str>) -> Option<Arc<LibraryRoot>> {
        let m = self.map.lock().ok()?;
        let r = m.get(&library_id)?;
        let same = configured.is_none_or(|c| r.configured == Path::new(c));
        (same && r.loaded_at.elapsed() < ROOT_TTL).then(|| Arc::clone(r))
    }

    async fn resolve_and_store(&self, library_id: Uuid, configured: String) -> Arc<LibraryRoot> {
        let configured = PathBuf::from(configured);
        let for_block = configured.clone();
        let canonical = tokio::task::spawn_blocking(move || std::fs::canonicalize(for_block).ok())
            .await
            .ok()
            .flatten();
        let root = Arc::new(LibraryRoot {
            configured,
            canonical,
            loaded_at: Instant::now(),
        });
        if let Ok(mut m) = self.map.lock() {
            m.insert(library_id, Arc::clone(&root));
        }
        root
    }
}

/// Open an issue's stored file through [`open_below_roots`], as an
/// [`archive::ArchiveFile`] that carries the file's [`archive::FileStamp`]
/// from the open (so the page cache's identity and version check costs no
/// further system call). Readers built from it (directly or through `zip_lru`) read only
/// through this handle. A refusal (symbolic link, outside the root, not a
/// regular file) is logged at WARN with the issue id and the reason, never
/// anything read from the file; a missing file only at DEBUG (the caller
/// has its own missing-file answer). A failed library lookup is
/// [`Refusal::Lookup`]: request paths answer 5xx for it, jobs retry.
pub async fn open_issue_archive(
    state: &AppState,
    row: &entity::issue::Model,
) -> Result<archive::ArchiveFile, Refusal> {
    let issue_id = row.id.as_str();
    let root = match state.library_roots.get(&state.db, row.library_id).await {
        Ok(Some(root)) => root,
        Ok(None) => {
            tracing::debug!(issue_id, "issue file not opened: library not found");
            return Err(Refusal::Missing);
        }
        Err(e) => {
            tracing::warn!(issue_id, error = %e, "issue file not opened: library lookup failed");
            return Err(Refusal::Lookup(e.to_string()));
        }
    };
    let path = row.file_path.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let (dir, name) = open_parent_below_roots(
            &root.configured,
            root.canonical.as_deref(),
            Path::new(&path),
        )?;
        dir.open_regular_identified(&name)
    })
    .await;
    match opened {
        Ok(Ok((f, stamp))) => {
            Ok(archive::ArchiveFile::new(f, PathBuf::from(&row.file_path)).with_stamp(stamp))
        }
        Ok(Err(Refusal::Missing)) => {
            tracing::debug!(issue_id, "issue file missing");
            Err(Refusal::Missing)
        }
        Ok(Err(reason)) => {
            tracing::warn!(issue_id, reason = %reason, "issue file refused");
            Err(reason)
        }
        Err(e) => {
            tracing::warn!(issue_id, error = %e, "issue file open task failed");
            Err(Refusal::Io(std::io::Error::other(e.to_string())))
        }
    }
}

/// The library root of `library_id` as configured and resolved, for a
/// caller that has only the id. `Ok(None)` when the library does not
/// exist; `Err` when the lookup failed. A caller that already loaded the
/// library row uses [`library_root_of`] instead.
pub async fn library_root(
    state: &AppState,
    library_id: Uuid,
) -> Result<Option<Arc<LibraryRoot>>, sea_orm::DbErr> {
    state.library_roots.get(&state.db, library_id).await
}

/// Whether `library_id`'s root is unavailable: it cannot be opened as a
/// folder, or it is empty. Asked only after an issue's file was found
/// missing, to tell "the file is gone" (410) from "the share is not
/// mounted" (503): an unmounted mount point or bind-mount source is either
/// absent or an empty folder, while a library whose file is really gone
/// still has its other folders. A library that no longer exists, or whose
/// root could not be looked up, counts as available (the missing-file
/// answer stands). Blocking work runs off the runtime.
pub async fn library_root_offline(state: &AppState, library_id: Uuid) -> bool {
    let Ok(Some(root)) = state.library_roots.get(&state.db, library_id).await else {
        return false;
    };
    let configured = root.configured.clone();
    tokio::task::spawn_blocking(move || match std::fs::read_dir(&configured) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true,
    })
    .await
    .unwrap_or(false)
}

/// The library root of a library row the caller already loaded (writers,
/// background jobs). No database query.
pub async fn library_root_of(state: &AppState, lib: &entity::library::Model) -> Arc<LibraryRoot> {
    state.library_roots.of(lib).await
}

impl LibraryRoot {
    /// A root resolved now, for callers outside the request path (the
    /// scanner, tests). Blocking (one `canonicalize`).
    pub fn resolve(configured: impl Into<PathBuf>) -> Self {
        let configured = configured.into();
        let canonical = std::fs::canonicalize(&configured).ok();
        Self {
            configured,
            canonical,
            loaded_at: Instant::now(),
        }
    }

    /// [`open_below_roots`] under this root.
    pub fn open_file(&self, path: &Path) -> Result<File, Refusal> {
        open_below_roots(&self.configured, self.canonical.as_deref(), path)
    }

    /// [`open_parent_below_roots`] under this root.
    pub fn open_parent(&self, path: &Path) -> Result<(LibraryDir, OsString), Refusal> {
        open_parent_below_roots(&self.configured, self.canonical.as_deref(), path)
    }

    /// The file at `path` as an [`archive::ArchiveFile`], opened under this
    /// root.
    pub fn open_archive(&self, path: &Path) -> Result<archive::ArchiveFile, Refusal> {
        Ok(archive::ArchiveFile::new(self.open_file(path)?, path))
    }
}

/// Largest sidecar [`read_sidecar`] reads.
pub const MAX_SIDECAR_BYTES: u64 = 1024 * 1024;

/// Read a small sidecar (`series.json`) at `path` below `root`, through the
/// same no-follow walk as every other library read: no link on the folder
/// path or at the name, a regular file only, read through the handle that
/// check opened. At most [`MAX_SIDECAR_BYTES`]; a larger file is an error
/// (`InvalidData`). Blocking.
pub fn read_sidecar(root: &Path, path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let refused = |r: Refusal| match r {
        Refusal::Io(e) => e,
        Refusal::Missing => std::io::Error::new(std::io::ErrorKind::NotFound, "file is missing"),
        other => std::io::Error::new(std::io::ErrorKind::InvalidInput, other.to_string()),
    };
    let (dir, name) = open_library_parent(root, path).map_err(refused)?;
    let f = dir.open_regular(&name).map_err(refused)?;
    let size = f.metadata()?.len();
    if size > MAX_SIDECAR_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("sidecar is {size} bytes, over the 1 MiB cap"),
        ));
    }
    let mut out = Vec::new();
    f.take(MAX_SIDECAR_BYTES + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "sidecar is larger than 1 MiB",
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn lib() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("library");
        std::fs::create_dir_all(root.join("Series")).unwrap();
        std::fs::write(root.join("Series").join("a.cbz"), b"PK-data").unwrap();
        (tmp, root)
    }

    #[test]
    fn regular_file_opens() {
        use std::io::Read;
        let (_tmp, root) = lib();
        let mut f = open_below_root(&root, &root.join("Series").join("a.cbz")).unwrap();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "PK-data");
    }

    #[test]
    fn final_symlink_is_refused() {
        let (tmp, root) = lib();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        let link = root.join("Series").join("b.cbz");
        symlink(&outside, &link).unwrap();
        assert!(matches!(
            open_below_root(&root, &link),
            Err(Refusal::Symlink)
        ));
        // A link to a file inside the root is refused just the same.
        let inner = root.join("Series").join("c.cbz");
        symlink(root.join("Series").join("a.cbz"), &inner).unwrap();
        assert!(matches!(
            open_below_root(&root, &inner),
            Err(Refusal::Symlink)
        ));
    }

    #[test]
    fn symlinked_parent_is_refused() {
        let (tmp, root) = lib();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("a.cbz"), b"x").unwrap();
        symlink(&elsewhere, root.join("Linked")).unwrap();
        assert!(matches!(
            open_below_root(&root, &root.join("Linked").join("a.cbz")),
            Err(Refusal::Symlink)
        ));
        // Pointing inside the root changes nothing.
        symlink(root.join("Series"), root.join("Alias")).unwrap();
        assert!(matches!(
            open_below_root(&root, &root.join("Alias").join("a.cbz")),
            Err(Refusal::Symlink)
        ));
    }

    #[test]
    fn root_that_is_a_link_is_followed() {
        let (tmp, root) = lib();
        let alias = tmp.path().join("alias-root");
        symlink(&root, &alias).unwrap();
        assert!(open_below_root(&alias, &alias.join("Series").join("a.cbz")).is_ok());
    }

    #[test]
    fn directory_is_refused() {
        let (_tmp, root) = lib();
        assert!(matches!(
            open_below_root(&root, &root.join("Series")),
            Err(Refusal::NotRegular)
        ));
    }

    #[test]
    fn outside_root_is_refused() {
        let (tmp, root) = lib();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        assert!(matches!(
            open_below_root(&root, &outside),
            Err(Refusal::OutsideRoot)
        ));
        assert!(matches!(
            open_below_root(
                &root,
                &root.join("Series").join("..").join("..").join("secret.txt")
            ),
            Err(Refusal::OutsideRoot)
        ));
        assert!(matches!(
            open_below_root(&root, &root),
            Err(Refusal::OutsideRoot)
        ));
    }

    #[test]
    fn missing_is_missing() {
        let (_tmp, root) = lib();
        assert!(matches!(
            open_below_root(&root, &root.join("Series").join("gone.cbz")),
            Err(Refusal::Missing)
        ));
        assert!(matches!(
            open_below_root(&root, &root.join("Gone").join("a.cbz")),
            Err(Refusal::Missing)
        ));
    }

    #[test]
    fn fifo_is_refused_without_blocking() {
        let (_tmp, root) = lib();
        let fifo = root.join("Series").join("pipe.cbz");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            FileType::Fifo,
            Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        // No writer ever opens the FIFO: a blocking open would hang here.
        let (tx, rx) = std::sync::mpsc::channel();
        let root2 = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(matches!(
                open_below_root(&root2, &fifo),
                Err(Refusal::NotRegular)
            ));
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            "FIFO refused as not a regular file"
        );
    }

    #[test]
    fn sidecar_link_is_refused() {
        let (tmp, root) = lib();
        let outside = tmp.path().join("other.json");
        std::fs::write(&outside, b"{}").unwrap();
        let link = root.join("Series").join("series.json");
        symlink(&outside, &link).unwrap();
        assert!(read_sidecar(&root, &link).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, b"{}").unwrap();
        assert_eq!(read_sidecar(&root, &link).unwrap(), b"{}");
    }

    #[test]
    fn sidecar_in_a_linked_folder_or_over_the_cap_is_refused() {
        let (tmp, root) = lib();
        let outside = tmp.path().join("elsewhere");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("series.json"), b"{}").unwrap();
        symlink(&outside, root.join("Linked")).unwrap();
        assert!(read_sidecar(&root, &root.join("Linked").join("series.json")).is_err());
        let big = root.join("Series").join("series.json");
        std::fs::write(&big, vec![b' '; (MAX_SIDECAR_BYTES + 1) as usize]).unwrap();
        let err = read_sidecar(&root, &big).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn rename_noreplace_never_replaces_an_existing_entry() {
        let (_tmp, root) = lib();
        let a = root.join("Series").join("a");
        let b = root.join("Series").join("b");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let (dir, _) = open_library_parent(&root, &a).unwrap();
        let err = dir
            .rename_noreplace(OsStr::new("a"), OsStr::new("b"))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&a).unwrap(), b"a");
        assert_eq!(std::fs::read(&b).unwrap(), b"b");
        dir.rename_noreplace(OsStr::new("a"), OsStr::new("c"))
            .unwrap();
        assert_eq!(std::fs::read(root.join("Series").join("c")).unwrap(), b"a");
    }

    #[test]
    fn remove_refuses_links_and_linked_folders() {
        let (tmp, root) = lib();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("a.cbz.bak"), b"keep").unwrap();
        // A link at the name: refused, neither the link nor its target goes.
        let link = root.join("Series").join("a.cbz.bak");
        symlink(outside.join("a.cbz.bak"), &link).unwrap();
        assert!(matches!(
            remove_library_file(&root, &link),
            Err(Refusal::Symlink)
        ));
        assert!(std::fs::symlink_metadata(&link).is_ok());
        // A folder that became a link: refused, the outside file stays.
        symlink(&outside, root.join("Linked")).unwrap();
        assert!(matches!(
            remove_library_file(&root, &root.join("Linked").join("a.cbz.bak")),
            Err(Refusal::Symlink)
        ));
        assert_eq!(std::fs::read(outside.join("a.cbz.bak")).unwrap(), b"keep");
        // A regular file goes.
        let real = root.join("Series").join("b.cbz.bak");
        std::fs::write(&real, b"x").unwrap();
        remove_library_file(&root, &real).unwrap();
        assert!(!real.exists());
    }

    /// The parent handle stays on the folder that was checked: a folder
    /// swapped for a link after the walk does not redirect a later open.
    #[test]
    fn parent_handle_survives_a_folder_swap() {
        use std::io::Read;
        let (tmp, root) = lib();
        let (dir, name) =
            open_parent_below_roots(&root, None, &root.join("Series").join("a.cbz")).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("a.cbz"), b"other").unwrap();
        std::fs::rename(root.join("Series"), root.join("Moved")).unwrap();
        symlink(&outside, root.join("Series")).unwrap();
        let mut s = String::new();
        dir.open_regular(&name)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "PK-data");
        assert_eq!(dir.kind(OsStr::new("missing")).unwrap(), EntryKind::Missing);
    }
}
