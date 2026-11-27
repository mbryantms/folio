//! `ComicArchive` trait — the operations the library scanner actually needs
//! from any comic archive format.
//!
//! Library Scanner v1, Milestone 12.
//!
//! Not part of the trait (intentionally):
//!   - `read_entry_range` (HTTP Range support) — only the formats with
//!     native random access ([`crate::cbz::Cbz`], [`crate::cbt::Cbt`])
//!     offer it, as inherent methods. Other formats would have to fully
//!     decompress per request, which is acceptable but lives in
//!     format-specific code.
//!   - `pipe_entry` / `build_pread_index` — same reasoning.
//!
//! Page-byte streaming is therefore wired for `.cbz` and `.cbt` (the
//! server's `zip_lru::CachedReader` dispatches on extension); `.cbr` and
//! `.cb7` can be converted to `.cbz` at scan time (per-library opt-ins).

use crate::{ArchiveEntry, ArchiveError, SkippedEntry};
use std::path::Path;

/// An entry read by [`ComicArchive::read_entry_bytes_salvaging`].
#[derive(Debug)]
pub struct EntryRead {
    pub bytes: Vec<u8>,
    /// Set when the bytes are what the reader could recover from an entry
    /// whose data is damaged; `None` for a clean read.
    pub damage: Option<EntryDamage>,
}

impl EntryRead {
    /// A clean read.
    pub fn intact(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            damage: None,
        }
    }
}

/// What was wrong with an entry read by
/// [`ComicArchive::read_entry_bytes_salvaging`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDamage {
    /// Static reason, see [`crate::damage`].
    pub reason: &'static str,
    /// The size the archive declared for the entry; the recovered bytes
    /// may be shorter.
    pub declared_size: u64,
}

/// Common surface for the scan pipeline. Any format we recognize implements
/// this; [`crate::open_file`] dispatches to the right reader on the sniffed
/// container. Every reader reads through the handle it was opened from
/// ([`crate::source`]).
pub trait ComicArchive: Send {
    fn entries(&self) -> &[ArchiveEntry];
    /// Image entries in natural-sort order. Caller treats this as the page
    /// list (spec §6.6).
    ///
    /// "Image" means the name carries an image extension **and** the
    /// leading bytes carry an image signature ([`crate::image_sniff`]).
    /// Readers run the sniff once at open; entries that fail it are
    /// excluded here and reported via [`Self::entries_skipped`].
    fn pages(&self) -> Vec<&ArchiveEntry>;
    /// Case-insensitive lookup of a metadata-style entry (`ComicInfo.xml`,
    /// `MetronInfo.xml`, etc.).
    fn find(&self, name: &str) -> Option<&ArchiveEntry>;
    /// Read the named entry into memory, applying the archive's per-entry
    /// size cap. Mutable because formats like RAR/7z need to advance an
    /// internal stream cursor.
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError>;
    /// Read at most `max_bytes` from the named entry. Implementations with
    /// random access should avoid inflating/reading the whole entry; the
    /// default keeps older formats correct until they can specialize.
    fn read_entry_prefix(&mut self, name: &str, max_bytes: usize) -> Result<Vec<u8>, ArchiveError> {
        let mut bytes = self.read_entry_bytes(name)?;
        bytes.truncate(max_bytes);
        Ok(bytes)
    }
    /// Read the named entry like [`Self::read_entry_bytes`], except that
    /// an entry whose *data* is damaged (it fails its checksum) comes back
    /// with whatever the reader decoded and an [`EntryDamage`] saying so,
    /// instead of an error. Every other failure, and every limit, is the
    /// same as `read_entry_bytes`. For converting a damaged archive with
    /// its damage recorded rather than refusing the whole file. Defaulted
    /// to a plain read; CBR overrides it.
    fn read_entry_bytes_salvaging(&mut self, name: &str) -> Result<EntryRead, ArchiveError> {
        self.read_entry_bytes(name).map(EntryRead::intact)
    }
    fn path(&self) -> &Path;

    /// If a recovery branch fired during open, returns a static tag
    /// naming the technique (see [`crate::recovery`]); otherwise `None`.
    /// Defaulted to `None`; CBZ and CBR override it (CB7/CBT inherit
    /// without changes). The scanner surfaces a `RecoveredArchive`
    /// health-issue when this is `Some`.
    fn recovery_used(&self) -> Option<&'static str> {
        None
    }

    /// Entries the archive dropped from the page index because a soft
    /// defense fired during open (`Cbz`'s compression-ratio cap, or the
    /// content sniff every reader runs over image-named entries). The
    /// scanner surfaces a `SkippedArchiveEntries` health-issue when
    /// non-empty. Defaulted to empty for readers with nothing to report.
    fn entries_skipped(&self) -> &[SkippedEntry] {
        &[]
    }

    /// The CRC-32 the archive records for the named entry's data, when the
    /// format keeps one that can be read without decoding (the ZIP central
    /// directory). `None` otherwise; CBZ overrides it.
    fn entry_crc32(&self, _name: &str) -> Option<u32> {
        None
    }
}
