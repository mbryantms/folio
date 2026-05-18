//! Vertical-Japanese OCR via the `manga-ocr` crate.
//!
//! [`MangaOcr::shared`] returns the process-wide singleton. Cold
//! init downloads two ONNX models (`encoder_model.onnx`,
//! `decoder_model.onnx`) and a vocab list from
//! `mayocream/manga-ocr-onnx` on Hugging Face — ~80 MB total, ~3 s
//! on a warm cache, ~30 s first time.
//!
//! Each `recognize` call runs the encoder once and then greedy-
//! decodes up to 300 tokens, taking the argmax over the decoder
//! logits per step until the `</s>` token. The upstream crate
//! doesn't expose per-token probabilities, so confidence is
//! reported as `1.0` for non-empty output and `0.0` for empty —
//! the M3 endpoint surfaces it as-is so the client can route low-
//! confidence Western fallback paths consistently.
//!
//! `MangaOCR::inference` takes `&mut self` and is CPU-bound; like
//! [`WesternOcr`] we wrap it in a [`std::sync::Mutex`] and the
//! pipeline (M3) drives recognize from `spawn_blocking`.

use std::sync::Mutex;

use image::DynamicImage;
use manga_ocr::MangaOCR;
use tokio::sync::OnceCell;

use super::{Recognition, Recognizer};

pub struct MangaOcr {
    inner: Mutex<MangaOCR>,
}

impl MangaOcr {
    /// Returns the shared singleton, initializing it on first call.
    /// Cold init blocks on the HF model download.
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<MangaOcr> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let inner = tokio::task::spawn_blocking(MangaOCR::new)
                .await
                .map_err(|e| anyhow::anyhow!("manga-ocr init task panicked: {e}"))??;
            Ok(MangaOcr {
                inner: Mutex::new(inner),
            })
        })
        .await
    }
}

impl Recognizer for MangaOcr {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("manga-ocr mutex poisoned"))?;
        let text = guard.inference(image)?;
        let trimmed = text.trim().to_owned();
        let confidence = if trimmed.is_empty() { 0.0 } else { 1.0 };
        Ok(Recognition {
            text: trimmed,
            confidence,
            words: None,
        })
    }
}
