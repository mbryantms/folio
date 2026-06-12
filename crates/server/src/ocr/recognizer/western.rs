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
//!  3. Binarizes at the Otsu level (falling back to the midpoint
//!     when the crop is near-uniform and Otsu degenerates).
//!  4. Detects polarity from the border ring and inverts
//!     white-on-black crops (dark panels, black caption boxes) so
//!     the lettering matches the dark-on-light distribution the
//!     LSTM was trained on.
//!  5. Pads with a white border — Tesseract's segmenter mis-handles
//!     glyphs touching the canvas edge, and bubble crops are tight
//!     by construction.
//!  6. Sets PSM 6 — "uniform block of text" — the right model for a
//!     snapped bubble crop. (PSM 11/sparse picks up *more* stray
//!     marks; PSM 3 drops short lines.)
//!  7. Walks the `ResultIterator` at word level so [`Recognition`]
//!     carries per-word text/confidence/bbox for the postprocess
//!     stage to filter on. The whole-page text fallback only fires
//!     when the iterator yields nothing.
//!
//! `tessedit_char_blacklist` is deliberately NOT set: with the LSTM
//! engine a blacklisted char is *substituted* by the next-best
//! alternative rather than omitted — a stray `|` becomes a stray
//! `l`/`I` — which corrupts genuine text instead of cleaning it.
//! Junk removal happens deterministically in
//! [`crate::ocr::postprocess`].
//!
//! Sequential ops (`set_image → recognize → get_utf8_text →
//! mean_text_conf`) share state on the same `BaseAPI`, so we wrap
//! the whole sequence in a [`std::sync::Mutex`] — two callers on
//! different threads would otherwise interleave each other's image
//! data and confuse the results.

use std::path::PathBuf;
use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use tesseract_rs::{TessPageIteratorLevel, TessPageSegMode, TesseractAPI};
use tokio::sync::OnceCell;

use super::{Recognition, Recognizer, Word};

/// Lanczos3 upscale factor applied before binarization. Word bboxes
/// coming back out of Tesseract are divided by this to land in
/// original-crop coordinates.
const UPSCALE: u32 = 3;

/// White border (post-upscale pixels) composited around the
/// binarized crop. ~8 source px — comfortably past the ~10 px
/// margin Tesseract's segmenter wants before it stops merging edge
/// glyphs into the canvas boundary.
const PAD: u32 = 24;

/// Pinned source DPI. Bubble crops are far too small for
/// Tesseract's resolution estimator, whose wild guesses destabilize
/// layout analysis; 300 is the "scanned print" value the LSTM
/// expects. Must be set *after* `set_image` (SetImage resets it).
const SOURCE_PPI: i32 = 300;

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
        api.set_source_resolution(SOURCE_PPI)
            .map_err(|e| anyhow::anyhow!("tesseract set_source_resolution: {e}"))?;
        api.recognize()
            .map_err(|e| anyhow::anyhow!("tesseract recognize: {e}"))?;
        let words = collect_words(&api);
        // Rebuild the text from the word walk so `text` and `words`
        // can't disagree; the whole-page accessor is the fallback
        // when the iterator yields nothing.
        let text = match &words {
            Some(ws) => join_words(ws),
            None => api
                .get_utf8_text()
                .map(|t| t.trim().to_owned())
                .map_err(|e| anyhow::anyhow!("tesseract get_utf8_text: {e}"))?,
        };
        let confidence = api
            .mean_text_conf()
            .map(|c| (c.clamp(0, 100) as f32) / 100.0)
            .unwrap_or(0.0);
        drop(api);
        Ok(Recognition {
            text,
            confidence,
            words,
        })
    }
}

/// Walk the `ResultIterator` at word level. Returns `None` when the
/// iterator isn't available or yields no non-empty words — callers
/// fall back to the whole-page text. Per-word errors (Tesseract
/// returns a null pointer for empty positions) skip that position
/// rather than failing the walk.
fn collect_words(api: &TesseractAPI) -> Option<Vec<Word>> {
    let it = api.get_iterator().ok()?;
    let mut words: Vec<Word> = Vec::new();
    let mut line_index: u32 = 0;
    let mut prev_line_box: Option<(i32, i32, i32, i32)> = None;
    loop {
        if let Ok((text, left, top, right, bottom, conf)) = it.get_current_word() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                // Line boundaries: the word's enclosing textline bbox
                // changes exactly when a new line starts. Cheaper and
                // safer than tracking page-iterator begin/end flags.
                let line_box = it
                    .get_bounding_box(TessPageIteratorLevel::RIL_TEXTLINE)
                    .ok();
                if let (Some(prev), Some(cur)) = (prev_line_box, line_box)
                    && prev != cur
                {
                    line_index += 1;
                }
                if line_box.is_some() {
                    prev_line_box = line_box;
                }
                words.push(Word {
                    text: trimmed.to_owned(),
                    confidence: (conf.clamp(0.0, 100.0)) / 100.0,
                    xmin: unmap(left),
                    ymin: unmap(top),
                    xmax: unmap(right),
                    ymax: unmap(bottom),
                    line_index,
                });
            }
        }
        match it.next(TessPageIteratorLevel::RIL_WORD) {
            Ok(true) => continue,
            // `Ok(false)` is end-of-results; an `Err` mid-walk means
            // the iterator handle went bad — keep what we have.
            _ => break,
        }
    }
    if words.is_empty() { None } else { Some(words) }
}

