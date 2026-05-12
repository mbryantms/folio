//! CB7 (7z-archived) comic reader — **scaffolded only** in Library
//! Scanner v1, Milestone 12. Same status as [`crate::cbr`].
//!
//! `sevenz-rust` 0.6 is in the workspace deps and the wiring point exists
//! in [`crate::open`]; the full implementation (decompress per entry to a
//! bounded buffer, enforce ratio guard, expose entries) is deferred.

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
