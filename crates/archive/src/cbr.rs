//! CBR (RAR-archived) comic reader — **read-only** (`archive-rewrite-1.0`
//! M6 fills in the body that was scaffolded in Library Scanner v1 M12).
//!
//! Backed by the `unrar` crate (high-level bindings over rarlab's unrar C
//! library — extract/list only; it cannot create RAR archives, which is
//! why the page editor *converts* CBR → CBZ rather than rewriting in
//! place). RAR is a stream-only format with no random access, so:
//!
//!   - [`Cbr::open_file`] does a cheap **list** pass: walk headers once to
//!     build the entry table + enforce the security caps. No payload is
//!     decompressed.
//!   - [`Cbr::read_entry_bytes`] does a **process** pass per call: walk
//!     from the front, `skip`-ing (no decompress) until the target entry,
//!     then `read` it. O(N) skips per read is cheap; only the requested
//!     entry is ever decompressed.
//!   - [`Cbr::open_file`] additionally runs **one** process pass to content-
//!     sniff image-named entries ([`crate::image_sniff`]). `unrar` has no
//!     partial read, so a candidate has to be decompressed in full just to
//!     see its leading bytes; the pass is therefore bounded to entries of
//!     at most [`CBR_SNIFF_MAX_ENTRY_BYTES`] — the misnamed-sidecar case
//!     the sniff exists for is a few KB of XML, while real pages are far
//!     larger and would make every open a whole-archive decompression.
//!     Bigger image-named entries stay trusted by extension (a large
//!     non-image still fails at the consumer, exactly as before). Entries
//!     whose bytes aren't an image are dropped from the index and reported
//!     via [`ComicArchive::entries_skipped`].
//!
//! **Handle, not name.** The UnRAR library opens archives by file name
//! only, so the reader cannot pass it the caller's handle directly. It
//! passes a name that reaches the handle instead
//! ([`ArchiveFile::rar_name`]): `/proc/self/fd/<n>` on Linux, which reopens
//! exactly the file the caller opened whatever happened to its path since.
//! Where `/proc/self/fd` is unavailable, or UnRAR cannot open the `/proc`
//! name (a sandbox that forbids reopening descriptors that way), the
//! display path is used instead and its device and inode are checked
//! against the handle before and after every pass, refusing a link or a
//! different file. The choice is made once, at open, and kept for every
//! later pass.
//!
//! Multi-part (multi-volume) RAR sets are not read: UnRAR would open the
//! sibling volumes by name, which no checked handle can cover. Opening one
//! fails with [`ArchiveError::Malformed`] carrying
//! [`MULTI_VOLUME_MESSAGE`] ([`ArchiveError::is_multi_volume_rar`]); the
//! scanner reports it as a malformed-archive finding with that reason.
//!
//! NOTICE: this file uses the `unrar` crate; its license requires
//! attribution to rarlab's UnRAR library.

use crate::source::ArchiveFile;
use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, SkippedEntry, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name, image_sniff,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unrar::{Archive, VolumeInfo};

/// The reason a multi-part RAR set is not read, as the user sees it.
pub const MULTI_VOLUME_MESSAGE: &str = "multi-part RAR archives are not supported: \
     only single-file archives are read; repack the parts as one archive";

const IGNORED_NAMES: &[&str] = &["Thumbs.db", "desktop.ini"];

/// Largest unpacked entry the open-time content sniff will decompress.
/// RAR offers no partial read, so every sniffed candidate costs a full
/// decompress; capping it keeps `Cbr::open_file` from turning into a pass over
/// every page. Comfortably above any plausible text sidecar, comfortably
/// below a real comic page.
pub const CBR_SNIFF_MAX_ENTRY_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
pub struct Cbr {
    /// The open file. Kept open for the reader's life: the name handed to
    /// UnRAR (`/proc/self/fd/<n>`) is only valid while it is.
    source: ArchiveFile,
    /// The name every pass hands UnRAR, chosen at open.
    rar: RarName,
    entries: Vec<ArchiveEntry>,
    limits: ArchiveLimits,
    /// Image-named entries whose bytes failed the content sniff at open.
    skipped: Vec<SkippedEntry>,
}

impl Cbr {
    /// Open a file outside any library root by name (links followed).
    /// Tests and temp files only; see [`crate::source`].
    pub fn open_path_unchecked(
        path: impl AsRef<Path>,
        limits: ArchiveLimits,
    ) -> Result<Self, ArchiveError> {
        Self::open_file(ArchiveFile::open_path_unchecked(path)?, limits)
    }

