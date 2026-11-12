//! The page detector of the current engine set: balloons, the text in
//! them, and free text.
//!
//! `ogkalu/comic-text-and-bubble-detector`, file
//! `detector-v4-s_int8.onnx`: an RT-DETR-v2 export, 8-bit quantized
//! (11 MB). One inference per page.
//!
//! - **Input** `images`: the page squashed to 640 × 640 (not cropped, not
//!   padded: the model was trained on resized pages), RGB, scaled by
//!   1/255, no mean/std normalisation. `orig_target_sizes`: the page's
//!   `[width, height]`, so the model returns boxes in page pixels.
//! - **Output** `labels`, `boxes` (`x0, y0, x1, y1` in page pixels) and
//!   `scores`, one row per query. The model needs no non-maximum
//!   suppression.
//! - **Classes**: 0 `bubble`, 1 `text_bubble` (the text inside a
//!   balloon), 2 `text_free` (text outside balloons). A page's detection
//!   keeps the boxes of all three ([`boxes`]). The **regions** Folio
//!   offers to clients, snaps a drawn rectangle to and reads on a tap
//!   are classes 0 and 2 ([`is_region_class`]): a balloon, or a piece of
//!   free text. Class 1 lies inside a class 0 box; it is kept for the
//!   one consumer that may want the text without the balloon around it
//!   ([`super::manga_crop`]).
//!
//! The session sits behind a [`std::sync::Mutex`] (`run` takes `&mut`),
//! taken for the inference only: the image work before and the box work
//! after run outside it. Only call [`BubbleDetector::detect`] from the
//! blocking pool.

use std::path::Path;
use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use tokio::sync::OnceCell;

use super::cache::{CachedBbox, CachedDetection};
use super::pipeline::PageDetector;
use super::{models, session};

/// Side of the square the page is squashed to.
pub const INPUT_SIZE: u32 = 640;

/// Rows scoring below this are dropped.
pub const SCORE_THRESHOLD: f32 = 0.5;

/// A balloon.
pub const CLASS_BUBBLE: u32 = 0;
/// The text inside a balloon. Kept in a page's detection, not offered
/// as a region.
pub const CLASS_TEXT_BUBBLE: u32 = 1;
/// Text outside balloons (captions on the art, sound effects, signs).
pub const CLASS_TEXT_FREE: u32 = 2;

/// Whether a detector class is one of the regions Folio offers: what
/// the text-regions route returns and what `detect: true` snaps to.
pub const fn is_region_class(class: u32) -> bool {
    class == CLASS_BUBBLE || class == CLASS_TEXT_FREE
}

/// Whether a class index is one the model defines.
const fn is_known_class(class: u32) -> bool {
    class <= CLASS_TEXT_FREE
}

const INPUT_IMAGES: &str = "images";
const INPUT_SIZES: &str = "orig_target_sizes";
const OUTPUT_LABELS: &str = "labels";
const OUTPUT_BOXES: &str = "boxes";
const OUTPUT_SCORES: &str = "scores";

/// The detector's session. [`BubbleDetector::shared`] is the server's
/// process-wide instance.
pub struct BubbleDetector {
    session: Mutex<Session>,
    first_use: super::FirstUse,
}

/// The model's rows for one page, before thresholding.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RawDetections {
    pub labels: Vec<i64>,
    /// `x0, y0, x1, y1` in page pixels.
    pub boxes: Vec<[f32; 4]>,
    pub scores: Vec<f32>,
}

