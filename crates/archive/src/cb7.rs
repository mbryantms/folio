//! CB7 (7z-archived) comic reader — **scaffolded only** in Library
//! Scanner v1, Milestone 12. Same status as [`crate::cbr`].
//!
//! The wiring point exists in [`crate::open`]; the full implementation
//! (decompress per entry to a bounded buffer, enforce ratio guard, expose
//! entries) is deferred.
//!
//! No 7z decoder is currently in the dependency graph. `sevenz-rust` used to
//! be pinned here unused; it was dropped because it is abandoned and carries
//! an unfixable extraction path-traversal advisory (RUSTSEC-2026-0245 /
//! RUSTSEC-2026-0246). When CB7 is implemented, take `sevenz-rust2` — the
//! maintained fork the advisory points at — and route entry extraction
//! through the same [`ArchiveLimits`](crate::ArchiveLimits) guards `cbz`
//! applies, rather than any library-provided extract-to-directory helper.

use crate::{ArchiveEntry, ArchiveError, ArchiveLimits, comic_archive::ComicArchive};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Cb7 {
    path: PathBuf,
}

impl Cb7 {
    pub fn open(path: impl AsRef<Path>, _limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        Err(ArchiveError::Malformed(format!(
            "CB7 support not yet implemented (path: {})",
            path.as_ref().display()
        )))
    }
}

impl ComicArchive for Cb7 {
    fn entries(&self) -> &[ArchiveEntry] {
        &[]
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        Vec::new()
    }
    fn find(&self, _name: &str) -> Option<&ArchiveEntry> {
        None
    }
    fn read_entry_bytes(&mut self, _name: &str) -> Result<Vec<u8>, ArchiveError> {
        Err(ArchiveError::Malformed(
            "CB7 support not yet implemented".into(),
        ))
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
