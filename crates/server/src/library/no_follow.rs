//! Opening library files without following symbolic links.
//!
//! Folio never follows a symbolic link below a library root: the scanner
//! skips them (`scanner::enumerate`, reported as
//! [`IssueKind::SymlinkSkipped`](crate::library::health::IssueKind::SymlinkSkipped)),
//! and every route that hands a stored issue file to a reader opens it
//! through [`open_issue`] here (or checks it with [`issue_path_ok`] where
//! the archive crate reopens the path). The library root's own path may
//! contain links (a mount point or `/data/comics -> /mnt/x` is a deployment
//! choice):
//! it is resolved once, exactly as `validate_library` canonicalizes it, and
//! nothing below it is followed.
//!
//! [`open_below_root`] walks from the root directory one path component at a
//! time with `openat(…, O_NOFOLLOW)`, so a link anywhere below the root (a
//! linked series folder, a linked publisher folder, a linked archive) is
//! refused, and a link swapped in during the walk is refused too: each step
//! opens a name relative to the directory handle the previous step proved,
//! never a path the kernel resolves again. The final open adds `O_NONBLOCK`
//! so a FIFO cannot hang the request, and the handle is `fstat`-ed: only a
//! regular file is returned. Callers read through that handle, never by
//! reopening the path.

use std::collections::HashMap;
use std::ffi::OsStr;
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
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideRoot => f.write_str("path is not below the library root"),
            Self::Missing => f.write_str("file is missing"),
            Self::Symlink => f.write_str("path goes through a symbolic link"),
            Self::NotRegular => f.write_str("not a regular file"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
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
    let Some((file_name, folders)) = names.split_last() else {
        return Err(Refusal::OutsideRoot);
    };

    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    // The root itself: its own links are followed (the deployment rule).
    let mut dir = rustix::fs::openat(rustix::fs::CWD, root, dir_flags, Mode::empty()).map_err(
        |e| match e {
            Errno::NOENT | Errno::NOTDIR => Refusal::Missing,
            e => Refusal::Io(e.into()),
        },
    )?;
    for name in folders {
        dir = rustix::fs::openat(&dir, *name, dir_flags | OFlags::NOFOLLOW, Mode::empty())
            .map_err(|e| refusal_for(&dir, name, e))?;
    }

    // Stat before opening so a device or FIFO is never opened at all.
    let pre = rustix::fs::statat(&dir, *file_name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| refusal_for(&dir, file_name, e))?;
    match FileType::from_raw_mode(pre.st_mode) {
        FileType::RegularFile => {}
        FileType::Symlink => return Err(Refusal::Symlink),
        _ => return Err(Refusal::NotRegular),
    }
    // Swapped between the stat and the open: `O_NOFOLLOW` refuses a link,
    // `O_NONBLOCK` keeps a FIFO from blocking, the `fstat` below refuses
    // anything that is not a regular file.
    let fd = rustix::fs::openat(
        &dir,
        *file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| refusal_for(&dir, file_name, e))?;
    let st = rustix::fs::fstat(&fd).map_err(|e| Refusal::Io(e.into()))?;
    if !FileType::from_raw_mode(st.st_mode).is_file() {
        return Err(Refusal::NotRegular);
    }
    Ok(File::from(fd))
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
    /// blocking pool. `None` when the library does not exist.
    pub async fn get(
        &self,
        db: &sea_orm::DatabaseConnection,
        library_id: Uuid,
    ) -> Option<Arc<LibraryRoot>> {
        if let Ok(m) = self.map.lock()
            && let Some(r) = m.get(&library_id)
            && r.loaded_at.elapsed() < ROOT_TTL
        {
            return Some(Arc::clone(r));
        }
        use sea_orm::{EntityTrait, QuerySelect};
        let configured = entity::library::Entity::find_by_id(library_id)
            .select_only()
            .column(entity::library::Column::RootPath)
            .into_tuple::<String>()
            .one(db)
            .await
            .ok()
            .flatten()?;
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
        Some(root)
    }
}

/// Open an issue's stored file through [`open_below_roots`]. A refusal
/// (symbolic link, outside the root, not a regular file) is logged at WARN
/// with the issue id and the reason, never anything read from the file; a
/// missing file only at DEBUG (the route has its own missing-file answer).
async fn try_open_issue(state: &AppState, row: &entity::issue::Model) -> Result<File, Refusal> {
    let issue_id = row.id.as_str();
    let Some(root) = state.library_roots.get(&state.db, row.library_id).await else {
        tracing::debug!(issue_id, "issue file not opened: library not found");
        return Err(Refusal::Missing);
    };
    let path = row.file_path.clone();
    let opened = tokio::task::spawn_blocking(move || {
        open_below_roots(
            &root.configured,
            root.canonical.as_deref(),
            Path::new(&path),
        )
    })
    .await;
    match opened {
        Ok(Ok(f)) => Ok(f),
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

/// Open an issue's stored file for a reader. `None` means the route answers
/// not-found (missing file included).
pub async fn open_issue(state: &AppState, row: &entity::issue::Model) -> Option<File> {
    try_open_issue(state, row).await.ok()
}

/// Check an issue's stored path with the same rules, for a caller that must
/// hand the *path* to the archive crate (which opens it again itself). The
/// handle is dropped; the path can still change between this check and the
/// archive crate's own open. A file that is simply missing is not refused
/// here: the route keeps its own missing-archive answer.
pub async fn issue_path_ok(state: &AppState, row: &entity::issue::Model) -> bool {
    matches!(
        try_open_issue(state, row).await,
        Ok(_) | Err(Refusal::Missing)
    )
}

/// Read a small sidecar (`series.json`) that sits in a folder the scan
/// already proved: refuses a symbolic link or anything but a regular file
/// in the final component. Blocking.
pub fn read_sidecar(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let st = rustix::fs::fstat(&fd)?;
    if !FileType::from_raw_mode(st.st_mode).is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut out = Vec::new();
    File::from(fd).read_to_end(&mut out)?;
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
        assert!(read_sidecar(&link).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, b"{}").unwrap();
        assert_eq!(read_sidecar(&link).unwrap(), b"{}");
    }
}
