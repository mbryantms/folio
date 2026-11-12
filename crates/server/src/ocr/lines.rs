//! The line finder: splits one balloon crop into its text lines.
//!
//! The Western recogniser reads one line at a time
//! ([`super::recognizer::ctc`]), so a balloon has to be cut into lines
//! first. This runs `PaddlePaddle/PP-OCRv5_mobile_det_onnx` (a DB text
//! detector, 5 MB) on the crop and returns the line rectangles in
//! reading order.
//!
//! - **Input**: the crop enlarged [`LINE_SCALE`]× (comic lettering is
//!   small for a detector trained on documents), the long side capped at
//!   [`LINE_MAX_LONG_SIDE`], both sides rounded to a multiple of 32;
//!   BGR, `(x/255 − mean) / std` with the ImageNet mean and std applied
//!   in that channel order (what PaddleOCR's own pipeline does), CHW.
//! - **Output**: a probability map of the input's size. Postprocess
//!   ([`db_boxes`]): pixels above [`DB_THRESH`] form components
//!   (8-connected); each component's axis-aligned box is scored by the
//!   mean probability inside it and dropped below [`DB_BOX_THRESH`];
//!   the box is grown on every side by `area × DB_UNCLIP_RATIO /
//!   perimeter` (DB shrinks text regions in training; this is the closed
//!   form of its polygon offset for a rectangle) and mapped back to crop
//!   pixels. Rotated boxes are not produced.
//! - **Order** ([`reading_order`]): rows top to bottom, left to right
//!   within a row.
//! - **No lines found**: the whole crop is read as one line.
//!
//! The session is locked for the inference only. Blocking: call from the
//! blocking pool.

use std::path::Path;
use std::sync::Mutex;

use image::{DynamicImage, GrayImage, Luma, imageops::FilterType};
use ndarray::{Array4, ArrayView2};
use ort::session::Session;
use ort::value::TensorRef;
use tokio::sync::OnceCell;

use super::{models, session};

/// The crop is enlarged by this factor before detection.
pub const LINE_SCALE: f64 = 2.0;
/// The enlarged crop's long side is at most this (a smaller factor is
/// used for a large crop).
pub const LINE_MAX_LONG_SIDE: f64 = 1600.0;
/// The model wants sides that are multiples of this.
const SIDE_MULTIPLE: f64 = 32.0;

/// ImageNet mean and std, applied to the B, G, R planes in this order.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Probability above which a pixel belongs to text.
pub const DB_THRESH: f32 = 0.3;
/// A box whose mean probability is below this is dropped.
pub const DB_BOX_THRESH: f32 = 0.6;
/// How far a box is grown back: `area × ratio / perimeter` per side.
pub const DB_UNCLIP_RATIO: f32 = 1.5;
/// A component thinner than this (map pixels) is dropped.
const DB_MIN_SIDE: f32 = 3.0;
/// A grown box thinner than this (map pixels) is dropped.
const DB_MIN_UNCLIPPED_SIDE: f32 = 5.0;
/// Components looked at, in the order their first pixel appears.
const DB_MAX_CANDIDATES: usize = 1000;

/// A line box must be wider and taller than this (crop pixels) to count
/// as found.
const MIN_FOUND_SIDE: u32 = 2;
/// A line is read only when both sides have at least this many pixels.
pub const MIN_READ_SIDE: u32 = 4;

/// A rectangle in crop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One box from the probability map, in crop pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LineBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub score: f32,
}

/// The line finder's session. [`LineFinder::shared`] is the server's
/// process-wide instance.
pub struct LineFinder {
    session: Mutex<Session>,
    input: String,
    output: String,
    first_use: super::FirstUse,
}

