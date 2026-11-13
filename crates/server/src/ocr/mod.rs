//! Server-side OCR pipeline: find the text regions of a page, read one.
//!
//! The engines, every session built here with an explicit thread count
//! ([`session`]):
//!
//! - [`bubbles`]: the page detector, an RT-DETR-v2 export that returns
//!   balloons and free text. Its boxes are the regions the server offers
//!   and the crop a tap reads.
//! - [`lines`]: the line finder, a DB text detector run on one balloon
//!   crop to split it into text lines.
//! - [`recognizer::ctc`]: the Western recogniser, a CTC line model that
//!   reads all lines of a balloon in one batch.
//! - [`recognizer::manga`]: the `manga-ocr` model for Japanese (encoder
//!   and decoder), on the crop [`manga_crop`] chooses.
//!
//! [`recognizer::western`] (Tesseract) and the Tesseract cleanup rules in
//! [`postprocess`] are no longer reached by any request. They stay
//! compiled for `crates/tools/ocr-bench`, which uses them as its baseline,
//! until the Tesseract build is removed.
//!
//! Around the engines:
//!
//! - [`pipeline`]: `detect → snap to a region → crop → recognize → clean`,
//!   behind `POST /me/issues/{id}/ocr` and the text-regions route.
//! - [`regions`]: which boxes of a page's detection are its regions, with
//!   the detector's duplicates removed, so clients get the final set.
//! - [`page_cache`]: in-process cache of decoded pages, so the
//!   balloon-by-balloon requests on one page decode it once.
//! - [`postprocess`]: pure-function cleanup of engine output.
//! - [`cache`]: Redis result and detection caches, keyed by an engine
//!   fingerprint.
//!
//! Models download from Hugging Face on first use, pinned to a commit
//! and checked against a SHA-256 before they load ([`models`]); the
//! cache is the hub cache under `$HOME/.cache/huggingface/hub`. The opt-in
//! `cuda` Cargo feature on this crate forwards to `ort/cuda`.

pub mod bubbles;
pub mod cache;
pub mod lines;
pub mod manga_crop;
pub mod models;
pub mod page_cache;
pub mod pipeline;
pub mod postprocess;
pub mod recognizer;
pub mod regions;
pub mod session;

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
