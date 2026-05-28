//! CBR (RAR-archived) comic reader — **read-only** (`archive-rewrite-1.0`
//! M6 fills in the body that was scaffolded in Library Scanner v1 M12).
//!
//! Backed by the `unrar` crate (high-level bindings over rarlab's unrar C
//! library — extract/list only; it cannot create RAR archives, which is
//! why the page editor *converts* CBR → CBZ rather than rewriting in
//! place). RAR is a stream-only format with no random access, so:
//!
//!   - [`Cbr::open`] does a cheap **list** pass: walk headers once to
//!     build the entry table + enforce the security caps. No payload is
//!     decompressed.
//!   - [`Cbr::read_entry_bytes`] does a **process** pass per call: walk
//!     from the front, `skip`-ing (no decompress) until the target entry,
//!     then `read` it. O(N) skips per read is cheap; only the requested
//!     entry is ever decompressed.
//!
//! NOTICE: this file uses the `unrar` crate; its license requires
//! attribution to rarlab's UnRAR library.

use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name,
};
use std::path::{Path, PathBuf};
use unrar::Archive;

const IGNORED_NAMES: &[&str] = &["Thumbs.db", "desktop.ini"];

#[derive(Debug)]
pub struct Cbr {
    path: PathBuf,
    entries: Vec<ArchiveEntry>,
    limits: ArchiveLimits,
}

impl Cbr {
    pub fn open(path: impl AsRef<Path>, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let path_buf = path.as_ref().to_path_buf();
        let listing = Archive::new(&path_buf)
            .open_for_listing()
            .map_err(|e| ArchiveError::Malformed(format!("cbr open: {e}")))?;

        let mut entries: Vec<ArchiveEntry> = Vec::new();
        let mut total_bytes: u64 = 0;

        for item in listing {
            let header = item.map_err(|e| ArchiveError::Malformed(format!("cbr header: {e}")))?;
            if header.is_directory() {
                continue;
            }
            let raw_name = header.filename.to_string_lossy().into_owned();
            let safe = sanitize_entry_name(&raw_name)?;
            let safe_name = safe.display;

            let leaf = std::path::Path::new(&safe_name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&safe_name);
            if IGNORED_NAMES.contains(&leaf) || leaf.starts_with('.') || leaf == "__MACOSX" {
                continue;
            }

            let size = header.unpacked_size;
            if size > limits.max_entry_bytes {
                return Err(ArchiveError::CapExceeded("entry size"));
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded("total bytes"));
            }

            entries.push(ArchiveEntry {
                index: entries.len(),
                name: safe_name,
                uncompressed_size: size,
                compressed_size: size,
            });
            if entries.len() as u64 > limits.max_entries {
                return Err(ArchiveError::CapExceeded("entry count"));
            }
        }

        Ok(Self {
            path: path_buf,
            entries,
            limits,
        })
    }
}

impl ComicArchive for Cbr {
    fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        let mut imgs: Vec<&ArchiveEntry> =
            self.entries.iter().filter(|e| is_image(&e.name)).collect();
        imgs.sort_by(|a, b| natord::compare(&a.name, &b.name));
        imgs
    }
    fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        let lower = name.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == lower)
    }
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let want = sanitize_entry_name(name)
            .map(|s| s.canonical)
            .unwrap_or_else(|_| name.to_ascii_lowercase());

        let mut cursor = Archive::new(&self.path)
            .open_for_processing()
            .map_err(|e| ArchiveError::Malformed(format!("cbr open: {e}")))?;

        loop {
            let Some(open) = cursor
                .read_header()
                .map_err(|e| ArchiveError::Malformed(format!("cbr header: {e}")))?
            else {
                break;
            };
            let header = open.entry();
            let raw = header.filename.to_string_lossy().into_owned();
            let canonical = sanitize_entry_name(&raw)
                .map(|s| s.canonical)
                .unwrap_or_else(|_| raw.to_ascii_lowercase());

            if header.is_file() && canonical == want {
                if header.unpacked_size > self.limits.max_entry_bytes {
                    return Err(ArchiveError::CapExceeded("entry size"));
                }
                let (data, _next) = open
                    .read()
                    .map_err(|e| ArchiveError::Malformed(format!("cbr read: {e}")))?;
                return Ok(data);
            }
            cursor = open
                .skip()
                .map_err(|e| ArchiveError::Malformed(format!("cbr skip: {e}")))?;
        }

        Err(ArchiveError::Malformed(format!("entry not found: {name}")))
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

fn is_image(name: &str) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "avif" | "gif" | "jxl")
    )
}
