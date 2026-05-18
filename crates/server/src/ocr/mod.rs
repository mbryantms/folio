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
//!
//! Models auto-download from Hugging Face on first use; cache lives
//! at `${COMIC_DATA_DIR}/models/` (honors `HF_HOME`). The opt-in
//! `cuda` Cargo feature on this crate forwards to
//! `comic-text-detector/cuda` for GPU acceleration.

pub mod cache;
pub mod detector;
pub mod pipeline;
pub mod recognizer;
