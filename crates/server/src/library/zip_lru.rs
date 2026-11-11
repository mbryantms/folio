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
//!
//! ## Handles, not paths
//!
//! The cache never opens a path. Every lookup passes the archive file the
//! caller just opened through the no-follow check
//! ([`crate::library::no_follow`]), as an [`ArchiveFile`]. On a miss the
//! reader is built from that handle and keeps reading through it (the
//! archive crate never reopens by name). On a hit the cached reader is
//! reused only when it was opened from the same version of the same file
//! as the caller's handle: the [`FileStamp`] (device, inode, size,
//! modification time, status-change time) the opener took with its one
//! `fstat`. Otherwise the entry is dropped and rebuilt from the caller's
//! handle. So a path that has since been replaced by a link is refused
//! before the cache is consulted, a path now naming a different file never
//! serves the old reader, and a file rewritten in place (same inode, new
//! size or times) never serves the old reader's entry offsets against the
//! new bytes (audit R-05). The comparison costs nothing beyond the `fstat`
//! every request already makes when it opens the file, so it runs on every
//! lookup rather than on a timer.
//!
//! ## Failed opens
//!
//! A file that does not open as an archive (malformed, truncated, a
//! format with no page reader) is remembered for [`FAILED_OPEN_TTL`],
//! keyed on the issue and the file's stamp, in a bounded map
//! ([`FAILED_OPEN_CAPACITY`] entries). Repeat requests inside that window
//! get the same error without reading the file again (audit S-23: a
//! broken archive was re-read in full on every page request). A repaired
//! or replaced file has a different stamp and is opened at once.
//!
//! ## A panic during a read
//!
//! A panic while a reader's lock is held poisons that lock. A poisoned
//! slot is never handed out again: the next lookup drops it and opens the
//! archive afresh (audit SRV-13), and [`lock_reader`] lets a caller that
//! already holds the poisoned handle fail that one request cleanly
//! instead of panicking.

use archive::cbt::Cbt;
use archive::cbz::{Cbz, PreadIndex};
use archive::{
    ArchiveEntry, ArchiveError, ArchiveFile, ArchiveLimits, ComicArchive, FileStamp, SkippedEntry,
};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A cached open archive: the locked handle (decompression / compressed reads)
/// plus the immutable lock-free read index for its stored entries (PERF-3),
/// both produced once at open. Cloning is two `Arc` bumps.
pub type CachedArchive = (Arc<Mutex<CachedReader>>, Arc<PreadIndex>);

fn stamp_of(file: &ArchiveFile) -> Result<FileStamp, ArchiveError> {
    file.stamp()
        .map_err(|e| ArchiveError::Io(format!("archive stat failed: {e}")))
}

/// A cache slot: the reader and the stamp of the file version it reads.
#[derive(Clone)]
struct Slot {
    archive: CachedArchive,
    stamp: FileStamp,
}

/// How long a failed open is remembered for one file version.
pub const FAILED_OPEN_TTL: Duration = Duration::from_secs(10);

/// How many failed opens are remembered at once (least recently failed
/// dropped first). Each entry is an issue id, a stamp and a short error.
pub const FAILED_OPEN_CAPACITY: usize = 256;

/// A remembered failed open.
struct FailedOpen {
    stamp: FileStamp,
    at: Instant,
    error: ArchiveError,
}

