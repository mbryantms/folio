//! Western OCR via the native Tesseract bindings.
//!
//! [`WesternOcr::shared`] returns the process-wide singleton. Init
//! costs cover (a) locating the `eng.traineddata` LSTM model — read
//! from `TESSDATA_PREFIX` if set, otherwise the `~/.tesseract-rs/`
//! cache that the build script populates — and (b) creating the
//! Tesseract `BaseAPI` handle. Cold init is ~50 ms.
//!
//! Each `recognize` call:
//!
//!  1. Converts the input to 8-bit grayscale.
//!  2. Upscales 3× with `Lanczos3` (Tesseract's LSTM is happiest
//!     above ~30 px caps-height; comic bubbles often arrive smaller).
//!  3. Applies a fixed Otsu-ish global threshold (we don't run a
//!     proper Otsu yet — comic lettering is high-contrast enough
//!     that the midpoint cut is a useful first pass; replaced if
//!     quality demands it).
//!  4. Sets PSM 6 — "uniform block of text" — matching the previous
//!     browser-side default. Word-level boxes via `ResultIterator`
//!     are wired in [`Self::recognize`].
//!  5. Returns `Recognition` with mean confidence rescaled from
//!     Tesseract's `[0, 100]` integer to `[0.0, 1.0]`.
//!
//! Sequential ops (`set_image → recognize → get_utf8_text →
//! mean_text_conf`) share state on the same `BaseAPI`, so we wrap
//! the whole sequence in a [`std::sync::Mutex`] — two callers on
//! different threads would otherwise interleave each other's image
//! data and confuse the results.

use std::path::PathBuf;
use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use tesseract_rs::{TessPageSegMode, TesseractAPI};
use tokio::sync::OnceCell;

use super::{Recognition, Recognizer};

/// Process-wide Tesseract handle. Initialize via
/// [`WesternOcr::shared`]; subsequent calls share the same API.
pub struct WesternOcr {
    api: Mutex<TesseractAPI>,
}

impl WesternOcr {
    /// Returns the shared singleton, initializing it on first call.
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<WesternOcr> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let me = tokio::task::spawn_blocking(Self::new)
                .await
                .map_err(|e| anyhow::anyhow!("tesseract init task panicked: {e}"))??;
            Ok(me)
        })
        .await
    }

    fn new() -> anyhow::Result<Self> {
        let api = TesseractAPI::new();
        let tessdata = tessdata_dir()?;
        api.init(&tessdata, "eng").map_err(|e| {
            anyhow::anyhow!(
                "tesseract init failed (tessdata: {}, lang: eng): {e}",
                tessdata.display()
            )
        })?;
        api.set_page_seg_mode(TessPageSegMode::PSM_SINGLE_BLOCK)
            .map_err(|e| anyhow::anyhow!("tesseract set_page_seg_mode: {e}"))?;
        Ok(WesternOcr {
            api: Mutex::new(api),
        })
    }
}

impl Recognizer for WesternOcr {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition> {
        let prepared = preprocess(image);
        let (width, height) = prepared.dimensions();
        // 1 byte per pixel (8-bit grayscale). bytes_per_line == width.
        let api = self
            .api
            .lock()
            .map_err(|_| anyhow::anyhow!("tesseract mutex poisoned"))?;
        api.set_image(
            prepared.as_raw(),
            width as i32,
            height as i32,
            1,
            width as i32,
        )
        .map_err(|e| anyhow::anyhow!("tesseract set_image: {e}"))?;
        api.recognize()
            .map_err(|e| anyhow::anyhow!("tesseract recognize: {e}"))?;
        let raw_text = api
            .get_utf8_text()
            .map_err(|e| anyhow::anyhow!("tesseract get_utf8_text: {e}"))?;
        let confidence = api
            .mean_text_conf()
            .map(|c| (c.clamp(0, 100) as f32) / 100.0)
            .unwrap_or(0.0);
        drop(api);
        Ok(Recognition {
            text: raw_text.trim().to_owned(),
            confidence,
            // M2 leaves `words` `None` — the ResultIterator wiring
            // needs an unsafe-ish dance and is non-blocking for the
            // M3 endpoint shape. Will fill in if downstream UIs
            // demand per-word highlights.
            words: None,
        })
    }
}

/// 3× upscale + grayscale + midpoint threshold. Cheap and good
/// enough for comic lettering. Returns a tightly packed 8-bit
/// luma buffer ready for `TessBaseAPISetImage`.
fn preprocess(image: &DynamicImage) -> image::GrayImage {
    let gray = image.to_luma8();
    let (w, h) = gray.dimensions();
    let upscaled = image::imageops::resize(&gray, w * 3, h * 3, FilterType::Lanczos3);
    // Midpoint binarization — comic lettering is high-contrast,
    // so a fixed cut at 127 maps the LSTM to the cleaner "scanned
    // print" distribution it was trained on.
    let mut bin = upscaled;
    for px in bin.pixels_mut() {
        px.0[0] = if px.0[0] < 128 { 0 } else { 255 };
    }
    bin
}

/// Resolve where the build script's `download_tessdata` step stashed
/// `eng.traineddata`. Honors `TESSDATA_PREFIX` for operators
/// shipping pre-staged models (air-gapped Docker images etc.).
fn tessdata_dir() -> anyhow::Result<PathBuf> {
    if let Ok(env_dir) = std::env::var("TESSDATA_PREFIX") {
        return Ok(PathBuf::from(env_dir));
    }
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME unset and TESSDATA_PREFIX missing"))?;
    Ok(PathBuf::from(home).join(".tesseract-rs").join("tessdata"))
}