/// Words joined with spaces, `\n` at line boundaries — mirrors what
/// `get_utf8_text` would produce for the same recognition, minus
/// trailing whitespace.
fn join_words(words: &[Word]) -> String {
    let mut out = String::new();
    let mut prev_line = words.first().map(|w| w.line_index).unwrap_or(0);
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(if w.line_index != prev_line { '\n' } else { ' ' });
        }
        out.push_str(&w.text);
        prev_line = w.line_index;
    }
    out
}

/// Map a prepared-image coordinate back into original-crop space:
/// strip the white padding, undo the upscale.
fn unmap(v: i32) -> f32 {
    ((v - PAD as i32).max(0) as f32) / UPSCALE as f32
}

/// Grayscale → 3× Lanczos3 upscale → Otsu binarization → polarity
/// fix → white border pad. Returns a tightly packed 8-bit luma
/// buffer ready for `TessBaseAPISetImage`.
fn preprocess(image: &DynamicImage) -> image::GrayImage {
    let gray = image.to_luma8();
    let (w, h) = gray.dimensions();
    let mut bin = image::imageops::resize(&gray, w * UPSCALE, h * UPSCALE, FilterType::Lanczos3);

    // Otsu picks the cut that best separates the crop's two
    // intensity populations (lettering vs balloon fill). A
    // near-uniform crop degenerates Otsu toward an extreme, which
    // would binarize everything to one side — fall back to the
    // midpoint there.
    let level = imageproc::contrast::otsu_level(&bin);
    let level = if level == 0 || level == u8::MAX {
        127
    } else {
        level
    };
    for px in bin.pixels_mut() {
        px.0[0] = if px.0[0] <= level { 0 } else { 255 };
    }

    if is_white_on_black(&bin) {
        for px in bin.pixels_mut() {
            px.0[0] = 255 - px.0[0];
        }
    }

    let (bw, bh) = bin.dimensions();
    let mut padded = image::GrayImage::from_pixel(bw + 2 * PAD, bh + 2 * PAD, image::Luma([255u8]));
    image::imageops::replace(&mut padded, &bin, i64::from(PAD), i64::from(PAD));
    padded
}

/// Border-ring polarity probe. Bubble interiors are mostly paper by
/// construction, so a majority-black 2 px perimeter means the crop
/// is white-on-black (dark panel / black caption box) and should be
/// inverted to the dark-on-light distribution Tesseract expects.
/// Ring-based beats whole-image counts: dense lettering can make a
/// dark-on-light crop majority-black overall.
fn is_white_on_black(bin: &image::GrayImage) -> bool {
    let (w, h) = bin.dimensions();
    if w == 0 || h == 0 {
        return false;
    }
    let ring = 2u32.min(w).min(h);
    let mut black = 0u64;
    let mut total = 0u64;
    for y in 0..h {
        let on_band = y < ring || y >= h - ring;
        for x in 0..w {
            if on_band || x < ring || x >= w - ring {
                total += 1;
                if bin.get_pixel(x, y).0[0] == 0 {
                    black += 1;
                }
            }
        }
    }
    total > 0 && black * 2 > total
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn uniform(w: u32, h: u32, v: u8) -> GrayImage {
        GrayImage::from_pixel(w, h, Luma([v]))
    }

    #[test]
    fn preprocess_pads_with_white_border() {
        let img = DynamicImage::ImageLuma8(uniform(10, 10, 200));
        let out = preprocess(&img);
        assert_eq!(out.width(), 10 * UPSCALE + 2 * PAD);
        assert_eq!(out.height(), 10 * UPSCALE + 2 * PAD);
        // Corner is border padding → white.
        assert_eq!(out.get_pixel(0, 0).0[0], 255);
    }

    #[test]
    fn preprocess_inverts_white_on_black() {
        // Black background, small white "glyph" in the center —
        // after polarity fix the background must be white.
        let mut img = uniform(40, 40, 10);
        for y in 18..22 {
            for x in 18..22 {
                img.put_pixel(x, y, Luma([245]));
            }
        }
        let out = preprocess(&DynamicImage::ImageLuma8(img));
        // Inside the padded area, just past the border: background.
        let inside = PAD + 2;
        assert_eq!(out.get_pixel(inside, inside).0[0], 255);
        // Center of the glyph (account for upscale + pad): black.
        let cx = PAD + 20 * UPSCALE;
        assert_eq!(out.get_pixel(cx, cx).0[0], 0);
    }

    #[test]
    fn near_uniform_crop_does_not_collapse_to_black() {
        // A flat light-gray crop: degenerate-Otsu guard must keep
        // the midpoint cut, mapping it to all-white (not all-black).
        let img = DynamicImage::ImageLuma8(uniform(20, 20, 180));
        let out = preprocess(&img);
        let inside = PAD + 5;
        assert_eq!(out.get_pixel(inside, inside).0[0], 255);
    }

    #[test]
    fn join_words_uses_line_index_for_newlines() {
        let mk = |text: &str, line: u32| Word {
            text: text.to_owned(),
            confidence: 0.9,
            xmin: 0.0,
            ymin: 0.0,
            xmax: 1.0,
            ymax: 1.0,
            line_index: line,
        };
        let words = vec![mk("HELLO", 0), mk("THERE", 0), mk("WORLD", 1)];
        assert_eq!(join_words(&words), "HELLO THERE\nWORLD");
    }
}