/// Lock a cached reader. A lock poisoned by a panic in an earlier read
/// is an error for this request (the reader's state is not trusted after
/// a panic part-way through a read), never a second panic; the cache
/// drops the poisoned slot on its next lookup.
pub fn lock_reader(
    reader: &Mutex<CachedReader>,
) -> Result<MutexGuard<'_, CachedReader>, ArchiveError> {
    reader
        .lock()
        .map_err(|_| ArchiveError::Io("archive reader unavailable after a failed read".into()))
}

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
    /// Open the reader matching the file's container (sniffed through the
    /// handle), falling back to the display path's extension
    /// (case-insensitive) when the bytes carry no magic. Reads only
    /// through `src`.
    pub fn open(src: ArchiveFile, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        use archive::container::{Container, detect_container};
        let ext = src
            .display_path()
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        let format = match detect_container(&src)? {
            Container::Unknown => ext.clone().unwrap_or_default(),
            known => known.comic_ext().unwrap_or_default().to_owned(),
        };
        match format.as_str() {
            "cbz" => Cbz::open_file(src, limits).map(Self::Cbz),
            "cbt" => Cbt::open_file(src, limits).map(Self::Cbt),
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
    inner: Mutex<LruCache<String, Slot>>,
    /// Post-audit T3.3: one gate per issue being opened. Concurrent misses
    /// for the same issue (a page strip's N parallel requests, a thumb job
    /// racing the reader) wait on the gate and reuse the winner's handle
    /// instead of each opening + indexing the archive and discarding all
    /// but one. Entries live only while an open is in flight.
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Archives actually opened (cache misses that reached the disk).
    /// Observable for tests; the `MISSES` counter is the metric.
    opens: AtomicU64,
    /// Opens attempted, successful or not (failed opens served from
    /// `failures` are not attempts). Observable for tests.
    open_attempts: AtomicU64,
    /// Recently failed opens, by issue id (see the module docs).
    failures: Mutex<LruCache<String, FailedOpen>>,
    failure_ttl: Duration,
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
            open_attempts: AtomicU64::new(0),
            failures: Mutex::new(LruCache::new(
                NonZeroUsize::new(FAILED_OPEN_CAPACITY).expect("non-zero"),
            )),
            failure_ttl: FAILED_OPEN_TTL,
            limits,
        };
        metrics::describe_gauge!(OPEN_FDS, "Open file descriptors held by the ZIP LRU");
        metrics::describe_counter!(HITS, "ZIP LRU cache hits");
        metrics::describe_counter!(MISSES, "ZIP LRU cache misses");
        metrics::describe_counter!(EVICTIONS, "ZIP LRU evictions");
        me.update_gauge();
        me
    }

    /// Acquire a handle to the issue's reader, building it from `file` (the
    /// archive the caller opened through the no-follow check) on a miss, or
    /// when the cached reader was opened from a different file. The
    /// returned `Arc<Mutex<CachedReader>>` lives at least as long as the
    /// caller holds it, even if the LRU evicts it in the meantime.
    ///
    /// Async since post-audit T3.3: a miss indexes the archive on the
    /// blocking pool (it reads the central directory and every stored
    /// entry's local header — milliseconds to hundreds of ms on a cold
    /// NAS), and concurrent misses for one issue open it once.
    pub async fn get_or_open(
        &self,
        issue_id: &str,
        file: ArchiveFile,
    ) -> Result<Arc<Mutex<CachedReader>>, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, file).await?.0.0)
    }

    /// Archives opened from disk so far (not served from the cache).
    pub fn opens(&self) -> u64 {
        self.opens.load(Ordering::Relaxed)
    }

    /// Opens attempted so far, failed ones included (a failure served from
    /// the failed-open memory is not an attempt).
    pub fn open_attempts(&self) -> u64 {
        self.open_attempts.load(Ordering::Relaxed)
    }

    /// The same cache with a different failed-open window (tests).
    #[must_use]
    pub fn with_failure_ttl(mut self, ttl: Duration) -> Self {
        self.failure_ttl = ttl;
        self
    }

    /// A remembered failure for this issue and file version, if still fresh.
    fn remembered_failure(&self, issue_id: &str, stamp: FileStamp) -> Option<ArchiveError> {
        let mut failures = self.failures.lock().unwrap_or_else(|p| p.into_inner());
        match failures.peek(issue_id) {
            Some(f) if f.stamp == stamp && f.at.elapsed() < self.failure_ttl => {
                Some(f.error.clone())
            }
            Some(_) => {
                failures.pop(issue_id);
                None
            }
            None => None,
        }
    }

    /// Like [`Self::get_or_open`] but also returns the [`PreadIndex`] for the
    /// issue's stored entries, so the page server can read uncompressed pages
    /// lock-free (PERF-3). The index is computed once at open and cached.
    pub async fn get_or_open_indexed(
        &self,
        issue_id: &str,
        file: ArchiveFile,
    ) -> Result<CachedArchive, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, file).await?.0)
    }

    /// Like [`Self::get_or_open_indexed`], plus whether this call opened
    /// the archive (a cache miss). The page server uses the flag to kick
    /// off once-per-open work (the WP-8.4 page-hash backfill) without
    /// repeating it on every page request.
    pub async fn get_or_open_indexed_tracked(
        &self,
        issue_id: &str,
        file: ArchiveFile,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        self.get_or_open_entry(issue_id, file).await
    }

    /// Cache fast path: a hit bumps the LRU and the `HITS` counter. A
    /// cached reader opened from a different file or file version than
    /// `stamp`, or one whose lock a panic poisoned, is not a hit: the slot
    /// is dropped so the caller's handle replaces it.
    fn cached(&self, issue_id: &str, stamp: FileStamp) -> Option<CachedArchive> {
        let mut cache = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match cache.get(issue_id) {
            Some(slot) if slot.stamp == stamp && !slot.archive.0.is_poisoned() => {
                metrics::counter!(HITS).increment(1);
                Some(slot.archive.clone())
            }
            Some(_) => {
                cache.pop(issue_id);
                metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
                None
            }
            None => None,
        }
    }

    /// The one lookup path: the caller's handle is compared with the cached
    /// slot by [`FileStamp`] (see the module docs), and a recently failed
    /// open of the same file version answers from memory.
    async fn get_or_open_entry(
        &self,
        issue_id: &str,
        file: ArchiveFile,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        let stamp = stamp_of(&file)?;
        if let Some(existing) = self.cached(issue_id, stamp) {
            return Ok((existing, false));
        }
        if let Some(err) = self.remembered_failure(issue_id, stamp) {
            return Err(err);
        }

        // Miss: single-flight per issue. The first caller through the gate
        // opens; everyone else waits on the gate, re-checks, and reuses the
        // winner's handle (the old loser-discard race opened the archive N
        // times for N concurrent misses and threw away N-1 handles).
        let gate = {
            let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                in_flight
                    .entry(issue_id.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let result = {
            let _held = gate.lock().await;
            if let Some(existing) = self.cached(issue_id, stamp) {
                Ok((existing, false))
            } else if let Some(err) = self.remembered_failure(issue_id, stamp) {
                Err(err)
            } else {
                self.open_and_insert(issue_id, file, stamp).await
            }
        };
        // Drop the gate once no open is in flight for this issue. Another
        // miss may have installed a fresh gate meanwhile; only remove ours.
        {
            let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
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
        file: ArchiveFile,
        stamp: FileStamp,
    ) -> Result<(CachedArchive, bool), ArchiveError> {
        let limits = self.limits;
        self.open_attempts.fetch_add(1, Ordering::Relaxed);
        // CBZ open parses the central directory, CBT walks the tar headers;
        // building the pread index reads each Stored entry's local header
        // for CBZ. All synchronous file I/O — keep it off the async runtime.
        let entry: Result<CachedArchive, ArchiveError> = tokio::task::spawn_blocking(move || {
            let reader = CachedReader::open(file, limits)?;
            let pread = Arc::new(reader.build_pread_index());
            Ok::<_, ArchiveError>((Arc::new(Mutex::new(reader)), pread))
        })
        .await
        .map_err(|e| ArchiveError::Io(format!("archive open task failed: {e}")))
        .and_then(|r| r);
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                self.failures.lock().unwrap_or_else(|p| p.into_inner()).put(
                    issue_id.to_owned(),
                    FailedOpen {
                        stamp,
                        at: Instant::now(),
                        error: error.clone(),
                    },
                );
                return Err(error);
            }
        };
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.failures
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop(issue_id);

        let mut cache = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let evicted = cache.push(
            issue_id.to_owned(),
            Slot {
                archive: entry.clone(),
                stamp,
            },
        );
        if evicted.is_some() {
            metrics::counter!(EVICTIONS).increment(1);
        }
        metrics::counter!(MISSES).increment(1);
        metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        Ok((entry, true))
    }

    /// Drop the issue's cached reader and any remembered failed open.
    pub fn invalidate(&self, issue_id: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop(issue_id);
        let mut cache = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if cache.pop(issue_id).is_some() {
            metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        }
    }

    fn update_gauge(&self) {
        let cache = self.inner.lock().unwrap_or_else(|p| p.into_inner());
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
            let file = ArchiveFile::open_path_unchecked(&path).unwrap();
            tasks.push(tokio::spawn(async move {
                lru.get_or_open_indexed_tracked("issue-a", file).await
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
        let open = |p: &Path| ArchiveFile::open_path_unchecked(p).unwrap();
        lru.get_or_open("a", open(&a)).await.unwrap();
        lru.get_or_open("b", open(&b)).await.unwrap();
        lru.get_or_open("a", open(&a)).await.unwrap();
        assert_eq!(lru.opens(), 2);
        lru.invalidate("a");
        lru.get_or_open("a", open(&a)).await.unwrap();
        assert_eq!(lru.opens(), 3);
    }

    #[tokio::test]
    async fn a_failed_open_releases_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.cbz");
        std::fs::write(&junk, b"not an archive").unwrap();
        let lru = ZipLru::new(8, ArchiveLimits::default());
        let file = ArchiveFile::open_path_unchecked(&junk).unwrap();
        assert!(lru.get_or_open("x", file).await.is_err());
        assert!(lru.in_flight.lock().unwrap().is_empty());
        assert_eq!(lru.opens(), 0);
    }

    /// A cached reader serves only the file it was opened from: when the
    /// issue's path now names a different file, the caller's fresh handle
    /// replaces the entry and its pages are what is read.
    #[tokio::test]
    async fn a_hit_for_a_different_file_rebuilds_from_the_callers_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cbz");
        write_cbz(&path, 2);
        let lru = ZipLru::new(8, ArchiveLimits::default());
        let first = lru
            .get_or_open("a", ArchiveFile::open_path_unchecked(&path).unwrap())
            .await
            .unwrap();
        // Same file again: a hit.
        let again = lru
            .get_or_open("a", ArchiveFile::open_path_unchecked(&path).unwrap())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(lru.opens(), 1);

        // The name now holds a different file (5 pages).
        let replacement = dir.path().join("b.cbz");
        write_cbz(&replacement, 5);
        std::fs::rename(&replacement, &path).unwrap();
        let swapped = lru
            .get_or_open("a", ArchiveFile::open_path_unchecked(&path).unwrap())
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &swapped), "not the stale reader");
        assert_eq!(lru.opens(), 2);
        assert_eq!(swapped.lock().unwrap().pages().len(), 5);
        // The old reader still reads the file it was opened from.
        assert_eq!(first.lock().unwrap().pages().len(), 2);
    }

    /// Audit S-23: a file that does not open is read once per window, not
    /// once per request; a different version of the file is tried at once,
    /// and so is the same version once the window has passed.
    #[tokio::test]
    async fn a_failed_open_is_remembered_per_file_version() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.cbz");
        std::fs::write(&junk, b"not an archive, and never will be").unwrap();
        let lru = ZipLru::new(8, ArchiveLimits::default());
        let open = || ArchiveFile::open_path_unchecked(&junk).unwrap();
        for _ in 0..20 {
            assert!(matches!(
                lru.get_or_open("x", open()).await,
                Err(ArchiveError::Malformed(_))
            ));
        }
        assert_eq!(lru.open_attempts(), 1, "20 requests, one read of the file");

        // Repaired in place (same name, new content): opened at once.
        write_cbz(&junk, 2);
        let reader = lru
            .get_or_open("x", open())
            .await
            .expect("repaired file opens");
        assert_eq!(reader.lock().unwrap().pages().len(), 2);
        assert_eq!(lru.open_attempts(), 2);
        assert_eq!(lru.opens(), 1);
    }

    #[tokio::test]
    async fn a_remembered_failure_expires() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.cbz");
        std::fs::write(&junk, b"not an archive").unwrap();
        let lru =
            ZipLru::new(8, ArchiveLimits::default()).with_failure_ttl(Duration::from_millis(50));
        let open = || ArchiveFile::open_path_unchecked(&junk).unwrap();
        assert!(lru.get_or_open("x", open()).await.is_err());
        assert!(lru.get_or_open("x", open()).await.is_err());
        assert_eq!(lru.open_attempts(), 1);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(lru.get_or_open("x", open()).await.is_err());
        assert_eq!(lru.open_attempts(), 2);
        // `invalidate` forgets it too.
        lru.invalidate("x");
        assert!(lru.get_or_open("x", open()).await.is_err());
        assert_eq!(lru.open_attempts(), 3);
    }

    /// Audit R-05, same inode: a file truncated and rewritten in place keeps
    /// its device and inode, so only the size / times show it changed. The
    /// cached reader must not be reused against the new bytes.
    #[tokio::test]
    async fn a_file_rewritten_in_place_is_reopened() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cbz");
        write_cbz(&path, 2);
        let ino = std::fs::metadata(&path).unwrap().ino();
        let lru = ZipLru::new(8, ArchiveLimits::default());
        let open = || ArchiveFile::open_path_unchecked(&path).unwrap();
        let first = lru.get_or_open("a", open()).await.unwrap();
        assert_eq!(first.lock().unwrap().pages().len(), 2);

        // `File::create` truncates the existing inode and writes into it.
        write_cbz(&path, 4);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), ino, "same inode");
        let second = lru.get_or_open("a", open()).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &second), "not the stale reader");
        assert_eq!(second.lock().unwrap().pages().len(), 4);
        assert_eq!(lru.opens(), 2);
        // Unchanged since: a hit.
        let third = lru.get_or_open("a", open()).await.unwrap();
        assert!(Arc::ptr_eq(&second, &third));
        assert_eq!(lru.opens(), 2);
    }

    /// Audit SRV-13: a panic while a read holds the reader's lock poisons
    /// it. The poisoned slot is not handed out again: the next lookup opens
    /// a fresh reader, and a holder of the old handle gets an error, not a
    /// second panic.
    #[tokio::test]
    async fn a_reader_poisoned_by_a_panic_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.cbz");
        write_cbz(&path, 3);
        let lru = ZipLru::new(8, ArchiveLimits::default());
        let open = || ArchiveFile::open_path_unchecked(&path).unwrap();
        let first = lru.get_or_open("a", open()).await.unwrap();

        // Test-only injection: panic while holding the lock, as a reader
        // that panicked part-way through a read would.
        let held = Arc::clone(&first);
        let joined = std::thread::spawn(move || {
            let _guard = held.lock().unwrap();
            panic!("injected panic during a read");
        })
        .join();
        assert!(joined.is_err());
        assert!(first.is_poisoned());
        assert!(lock_reader(&first).is_err(), "an error, not a panic");

        let rebuilt = lru.get_or_open("a", open()).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert!(!rebuilt.is_poisoned());
        assert_eq!(lock_reader(&rebuilt).unwrap().pages().len(), 3);
        assert_eq!(lru.opens(), 2);
    }
}
