//! Server-side OCR pipeline (text-detection-1.0 plan, Phase 1).
//!
//! Layout, all scaffolded by M1 and progressively filled by M2–M5:
//!
//! - [`detector`] — singleton wrapping `comic-text-detector` so the
//!   ONNX session is cold-loaded once per process (~1–2 s) and
//!   inference is serialized through a mutex.
//! - [`recognizer`] — `Recognizer` trait + impls (`WesternOcr` via
//!   `tesseract-rs` and `MangaOcr` via `manga-ocr`). M1 ships only
//!   the trait + a `Recognition` shape; M2 wires the impls.
//! - [`pipeline`] — `detect → snap-to-polygon → crop → recognize`,
//!   exposed via `POST /me/issues/{id}/ocr` (added in M3).
//! - [`page_cache`] — in-process cache of decoded pages, so the
//!   balloon-by-balloon requests on one page decode it once.
//! - [`postprocess`] — pure-function cleanup of raw engine output
//!   (confidence filtering, junk-token rules, whitespace shaping).
//!   Runs inside the pipeline right after recognition.
//!
//! Models download from Hugging Face on first use, pinned to a commit
//! and checked against a SHA-256 before they load ([`models`]); the
//! cache is the hub cache under `$HOME/.cache/huggingface/hub`. The opt-in
//! `cuda` Cargo feature on this crate forwards to
//! `comic-text-detector/cuda` for GPU acceleration.

pub mod cache;
pub mod detector;
pub mod models;
pub mod page_cache;
pub mod pipeline;
pub mod postprocess;
pub mod recognizer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Record `folio_ocr_model_load_seconds{model}`: one model's verified
/// download (when needed) and session build, once per process.
pub(crate) fn record_model_load(model: &'static str, took: Duration) {
    metrics::histogram!("folio_ocr_model_load_seconds", "model" => model)
        .record(took.as_secs_f64());
}

/// Records `folio_ocr_model_first_use_seconds{model}` for the first
/// inference of a loaded model only (session warm-up included).
pub(crate) struct FirstUse(AtomicBool);

impl FirstUse {
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(crate) fn record(&self, model: &'static str, took: Duration) {
        if !self.0.swap(true, Ordering::Relaxed) {
            metrics::histogram!("folio_ocr_model_first_use_seconds", "model" => model)
                .record(took.as_secs_f64());
        }
    }
}
