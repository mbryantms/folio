//! Where an archive's bytes come from: a file handle the caller opened.
//!
//! Every reader in this crate reads through an [`ArchiveFile`]: one open
//! file description plus the path it is displayed as in errors and logs.
//! The readers never open the display path again. They read the handle with
//! positioned reads (`pread`), so any number of independent cursors
//! ([`ArchiveFile::reader`]) share one descriptor without disturbing each
//! other, and every byte a reader returns comes from the file the caller
//! opened, even if the name has since been renamed, replaced or turned into
//! a symbolic link.
//!
//! The caller decides how the handle is opened. The server opens files that
//! live under a library root through its no-follow opener and hands the
//! resulting `File` over with [`ArchiveFile::new`]. Files that are not
//! library files (tests, a freshly written temp file) can be opened with
//! [`ArchiveFile::open_path_unchecked`], which follows symbolic links like
//! any ordinary open and must never be given a library path.
//!
//! The RAR reader is the one exception to "read through the handle": the
//! UnRAR library opens files by name only. See [`ArchiveFile::rar_name`].

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An open archive file and the path it is shown as. Cheap to clone (an
/// `Arc` bump); clones share the descriptor.
#[derive(Debug, Clone)]
pub struct ArchiveFile {
    file: Arc<File>,
    display: PathBuf,
    /// `(device, inode)` the caller already took from this handle
    /// ([`ArchiveFile::with_identity`]), so [`ArchiveFile::identity`] need
    /// not `fstat` again.
    known_identity: Option<(u64, u64)>,
    /// Whether [`ArchiveFile::rar_name`] may use `/proc/self/fd/<n>`.
    proc_fd: bool,
}

impl ArchiveFile {
    /// Wrap a file the caller already opened. `display_path` is used only
    /// for error messages, logs and [`crate::ComicArchive::path`]; it is
    /// never opened.
    pub fn new(file: File, display_path: impl Into<PathBuf>) -> Self {
        Self {
            file: Arc::new(file),
            display: display_path.into(),
            known_identity: None,
            proc_fd: true,
        }
    }

    /// Record the `(device, inode)` the caller took from this same handle
    /// (an `fstat` it already made while opening it, off the async
    /// runtime). [`ArchiveFile::identity`] then answers without a system
    /// call. The caller vouches that the pair describes this handle.
    pub fn with_identity(mut self, identity: (u64, u64)) -> Self {
        self.known_identity = Some(identity);
        self
    }

    /// Never hand UnRAR `/proc/self/fd/<n>`; use the checked display path
    /// instead ([`ArchiveFile::rar_name`]). For tests of that path, and for
    /// a deployment where reopening a descriptor through `/proc` is not
    /// permitted.
    pub fn without_proc_fd(mut self) -> Self {
        self.proc_fd = false;
        self
    }

    /// Open `path` with an ordinary open, following symbolic links. For
    /// files outside any library root only (tests, temp files the caller
    /// created). A library file must be opened by the caller's checked
    /// opener and passed to [`ArchiveFile::new`].
    pub fn open_path_unchecked(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        Ok(Self::new(File::open(path)?, path))
    }

    /// The path this file is shown as.
    pub fn display_path(&self) -> &Path {
        &self.display
    }

    /// The underlying handle.
    pub fn file(&self) -> &File {
        &self.file
    }

    /// A new independent cursor over the file, starting at offset 0.
    pub fn reader(&self) -> PosReader {
        PosReader {
            file: Arc::clone(&self.file),
            pos: 0,
        }
    }

    /// Current length of the file, from the handle (`fstat`).
    pub fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// True when the file is empty.
    pub fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Fill `buf` from `offset`, or fail with `UnexpectedEof`.
    pub fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let n = read_at(&self.file, &mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "archive file ended early",
                ));
            }
            done += n;
        }
        Ok(())
    }

    /// Read up to `buf.len()` bytes from `offset`; returns how many were
    /// read (short only at end of file).
    pub fn read_up_to_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let n = read_at(&self.file, &mut buf[done..], offset + done as u64)?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    /// The whole file, read through the handle.
    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.reader().read_to_end(&mut out)?;
        Ok(out)
    }

    /// `(device, inode)` of the open file: the pair recorded with
    /// [`ArchiveFile::with_identity`], else from the handle (`fstat`).
    #[cfg(unix)]
    pub fn identity(&self) -> io::Result<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        if let Some(known) = self.known_identity {
            return Ok(known);
        }
        let m = self.file.metadata()?;
        Ok((m.dev(), m.ino()))
    }

    /// A name the UnRAR library can open that reaches exactly this open
    /// file, plus whether the caller must confirm the file's identity
    /// around each pass ([`ArchiveFile::check_rar_name`]).
    ///
    /// On Linux this is `/proc/self/fd/<n>`: opening it reopens the file
    /// description this handle refers to, whatever has happened to the
    /// original name since. Where `/proc/self/fd` is not available (another
    /// platform, `/proc` not mounted, or [`ArchiveFile::without_proc_fd`])
    /// it falls back to the display path, and
    /// [`ArchiveFile::check_rar_name`] must be called before and after
    /// each pass: it refuses the read when the name no longer leads to the
    /// same file (device and inode) or is a symbolic link. That fallback
    /// narrows the window to the pass itself rather than closing it. The
    /// RAR reader also falls back when UnRAR cannot open the `/proc` name
    /// (a sandbox that forbids reopening descriptors through `/proc`).
    pub fn rar_name(&self) -> (PathBuf, bool) {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            if self.proc_fd && Path::new("/proc/self/fd").is_dir() {
                return (
                    PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd())),
                    false,
                );
            }
        }
        (self.display.clone(), true)
    }

    /// Fallback identity check for [`ArchiveFile::rar_name`]: the display
    /// path must not be a symbolic link and must name the same file as the
    /// handle.
    #[cfg(unix)]
    pub fn check_rar_name(&self) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let named = std::fs::symlink_metadata(&self.display)?;
        if named.file_type().is_symlink() {
            return Err(io::Error::other("archive path is a symbolic link"));
        }
        if (named.dev(), named.ino()) != self.identity()? {
            return Err(io::Error::other(
                "archive path no longer names the opened file",
            ));
        }
        Ok(())
    }

    /// Without device/inode numbers the fallback cannot be checked.
    #[cfg(not(unix))]
    pub fn check_rar_name(&self) -> io::Result<()> {
        Err(io::Error::other(
            "RAR archives need /proc/self/fd or a Unix identity check",
        ))
    }
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    loop {
        match file.read_at(buf, offset) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

/// A `Read + Seek` cursor over an [`ArchiveFile`] that reads with
/// positioned reads, so it never moves the shared descriptor's offset and
/// any number of cursors can be used at once.
#[derive(Debug)]
pub struct PosReader {
    file: Arc<File>,
    pos: u64,
}

impl Read for PosReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = read_at(&self.file, buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for PosReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.file.metadata()?.len().checked_add_signed(d),
        };
        match next {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the file",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cursors_are_independent_and_read_through_the_handle() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"0123456789").unwrap();
        let f = ArchiveFile::open_path_unchecked(&p).unwrap();
        let mut a = f.reader();
        let mut b = f.reader();
        b.seek(SeekFrom::Start(5)).unwrap();
        let mut x = [0u8; 3];
        a.read_exact(&mut x).unwrap();
        assert_eq!(&x, b"012");
        b.read_exact(&mut x).unwrap();
        assert_eq!(&x, b"567");

        // The name now leads elsewhere; the handle still reads the original.
        std::fs::rename(&p, dir.path().join("moved.bin")).unwrap();
        let mut other = std::fs::File::create(&p).unwrap();
        other.write_all(b"XXXXXXXXXX").unwrap();
        assert_eq!(f.read_all().unwrap(), b"0123456789");
        let mut tail = [0u8; 4];
        assert_eq!(f.read_up_to_at(&mut tail, 8).unwrap(), 2);
        assert_eq!(&tail[..2], b"89");
    }
}