impl BubbleDetector {
    /// The shared instance, loaded on first call: the pinned model is
    /// fetched (if needed) and verified, then the session is built.
    /// Errors only while that has not succeeded yet.
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<BubbleDetector> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let started = std::time::Instant::now();
            let me = Self::load().await?;
            super::record_model_load("detector", started.elapsed());
            Ok(me)
        })
        .await
    }

    /// A detector of its own (tools; the server uses [`Self::shared`]).
    pub async fn load() -> anyhow::Result<Self> {
        models::ensure_pinned(models::BUBBLE_DETECTOR_FILES).await?;
        let path = models::cached_path(&models::BUBBLE_DETECTOR_FILES[0])?;
        let threads = session::intra_threads(session::MAX_INTRA_THREADS);
        tokio::task::spawn_blocking(move || Self::from_file(&path, threads))
            .await
            .map_err(|e| anyhow::anyhow!("detector init task panicked: {e}"))?
    }

    fn from_file(path: &Path, intra_threads: usize) -> anyhow::Result<Self> {
        let session = session::build(path, intra_threads)?;
        let (inputs, outputs) = (
            session::input_names(&session),
            session::output_names(&session),
        );
        let has = |names: &[&str], wanted: &[&str]| wanted.iter().all(|w| names.contains(w));
        if !has(&inputs, &[INPUT_IMAGES, INPUT_SIZES])
            || !has(&outputs, &[OUTPUT_LABELS, OUTPUT_BOXES, OUTPUT_SCORES])
        {
            anyhow::bail!(
                "the detector model has inputs {inputs:?} and outputs {outputs:?}; \
                 expected {INPUT_IMAGES}, {INPUT_SIZES} and {OUTPUT_LABELS}, {OUTPUT_BOXES}, {OUTPUT_SCORES}"
            );
        }
        Ok(BubbleDetector {
            session: Mutex::new(session),
            first_use: super::FirstUse::new(),
        })
    }

    /// One inference: the rows the model returns for `page`.
    fn run(&self, page: &DynamicImage) -> anyhow::Result<RawDetections> {
        let images = preprocess(page);
        let sizes = target_sizes(page.width(), page.height());
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("detector mutex poisoned"))?;
        let outputs = session.run(ort::inputs![
            INPUT_IMAGES => TensorRef::from_array_view(images.view())?,
            INPUT_SIZES => TensorRef::from_array_view(sizes.view())?,
        ])?;
        let labels = outputs[OUTPUT_LABELS].try_extract_array::<i64>()?;
        let boxes = outputs[OUTPUT_BOXES].try_extract_array::<f32>()?;
        let scores = outputs[OUTPUT_SCORES].try_extract_array::<f32>()?;
        let rows = scores.len();
        if labels.len() != rows || boxes.len() != rows * 4 {
            anyhow::bail!(
                "detector output shapes disagree: labels {:?}, boxes {:?}, scores {:?}",
                labels.shape(),
                boxes.shape(),
                scores.shape()
            );
        }
        let flat: Vec<f32> = boxes.iter().copied().collect();
        Ok(RawDetections {
            labels: labels.iter().copied().collect(),
            boxes: flat.as_chunks::<4>().0.to_vec(),
            scores: scores.iter().copied().collect(),
        })
    }
}

impl PageDetector for BubbleDetector {
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection> {
        let started = std::time::Instant::now();
        let raw = self.run(page)?;
        let bboxes = boxes(&raw, page.width(), page.height());
        self.first_use.record("detector", started.elapsed());
        Ok(CachedDetection {
            page_w: page.width(),
            page_h: page.height(),
            bboxes,
        })
    }
}

/// The page as the model's `images` input: squashed to
/// [`INPUT_SIZE`] × [`INPUT_SIZE`] (bilinear), RGB, each value / 255,
/// shape `[1, 3, 640, 640]`.
pub(crate) fn preprocess(page: &DynamicImage) -> Array4<f32> {
    let rgb = page
        .resize_exact(INPUT_SIZE, INPUT_SIZE, FilterType::Triangle)
        .to_rgb8();
    let side = INPUT_SIZE as usize;
    let mut x = Array4::<f32>::zeros((1, 3, side, side));
    for (px, py, p) in rgb.enumerate_pixels() {
        for c in 0..3 {
            x[[0, c, py as usize, px as usize]] = f32::from(p.0[c]) / 255.0;
        }
    }
    x
}

/// The `orig_target_sizes` input: `[[width, height]]` of the page. The
/// model scales its boxes with it, so they come back in page pixels
/// whatever the page's shape.
pub(crate) fn target_sizes(page_w: u32, page_h: u32) -> ndarray::Array2<i64> {
    ndarray::arr2(&[[i64::from(page_w), i64::from(page_h)]])
}

