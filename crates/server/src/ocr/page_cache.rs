//! In-process cache of decoded OCR pages.
//!
//! Reading a page's balloons one by one sends one OCR request per
//! balloon, and each needs the whole decoded page (a crop, or the
//! detector's full-page input). Without this cache every one of them
//! read the entry from the archive and decoded it again, under an OCR
//! permit: a 20-balloon page was decoded 20 times.
//!
//! - **Key**: the issue's `content_hash` and the page index. Nothing a
//!   user controls: the handler reads both from the issue row after the
//!   visibility check. A rescan that changes the file changes the hash,
//!   so a changed file is a miss.
//! - **Bound**: [`DECODED_PAGE_CACHE_BYTES`] of decoded pixels, counted
//!   as width × height × bytes per pixel of the decoded image (what the
//!   buffer holds). Least-recently-used pages are evicted to fit; a page
//!   larger than the whole bound is not cached.
//! - **Expiry**: a page unused for [`DECODED_PAGE_IDLE`] is dropped, on
//!   the next lookup or by a small background sweep that runs only while
//!   the cache holds pages (so an idle server gives the memory back
//!   within 1.5 × the idle time).
//! - **Single flight**: the first request for a page claims it
//!   ([`Lookup::Claimed`]) and decodes it; requests for the same page
//!   that arrive meanwhile wait for that decode ([`Lookup::Wait`]) and
//!   then find it. The claim is fulfilled from inside the blocking
//!   decode, so a claimer whose request is dropped still fills the
//!   cache; a claim dropped without a page (decode error, panic) wakes
//!   the waiters, and the next one claims and decodes.
//!
//! Locking: one `std::sync::Mutex` over the map, held only for map
//! updates — never while decoding, never across an `.await`, never
//! together with the Tesseract or detector mutexes.
//!
//! The handler (`api::issue_ocr`) consults the cache only after the
//! visibility check and the archive checks, and only while holding an OCR
//! permit, so memory is the cache bound plus at most one page per permit,
//! and a waiter can only wait on a decode that is itself running under a
//! permit and ends.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use image::DynamicImage;
use tokio::sync::watch;

/// Decoded bytes the cache may hold. A 2000 × 3000 page decodes to
/// 18 MB (RGB) or 24 MB (RGBA), so this keeps five to seven pages: the
/// page a reader is working through, plus a few for other readers.
pub const DECODED_PAGE_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// A page not used for this long is dropped. Long enough to read the
/// next balloon, short enough to give the memory back soon after.
pub const DECODED_PAGE_IDLE: Duration = Duration::from_secs(60);

/// One page of one file version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageKey {
    content_hash: String,
    page: u32,
}

impl PageKey {
    pub fn new(content_hash: &str, page: u32) -> Self {
        Self {
            content_hash: content_hash.to_owned(),
            page,
        }
    }

    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn page(&self) -> u32 {
        self.page
    }
}

/// A decoded page shared between the cache and the requests using it.
pub type SharedPage = Arc<DynamicImage>;

/// Bytes a decoded page holds: width × height × bytes per pixel.
pub fn decoded_bytes(page: &DynamicImage) -> usize {
    let bytes = u64::from(page.width())
        * u64::from(page.height())
        * u64::from(page.color().bytes_per_pixel());
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

enum Slot {
    Ready {
        page: SharedPage,
        bytes: usize,
        last_used: Instant,
    },
    Loading {
        claim_id: u64,
        done: watch::Receiver<bool>,
    },
}

#[derive(Default)]
struct State {
    slots: HashMap<PageKey, Slot>,
    /// Sum of the `Ready` slots' bytes.
    bytes: usize,
    next_claim: u64,
    /// A sweep task is running.
    sweeping: bool,
}

/// What [`DecodedPages::lookup_or_claim`] found.
pub enum Lookup {
    /// The page is decoded.
    Hit(SharedPage),
    /// Another request is decoding it: wait on this (it turns `true`, or
    /// closes when that decode failed), then look up again.
    Wait(watch::Receiver<bool>),
    /// Nobody has it: the caller decodes it and hands it to the claim.
    Claimed(Claim),
}

pub struct DecodedPages {
    state: Mutex<State>,
    max_bytes: usize,
    idle: Duration,
}

/// The process-wide cache the OCR routes use.
pub fn decoded_pages() -> &'static Arc<DecodedPages> {
    static PAGES: LazyLock<Arc<DecodedPages>> =
        LazyLock::new(|| DecodedPages::new(DECODED_PAGE_CACHE_BYTES, DECODED_PAGE_IDLE));
    &PAGES
}

