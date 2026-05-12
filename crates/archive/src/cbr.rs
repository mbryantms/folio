//! CBR (RAR-archived) comic reader — **scaffolded only** in Library
//! Scanner v1, Milestone 12.
//!
//! `unrar` 0.5 is in the workspace deps and the wiring point exists in
//! [`crate::open`], but the full implementation (entry enumeration with
//! the typestate-based cursor API, security-limit enforcement, two-pass
//! read so `read_entry_bytes` doesn't have to re-walk the archive) is
//! deferred. Today we surface a clear "format not implemented" so the
//! scanner can emit a `UnsupportedArchiveFormat` health issue.
//!
//! Documented carry-over to a follow-up plan; the trait shape is in place,
//! so dropping in a real impl is a one-file change.

use crate::{ArchiveEntry, ArchiveError, ArchiveLimits, comic_archive::ComicArchive};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Cbr {
    path: PathBuf,
}

impl Cbr {
    pub fn open(path: impl AsRef<Path>, _limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        Err(ArchiveError::Malformed(format!(
            "CBR support not yet implemented (path: {})",
            path.as_ref().display()
        )))
    }
}

impl ComicArchive for Cbr {
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
            "CBR support not yet implemented".into(),
        ))
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
