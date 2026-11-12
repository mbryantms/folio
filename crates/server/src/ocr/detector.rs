//! `comic-text-detector` singleton: the page detector of the legacy
//! engine set (`COMIC_OCR_LEGACY_ENGINES=true`). The current set's
//! detector is [`super::bubbles`]. This module goes away with the legacy
//! set one release after the engine change.
//!
//! The upstream crate builds its ONNX Runtime session with the default
//! thread count (one per physical core of the machine, whatever the
//! process may use), which is why a page takes tens of seconds in a
//! container limited to a few cores. That is left as it was: the legacy
//! set exists to behave exactly like the previous release.
//!
//! The upstream crate's [`ComicTextDetector::new`] loads a ~90 MB ONNX
//! from the Hugging Face cache. [`super::models::ensure_pinned`] puts a
//! digest-verified copy from a pinned commit there first, so the
//! constructor never downloads anything itself. Cold start takes ~1–2 s
//! plus the hash of the cached file. We pay that cost once per process behind a
//! [`tokio::sync::OnceCell`] and serialize subsequent inferences
//! through a [`std::sync::Mutex`] because `inference` takes
//! `&mut self`.
//!
//! The mutex is sync (not `tokio::sync::Mutex`) on purpose: the
//! [pipeline][crate::ocr::pipeline] runs inference inside
//! [`tokio::task::spawn_blocking`], where holding a sync mutex is
//! safe and avoids the extra `.await` indirection.

use std::sync::{Mutex, MutexGuard};

use comic_text_detector::ComicTextDetector;
use tokio::sync::OnceCell;

/// Process-wide detector handle. Initialized lazily on first
/// [`Detector::shared`] call; subsequent calls reuse the same session.
pub struct Detector {
    inner: Mutex<ComicTextDetector>,
    first_use: super::FirstUse,
}

impl Detector {
    /// Returns the shared singleton, initializing it on first call.
    /// Holds a static [`OnceCell`] internally — the boxed `Detector`
    /// lives for the rest of the process.
    ///
    /// Errors only on the first call (model download / session build).
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<Detector> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let started = std::time::Instant::now();
            // Fetch (if needed) and verify the pinned model; a mismatch
            // fails here, before the session ever reads the file.
            super::models::ensure_pinned(super::models::DETECTOR_FILES).await?;
            // Session::builder + HF cache lookup both block; run on the
            // blocking pool so we don't stall the reactor during a
            // cold start that may take seconds.
            let detector = tokio::task::spawn_blocking(ComicTextDetector::new)
                .await
                .map_err(|e| anyhow::anyhow!("detector init task panicked: {e}"))??;
            super::record_model_load("detector", started.elapsed());
            Ok(Detector {
                inner: Mutex::new(detector),
                first_use: super::FirstUse::new(),
            })
        })
        .await
    }

    /// Note one inference's time; only the first is recorded
    /// (`folio_ocr_model_first_use_seconds{model="detector"}`).
    pub fn note_inference(&self, took: std::time::Duration) {
        self.first_use.record("detector", took);
    }

    /// Sync lock — only call inside [`tokio::task::spawn_blocking`],
    /// never across an `.await`.
    pub fn lock(&self) -> anyhow::Result<MutexGuard<'_, ComicTextDetector>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("detector mutex poisoned"))
    }
}