impl DecodedPages {
    pub fn new(max_bytes: usize, idle: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            max_bytes,
            idle,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic mid-update leaves at worst a stale slot; the map is
        // still usable.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Find `key`, or claim it for decoding. Counts
    /// `folio_ocr_page_cache_hits_total` / `_misses_total`.
    pub fn lookup_or_claim(self: &Arc<Self>, key: &PageKey) -> Lookup {
        let now = Instant::now();
        let mut st = self.lock();
        self.expire(&mut st, now);
        match st.slots.get_mut(key) {
            Some(Slot::Ready {
                page, last_used, ..
            }) => {
                *last_used = now;
                metrics::counter!("folio_ocr_page_cache_hits_total").increment(1);
                Lookup::Hit(Arc::clone(page))
            }
            Some(Slot::Loading { done, .. }) => Lookup::Wait(done.clone()),
            None => {
                metrics::counter!("folio_ocr_page_cache_misses_total").increment(1);
                let (tx, rx) = watch::channel(false);
                st.next_claim += 1;
                let claim_id = st.next_claim;
                st.slots
                    .insert(key.clone(), Slot::Loading { claim_id, done: rx });
                Lookup::Claimed(Claim {
                    cache: Arc::clone(self),
                    key: key.clone(),
                    claim_id,
                    done: Some(tx),
                })
            }
        }
    }

    /// Drop `Ready` pages unused for [`Self::idle`].
    fn expire(&self, st: &mut State, now: Instant) {
        let idle = self.idle;
        let mut freed = 0;
        st.slots.retain(|_, slot| match slot {
            Slot::Ready {
                bytes, last_used, ..
            } if now.duration_since(*last_used) >= idle => {
                freed += *bytes;
                false
            }
            _ => true,
        });
        st.bytes -= freed;
    }

    /// Expire idle pages; `false` (and the sweep flag cleared) when no
    /// page is left to expire later.
    fn sweep(&self) -> bool {
        let mut st = self.lock();
        self.expire(&mut st, Instant::now());
        let any = st
            .slots
            .values()
            .any(|slot| matches!(slot, Slot::Ready { .. }));
        if !any {
            st.sweeping = false;
        }
        any
    }

    /// Run [`Self::sweep`] every half idle period until the cache is empty.
    fn spawn_sweeper(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // No runtime (a plain thread): pages still expire on lookup.
            self.lock().sweeping = false;
            return;
        };
        let me = Arc::clone(self);
        runtime.spawn(async move {
            let mut guard = SweepGuard {
                cache: Arc::clone(&me),
                armed: true,
            };
            loop {
                tokio::time::sleep(me.idle / 2).await;
                if !me.sweep() {
                    guard.armed = false;
                    break;
                }
            }
        });
    }

    /// Forget every page of `content_hash`. Debug builds only: lets a
    /// test act as if the idle expiry had passed.
    #[cfg(debug_assertions)]
    pub fn forget_for_tests(&self, content_hash: &str) {
        let mut st = self.lock();
        let mut freed = 0;
        st.slots.retain(|k, slot| match slot {
            Slot::Ready { bytes, .. } if k.content_hash == content_hash => {
                freed += *bytes;
                false
            }
            _ => true,
        });
        st.bytes -= freed;
    }

    /// `(pages, bytes)` held. For tests and diagnostics.
    pub fn usage(&self) -> (usize, usize) {
        let st = self.lock();
        let pages = st
            .slots
            .values()
            .filter(|slot| matches!(slot, Slot::Ready { .. }))
            .count();
        (pages, st.bytes)
    }

