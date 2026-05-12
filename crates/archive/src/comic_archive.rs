//! `ComicArchive` trait — the operations the library scanner actually needs
//! from any comic archive format.
//!
//! Library Scanner v1, Milestone 12.
//!
//! Not part of the trait (intentionally):
//!   - `read_entry_range` (HTTP Range support) — only Cbz natively supports
//!     random access. Other formats would have to fully decompress per
//!     request, which is acceptable but lives in format-specific code.
//!   - `pipe_entry` — same reasoning.
//!
//! Page-byte streaming for `.cbr`/`.cb7`/`.cbt` is therefore deferred; the
//! reader UI today handles `.cbz` only via [`crate::cbz::Cbz`]'s richer API.

use crate::{ArchiveEntry, ArchiveError};
use std::path::Path;

/// Common surface for the scan pipeline. Any format we recognize implements
/// this; [`crate::open`] dispatches to the right reader on extension.
pub trait ComicArchive: Send {
    fn entries(&self) -> &[ArchiveEntry];
    /// Image entries in natural-sort order. Caller treats this as the page
    /// list (spec §6.6).
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
    fn path(&self) -> &Path;
}
