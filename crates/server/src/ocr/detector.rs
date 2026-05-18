//! `comic-text-detector` singleton.
//!
//! The upstream crate's [`ComicTextDetector::new`] downloads a ~50 MB
//! ONNX from Hugging Face on first call (cached under `HF_HOME`) and
//! takes ~1–2 s cold. We pay that cost once per process behind a
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
            // Session::builder + HF download both block; run on the
            // blocking pool so we don't stall the reactor during a
            // cold start that may take seconds.
            let detector = tokio::task::spawn_blocking(ComicTextDetector::new)
                .await
                .map_err(|e| anyhow::anyhow!("detector init task panicked: {e}"))??;
            Ok(Detector {
                inner: Mutex::new(detector),
            })
        })
        .await
    }

    /// Sync lock — only call inside [`tokio::task::spawn_blocking`],
    /// never across an `.await`.
    pub fn lock(&self) -> anyhow::Result<MutexGuard<'_, ComicTextDetector>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("detector mutex poisoned"))
    }
}
