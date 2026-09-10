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
//!   - [`Cbr::open`] additionally runs **one** process pass to content-
//!     sniff every image-named entry ([`crate::image_sniff`]). `unrar`
//!     has no partial read, so each candidate is decompressed once for
//!     its leading bytes — a full pass over the page data, comparable to
//!     what the scanner's dimension probe already costs on this format.
//!     Entries whose bytes aren't an image are dropped from the index and
//!     reported via [`ComicArchive::entries_skipped`].
//!
//! NOTICE: this file uses the `unrar` crate; its license requires
//! attribution to rarlab's UnRAR library.

use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, SkippedEntry, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name, image_sniff,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unrar::Archive;

const IGNORED_NAMES: &[&str] = &["Thumbs.db", "desktop.ini"];

#[derive(Debug)]
pub struct Cbr {
    path: PathBuf,
    entries: Vec<ArchiveEntry>,
    limits: ArchiveLimits,
    /// Image-named entries whose bytes failed the content sniff at open.
    skipped: Vec<SkippedEntry>,
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

        let mut me = Self {
            path: path_buf,
            entries,
            limits,
            skipped: Vec::new(),
        };
        me.drop_non_image_pages();
        Ok(me)
    }

    /// Content-sniff every page candidate in a single process pass and
    /// drop the ones whose leading bytes aren't an image signature. If the
    /// pass itself fails (damaged volume, unsupported method), every entry
    /// is kept — the per-entry read will report the real error later.
    fn drop_non_image_pages(&mut self) {
        let candidates: HashSet<String> = self
            .entries
            .iter()
            .filter(|e| image_sniff::has_image_extension(&e.name))
            .map(|e| e.name.to_ascii_lowercase())
            .collect();
        if candidates.is_empty() {
            return;
        }
        let non_images = match sniff_candidates(&self.path, &candidates, self.limits) {
            Ok(set) => set,
            Err(e) => {
                tracing::debug!(
                    path = %self.path.display(),
                    error = %e,
                    "cbr: content sniff pass failed; keeping every entry",
                );
                return;
            }
        };
        if non_images.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.entries);
        for entry in entries {
            if !non_images.contains(&entry.name.to_ascii_lowercase()) {
                self.entries.push(entry);
                continue;
            }
            tracing::warn!(
                path = %self.path.display(),
                entry = %entry.name,
                size = entry.uncompressed_size,
                "cbr: dropping image-named entry whose bytes aren't an image",
            );
            self.skipped.push(SkippedEntry {
                name: entry.name.clone(),
                uncompressed_size: entry.uncompressed_size,
                compressed_size: entry.compressed_size,
                reason: image_sniff::SKIP_REASON_NOT_AN_IMAGE,
            });
        }
    }
}

/// One front-to-back process pass: `read` every entry whose canonical name
/// is in `candidates`, `skip` the rest. Returns the canonical names whose
/// bytes did **not** sniff as an image.
fn sniff_candidates(
    path: &Path,
    candidates: &HashSet<String>,
    limits: ArchiveLimits,
) -> Result<HashSet<String>, ArchiveError> {
    let mut non_images = HashSet::new();
    let mut cursor = Archive::new(path)
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
        let is_candidate = header.is_file()
            && header.unpacked_size <= limits.max_entry_bytes
            && candidates.contains(&canonical);
        if !is_candidate {
            cursor = open
                .skip()
                .map_err(|e| ArchiveError::Malformed(format!("cbr skip: {e}")))?;
            continue;
        }
        let (data, next) = open
            .read()
            .map_err(|e| ArchiveError::Malformed(format!("cbr read: {e}")))?;
        let head = &data[..data.len().min(image_sniff::SNIFF_LEN)];
        if image_sniff::sniff(head).is_none() {
            non_images.insert(canonical);
        }
        cursor = next;
    }
    Ok(non_images)
}

impl ComicArchive for Cbr {
    fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        let mut imgs: Vec<&ArchiveEntry> = self
            .entries
            .iter()
            .filter(|e| image_sniff::has_image_extension(&e.name))
            .collect();
        imgs.sort_by(|a, b| natord::compare(&a.name, &b.name));
        imgs
    }
    fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        let lower = name.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == lower)
    }
    fn entries_skipped(&self) -> &[SkippedEntry] {
        &self.skipped
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