/// A page's detection from the model's rows: rows at or above
/// [`SCORE_THRESHOLD`], of any of the model's three classes, clamped to
/// the page; a box with nothing left on the page is dropped. Model order
/// is kept. Which of them are regions is [`is_region_class`].
pub(crate) fn boxes(raw: &RawDetections, page_w: u32, page_h: u32) -> Vec<CachedBbox> {
    let (w, h) = (page_w as f32, page_h as f32);
    raw.scores
        .iter()
        .zip(&raw.labels)
        .zip(&raw.boxes)
        .filter_map(|((&score, &label), b)| {
            if score.is_nan() || score < SCORE_THRESHOLD {
                return None;
            }
            let class = u32::try_from(label).ok().filter(|c| is_known_class(*c))?;
            if b.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let (xmin, xmax) = (b[0].clamp(0.0, w), b[2].clamp(0.0, w));
            let (ymin, ymax) = (b[1].clamp(0.0, h), b[3].clamp(0.0, h));
            (xmax > xmin && ymax > ymin).then_some(CachedBbox {
                xmin,
                ymin,
                xmax,
                ymax,
                confidence: score,
                class,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn the_page_is_squashed_to_the_square_and_scaled_to_unit_range() {
        // A wide page, left half red, right half blue.
        let mut page = RgbImage::new(400, 100);
        for (x, _, p) in page.enumerate_pixels_mut() {
            *p = if x < 200 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            };
        }
        let x = preprocess(&DynamicImage::ImageRgb8(page));
        assert_eq!(x.shape(), &[1, 3, 640, 640]);
        // Squashed, not letterboxed: the top and bottom rows hold the
        // page, and the halves still meet in the middle.
        for y in [0, 320, 639] {
            // Left: red channel 1, blue 0. Right: the reverse.
            assert!((x[[0, 0, y, 100]] - 1.0).abs() < 1e-6);
            assert!(x[[0, 2, y, 100]].abs() < 1e-6);
            assert!(x[[0, 0, y, 540]].abs() < 1e-6);
            assert!((x[[0, 2, y, 540]] - 1.0).abs() < 1e-6);
            assert!(x[[0, 1, y, 100]].abs() < 1e-6, "green stays 0");
        }
        assert!(x.iter().all(|v| (0.0..=1.0).contains(v)), "no mean/std");
    }

    #[test]
    fn the_target_size_is_width_then_height() {
        let sizes = target_sizes(1440, 1963);
        assert_eq!(sizes.shape(), &[1, 2]);
        assert_eq!(sizes[[0, 0]], 1440);
        assert_eq!(sizes[[0, 1]], 1963);
    }

    fn raw(rows: &[(i64, [f32; 4], f32)]) -> RawDetections {
        RawDetections {
            labels: rows.iter().map(|r| r.0).collect(),
            boxes: rows.iter().map(|r| r.1).collect(),
            scores: rows.iter().map(|r| r.2).collect(),
        }
    }

    /// A detection keeps the model's three classes, in model order, and
    /// nothing with a class index the model does not define.
    #[test]
    fn a_detection_keeps_all_three_classes_and_no_unknown_one() {
        let got = boxes(
            &raw(&[
                (0, [10.0, 20.0, 110.0, 80.0], 0.9),
                (1, [20.0, 30.0, 100.0, 70.0], 0.95),
                (2, [300.0, 400.0, 380.0, 430.0], 0.7),
                (3, [1.0, 1.0, 5.0, 5.0], 0.99),
                (7, [1.0, 1.0, 5.0, 5.0], 0.99),
                (-1, [1.0, 1.0, 5.0, 5.0], 0.99),
            ]),
            1000,
            1500,
        );
        let classes: Vec<u32> = got.iter().map(|b| b.class).collect();
        assert_eq!(classes, [CLASS_BUBBLE, CLASS_TEXT_BUBBLE, CLASS_TEXT_FREE]);
        assert_eq!(
            (got[0].xmin, got[0].ymin, got[0].xmax, got[0].ymax),
            (10.0, 20.0, 110.0, 80.0)
        );
        assert!((got[2].confidence - 0.7).abs() < 1e-6);
    }

    /// Balloons and free text are regions; the text inside a balloon is
    /// kept in the detection but is not one.
    #[test]
    fn balloons_and_free_text_are_regions_and_text_in_balloons_is_not() {
        assert!(is_region_class(CLASS_BUBBLE));
        assert!(is_region_class(CLASS_TEXT_FREE));
        assert!(!is_region_class(CLASS_TEXT_BUBBLE));
        assert!(!is_region_class(3));
    }

    #[test]
    fn rows_below_the_threshold_are_dropped_and_the_threshold_itself_is_kept() {
        let b = [0.0, 0.0, 10.0, 10.0];
        let got = boxes(
            &raw(&[(0, b, 0.49), (0, b, 0.5), (0, b, f32::NAN), (0, b, 0.51)]),
            100,
            100,
        );
        let scores: Vec<f32> = got.iter().map(|b| b.confidence).collect();
        assert_eq!(scores, [0.5, 0.51]);
    }

    /// The model returns page pixels (it is given the page size); what is
    /// left to do is keep them on the page, whatever its shape.
    #[test]
    fn boxes_stay_in_page_pixels_and_are_clamped_to_a_non_square_page() {
        let (w, h) = (795, 1100);
        let got = boxes(
            &raw(&[
                // Inside a tall page: untouched, beyond the square's side.
                (0, [600.5, 900.25, 790.0, 1090.75], 0.9),
                // Overshoots every edge.
                (0, [-12.0, -3.0, 820.0, 1130.0], 0.9),
                // Wholly outside, collapsed, and not finite: dropped.
                (0, [800.0, 10.0, 900.0, 20.0], 0.9),
                (0, [50.0, 50.0, 50.0, 90.0], 0.9),
                (0, [f32::NAN, 0.0, 10.0, 10.0], 0.9),
                (0, [0.0, 0.0, f32::INFINITY, 10.0], 0.9),
            ]),
            w,
            h,
        );
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(
            (got[0].xmin, got[0].ymin, got[0].xmax, got[0].ymax),
            (600.5, 900.25, 790.0, 1090.75)
        );
        assert_eq!(
            (got[1].xmin, got[1].ymin, got[1].xmax, got[1].ymax),
            (0.0, 0.0, 795.0, 1100.0)
        );
    }

    #[test]
    fn no_rows_is_no_boxes() {
        assert!(boxes(&RawDetections::default(), 100, 100).is_empty());
    }
}
