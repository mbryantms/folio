//! Recognizer smoke tests (text-detection-1.0 plan, M2).
//!
//! These tests verify the `Recognizer` trait wiring around the
//! third-party `tesseract-rs` and `manga-ocr` crates. They are
//! intentionally minimal: M2 is about getting the API surface
//! right, not measuring OCR quality (that's testing what upstream
//! does, which we don't own).
//!
//! Most tests here are `#[ignore]` because they:
//!
//!  - Trigger native Tesseract C++ compilation on the first build
//!    (~10 min cmake job, cached at `~/.tesseract-rs/`), OR
//!  - Download ~80 MB of ONNX from Hugging Face on first init, OR
//!  - Run real ML inference (a few hundred ms each).
//!
//! Run them explicitly with `cargo test -p server --test
//! ocr_recognizer -- --ignored`. The non-ignored cases only check
//! that the trait shape compiles, which CI runs on every push.

use image::{DynamicImage, GrayImage, Luma};
use server::ocr::recognizer::{Recognizer, manga::MangaOcr, western::WesternOcr};

/// Compile-time witness that both impls actually satisfy
/// `Recognizer`. If a future refactor accidentally drops the trait
/// bound the test crate won't compile — caught by `cargo check`
/// before any model has to load.
fn _trait_witness<'a>(
    w: &'a WesternOcr,
    m: &'a MangaOcr,
) -> (&'a dyn Recognizer, &'a dyn Recognizer) {
    (w, m)
}

#[test]
fn trait_witness_compiles() {
    // Body intentionally empty — value is in `_trait_witness`'s
    // existence. Keeping the wrapper test prevents a clippy /
    // `unused` lint from gating builds in CI.
}

/// Blank white grayscale tile. Tesseract returns an empty string +
/// 0 confidence; the test only asserts the call doesn't panic and
/// that the result envelope is well-shaped.
fn blank_tile(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([255])))
}

#[tokio::test]
#[ignore = "requires the build-time-compiled tesseract toolchain"]
async fn western_recognizes_a_blank_tile_without_panicking() {
    let ocr = WesternOcr::shared()
        .await
        .expect("western init should succeed once tessdata is staged");
    let out = ocr
        .recognize(&blank_tile(120, 60))
        .expect("recognize should return Ok even for empty input");
    // No text expected — blank image. We only assert the contract.
    assert!(
        out.text.is_empty(),
        "blank tile produced text: {:?}",
        out.text
    );
    assert!(
        (0.0..=1.0).contains(&out.confidence),
        "confidence outside [0, 1]: {}",
        out.confidence
    );
    assert!(
        out.words.is_none(),
        "M2 doesn't populate per-word boxes yet"
    );
}

#[tokio::test]
#[ignore = "requires the manga-ocr ONNX models (HF download, ~80 MB)"]
async fn manga_recognizes_a_blank_tile_without_panicking() {
    let ocr = MangaOcr::shared()
        .await
        .expect("manga-ocr init should succeed once models are cached");
    let out = ocr
        .recognize(&blank_tile(224, 224))
        .expect("recognize should return Ok even for empty input");
    // manga-ocr is greedy-decode and may hallucinate a short string
    // on blank input — we don't assert text content, only envelope.
    assert!(
        (0.0..=1.0).contains(&out.confidence),
        "confidence outside [0, 1]: {}",
        out.confidence
    );
    assert!(
        out.words.is_none(),
        "M2 doesn't populate per-word boxes yet"
    );
}