    /// Open from a file the caller already opened. Every pass reaches the
    /// same file (see the module docs).
    pub fn open_file(source: ArchiveFile, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let (listing, rar) = open_listing(&source)?;
        if !matches!(listing.volume_info(), VolumeInfo::None) {
            return Err(ArchiveError::Malformed(MULTI_VOLUME_MESSAGE.to_owned()));
        }

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

        rar.finish_pass(&source)?;
        let mut me = Self {
            source,
            rar,
            entries,
            limits,
            skipped: Vec::new(),
        };
        me.drop_non_image_pages();
        Ok(me)
    }

    /// Content-sniff the small page candidates (see
    /// [`CBR_SNIFF_MAX_ENTRY_BYTES`]) in a single process pass and drop
    /// the ones whose leading bytes aren't an image signature. If the pass
    /// itself fails (damaged volume, unsupported method), every entry is
    /// kept — the per-entry read will report the real error later.
    fn drop_non_image_pages(&mut self) {
        let candidates: HashSet<String> = self
            .entries
            .iter()
            .filter(|e| {
                image_sniff::has_image_extension(&e.name)
                    && e.uncompressed_size <= CBR_SNIFF_MAX_ENTRY_BYTES
            })
            .map(|e| e.name.to_ascii_lowercase())
            .collect();
        if candidates.is_empty() {
            return;
        }
        let non_images = match sniff_candidates(&self.source, &self.rar, &candidates, self.limits) {
            Ok(set) => set,
            Err(e) => {
                tracing::debug!(
                    path = %self.source.display_path().display(),
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
                path = %self.source.display_path().display(),
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
/// is in `candidates` (already size-bounded by the caller), `skip` the
/// rest. Returns the canonical names whose bytes did **not** sniff as an
/// image.
fn sniff_candidates(
    source: &ArchiveFile,
    rar: &RarName,
    candidates: &HashSet<String>,
    limits: ArchiveLimits,
) -> Result<HashSet<String>, ArchiveError> {
    let mut non_images = HashSet::new();
    let mut cursor = rar.open_for_processing(source)?;
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
    rar.finish_pass(source)?;
    Ok(non_images)
}

/// The name UnRAR opens for every pass over one reader's file, and whether
/// it is the checked fallback (see the module docs).
#[derive(Debug, Clone)]
struct RarName {
    name: PathBuf,
    /// The display path: its identity is checked before and after a pass.
    check: bool,
}

/// Open the listing pass and settle the name every later pass uses:
/// `/proc/self/fd/<n>` when [`ArchiveFile::rar_name`] offers it and UnRAR
/// can open it, else the display path under the identity check.
fn open_listing(
    source: &ArchiveFile,
) -> Result<
    (
        unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>,
        RarName,
    ),
    ArchiveError,
> {
    let (name, check) = source.rar_name();
    if !check {
        match Archive::new(&name).open_for_listing() {
            Ok(listing) => return Ok((listing, RarName { name, check: false })),
            Err(e) => tracing::debug!(
                path = %source.display_path().display(),
                error = %e,
                "cbr: opening through /proc/self/fd failed; using the checked path",
            ),
        }
    }
    let rar = RarName {
        name: source.display_path().to_path_buf(),
        check: true,
    };
    source.check_rar_name()?;
    let listing = Archive::new(&rar.name)
        .open_for_listing()
        .map_err(|e| ArchiveError::Malformed(format!("cbr open: {e}")))?;
    Ok((listing, rar))
}

impl RarName {
    /// Start a processing pass over the caller's file.
    fn open_for_processing(
        &self,
        source: &ArchiveFile,
    ) -> Result<unrar::OpenArchive<unrar::Process, unrar::CursorBeforeHeader>, ArchiveError> {
        if self.check {
            source.check_rar_name()?;
        }
        Archive::new(&self.name)
            .open_for_processing()
            .map_err(|e| ArchiveError::Malformed(format!("cbr open: {e}")))
    }

    /// End of a pass: on the fallback path, confirm the name still led to
    /// the caller's file for the whole pass.
    fn finish_pass(&self, source: &ArchiveFile) -> Result<(), ArchiveError> {
        if self.check {
            source.check_rar_name()?;
        }
        Ok(())
    }
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

        let mut cursor = self.rar.open_for_processing(&self.source)?;

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
                self.rar.finish_pass(&self.source)?;
                return Ok(data);
            }
            cursor = open
                .skip()
                .map_err(|e| ArchiveError::Malformed(format!("cbr skip: {e}")))?;
        }

        Err(ArchiveError::Malformed(format!("entry not found: {name}")))
    }
    fn path(&self) -> &Path {
        self.source.display_path()
    }
}

impl Cbr {
    /// Whether this reader went through the checked display path rather
    /// than `/proc/self/fd` (see the module docs). For tests.
    pub fn uses_checked_path(&self) -> bool {
        self.rar.check
    }
}