    /// Remove `key`'s `Loading` slot if it is still `claim_id`'s.
    fn release_claim(st: &mut State, key: &PageKey, claim_id: u64) {
        if matches!(st.slots.get(key), Some(Slot::Loading { claim_id: id, .. }) if *id == claim_id)
        {
            st.slots.remove(key);
        }
    }
}

/// Clears the sweep flag if the sweep task is dropped early (its runtime
/// shut down), so the next insert starts a new one.
struct SweepGuard {
    cache: Arc<DecodedPages>,
    armed: bool,
}

impl Drop for SweepGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cache.lock().sweeping = false;
        }
    }
}

/// The right to decode one page into the cache. Fulfil it with the page,
/// or drop it: either wakes the requests waiting for this page.
pub struct Claim {
    cache: Arc<DecodedPages>,
    key: PageKey,
    claim_id: u64,
    done: Option<watch::Sender<bool>>,
}

impl Claim {
    pub fn key(&self) -> &PageKey {
        &self.key
    }

    /// Store the decoded page (evicting least-recently-used pages to
    /// stay within the bound) and wake the waiters. Synchronous: call it
    /// from the blocking decode, so it happens even when the request that
    /// claimed the page is gone.
    pub fn fulfil(mut self, page: SharedPage) {
        let bytes = decoded_bytes(&page);
        let max = self.cache.max_bytes;
        let start_sweep = {
            let mut st = self.cache.lock();
            DecodedPages::release_claim(&mut st, &self.key, self.claim_id);
            if bytes > max {
                false
            } else {
                while st.bytes + bytes > max {
                    let oldest = st
                        .slots
                        .iter()
                        .filter_map(|(k, slot)| match slot {
                            Slot::Ready { last_used, .. } => Some((*last_used, k.clone())),
                            Slot::Loading { .. } => None,
                        })
                        .min_by_key(|(at, _)| *at)
                        .map(|(_, k)| k);
                    let Some(oldest) = oldest else { break };
                    if let Some(Slot::Ready { bytes: b, .. }) = st.slots.remove(&oldest) {
                        st.bytes -= b;
                    }
                }
                st.slots.insert(
                    self.key.clone(),
                    Slot::Ready {
                        page,
                        bytes,
                        last_used: Instant::now(),
                    },
                );
                st.bytes += bytes;
                let start = !st.sweeping;
                st.sweeping = true;
                start
            }
        };
        if let Some(done) = self.done.take() {
            let _ = done.send(true);
        }
        if start_sweep {
            self.cache.spawn_sweeper();
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if self.done.is_some() {
            // Not fulfilled: free the slot; dropping the sender wakes the
            // waiters, and the next lookup claims the page again.
            let mut st = self.cache.lock();
            DecodedPages::release_claim(&mut st, &self.key, self.claim_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{RgbImage, RgbaImage};

    fn rgb(w: u32, h: u32) -> SharedPage {
        Arc::new(DynamicImage::ImageRgb8(RgbImage::new(w, h)))
    }

    fn key(n: u32) -> PageKey {
        PageKey::new("hash", n)
    }

    fn claim(cache: &Arc<DecodedPages>, k: &PageKey) -> Claim {
        match cache.lookup_or_claim(k) {
            Lookup::Claimed(c) => c,
            _ => panic!("expected a miss for {k:?}"),
        }
    }

    fn is_hit(cache: &Arc<DecodedPages>, k: &PageKey) -> bool {
        match cache.lookup_or_claim(k) {
            Lookup::Hit(_) => true,
            Lookup::Claimed(_) | Lookup::Wait(_) => false,
        }
    }

    #[test]
    fn bytes_are_width_times_height_times_bytes_per_pixel() {
        assert_eq!(decoded_bytes(&rgb(2000, 3000)), 18_000_000);
        let rgba = DynamicImage::ImageRgba8(RgbaImage::new(2000, 3000));
        assert_eq!(decoded_bytes(&rgba), 24_000_000);
        let luma = DynamicImage::ImageLuma8(image::GrayImage::new(10, 10));
        assert_eq!(decoded_bytes(&luma), 100);
    }

    #[test]
    fn a_decoded_page_is_a_hit_and_a_changed_hash_is_a_miss() {
        let cache = DecodedPages::new(10_000, Duration::from_secs(60));
        claim(&cache, &key(0)).fulfil(rgb(10, 10));
        assert!(is_hit(&cache, &key(0)));
        assert!(!is_hit(&cache, &PageKey::new("other-hash", 0)));
        assert!(!is_hit(&cache, &key(1)));
        assert_eq!(cache.usage(), (1, 300));
    }

    #[test]
    fn the_byte_bound_evicts_the_least_recently_used_page() {
        // Room for two 300-byte pages.
        let cache = DecodedPages::new(700, Duration::from_secs(60));
        claim(&cache, &key(0)).fulfil(rgb(10, 10));
        std::thread::sleep(Duration::from_millis(2));
        claim(&cache, &key(1)).fulfil(rgb(10, 10));
        std::thread::sleep(Duration::from_millis(2));
        // Page 0 used again: page 1 is now the least recently used.
        assert!(is_hit(&cache, &key(0)));
        std::thread::sleep(Duration::from_millis(2));
        claim(&cache, &key(2)).fulfil(rgb(10, 10));
        assert_eq!(cache.usage(), (2, 600));
        assert!(is_hit(&cache, &key(0)));
        assert!(is_hit(&cache, &key(2)));
        assert!(!is_hit(&cache, &key(1)), "evicted");
    }

    #[test]
    fn a_page_larger_than_the_bound_is_not_cached() {
        let cache = DecodedPages::new(299, Duration::from_secs(60));
        claim(&cache, &key(0)).fulfil(rgb(10, 10));
        assert_eq!(cache.usage(), (0, 0));
        assert!(!is_hit(&cache, &key(0)));
    }

    #[test]
    fn an_idle_page_expires() {
        // 300 ms idle: the hit right after the insert has ample margin on a
        // loaded runner.
        let cache = DecodedPages::new(10_000, Duration::from_millis(300));
        claim(&cache, &key(0)).fulfil(rgb(10, 10));
        assert!(is_hit(&cache, &key(0)));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!is_hit(&cache, &key(0)), "expired");
    }

    #[tokio::test]
    async fn a_second_request_waits_for_the_first_decode() {
        let cache = DecodedPages::new(10_000, Duration::from_secs(60));
        let first = claim(&cache, &key(0));
        let Lookup::Wait(mut waiting) = cache.lookup_or_claim(&key(0)) else {
            panic!("the second request must wait, not decode")
        };
        let decode = std::thread::spawn(move || first.fulfil(rgb(10, 10)));
        assert!(waiting.wait_for(|d| *d).await.is_ok());
        decode.join().unwrap();
        assert!(is_hit(&cache, &key(0)), "and then finds the page");
    }

    #[tokio::test]
    async fn a_dropped_claim_wakes_the_waiters_and_frees_the_page() {
        let cache = DecodedPages::new(10_000, Duration::from_secs(60));
        let first = claim(&cache, &key(0));
        let Lookup::Wait(mut waiting) = cache.lookup_or_claim(&key(0)) else {
            panic!("expected to wait")
        };
        drop(first); // the decode failed
        assert!(waiting.wait_for(|d| *d).await.is_err(), "woken, no page");
        // The next request claims it and decodes.
        let again = claim(&cache, &key(0));
        again.fulfil(rgb(10, 10));
        assert!(is_hit(&cache, &key(0)));
    }

    /// The page is fulfilled from a blocking-pool thread, as the OCR decode
    /// does, so the sweep is started the way production starts it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_pages_are_released_without_further_requests() {
        let cache = DecodedPages::new(10_000, Duration::from_millis(300));
        let c = claim(&cache, &key(0));
        tokio::task::spawn_blocking(move || c.fulfil(rgb(10, 10)))
            .await
            .unwrap();
        assert_eq!(cache.usage(), (1, 300));
        assert!(cache.lock().sweeping, "the sweep started");
        for _ in 0..300 {
            if cache.usage() == (0, 0) {
                // The sweep stops once the cache is empty.
                assert!(!cache.lock().sweeping);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the sweep never released the idle page");
    }
}