impl LineFinder {
    /// The shared instance, loaded on first call (pinned model fetched
    /// and verified, session built).
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<LineFinder> = OnceCell::const_new();
        CELL.get_or_try_init(|| async {
            let started = std::time::Instant::now();
            let me = Self::load().await?;
            super::record_model_load("line_finder", started.elapsed());
            Ok(me)
        })
        .await
    }

    /// A line finder of its own (tools; the server uses [`Self::shared`]).
    pub async fn load() -> anyhow::Result<Self> {
        models::ensure_pinned(models::LINE_FINDER_FILES).await?;
        let path = models::cached_path(&models::LINE_FINDER_FILES[0])?;
        let threads = session::intra_threads(session::LINE_FINDER_MAX_INTRA_THREADS);
        tokio::task::spawn_blocking(move || Self::from_file(&path, threads))
            .await
            .map_err(|e| anyhow::anyhow!("line finder init task panicked: {e}"))?
    }

    fn from_file(path: &Path, intra_threads: usize) -> anyhow::Result<Self> {
        let session = session::build(path, intra_threads)?;
        let (Some(input), Some(output)) = (session.inputs.first(), session.outputs.first()) else {
            anyhow::bail!("the line finder model has no input or no output");
        };
        Ok(LineFinder {
            input: input.name.clone(),
            output: output.name.clone(),
            session: Mutex::new(session),
            first_use: super::FirstUse::new(),
        })
    }

    /// The text lines of `crop` in reading order, in crop pixels; the
    /// whole crop as one line when none is found.
    pub fn find(&self, crop: &DynamicImage) -> anyhow::Result<Vec<LineRect>> {
        let started = std::time::Instant::now();
        let (w, h) = (crop.width(), crop.height());
        let x = preprocess(crop);
        let prob = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| anyhow::anyhow!("line finder mutex poisoned"))?;
            let outputs = session.run(ort::inputs![
                self.input.as_str() => TensorRef::from_array_view(x.view())?
            ])?;
            let map = outputs[self.output.as_str()].try_extract_array::<f32>()?;
            let shape = map.shape().to_vec();
            if shape.len() != 4 || shape[0] != 1 || shape[1] != 1 {
                anyhow::bail!("line finder: unexpected output shape {shape:?}");
            }
            map.to_owned()
                .into_shape_with_order((shape[2], shape[3]))
                .map_err(|e| anyhow::anyhow!("line finder: {e}"))?
        };
        let mut boxes: Vec<[f32; 4]> = db_boxes(prob.view(), w, h)
            .into_iter()
            .map(|b| [b.x0, b.y0, b.x1, b.y1])
            .collect();
        reading_order(&mut boxes);
        let lines = lines_or_whole(&boxes, w, h);
        self.first_use.record("line_finder", started.elapsed());
        Ok(lines)
    }
}

/// The size the crop is resized to: [`LINE_SCALE`]×, less when the long
/// side would pass [`LINE_MAX_LONG_SIDE`], each side rounded to a
/// multiple of 32 (at least 32).
pub(crate) fn input_size(w: u32, h: u32) -> (u32, u32) {
    let round = |v: f64| ((v / SIDE_MULTIPLE).round() as u32).max(1) * SIDE_MULTIPLE as u32;
    let scale = LINE_SCALE.min(LINE_MAX_LONG_SIDE / f64::from(w.max(h).max(1)));
    (round(f64::from(w) * scale), round(f64::from(h) * scale))
}

