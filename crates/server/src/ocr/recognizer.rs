//! `Recognizer` trait + result type.
//!
//! Trait surface and `Recognition`/`Word`/`Language` shapes live in
//! this parent module so they're stable for callers (the pipeline +
//! request handler in M3) regardless of which engine is selected.
//!
//! Implementations:
//!
//! - [`western::WesternOcr`] — native Tesseract 5 with the
//!   `tessdata_best` LSTM models, vendored via `tesseract-rs`.
//! - [`manga::MangaOcr`] — vertical Japanese OCR backed by
//!   `manga-ocr`'s ViT encoder + BERT decoder on ONNX Runtime.

pub mod manga;
pub mod western;

use image::DynamicImage;

/// Output of a single OCR pass over a cropped bubble.
///
/// `words` is `None` when the engine doesn't expose per-word boxes
/// (manga-ocr in particular returns a single string). The client
/// uses `text` for `marker.selection.text`; the optional fields are
/// surfaced if present so the reader can render word-level
/// highlights or replace the user's drag rect with the refined
/// bubble outline.
#[derive(Debug, Clone)]
pub struct Recognition {
    pub text: String,
    pub confidence: f32,
    pub words: Option<Vec<Word>>,
}

#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub confidence: f32,
    pub xmin: f32,
    pub ymin: f32,
    pub xmax: f32,
    pub ymax: f32,
}

/// Language hint coming in on `POST /me/issues/{id}/ocr`. Defaults
/// to `Western` when the request body omits `lang`. Phase 2 wires a
/// per-series default (`series.text_language`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Western,
    Manga,
}

/// Anything that can take a cropped image and return text. M2 ships
/// two implementations; M3 dispatches between them based on the
/// request body's `lang` hint.
pub trait Recognizer: Send + Sync {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition>;
}
