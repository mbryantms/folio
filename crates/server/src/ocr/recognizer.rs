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

pub mod ctc;
pub mod manga;

use image::DynamicImage;

/// Output of a single OCR pass over a cropped bubble.
///
/// `words` holds the pieces the engine read, with their boxes in crop
/// pixels: one entry per text line for the CTC line recogniser (its unit
/// is the line). `None` when the engine exposes no pieces (manga-ocr
/// returns a single string) or read nothing. The client uses `text` for `marker.selection.text`.
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
    /// 0-based index of the text line the entry belongs to, in reading
    /// order (for a line entry: the line's own index).
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
