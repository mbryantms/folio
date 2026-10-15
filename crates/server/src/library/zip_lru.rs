//! Bounded LRU of open archive handles (§7.2.1).
//!
//! Keeps `(issue_id → Arc<Mutex<CachedReader>>)` so repeat page reads of a
//! hot issue avoid re-parsing the archive index and re-opening the FD.
//! Eviction drops the reader, which closes the underlying `File`.
//!
//! The name predates multi-format support: the cache started as CBZ-only
//! and the `zip_lru` config / metric names are public surface, so they
//! stay. Today it dispatches on the sniffed container
//! ([`archive::container`]), falling back to the extension when the bytes
//! carry no magic — a ZIP opens the zip reader, a tar opens the tar
//! reader, whatever the file is called. Both expose the same
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
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Open the reader matching `path`'s container (sniffed), falling back
    /// to its extension (case-insensitive) when the bytes carry no magic.
    pub fn open(path: &Path, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        use archive::container::{Container, detect_container};
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        let format = match detect_container(path)? {
            Container::Unknown => ext.clone().unwrap_or_default(),
            known => known.comic_ext().unwrap_or_default().to_owned(),
        };
        match format.as_str() {
            "cbz" => Cbz::open(path, limits).map(Self::Cbz),
            "cbt" => Cbt::open(path, limits).map(Self::Cbt),
            other => Err(ArchiveError::Malformed(format!(
                "no page reader for archive format: {other:?} (extension {ext:?})"
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
    /// Post-audit T3.3: one gate per issue being opened. Concurrent misses
    /// for the same issue (a page strip's N parallel requests, a thumb job
    /// racing the reader) wait on the gate and reuse the winner's handle
    /// instead of each opening + indexing the archive and discarding all
    /// but one. Entries live only while an open is in flight.
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Archives actually opened (cache misses that reached the disk).
    /// Observable for tests; the `MISSES` counter is the metric.
    opens: AtomicU64,
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
        let me = Self {
            inner,
            in_flight: Mutex::new(HashMap::new()),
            opens: AtomicU64::new(0),
            limits,
        };
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
    ///
    /// Async since post-audit T3.3: a miss opens and indexes the archive
    /// on the blocking pool (it reads the central directory and every
    /// stored entry's local header — milliseconds to hundreds of ms on a
    /// cold NAS), and concurrent misses for one issue open it once.
    pub async fn get_or_open(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<Arc<Mutex<CachedReader>>, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, path).await?.0.0)
    }

    /// Archives opened from disk so far (not served from the cache).
    pub fn opens(&self) -> u64 {
        self.opens.load(Ordering::Relaxed)
    }

    /// Like [`Self::get_or_open`] but also returns the [`PreadIndex`] for the
    /// issue's stored entries, so the page server can read uncompressed pages
    /// lock-free (PERF-3). The index is computed once at open and cached.
    pub async fn get_or_open_indexed(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<CachedArchive, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, path).await?.0)
    }

    /// Like [`Self::get_or_open_indexed`], plus whether this call opened
    /// the archive (a cache miss). The page server uses the flag to kick
    /// off once-per-open work (the WP-8.4 page-hash backfill) without
    /// repeating it on every page request.
    pub async fn get_or_open_indexed_tracked(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        self.get_or_open_entry(issue_id, path).await
    }

    /// Cache fast path: a hit bumps the LRU and the `HITS` counter.
    fn cached(&self, issue_id: &str) -> Option<CachedArchive> {
        let mut cache = self.inner.lock().unwrap();
        let hit = cache.get(issue_id).cloned();
        if hit.is_some() {
            metrics::counter!(HITS).increment(1);
        }
        hit
    }

    async fn get_or_open_entry(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        if let Some(existing) = self.cached(issue_id) {
            return Ok((existing, false));
        }

        // Miss: single-flight per issue. The first caller through the gate
        // opens; everyone else waits on the gate, re-checks, and reuses the
        // winner's handle (the old loser-discard race opened the archive N
        // times for N concurrent misses and threw away N-1 handles).
        let gate = {
            let mut in_flight = self.in_flight.lock().unwrap();
            Arc::clone(
                in_flight
                    .entry(issue_id.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let result = {
            let _held = gate.lock().await;
            if let Some(existing) = self.cached(issue_id) {
                Ok((existing, false))
            } else {
                self.open_and_insert(issue_id, path).await
            }
        };
        // Drop the gate once no open is in flight for this issue. Another
        // miss may have installed a fresh gate meanwhile; only remove ours.
        {
            let mut in_flight = self.in_flight.lock().unwrap();
            if in_flight
                .get(issue_id)
                .is_some_and(|g| Arc::ptr_eq(g, &gate))
            {
                in_flight.remove(issue_id);
            }
        }
        result
    }

    /// Open + index on the blocking pool, then insert. Called with the
    /// issue's gate held, so this is the only open for `issue_id` in flight.
    async fn open_and_insert(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        let limits = self.limits;
        let path = path.to_path_buf();
        // CBZ open parses the central directory, CBT walks the tar headers;
        // building the pread index reads each Stored entry's local header
        // for CBZ. All synchronous file I/O — keep it off the async runtime.
        let entry: CachedArchive = tokio::task::spawn_blocking(move || {
            let reader = CachedReader::open(&path, limits)?;
            let pread = Arc::new(reader.build_pread_index());
            Ok::<_, ArchiveError>((Arc::new(Mutex::new(reader)), pread))
        })
        .await
        .map_err(|e| ArchiveError::Io(format!("archive open task failed: {e}")))??;
        self.opens.fetch_add(1, Ordering::Relaxed);

        let mut cache = self.inner.lock().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_cbz(path: &Path, pages: u32) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 0..pages {
            let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
            png.extend_from_slice(&i.to_le_bytes());
            png.extend(std::iter::repeat_n(0u8, 32));
            zw.start_file(format!("p{i:03}.png"), opts).unwrap();
            zw.write_all(&png).unwrap();
        }
        zw.finish().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_misses_open_the_archive_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cbz");
        write_cbz(&path, 3);
        let lru = Arc::new(ZipLru::new(8, ArchiveLimits::default()));

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let lru = Arc::clone(&lru);
            let path = path.clone();
            tasks.push(tokio::spawn(async move {
                lru.get_or_open_indexed_tracked("issue-a", &path).await
            }));
        }
        let mut opened_flags = 0;
        let mut handles: Vec<CachedArchive> = Vec::new();
        for t in tasks {
            let (entry, opened) = t.await.unwrap().expect("open");
            if opened {
                opened_flags += 1;
            }
            handles.push(entry);
        }
        assert_eq!(lru.opens(), 1, "16 concurrent misses must open once");
        assert_eq!(opened_flags, 1, "exactly one caller reports the open");
        // Everyone shares the one live handle.
        assert!(handles.iter().all(|(h, _)| Arc::ptr_eq(h, &handles[0].0)));
        // The gate is gone once the open finished.
        assert!(lru.in_flight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn distinct_issues_open_independently_and_invalidate_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        let b = dir.path().join("b.cbz");
        write_cbz(&a, 2);
        write_cbz(&b, 2);
        let lru = ZipLru::new(8, ArchiveLimits::default());
        lru.get_or_open("a", &a).await.unwrap();
        lru.get_or_open("b", &b).await.unwrap();
        lru.get_or_open("a", &a).await.unwrap();
        assert_eq!(lru.opens(), 2);
        lru.invalidate("a");
        lru.get_or_open("a", &a).await.unwrap();
        assert_eq!(lru.opens(), 3);
    }

    #[tokio::test]
    async fn a_failed_open_releases_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.cbz");
        let lru = ZipLru::new(8, ArchiveLimits::default());
        assert!(lru.get_or_open("x", &missing).await.is_err());
        assert!(lru.in_flight.lock().unwrap().is_empty());
        assert_eq!(lru.opens(), 0);
    }
}
