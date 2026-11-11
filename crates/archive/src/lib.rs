//! Archive readers for comic-book containers: CBZ + CBT (read/write), CBR +
//! CB7 (read-only; converted to CBZ on scan when a library opts in).
//!
//! The defense limits in [`ArchiveLimits`] (defaults from §4.1.1 of the spec)
//! are enforced by **every** reader. A bad archive is rejected with a typed error
//! and the file is marked `state='malformed'` (or `'encrypted'` for §4.6 hits)
//! by the caller.
//!
//! ## Handles, not names
//!
//! Every reader reads from a file handle the caller opened
//! ([`source::ArchiveFile`]): [`open_file`] and the per-format `open_file`
//! constructors take one, and no reader opens its path again. CBZ, CBT and
//! CB7 read the handle with positioned reads (independent cursors over one
//! descriptor); CBR goes through the UnRAR library, which opens by name,
//! so it is given `/proc/self/fd/<n>` for the handle on Linux and otherwise
//! (where this process cannot reopen descriptors through `/proc`) checks the name's device and
//! inode against the handle around each pass (see
//! [`source::ArchiveFile::rar_name`]). A multi-part RAR set is refused
//! ([`ArchiveError::is_multi_volume_rar`]). The writers take an open
//! `File` to write into. The path-taking constructors (`open_path_unchecked`)
//! open by name and follow links: they exist for files outside any library
//! root (tests, temp files) and must not be given a library path; the
//! server opens library files through its no-follow opener and passes the
//! handle.

use std::path::Path;
use std::time::Duration;

pub mod cb7;
pub mod cbr;
pub mod cbt;
pub mod cbt_write;
pub mod cbz;
pub mod cbz_write;
pub mod comic_archive;
pub mod container;
pub mod entry_name;
pub mod image_sniff;
pub mod rewrite_policy;
pub mod source;

pub use comic_archive::ComicArchive;
pub use source::{ArchiveFile, FileStamp};

/// Open a comic archive of any supported format from an open file.
///
/// Dispatch is by **container**, not extension: the leading bytes are
/// sniffed ([`container::detect_container`]) and the matching reader is
/// chosen, so a RAR named `.cbz` opens through the RAR reader exactly as
/// YACreader / ComicRack / Komga would open it. Only when the bytes carry
/// no recognizable magic (a v7 tar, a truncated file, junk) does the
/// extension of the display path decide, which also keeps the old typed
/// error messages for genuinely corrupt `.cbz` files. A file with neither a
/// recognizable container nor a known extension is `Malformed`.
///
/// Returns the boxed reader as `dyn ComicArchive` so the scanner doesn't
/// branch on format. Every read goes through `src`'s handle.
///
/// Supported today:
///   - `.cbz` — full (random-access reader + writer)
///   - `.cbt` — full (tar reader + writer)
///   - `.cbr` — read-only (unrar-backed; `archive-rewrite-1.0` M6). The
///     page editor converts CBR → CBZ on edit since RAR can't be written.
///   - `.cb7` — read-only (`sevenz-rust2`-backed; WP-6.5). Like CBR there
///     is no writer: the scanner converts CB7 → CBZ on scan when the
///     library opts in (`auto_convert_cb7_on_scan`).
pub fn open_file(
    src: source::ArchiveFile,
    limits: ArchiveLimits,
) -> Result<Box<dyn ComicArchive>, ArchiveError> {
    let ext = src
        .display_path()
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    let format = match container::detect_container(&src)? {
        container::Container::Zip => "cbz",
        container::Container::Rar => "cbr",
        container::Container::SevenZ => "cb7",
        container::Container::Tar => "cbt",
        container::Container::Unknown => ext.as_deref().unwrap_or(""),
    };
    match format {
        "cbz" => {
            let c = cbz::Cbz::open_file(src, limits)?;
            Ok(Box::new(c) as Box<dyn ComicArchive>)
        }
        "cbt" => {
            let c = cbt::Cbt::open_file(src, limits)?;
            Ok(Box::new(c) as Box<dyn ComicArchive>)
        }
        "cbr" => cbr::Cbr::open_file(src, limits).map(|c| Box::new(c) as _),
        "cb7" => cb7::Cb7::open_file(src, limits).map(|c| Box::new(c) as _),
        _ => Err(ArchiveError::Malformed(format!(
            "unsupported archive extension: {:?}",
            ext
        ))),
    }
}

