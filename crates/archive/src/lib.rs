//! Archive readers for comic-book containers (CBZ in Phase 1a; CBR/CB7/CBT/EPUB in 1b).
//!
//! The defense limits in [`ArchiveLimits`] (defaults from §4.1.1 of the spec)
//! are enforced by **every** reader. A bad archive is rejected with a typed error
//! and the file is marked `state='malformed'` (or `'encrypted'` for §4.6 hits)
//! by the caller.

use std::path::Path;
use std::time::Duration;

pub mod cb7;
pub mod cbr;
pub mod cbt;
pub mod cbz;
pub mod comic_archive;
pub mod entry_name;

pub use comic_archive::ComicArchive;

/// Open a comic archive of any supported format. Dispatch is by extension.
/// Returns the boxed reader as `dyn ComicArchive` so the scanner doesn't
/// branch on format.
///
/// Library Scanner v1, Milestone 12. Supported today:
///   - `.cbz` — full
///   - `.cbt` — full
///   - `.cbr`, `.cb7` — scaffolded; both currently return
///     [`ArchiveError::Malformed`] with a "format not implemented" message
///     so the scanner can emit `UnsupportedArchiveFormat` health issues
///     without crashing the walk. Documented carry-over to a follow-up plan.
pub fn open(path: &Path, limits: ArchiveLimits) -> Result<Box<dyn ComicArchive>, ArchiveError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("cbz") => {
            let c = cbz::Cbz::open(path, limits)?;
            Ok(Box::new(c) as Box<dyn ComicArchive>)
        }
        Some("cbt") => {
            let c = cbt::Cbt::open(path, limits)?;
            Ok(Box::new(c) as Box<dyn ComicArchive>)
        }
        Some("cbr") => cbr::Cbr::open(path, limits).map(|c| Box::new(c) as _),
        Some("cb7") => cb7::Cb7::open(path, limits).map(|c| Box::new(c) as _),
        _ => Err(ArchiveError::Malformed(format!(
            "unsupported archive extension: {:?}",
            ext
        ))),
    }
}

/// `ComicArchive` impl on the existing `Cbz` so `open()` can box it directly.
impl ComicArchive for cbz::Cbz {
    fn entries(&self) -> &[ArchiveEntry] {
        self.entries()
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        self.pages()
    }
    fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        self.find(name)
    }
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        self.read_entry_bytes_by_name(name)
    }
    fn read_entry_prefix(&mut self, name: &str, max_bytes: usize) -> Result<Vec<u8>, ArchiveError> {
        self.read_entry_prefix_by_name(name, max_bytes)
    }
    fn path(&self) -> &Path {
        self.path()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub max_entries: u64,
    pub max_total_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_compression_ratio: u32,
    pub max_nesting_depth: u8,
    pub subprocess_wall_timeout: Duration,
    pub subprocess_rss_bytes: u64,
}

impl Default for ArchiveLimits {
    /// Defaults match §4.1.1 of the spec.
    fn default() -> Self {
        Self {
            max_entries: 50_000,
            max_total_bytes: 8 * 1024 * 1024 * 1024,
            max_entry_bytes: 512 * 1024 * 1024,
            max_compression_ratio: 200,
            max_nesting_depth: 1,
            subprocess_wall_timeout: Duration::from_secs(60),
            subprocess_rss_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("archive entry rejected (zip-slip / invalid path): {0}")]
    UnsafeEntry(String),
    #[error("archive cap exceeded: {0}")]
    CapExceeded(&'static str),
    #[error("archive encrypted")]
    Encrypted,
    #[error("io: {0}")]
    Io(String),
    #[error("malformed archive: {0}")]
    Malformed(String),
}

impl From<std::io::Error> for ArchiveError {
    fn from(e: std::io::Error) -> Self {
        ArchiveError::Io(e.to_string())
    }
}

/// One entry inside an archive, surfaced for both metadata and page reads.
#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    /// Zero-based index in the entry order *as the archive lists them*
    /// (NOT the page sort order — see [`cbz::Cbz::pages`] for that).
    pub index: usize,
    /// Sanitized entry name (forward slashes; never escapes archive root).
    pub name: String,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
}
