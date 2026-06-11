//! Bounded LRU of open `Cbz` handles (§7.2.1).
//!
//! Keeps `(issue_id → Arc<Mutex<Cbz>>)` so repeat page reads of a hot issue
//! avoid re-parsing the central directory and re-opening the FD. Eviction
//! drops the `Cbz`, which closes the underlying `File` (`zip` crate `Drop`).
//!
//! All access goes through a short critical section on the cache mutex; the
//! per-entry `Mutex<Cbz>` is held for the duration of the read. CBZ reads are
//! I/O-bound and brief; the brief mutex hold is fine for v1.

use archive::cbz::{Cbz, PreadIndex};
use archive::{ArchiveError, ArchiveLimits};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A cached open archive: the locked handle (decompression / compressed reads)
/// plus the immutable lock-free read index for its Stored entries (PERF-3),
/// both produced once at open. Cloning is two `Arc` bumps.
pub type CachedArchive = (Arc<Mutex<Cbz>>, Arc<PreadIndex>);

const HITS: &str = "folio_zip_lru_hits_total";
const MISSES: &str = "folio_zip_lru_misses_total";
const EVICTIONS: &str = "folio_zip_lru_evictions_total";
const OPEN_FDS: &str = "folio_zip_lru_open_fds";

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

    /// Acquire a handle to the issue's `Cbz`, opening (and inserting) on miss.
    /// The returned `Arc<Mutex<Cbz>>` lives at least as long as the caller holds it,
    /// even if the LRU evicts it in the meantime.
    pub fn get_or_open(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<Arc<Mutex<Cbz>>, ArchiveError> {
        Ok(self.get_or_open_entry(issue_id, path)?.0)
    }

    /// Like [`Self::get_or_open`] but also returns the [`PreadIndex`] for the
    /// issue's Stored entries, so the page server can read uncompressed pages
    /// lock-free (PERF-3). The index is computed once at open and cached.
    pub fn get_or_open_indexed(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<CachedArchive, ArchiveError> {
        self.get_or_open_entry(issue_id, path)
    }

    fn get_or_open_entry(
        &self,
        issue_id: &str,
        path: &Path,
    ) -> Result<CachedArchive, ArchiveError> {
        {
            let mut cache = self.inner.lock().unwrap();
            if let Some(existing) = cache.get(issue_id) {
                metrics::counter!(HITS).increment(1);
                return Ok(existing.clone());
            }
        }

        // Miss: open outside the lock (CBZ open parses the central directory;
        // building the pread index reads each Stored entry's local header).
        let cbz = Cbz::open(path, self.limits)?;
        let pread = Arc::new(cbz.build_pread_index());
        let arc = Arc::new(Mutex::new(cbz));
        let entry: CachedArchive = (arc, pread);

        let mut cache = self.inner.lock().unwrap();
        // Another caller may have raced and inserted while we were opening.
        // Honor the racing entry to keep a single live handle per issue.
        if let Some(existing) = cache.get(issue_id) {
            metrics::counter!(HITS).increment(1);
            return Ok(existing.clone());
        }
        let evicted = cache.push(issue_id.to_owned(), entry.clone());
        if evicted.is_some() {
            metrics::counter!(EVICTIONS).increment(1);
        }
        metrics::counter!(MISSES).increment(1);
        metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        Ok(entry)
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
