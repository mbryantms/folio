//! Bounded LRU of open archive handles (§7.2.1).
//!
//! Keeps `(issue_id → Arc<Mutex<CachedReader>>)` so repeat page reads of a
//! hot issue avoid re-parsing the archive index and re-opening the FD.
//! Eviction drops the reader, which closes the underlying `File`.
//!
//! The name predates multi-format support: the cache started as CBZ-only
//! and the `zip_lru` config / metric names are public surface, so they
//! stay. Today it dispatches on extension — `.cbz` opens the zip reader
//! unchanged, `.cbt` opens the tar reader. Both expose the same
//! random-access page surface ([`CachedReader`]) and a [`PreadIndex`], so
//! the page server's zero-lock streaming path is format-agnostic.
//!
//! All access goes through a short critical section on the cache mutex; the
//! per-entry `Mutex<CachedReader>` is held for the duration of a locked
//! read. Reads are I/O-bound and brief; the brief mutex hold is fine for v1.

use archive::cbt::Cbt;
use archive::cbz::{Cbz, PreadIndex};
use archive::{ArchiveEntry, ArchiveError, ArchiveLimits, ComicArchive, SkippedEntry};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A cached open archive: the locked handle (decompression / compressed reads)
/// plus the immutable lock-free read index for its stored entries (PERF-3),
/// both produced once at open. Cloning is two `Arc` bumps.
pub type CachedArchive = (Arc<Mutex<CachedReader>>, Arc<PreadIndex>);

const HITS: &str = "folio_zip_lru_hits_total";
const MISSES: &str = "folio_zip_lru_misses_total";
const EVICTIONS: &str = "folio_zip_lru_evictions_total";
const OPEN_FDS: &str = "folio_zip_lru_open_fds";

/// The per-issue reader the page server holds. Each variant offers the
/// random-access surface (`pages` / `read_entry_range` / `pipe_entry`)
/// the byte-serving handlers need, so they never branch on format.
///
/// Formats without a random-access reader (`.cbr`, `.cb7`) are not
/// cached here: [`CachedReader::open`] returns [`ArchiveError::Malformed`]
/// for them, which the handlers map to `archive_unreadable` exactly as
/// before. A library that opts into `auto_convert_cbr_on_scan` /
/// `auto_convert_cb7_on_scan` gets its CBRs / CB7s rewritten to CBZ at scan
/// time (`scanner::cbr_convert`).
pub enum CachedReader {
    Cbz(Cbz),
    Cbt(Cbt),
}

impl CachedReader {
    /// Open the reader matching `path`'s extension (case-insensitive).
    pub fn open(path: &Path, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("cbz") => Cbz::open(path, limits).map(Self::Cbz),
            Some("cbt") => Cbt::open(path, limits).map(Self::Cbt),
            other => Err(ArchiveError::Malformed(format!(
                "no page reader for archive extension: {other:?}"
            ))),
        }
    }

    /// The zero-lock stream index. CBZ yields only its `Stored` entries;
    /// CBT yields every entry (tar stores everything verbatim).
    pub fn build_pread_index(&self) -> PreadIndex {
        match self {
            Self::Cbz(c) => c.build_pread_index(),
            Self::Cbt(c) => c.build_pread_index(),
        }
    }

    /// Image entries in natural-sort order (the page list).
    pub fn pages(&self) -> Vec<&ArchiveEntry> {
        match self {
            Self::Cbz(c) => c.pages(),
            Self::Cbt(c) => ComicArchive::pages(c),
        }
    }

    /// Read `[start, start + len)` of an entry; clamped to the entry's
    /// size, per-entry cap enforced.
    pub fn read_entry_range(
        &mut self,
        entry: &ArchiveEntry,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, ArchiveError> {
        match self {
            Self::Cbz(c) => c.read_entry_range(entry, start, len),
            Self::Cbt(c) => c.read_entry_range(entry, start, len),
        }
    }

    /// Stream a whole entry into `sink`, caps enforced.
    pub fn pipe_entry<W: std::io::Write>(
        &mut self,
        entry: &ArchiveEntry,
        sink: &mut W,
    ) -> Result<u64, ArchiveError> {
        match self {
            Self::Cbz(c) => c.pipe_entry(entry, sink),
            Self::Cbt(c) => c.pipe_entry(entry, sink),
        }
    }
}