/// [`open_file`] for a file outside any library root, opened by name with
/// an ordinary open (links followed). Tests and temp files only; a library
/// file is opened by the caller's checked opener and passed to
/// [`open_file`].
pub fn open_path_unchecked(
    path: &Path,
    limits: ArchiveLimits,
) -> Result<Box<dyn ComicArchive>, ArchiveError> {
    open_file(source::ArchiveFile::open_path_unchecked(path)?, limits)
}

/// `ComicArchive` impl on the existing `Cbz` so `open_file()` can box it directly.
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
    fn recovery_used(&self) -> Option<&'static str> {
        self.recovery_used()
    }
    fn entries_skipped(&self) -> &[SkippedEntry] {
        self.entries_skipped()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub max_entries: u64,
    pub max_total_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_compression_ratio: u32,
    pub max_nesting_depth: u8,
    // NOTE (audit ARC-4): the two `subprocess_*` limits below are currently
    // INERT. CBR handling is in-process FFI (`unrar` reads to memory) — no child
    // process is ever spawned, so there is nothing to bound.
    // They are retained as the contract for a future out-of-process extractor;
    // until such a path exists, do not assume archive work is wall-clock/RSS
    // capped (per-entry/total byte caps above are the live defenses).
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

#[derive(Debug, Clone, thiserror::Error)]
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

impl ArchiveError {
    /// The RAR reader refused a multi-part set
    /// ([`cbr::MULTI_VOLUME_MESSAGE`]).
    pub fn is_multi_volume_rar(&self) -> bool {
        matches!(self, Self::Malformed(m) if m == cbr::MULTI_VOLUME_MESSAGE)
    }
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

/// An entry the archive crate dropped from the page index because a
/// soft defense fired — the compression-ratio cap in [`cbz::Cbz`], or
/// the content sniff in every reader (an image-named entry whose bytes
/// aren't an image; see [`image_sniff`]). Surfaced via
/// [`ComicArchive::entries_skipped`] so the scanner can emit a
/// structured health-issue instead of users discovering "wait, where
/// are pages 47 and 48?" months later.
#[derive(Debug, Clone)]
pub struct SkippedEntry {
    /// Entry name as recorded by the zip CD. May contain weird bytes;
    /// callers must defensively render.
    pub name: String,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    /// Static reason string — match-on safely. Today's universe:
    /// `"compression ratio cap"` (cbz.rs) and
    /// [`image_sniff::SKIP_REASON_NOT_AN_IMAGE`] (every reader). New
    /// reasons should be added near the code that emits them so they
    /// stay grep-able.
    pub reason: &'static str,
}

/// Recovery-flavor tag attached to an opened archive when one of the
/// recovery branches in [`cbz::open_zip_with_recovery`] fired. The
/// archive opens and reads normally; this tells the scanner that the
/// underlying file was structurally non-standard so it can emit a
/// `RecoveredArchive` health-issue. Static strings to keep them
/// grep-able.
pub mod recovery {
    /// The Info-ZIP Unicode-Path extra (`0x7075`) had a stored CRC32
    /// that didn't match the CDFH filename's CP437 bytes — pre-2026
    /// publisher tool bug. Recovery stripped every `0x7075` extra.
    /// zip ≥ 9 tolerates the bad extra itself (it falls back to the
    /// raw name), so this branch no longer fires for that file; it is
    /// kept for the error shape it matches, should the crate ever
    /// reject such an extra again.
    pub const UNICODE_PATH_CRC_STRIP: &str = "unicode-path-crc-strip";
    /// One or more CDFH `local_header_offset` fields pointed at
    /// garbage. Recovery scanned the data area forward, matched each
    /// entry's actual LFH by filename, and rewrote the CD with
    /// corrected offsets.
    pub const CDFH_OFFSET_REBUILD: &str = "cdfh-offset-rebuild";
}
