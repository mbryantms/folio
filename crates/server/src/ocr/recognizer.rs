//! `Recognizer` trait + result type.
//!
//! Trait surface and `Recognition`/`Word`/`Language` shapes live in
//! this parent module so they're stable for callers (the pipeline and
//! the request handler) regardless of which engine is selected.
//!
//! Implementations:
//!
//! - [`ctc::LineOcr`]: Western text. A line finder splits the balloon, a
//!   CTC line model reads the lines.
//! - [`manga::MangaOcr`]: vertical Japanese OCR, the `manga-ocr` model's
//!   ViT encoder + BERT decoder on two ONNX Runtime sessions Folio builds
//!   itself.
//! - [`western::WesternOcr`]: native Tesseract 5 with the `tessdata_best`
//!   LSTM models, vendored via `tesseract-rs`. No request reaches it; it is
//!   kept for `crates/tools/ocr-bench` until the Tesseract build is
//!   removed.

pub mod ctc;
pub mod manga;
pub mod western;

use image::DynamicImage;

/// Output of a single OCR pass over a cropped bubble.
///
/// `words` holds the pieces the engine read, with their boxes in crop
/// pixels: one entry per word for Tesseract, one entry per text line for
/// the CTC line recogniser (its unit is the line). `None` when the
/// engine exposes no pieces (manga-ocr returns a single string) or read
/// nothing. The client uses `text` for `marker.selection.text`.
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
    /// 0-based index of the textline the word sits on, in reading
    /// order (for a line entry: the line's own index). Lets the
    /// postprocess stage join hyphenated line breaks without
    /// re-deriving geometry from the bboxes.
    pub line_index: u32,
}

/// Language hint coming in on `POST /me/issues/{id}/ocr`. Defaults
/// to `Western` when the request body omits `lang`. Phase 2 wires a
/// per-series default (`series.text_language`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Western,
    Manga,
}

/// Anything that can take a cropped image and return text. The
/// pipeline picks the implementation from the request's language
/// (`ocr::pipeline::prepare_models`).
pub trait Recognizer: Send + Sync {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition>;
}