/// The crop as the model's input: resized to [`input_size`] (bilinear),
/// planes in B, G, R order, `(x/255 − mean) / std`, shape `[1, 3, h, w]`.
pub(crate) fn preprocess(crop: &DynamicImage) -> Array4<f32> {
    let (nw, nh) = input_size(crop.width(), crop.height());
    let rgb = crop.resize_exact(nw, nh, FilterType::Triangle).to_rgb8();
    let mut x = Array4::<f32>::zeros((1, 3, nh as usize, nw as usize));
    for (px, py, p) in rgb.enumerate_pixels() {
        let [r, g, b] = p.0;
        for (c, v) in [b, g, r].into_iter().enumerate() {
            x[[0, c, py as usize, px as usize]] = (f32::from(v) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    x
}

/// DB postprocess: the text boxes of a probability map, in pixels of the
/// `src_w × src_h` crop the map was computed for. See the module docs.
pub(crate) fn db_boxes(prob: ArrayView2<'_, f32>, src_w: u32, src_h: u32) -> Vec<LineBox> {
    let (map_h, map_w) = prob.dim();
    if map_h == 0 || map_w == 0 {
        return Vec::new();
    }
    let mut text = GrayImage::new(map_w as u32, map_h as u32);
    for ((y, x), &p) in prob.indexed_iter() {
        if p > DB_THRESH {
            text.put_pixel(x as u32, y as u32, Luma([255]));
        }
    }
    let labels = imageproc::region_labelling::connected_components(
        &text,
        imageproc::region_labelling::Connectivity::Eight,
        Luma([0u8]),
    );
    // Bounds per component, indexed by label (labels start at 1 and are
    // handed out in raster order).
    let mut bounds: Vec<Option<[usize; 4]>> = Vec::new();
    for (x, y, label) in labels.enumerate_pixels() {
        let label = label.0[0] as usize;
        if label == 0 {
            continue;
        }
        if bounds.len() <= label {
            bounds.resize(label + 1, None);
        }
        let (x, y) = (x as usize, y as usize);
        let b = bounds[label].get_or_insert([x, y, x, y]);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    }
    let (sx, sy) = (
        f64::from(src_w) / map_w as f64,
        f64::from(src_h) / map_h as f64,
    );
    let mut out = Vec::new();
    for b in bounds.into_iter().flatten().take(DB_MAX_CANDIDATES) {
        let (bw, bh) = ((b[2] - b[0] + 1) as f32, (b[3] - b[1] + 1) as f32);
        if bw.min(bh) < DB_MIN_SIDE {
            continue;
        }
        let mut sum = 0.0f32;
        for y in b[1]..=b[3] {
            for x in b[0]..=b[2] {
                sum += prob[[y, x]];
            }
        }
        let score = sum / (bw * bh);
        if score < DB_BOX_THRESH {
            continue;
        }
        let grow = bw * bh * DB_UNCLIP_RATIO / (2.0 * (bw + bh));
        let (x0, y0) = (b[0] as f32 - grow, b[1] as f32 - grow);
        let (x1, y1) = (b[2] as f32 + 1.0 + grow, b[3] as f32 + 1.0 + grow);
        if (x1 - x0).min(y1 - y0) < DB_MIN_UNCLIPPED_SIDE {
            continue;
        }
        out.push(LineBox {
            x0: (f64::from(x0) * sx).clamp(0.0, f64::from(src_w)) as f32,
            y0: (f64::from(y0) * sy).clamp(0.0, f64::from(src_h)) as f32,
            x1: (f64::from(x1) * sx).clamp(0.0, f64::from(src_w)) as f32,
            y1: (f64::from(y1) * sy).clamp(0.0, f64::from(src_h)) as f32,
            score,
        });
    }
    out
}

/// Sort `x0, y0, x1, y1` boxes into reading order: rows top to bottom,
/// left to right within a row. A box joins the current row when its
/// vertical centre is within half the shorter height of the row's first
/// box.
pub(crate) fn reading_order(boxes: &mut Vec<[f32; 4]>) {
    boxes.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut rows: Vec<Vec<[f32; 4]>> = Vec::new();
    for b in boxes.drain(..) {
        let centre = (b[1] + b[3]) / 2.0;
        let height = b[3] - b[1];
        if let Some(row) = rows.last_mut() {
            let first = row[0];
            let row_centre = (first[1] + first[3]) / 2.0;
            if (centre - row_centre).abs() < 0.5 * height.min(first[3] - first[1]) {
                row.push(b);
                continue;
            }
        }
        rows.push(vec![b]);
    }
    for mut row in rows {
        row.sort_by(|a, b| a[0].total_cmp(&b[0]));
        boxes.extend(row);
    }
}

/// Ordered boxes as whole-pixel rectangles on a `w × h` crop (rounded,
/// clamped; a box not wider and taller than [`MIN_FOUND_SIDE`] is not a
/// line), or the whole crop as one line when none is left.
pub(crate) fn lines_or_whole(boxes: &[[f32; 4]], w: u32, h: u32) -> Vec<LineRect> {
    let edge = |v: f32, size: u32| (v.max(0.0).round() as u32).min(size);
    let lines: Vec<LineRect> = boxes
        .iter()
        .filter_map(|b| {
            let (x0, y0) = (edge(b[0], w), edge(b[1], h));
            let (x1, y1) = (edge(b[2], w), edge(b[3], h));
            (x1 > x0 + MIN_FOUND_SIDE && y1 > y0 + MIN_FOUND_SIDE).then_some(LineRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            })
        })
        .collect();
    if lines.is_empty() {
        vec![LineRect { x: 0, y: 0, w, h }]
    } else {
        lines
    }
}

/// Cut the lines that are large enough to read ([`MIN_READ_SIDE`]) from
/// the crop. Returns each kept rectangle with its image, in order.
pub(crate) fn cut(crop: &DynamicImage, lines: &[LineRect]) -> Vec<(LineRect, DynamicImage)> {
    lines
        .iter()
        .filter(|r| r.w >= MIN_READ_SIDE && r.h >= MIN_READ_SIDE)
        .map(|r| (*r, crop.crop_imm(r.x, r.y, r.w, r.h)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use ndarray::Array2;

    #[test]
    fn the_crop_is_doubled_capped_and_rounded_to_32() {
        // 2x, rounded to the nearest multiple of 32.
        assert_eq!(input_size(300, 200), (608, 416));
        assert_eq!(input_size(160, 96), (320, 192));
        // The long side stops at 1600: a smaller factor for a large crop.
        assert_eq!(input_size(1000, 500), (1600, 800));
        assert_eq!(input_size(2000, 3000), (1056, 1600));
        // Never below one multiple.
        assert_eq!(input_size(1, 1), (32, 32));
        assert_eq!(input_size(4000, 4), (1600, 32));
    }

    #[test]
    fn the_input_is_bgr_with_the_imagenet_normalisation() {
        // 16 x 16 becomes 32 x 32; a flat colour stays flat.
        let crop = DynamicImage::ImageRgb8(RgbImage::from_pixel(16, 16, Rgb([255, 0, 128])));
        let x = preprocess(&crop);
        assert_eq!(x.shape(), &[1, 3, 32, 32]);
        let want = |v: f32, c: usize| (v / 255.0 - MEAN[c]) / STD[c];
        // Plane 0 is blue (128), plane 1 green (0), plane 2 red (255).
        for (plane, value) in [(0, 128.0), (1, 0.0), (2, 255.0)] {
            let got = x[[0, plane, 10, 20]];
            assert!((got - want(value, plane)).abs() < 1e-5, "plane {plane}");
        }
    }

    /// A map with rectangles of the given probability on a zero ground.
    fn map(w: usize, h: usize, blobs: &[([usize; 4], f32)]) -> Array2<f32> {
        let mut m = Array2::<f32>::zeros((h, w));
        for ([x0, y0, x1, y1], p) in blobs {
            for y in *y0..*y1 {
                for x in *x0..*x1 {
                    m[[y, x]] = *p;
                }
            }
        }
        m
    }

    #[test]
    fn a_text_blob_becomes_one_grown_box_in_crop_pixels() {
        // A 40 x 10 blob at (20, 30) on a 128 x 96 map of a 64 x 48 crop
        // (the crop was doubled).
        let m = map(128, 96, &[([20, 30, 60, 40], 0.9)]);
        let boxes = db_boxes(m.view(), 64, 48);
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        let b = boxes[0];
        assert!((b.score - 0.9).abs() < 1e-5);
        // Grown by area * 1.5 / perimeter = 400 * 1.5 / 100 = 6 map
        // pixels a side, then halved into crop pixels.
        assert!((b.x0 - 7.0).abs() < 1e-4, "{b:?}");
        assert!((b.y0 - 12.0).abs() < 1e-4, "{b:?}");
        assert!((b.x1 - 33.0).abs() < 1e-4, "{b:?}");
        assert!((b.y1 - 23.0).abs() < 1e-4, "{b:?}");
    }

    #[test]
    fn separate_blobs_are_separate_boxes_and_touching_ones_merge() {
        let m = map(
            200,
            100,
            &[
                ([10, 10, 80, 22], 0.9),
                ([10, 40, 80, 52], 0.9),
                // Touches the second at a corner only: with 8-connected
                // components it is part of it, not a third box.
                ([80, 52, 84, 56], 0.9),
            ],
        );
        let boxes = db_boxes(m.view(), 200, 100);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
    }

    #[test]
    fn weak_thin_and_faint_blobs_are_dropped() {
        let m = map(
            200,
            120,
            &[
                // Below the pixel threshold: not text at all.
                ([10, 10, 80, 22], 0.29),
                // Above it, but the box's mean stays under 0.6.
                ([10, 40, 80, 52], 0.5),
                // Confident but two pixels tall.
                ([10, 70, 80, 72], 0.95),
                // Kept.
                ([10, 90, 80, 102], 0.95),
            ],
        );
        let boxes = db_boxes(m.view(), 200, 120);
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        assert!(boxes[0].y0 > 80.0);
    }

    #[test]
    fn a_grown_box_is_clamped_to_the_crop() {
        let m = map(64, 64, &[([0, 0, 64, 20], 0.9)]);
        let b = db_boxes(m.view(), 64, 64)[0];
        assert_eq!((b.x0, b.y0, b.x1), (0.0, 0.0, 64.0));
        assert!(b.y1 > 20.0 && b.y1 <= 64.0);
    }

    #[test]
    fn an_empty_map_has_no_boxes() {
        assert!(db_boxes(Array2::<f32>::zeros((32, 32)).view(), 16, 16).is_empty());
        assert!(db_boxes(Array2::<f32>::zeros((0, 0)).view(), 16, 16).is_empty());
    }

    #[test]
    fn lines_are_ordered_by_rows_then_columns() {
        let mut b = vec![
            [100.0, 52.0, 200.0, 80.0],
            [0.0, 0.0, 90.0, 30.0],
            [0.0, 50.0, 90.0, 80.0],
            [100.0, 2.0, 200.0, 28.0],
        ];
        reading_order(&mut b);
        assert_eq!(
            b,
            vec![
                [0.0, 0.0, 90.0, 30.0],
                [100.0, 2.0, 200.0, 28.0],
                [0.0, 50.0, 90.0, 80.0],
                [100.0, 52.0, 200.0, 80.0],
            ]
        );
    }

    #[test]
    fn stacked_lines_stay_in_their_own_rows() {
        // Three lines of a balloon, given out of order; each is its own row.
        let mut b = vec![
            [12.0, 60.0, 110.0, 80.0],
            [20.0, 10.0, 100.0, 30.0],
            [5.0, 35.0, 115.0, 55.0],
        ];
        reading_order(&mut b);
        let tops: Vec<f32> = b.iter().map(|r| r[1]).collect();
        assert_eq!(tops, [10.0, 35.0, 60.0]);
    }

    #[test]
    fn boxes_become_whole_pixel_lines_inside_the_crop() {
        let lines = lines_or_whole(
            &[[1.4, 2.6, 50.5, 20.4], [-3.0, 30.0, 300.0, 44.0]],
            100,
            40,
        );
        assert_eq!(
            lines,
            [
                LineRect {
                    x: 1,
                    y: 3,
                    w: 50,
                    h: 17
                },
                LineRect {
                    x: 0,
                    y: 30,
                    w: 100,
                    h: 10
                },
            ]
        );
    }

    #[test]
    fn no_usable_line_means_the_whole_crop() {
        let whole = [LineRect {
            x: 0,
            y: 0,
            w: 120,
            h: 60,
        }];
        assert_eq!(lines_or_whole(&[], 120, 60), whole);
        // Two pixels tall after rounding: not a line.
        assert_eq!(lines_or_whole(&[[10.0, 10.0, 80.0, 12.0]], 120, 60), whole);
    }

    #[test]
    fn only_lines_large_enough_to_read_are_cut() {
        let crop = DynamicImage::ImageRgb8(RgbImage::new(100, 50));
        let lines = [
            LineRect {
                x: 0,
                y: 0,
                w: 60,
                h: 12,
            },
            // Three pixels tall: found, not read.
            LineRect {
                x: 0,
                y: 20,
                w: 60,
                h: 3,
            },
            LineRect {
                x: 10,
                y: 30,
                w: 4,
                h: 4,
            },
        ];
        let cut = cut(&crop, &lines);
        assert_eq!(cut.len(), 2);
        assert_eq!(cut[0].0, lines[0]);
        assert_eq!((cut[0].1.width(), cut[0].1.height()), (60, 12));
        assert_eq!(cut[1].0, lines[2]);
    }
}
