//! Bounded LRU of open `Cbz` handles (§7.2.1).
//!
//! Keeps `(issue_id → Arc<Mutex<Cbz>>)` so repeat page reads of a hot issue
//! avoid re-parsing the central directory and re-opening the FD. Eviction
//! drops the `Cbz`, which closes the underlying `File` (`zip` crate `Drop`).
//!
//! All access goes through a short critical section on the cache mutex; the
//! per-entry `Mutex<Cbz>` is held for the duration of the read. CBZ reads are
//! I/O-bound and brief; the brief mutex hold is fine for v1.

use archive::{ArchiveError, ArchiveLimits, cbz::Cbz};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};

const HITS: &str = "comic_zip_lru_hits_total";
const MISSES: &str = "comic_zip_lru_misses_total";
const EVICTIONS: &str = "comic_zip_lru_evictions_total";
const OPEN_FDS: &str = "comic_zip_lru_open_fds";

pub struct ZipLru {
    inner: Mutex<LruCache<String, Arc<Mutex<Cbz>>>>,
}

impl ZipLru {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).unwrap();
        let inner = Mutex::new(LruCache::new(cap));
        let me = Self { inner };
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
        {
            let mut cache = self.inner.lock().unwrap();
            if let Some(existing) = cache.get(issue_id) {
                metrics::counter!(HITS).increment(1);
                return Ok(existing.clone());
            }
        }

        // Miss: open outside the lock (CBZ open parses the central directory).
        let cbz = Cbz::open(path, ArchiveLimits::default())?;
        let arc = Arc::new(Mutex::new(cbz));

        let mut cache = self.inner.lock().unwrap();
        // Another caller may have raced and inserted while we were opening.
        // Honor the racing entry to keep a single live handle per issue.
        if let Some(existing) = cache.get(issue_id) {
            metrics::counter!(HITS).increment(1);
            return Ok(existing.clone());
        }
        let evicted = cache.push(issue_id.to_owned(), arc.clone());
        if evicted.is_some() {
            metrics::counter!(EVICTIONS).increment(1);
        }
        metrics::counter!(MISSES).increment(1);
        metrics::gauge!(OPEN_FDS).set(cache.len() as f64);
        Ok(arc)
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
