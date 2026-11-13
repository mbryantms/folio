//! Server-side OCR pipeline: find the text regions of a page, read one.
//!
//! Two engine sets exist ([`EngineSet`]); the pipeline's prepare functions
//! take the set as a value, so one process can run both (a benchmark
//! tool) while a server runs the one its configuration names.
//!
//! The current set, every session built here with an explicit thread
//! count ([`session`]):
//!
//! - [`bubbles`]: the page detector, an RT-DETR-v2 export that returns
//!   balloons and free text. Its boxes are the regions the server offers
//!   and the crop a tap reads.
//! - [`lines`]: the line finder, a DB text detector run on one balloon
//!   crop to split it into text lines.
//! - [`recognizer::ctc`]: the Western recogniser, a CTC line model that
//!   reads all lines of a balloon in one batch.
//! - [`recognizer::manga`]: `manga-ocr` for Japanese, on the crop
//!   [`manga_crop`] chooses (the balloon box for now).
//!
//! The legacy set (`COMIC_OCR_LEGACY_ENGINES=true`, kept for one release
//! so a deploy can go back without a revert): [`detector`]
//! (`comic-text-detector`) and [`recognizer::western`] (Tesseract).
//!
//! Shared by both:
//!
//! - [`pipeline`]: `detect → snap to a region → crop → recognize → clean`,
//!   behind `POST /me/issues/{id}/ocr` and the text-regions route.
//! - [`regions`]: which boxes of a page's detection are its regions, with
//!   each detector's duplicates removed, so clients get the final set.
//! - [`page_cache`]: in-process cache of decoded pages, so the
//!   balloon-by-balloon requests on one page decode it once.
//! - [`postprocess`]: pure-function cleanup of engine output.
//! - [`cache`]: Redis result and detection caches, keyed by an engine
//!   fingerprint that differs between the two sets.
//!
//! Models download from Hugging Face on first use, pinned to a commit
//! and checked against a SHA-256 before they load ([`models`]); the
//! cache is the hub cache under `$HOME/.cache/huggingface/hub`. The opt-in
//! `cuda` Cargo feature on this crate forwards to `ort/cuda`.

pub mod bubbles;
pub mod cache;
pub mod detector;
pub mod lines;
pub mod manga_crop;
pub mod models;
pub mod page_cache;
pub mod pipeline;
pub mod postprocess;
pub mod recognizer;
pub mod regions;
pub(crate) mod session;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Which engines find and read text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EngineSet {
    /// The balloon detector, the line finder and the CTC line recogniser
    /// ([`bubbles`], [`lines`], [`recognizer::ctc`]).
    #[default]
    Current,
    /// `comic-text-detector` and Tesseract, as before the engine change.
    /// Temporary: selected with `COMIC_OCR_LEGACY_ENGINES=true`, removed
    /// together with the Tesseract build one release later.
    Legacy,
}

impl EngineSet {
    /// The set a server with this configuration runs.
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        if cfg.ocr_legacy_engines {
            EngineSet::Legacy
        } else {
            EngineSet::Current
        }
    }

    /// Stable name, part of the cache-key fingerprints.
    pub const fn label(self) -> &'static str {
        match self {
            EngineSet::Current => "current",
            EngineSet::Legacy => "legacy",
        }
    }

    /// Whether a class of this set's page detector is a region class: one
    /// whose boxes the text-regions route returns and `detect: true`
    /// snaps to (after [`regions::regions`] removed the set's
    /// duplicates). The current detector's are balloons and free text
    /// ([`bubbles::is_region_class`]); the text inside a balloon stays in
    /// the page's detection but is not offered. Every class of the
    /// legacy detector is one, as before.
    pub const fn is_region_class(self, class: u32) -> bool {
        match self {
            EngineSet::Current => bubbles::is_region_class(class),
            EngineSet::Legacy => true,
        }
    }
}

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