/// So a locked handle coerces to `&mut dyn ComicArchive` for the shared
/// thumbnail / decode helpers that already take the trait object.
impl ComicArchive for CachedReader {
    fn entries(&self) -> &[ArchiveEntry] {
        match self {
            Self::Cbz(c) => ComicArchive::entries(c),
            Self::Cbt(c) => ComicArchive::entries(c),
        }
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        CachedReader::pages(self)
    }
    fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        match self {
            Self::Cbz(c) => ComicArchive::find(c, name),
            Self::Cbt(c) => ComicArchive::find(c, name),
        }
    }
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        match self {
            Self::Cbz(c) => ComicArchive::read_entry_bytes(c, name),
            Self::Cbt(c) => ComicArchive::read_entry_bytes(c, name),
        }
    }
    fn read_entry_prefix(&mut self, name: &str, max_bytes: usize) -> Result<Vec<u8>, ArchiveError> {
        match self {
            Self::Cbz(c) => ComicArchive::read_entry_prefix(c, name, max_bytes),
            Self::Cbt(c) => ComicArchive::read_entry_prefix(c, name, max_bytes),
        }
    }
    fn path(&self) -> &Path {
        match self {
            Self::Cbz(c) => ComicArchive::path(c),
            Self::Cbt(c) => ComicArchive::path(c),
        }
    }
    fn recovery_used(&self) -> Option<&'static str> {
        match self {
            Self::Cbz(c) => ComicArchive::recovery_used(c),
            Self::Cbt(c) => ComicArchive::recovery_used(c),
        }
    }
    fn entries_skipped(&self) -> &[SkippedEntry] {
        match self {
            Self::Cbz(c) => ComicArchive::entries_skipped(c),
            Self::Cbt(c) => ComicArchive::entries_skipped(c),
        }
    }
}

pub struct ZipLru {
    inner: Mutex<LruCache<String, CachedArchive>>,
    /// Archive caps applied at open time. Captured at boot from
    /// `Config::archive_limits()` so a `COMIC_ARCHIVE_MAX_*` override
    /// flows through every cached open. `Copy`, ~64 bytes — cheap to
    /// keep alongside the cache.
    limits: ArchiveLimits,
}

impl ZipLru {
    pub fn new(capacity: usize, limits: ArchiveLimits) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        let inner = Mutex::new(LruCache::new(cap));
        let me = Self { inner, limits };
        metrics::describe_gauge!(OPEN_FDS, "Open file descriptors held by the ZIP LRU");
        metrics::describe_counter!(HITS, "ZIP LRU cache hits");
        metrics::describe_counter!(MISSES, "ZIP LRU cache misses");
        metrics::describe_counter!(EVICTIONS, "ZIP LRU evictions");
        me.update_gauge();
        me
    }

    /// Acquire a handle to the issue's reader, opening (and inserting) on
    /// miss. The returned `Arc<Mutex<CachedReader>>` lives at least as long
    /// as the caller holds it, even if the LRU evicts it in the meantime.
    pub fn get_or_open(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<Arc<Mutex<CachedReader>>, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, path)?.0.0)
    }

    /// Like [`Self::get_or_open`] but also returns the [`PreadIndex`] for the
    /// issue's stored entries, so the page server can read uncompressed pages
    /// lock-free (PERF-3). The index is computed once at open and cached.
    pub fn get_or_open_indexed(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<CachedArchive, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, path)?.0)
    }

    /// Like [`Self::get_or_open_indexed`], plus whether this call opened
    /// the archive (a cache miss). The page server uses the flag to kick
    /// off once-per-open work (the WP-8.4 page-hash backfill) without
    /// repeating it on every page request.
    pub fn get_or_open_indexed_tracked(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        self.get_or_open_entry(issue_id, path)
    }

    fn get_or_open_entry(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        {
            let mut cache = self.inner.lock().unwrap();
            if let Some(existing) = cache.get(issue_id) {
                metrics::counter!(HITS).increment(1);
                return Ok((existing.clone(), false));
            }
        }

        // Miss: open outside the lock (CBZ open parses the central directory,
        // CBT walks the tar headers; building the pread index reads each
        // Stored entry's local header for CBZ and is free for CBT).
        let reader = CachedReader::open(path, self.limits)?;
        let pread = Arc::new(reader.build_pread_index());
        let arc = Arc::new(Mutex::new(reader));
        let entry: CachedArchive = (arc, pread);

        let mut cache = self.inner.lock().unwrap();
        // Another caller may have raced and inserted while we were opening.
        // Honor the racing entry to keep a single live handle per issue.
        if let Some(existing) = cache.get(issue_id) {
            metrics::counter!(HITS).increment(1);
            return Ok((existing.clone(), false));
        }
        let evicted = cache.push(issue_id.to_owned(), entry.clone());
        if evicted.is_some() {
            metrics::counter!(EVICTIONS).increment(1);
        }
        metrics::counter!(MISSES).increment(1);
        metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        Ok((entry, true))
    }

    pub fn invalidate(&self, issue_id: &str) {
        let mut cache = self.inner.lock().unwrap();
        if cache.pop(issue_id).is_some() {
            metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        }
    }

    fn update_gauge(&self) {
        let cache = self.inner.lock().unwrap();
        metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
    }
}
