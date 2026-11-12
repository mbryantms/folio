//! The Western recogniser of the current engine set: a CTC line model
//! behind a line finder.
//!
//! [`LineOcr`] is the [`Recognizer`] the pipeline calls with a balloon
//! crop. It asks the line finder ([`crate::ocr::lines`]) for the crop's
//! text lines, cuts them out, and reads them all in one batch with
//! [`CtcRecognizer`], a PP-OCR line recogniser described by a
//! [`LineRecModel`] (which file is the model, where the characters come
//! from), so another model of the same family is a registry change in
//! [`crate::ocr::models`].
//!
//! # The model's contract
//!
//! - **Input** `[n, 3, 48, w]`: each line resized to height
//!   [`REC_HEIGHT`] keeping its aspect ratio, planes in B, G, R order,
//!   `(x/255 − 0.5) / 0.5`. The batch is as wide as its widest line, at
//!   least [`REC_MIN_WIDTH`]; narrower lines are padded on the right with
//!   zeros. (PaddleOCR's `resize_norm_img` for its batched predictor.)
//! - **Output** `[n, steps, classes]` probabilities. Class 0 is the CTC
//!   blank, classes `1..=len` are the dictionary, and the last class is
//!   the space: `classes = len + 2`. This is checked when the engine
//!   loads; on a mismatch the engine does not start, because every
//!   character would decode as its neighbour.
//! - **Decode** ([`ctc_text`]): per step the most probable class;
//!   repeats collapse, blanks drop. A line's confidence is the mean
//!   probability of the classes it kept.
//!
//! The session is locked for the inference and the per-step argmax only.
//! Blocking: call from the blocking pool.

use std::path::Path;
use std::sync::Mutex;

use image::{DynamicImage, imageops::FilterType};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use tokio::sync::OnceCell;

use super::{Recognition, Recognizer, Word};
use crate::ocr::lines::{self, LineFinder, LineRect};
use crate::ocr::models::{self, DictSource, LineRecModel};
use crate::ocr::session;

/// Height every line is resized to.
pub const REC_HEIGHT: u32 = 48;
/// Narrowest batch (the model's nominal `3 × 48 × 320` input).
pub const REC_MIN_WIDTH: u32 = 320;
/// Widest batch: a longer line is squeezed to this (PaddleOCR's own
/// ceiling). 66 times the line height, so no balloon line reaches it; it
/// bounds the tensor for a drawn region that is one long strip.
pub const REC_MAX_WIDTH: u32 = 3200;
/// Most input columns (lines × batch width) in one run; lines past it go
/// into a following run. A balloon stays far below (twenty lines 800
/// wide are 16,000), so its lines are always read in a single batch.
const REC_MAX_BATCH_COLUMNS: usize = 96_000;

/// The class CTC uses for "no character".
const CTC_BLANK: usize = 0;
/// Classes the model has beyond the dictionary: the blank and the space.
const EXTRA_CLASSES: usize = 2;

/// One recognised line: its text (trimmed) and confidence in `[0, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub struct LineText {
    pub text: String,
    pub confidence: f32,
}

/// A CTC line recogniser session with its dictionary.
pub struct CtcRecognizer {
    model: &'static LineRecModel,
    session: Mutex<Session>,
    input: String,
    output: String,
    dict: Vec<char>,
    first_use: crate::ocr::FirstUse,
}

impl CtcRecognizer {
    /// Fetch (if needed) and verify the model's pinned files, read its
    /// dictionary, build the session and check that the dictionary fits
    /// the model's classes.
    pub async fn load(model: &'static LineRecModel) -> anyhow::Result<Self> {
        models::ensure_pinned(model.files).await?;
        let pin = |name: &str| {
            model
                .file(name)
                .ok_or_else(|| anyhow::anyhow!("{}: no pinned file named {name}", model.id))
        };
        let onnx = models::cached_path(pin(model.onnx)?)?;
        let DictSource::PaddleInferenceYml(yml) = model.dictionary;
        let yml = models::cached_path(pin(yml)?)?;
        let threads = session::intra_threads(session::MAX_INTRA_THREADS);
        tokio::task::spawn_blocking(move || Self::from_files(model, &onnx, &yml, threads))
            .await
            .map_err(|e| anyhow::anyhow!("line recogniser init task panicked: {e}"))?
    }

    fn from_files(
        model: &'static LineRecModel,
        onnx: &Path,
        yml: &Path,
        intra_threads: usize,
    ) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(yml)
            .map_err(|e| anyhow::anyhow!("{}: could not read the dictionary: {e}", model.id))?;
        let dict = read_dictionary(&raw)
            .map_err(|e| anyhow::anyhow!("{}: unusable dictionary: {e}", model.id))?;
        let mut session = session::build(onnx, intra_threads)?;
        let (Some(input), Some(output)) = (session.inputs.first(), session.outputs.first()) else {
            anyhow::bail!("{}: the model has no input or no output", model.id);
        };
        let (input, output) = (input.name.clone(), output.name.clone());
        let classes = output_classes(&mut session, &input, &output)?;
        check_classes(dict.len(), classes).map_err(|e| anyhow::anyhow!("{}: {e}", model.id))?;
        Ok(CtcRecognizer {
            model,
            session: Mutex::new(session),
            input,
            output,
            dict,
            first_use: crate::ocr::FirstUse::new(),
        })
    }

    /// The model this recogniser runs.
    pub fn model(&self) -> &'static LineRecModel {
        self.model
    }

    /// Characters in the dictionary (the model has two classes more).
    pub fn dictionary_len(&self) -> usize {
        self.dict.len()
    }

    /// Read line images, in order. One run for a balloon's lines (see
    /// [`REC_MAX_BATCH_COLUMNS`]).
    pub fn read(&self, lines: &[DynamicImage]) -> anyhow::Result<Vec<LineText>> {
        let started = std::time::Instant::now();
        let mut out = Vec::with_capacity(lines.len());
        for batch in batches(lines) {
            let x = preprocess(batch);
            let steps = self.run(&x, batch.len())?;
            out.extend(steps.iter().map(|s| ctc_text(s, &self.dict)));
        }
        if !lines.is_empty() {
            self.first_use.record("western", started.elapsed());
        }
        Ok(out)
    }

    /// One inference: per line, the best class and its probability at
    /// every step.
    fn run(&self, x: &Array4<f32>, lines: usize) -> anyhow::Result<Vec<Vec<(usize, f32)>>> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("line recogniser mutex poisoned"))?;
        let outputs = session.run(ort::inputs![
            self.input.as_str() => TensorRef::from_array_view(x.view())?
        ])?;
        let y = outputs[self.output.as_str()].try_extract_array::<f32>()?;
        let shape = y.shape();
        if shape.len() != 3 || shape[0] != lines {
            anyhow::bail!(
                "{}: unexpected output shape {shape:?} for {lines} lines",
                self.model.id
            );
        }
        check_classes(self.dict.len(), shape[2])
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.model.id))?;
        Ok(y.outer_iter()
            .map(|line| {
                line.outer_iter()
                    .map(|step| best_class(step.iter()))
                    .collect()
            })
            .collect())
    }
}

/// The model's class count: the output's last dimension when the model
/// declares it, else read from one run on an empty line.
fn output_classes(session: &mut Session, input: &str, output: &str) -> anyhow::Result<usize> {
    let declared = session
        .outputs
        .first()
        .and_then(|o| o.output_type.tensor_shape())
        .and_then(|shape| shape.last().copied());
    if let Some(classes) = declared
        && classes > 0
    {
        return Ok(classes as usize);
    }
    let x = Array4::<f32>::zeros((1, 3, REC_HEIGHT as usize, REC_MIN_WIDTH as usize));
    let outputs = session.run(ort::inputs![input => TensorRef::from_array_view(x.view())?])?;
    let y = outputs[output].try_extract_array::<f32>()?;
    match y.shape() {
        [_, _, classes] => Ok(*classes),
        other => anyhow::bail!("unexpected output shape {other:?}"),
    }
}

/// The dictionary must leave exactly the blank and the space unnamed.
pub(crate) fn check_classes(dictionary_len: usize, classes: usize) -> anyhow::Result<()> {
    if classes == dictionary_len + EXTRA_CLASSES {
        Ok(())
    } else {
        anyhow::bail!(
            "the model has {classes} classes but the dictionary has {dictionary_len} characters \
             (expected {} classes: blank, the dictionary, space)",
            dictionary_len + EXTRA_CLASSES
        )
    }
}

/// The largest value and its index (the first of equals; `(0, MIN)` for
/// an empty step).
fn best_class<'a>(step: impl Iterator<Item = &'a f32>) -> (usize, f32) {
    let (mut best, mut best_p) = (0usize, f32::MIN);
    for (class, &p) in step.enumerate() {
        if p > best_p {
            best_p = p;
            best = class;
        }
    }
    (best, best_p)
}

/// Greedy CTC decode of one line: drop the blank (class 0), collapse
/// repeats, map `1..=len` through the dictionary and the last class to a
/// space. Returns the trimmed text and the mean probability of the kept
/// steps (0 when nothing was kept), clamped to `[0, 1]`.
pub(crate) fn ctc_text(steps: &[(usize, f32)], dict: &[char]) -> LineText {
    let space = dict.len() + 1;
    let mut text = String::new();
    let (mut sum, mut kept) = (0.0f32, 0u32);
    let mut prev = usize::MAX;
    for &(class, p) in steps {
        if class != CTC_BLANK && class != prev {
            if class == space {
                text.push(' ');
            } else if let Some(&ch) = dict.get(class - 1) {
                text.push(ch);
            }
            sum += p;
            kept += 1;
        }
        prev = class;
    }
    let confidence = if kept == 0 { 0.0 } else { sum / kept as f32 };
    LineText {
        text: text.trim().to_owned(),
        confidence: if confidence.is_nan() {
            0.0
        } else {
            confidence.clamp(0.0, 1.0)
        },
    }
}

fn aspect(line: &DynamicImage) -> f64 {
    f64::from(line.width()) / f64::from(line.height().max(1))
}

/// Width of a batch holding `lines`: the widest line at height
/// [`REC_HEIGHT`], between [`REC_MIN_WIDTH`] and [`REC_MAX_WIDTH`].
pub(crate) fn batch_width(lines: &[DynamicImage]) -> u32 {
    let widest = lines
        .iter()
        .map(aspect)
        .fold(f64::from(REC_MIN_WIDTH) / f64::from(REC_HEIGHT), f64::max);
    ((f64::from(REC_HEIGHT) * widest) as u32).min(REC_MAX_WIDTH)
}

/// Split `lines` into runs of at most [`REC_MAX_BATCH_COLUMNS`] input
/// columns each (a run always takes at least one line). Order is kept.
fn batches(lines: &[DynamicImage]) -> Vec<&[DynamicImage]> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start + 1;
        while end < lines.len()
            && (end + 1 - start) * batch_width(&lines[start..=end]) as usize
                <= REC_MAX_BATCH_COLUMNS
        {
            end += 1;
        }
        out.push(&lines[start..end]);
        start = end;
    }
    out
}

/// A batch of lines as the model's input: `[n, 3, 48, batch_width]`,
/// each line resized to height 48 (bilinear) with its own width, B, G, R
/// planes, `(x/255 − 0.5) / 0.5`, zeros to the right of it.
pub(crate) fn preprocess(lines: &[DynamicImage]) -> Array4<f32> {
    let batch_w = batch_width(lines);
    let mut x = Array4::<f32>::zeros((lines.len(), 3, REC_HEIGHT as usize, batch_w as usize));
    for (n, line) in lines.iter().enumerate() {
        let w = ((f64::from(REC_HEIGHT) * aspect(line)).ceil() as u32).clamp(1, batch_w);
        let rgb = line
            .resize_exact(w, REC_HEIGHT, FilterType::Triangle)
            .to_rgb8();
        for (px, py, p) in rgb.enumerate_pixels() {
            let [r, g, b] = p.0;
            for (c, v) in [b, g, r].into_iter().enumerate() {
                x[[n, c, py as usize, px as usize]] = (f32::from(v) / 255.0 - 0.5) / 0.5;
            }
        }
    }
    x
}

// ─── dictionary ──────────────────────────────────────────────────────

/// Characters YAML gives a meaning at the start of an unquoted scalar
/// (or, for `~`, as the whole of one). An unquoted entry that is one of
/// them is refused: the file would mean something else to a YAML parser.
const YAML_INDICATORS: &str = "[]{},#&*!|>'\"%@`-?:~";

/// The character list of a PaddleOCR `inference.yml`: the
/// `character_dict` sequence in its top-level `PostProcess` mapping.
///
/// A reader for that one list, not a YAML parser, and a strict one: the
/// file is pinned by digest, so anything this does not recognise means
/// the pin moved to a file written differently, and the engine must not
/// start with a guessed dictionary. It accepts exactly:
///
/// - a file that ends with a line break and has no carriage returns;
/// - one line `PostProcess:` and, inside that block, one line
///   `  character_dict:`;
/// - then one entry per line, `  - X`, where `X` is a single character,
///   unquoted or in single quotes (`''''` is the quote itself), never a
///   control character, and never an unquoted character YAML would read
///   as something other than text (an indicator, a digit, whitespace);
/// - the list ending at the end of the file or at the next mapping key.
///
/// Everything else is an error. A file cut off at a line boundary still
/// parses; the class check at load ([`check_classes`]) refuses it.
pub(crate) fn read_dictionary(yml: &str) -> anyhow::Result<Vec<char>> {
    if yml.contains('\r') {
        anyhow::bail!("carriage returns in the file");
    }
    let Some(body) = yml.strip_suffix('\n') else {
        anyhow::bail!("the file does not end with a line break (cut off?)");
    };
    let lines: Vec<&str> = body.split('\n').collect();
    let is_top_level = |l: &str| l.starts_with(|c: char| c.is_ascii_alphabetic());
    let is_key = |l: &str| {
        let key = l.strip_prefix("  ").unwrap_or(l);
        key.starts_with(|c: char| c.is_ascii_alphabetic()) && key.contains(':')
    };

    let mut blocks = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == "PostProcess:");
    let (Some((post, _)), None) = (blocks.next(), blocks.next()) else {
        anyhow::bail!("expected exactly one top-level PostProcess block");
    };
    let block_end = lines[post + 1..]
        .iter()
        .position(|l| is_top_level(l))
        .map_or(lines.len(), |i| post + 1 + i);
    let mut lists = (post + 1..block_end).filter(|&i| lines[i].trim_end() == "  character_dict:");
    let (Some(list), None) = (lists.next(), lists.next()) else {
        anyhow::bail!("expected exactly one character_dict list in PostProcess");
    };
    if lines[list] != "  character_dict:" {
        anyhow::bail!("the character_dict line has trailing characters");
    }
    if lines
        .iter()
        .enumerate()
        .any(|(i, l)| i != list && l.trim() == "character_dict:")
    {
        anyhow::bail!("more than one character_dict in the file");
    }

    let mut dict = Vec::new();
    let mut next = list + 1;
    while let Some(item) = lines.get(next).and_then(|l| l.strip_prefix("  - ")) {
        dict.push(
            dictionary_entry(item)
                .map_err(|e| anyhow::anyhow!("entry {} ({item:?}): {e}", dict.len() + 1))?,
        );
        next += 1;
    }
    if let Some(after) = lines.get(next)
        && !is_key(after)
    {
        anyhow::bail!("unexpected line after {} entries: {after:?}", dict.len());
    }
    if dict.is_empty() {
        anyhow::bail!("the character_dict list is empty");
    }
    Ok(dict)
}

/// One list entry (the text after `  - `) as its character.
fn dictionary_entry(item: &str) -> anyhow::Result<char> {
    let (value, quoted) = if let Some(rest) = item.strip_prefix('\'') {
        let Some(inner) = rest.strip_suffix('\'') else {
            anyhow::bail!("unterminated quote");
        };
        // Inside single quotes the only escape is a doubled quote.
        let mut value = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\'' && chars.next() != Some('\'') {
                anyhow::bail!("a quote inside the quotes is not doubled");
            }
            value.push(c);
        }
        (value, true)
    } else {
        (item.to_owned(), false)
    };
    let mut chars = value.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        anyhow::bail!("not a single character");
    };
    if c.is_control() {
        anyhow::bail!("a control character");
    }
    if !quoted && (YAML_INDICATORS.contains(c) || c.is_ascii_digit() || c == ' ' || c == '\t') {
        anyhow::bail!("an unquoted character YAML does not read as text");
    }
    Ok(c)
}

// ─── the balloon recogniser ──────────────────────────────────────────

/// Line finder + line recogniser: reads a balloon crop.
pub struct LineOcr {
    finder: &'static LineFinder,
    rec: CtcRecognizer,
}

impl LineOcr {
    /// The server's shared instance, running
    /// [`models::WESTERN_REC`]. Loaded on first call.
    pub async fn shared() -> anyhow::Result<&'static Self> {
        static CELL: OnceCell<LineOcr> = OnceCell::const_new();
        CELL.get_or_try_init(|| Self::load(models::WESTERN_REC))
            .await
    }

    /// An instance running `model`, with the shared line finder (tools
    /// comparing recognisers; the server uses [`Self::shared`]).
    pub async fn load(model: &'static LineRecModel) -> anyhow::Result<Self> {
        let finder = LineFinder::shared().await?;
        let started = std::time::Instant::now();
        let rec = CtcRecognizer::load(model).await?;
        crate::ocr::record_model_load("western", started.elapsed());
        Ok(LineOcr { finder, rec })
    }

    /// The line recogniser behind this instance.
    pub fn recognizer(&self) -> &CtcRecognizer {
        &self.rec
    }
}

impl Recognizer for LineOcr {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition> {
        let found = self.finder.find(image)?;
        let (rects, images): (Vec<LineRect>, Vec<DynamicImage>) =
            lines::cut(image, &found).into_iter().unzip();
        let read = self.rec.read(&images)?;
        Ok(assemble(&rects, read))
    }
}

/// A balloon's recognition from its lines, in reading order: the
/// non-empty lines joined with line breaks, the mean confidence over
/// every line read (an unreadable line counts as 0), and one [`Word`]
/// entry per non-empty line carrying its text, confidence and box in
/// crop pixels.
pub(crate) fn assemble(rects: &[LineRect], read: Vec<LineText>) -> Recognition {
    let confidence = if read.is_empty() {
        0.0
    } else {
        read.iter().map(|l| l.confidence).sum::<f32>() / read.len() as f32
    };
    let lines: Vec<Word> = rects
        .iter()
        .zip(read)
        .filter(|(_, line)| !line.text.is_empty())
        .enumerate()
        .map(|(index, (r, line))| Word {
            text: line.text,
            confidence: line.confidence,
            xmin: r.x as f32,
            ymin: r.y as f32,
            xmax: (r.x + r.w) as f32,
            ymax: (r.y + r.h) as f32,
            line_index: index as u32,
        })
        .collect();
    let text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Recognition {
        text,
        confidence,
        words: (!lines.is_empty()).then_some(lines),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn flat(w: u32, h: u32, rgb: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb(rgb)))
    }

    // ── preprocessing ──

    #[test]
    fn a_batch_is_at_least_320_wide_and_as_wide_as_its_widest_line() {
        // Short lines: the minimum.
        assert_eq!(batch_width(&[flat(100, 40, [0; 3])]), 320);
        assert_eq!(batch_width(&[flat(10, 200, [0; 3])]), 320);
        // 600 x 30 is 20 times as wide as tall: 960 at height 48.
        assert_eq!(
            batch_width(&[flat(100, 40, [0; 3]), flat(600, 30, [0; 3])]),
            960
        );
        // A strip stops at the ceiling.
        assert_eq!(batch_width(&[flat(8000, 20, [0; 3])]), REC_MAX_WIDTH);
    }

    #[test]
    fn lines_are_height_48_bgr_scaled_to_plus_minus_one_and_padded_with_zeros() {
        // 96 x 96 → 48 wide; 384 x 48 → 384 wide; the batch is 384.
        let x = preprocess(&[flat(96, 96, [255, 0, 0]), flat(384, 48, [255, 255, 255])]);
        assert_eq!(x.shape(), &[2, 3, 48, 384]);
        // Line 0, inside its 48 columns: blue plane −1, green −1, red +1.
        for y in [0, 24, 47] {
            assert!((x[[0, 0, y, 20]] + 1.0).abs() < 1e-6);
            assert!((x[[0, 1, y, 20]] + 1.0).abs() < 1e-6);
            assert!((x[[0, 2, y, 20]] - 1.0).abs() < 1e-6);
            // To the right of the line: padding, exactly zero.
            for c in 0..3 {
                assert_eq!(x[[0, c, y, 48]], 0.0);
                assert_eq!(x[[0, c, y, 383]], 0.0);
            }
        }
        // Line 1 fills the batch: white is +1 everywhere.
        assert!(
            x.index_axis(ndarray::Axis(0), 1)
                .iter()
                .all(|v| (*v - 1.0).abs() < 1e-6)
        );
    }

    #[test]
    fn a_line_wider_than_the_ceiling_is_squeezed_into_the_batch() {
        let x = preprocess(&[flat(8000, 20, [0, 0, 0])]);
        assert_eq!(x.shape(), &[1, 3, 48, REC_MAX_WIDTH as usize]);
        // Black fills the whole width: no padding left.
        assert!(x.iter().all(|v| (*v + 1.0).abs() < 1e-6));
    }

    #[test]
    fn a_balloon_is_one_batch_and_a_huge_region_is_split_in_order() {
        let balloon: Vec<DynamicImage> = (0..20).map(|_| flat(400, 30, [0; 3])).collect();
        assert_eq!(batches(&balloon).len(), 1);
        assert!(batches(&[]).is_empty());
        // 40 strips at the ceiling: 30 fit one run.
        let strips: Vec<DynamicImage> = (0..40).map(|_| flat(4000, 20, [0; 3])).collect();
        let runs = batches(&strips);
        let sizes: Vec<usize> = runs.iter().map(|r| r.len()).collect();
        assert_eq!(sizes, [30, 10]);
        for r in &runs {
            assert!(r.len() * batch_width(r) as usize <= REC_MAX_BATCH_COLUMNS);
        }
    }

    // ── CTC decode ──

    const DICT: [char; 4] = ['A', 'B', 'C', '!'];
    /// Blank is 0, `A`..`!` are 1..=4, the space is the last class.
    const SPACE: usize = 5;

    fn steps(classes: &[usize]) -> Vec<(usize, f32)> {
        classes.iter().map(|&c| (c, 0.9)).collect()
    }

    #[test]
    fn blanks_are_dropped_and_repeats_collapse() {
        // A A _ A B B _ C → "AABC": the blank separates a real double.
        let line = ctc_text(&steps(&[1, 1, 0, 1, 2, 2, 0, 3]), &DICT);
        assert_eq!(line.text, "AABC");
        assert!((line.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn the_last_class_is_the_space() {
        let line = ctc_text(&steps(&[1, 2, SPACE, SPACE, 3, 4]), &DICT);
        assert_eq!(line.text, "AB C!");
        // A blank between two spaces keeps both; edges are trimmed.
        let line = ctc_text(&steps(&[SPACE, 1, SPACE, 0, SPACE, 2, SPACE]), &DICT);
        assert_eq!(line.text, "A  B");
    }

    #[test]
    fn confidence_is_the_mean_over_kept_steps_only() {
        let line = ctc_text(
            &[
                (0, 0.99),
                (1, 0.8),
                (1, 0.1),
                (0, 0.99),
                (SPACE, 0.6),
                (2, 1.0),
            ],
            &DICT,
        );
        assert_eq!(line.text, "A B");
        // 0.8 (first A; its repeat is not counted), 0.6 (space), 1.0 (B).
        assert!((line.confidence - 0.8).abs() < 1e-6, "{}", line.confidence);
    }

    #[test]
    fn nothing_but_blanks_is_empty_text_with_zero_confidence() {
        assert_eq!(
            ctc_text(&steps(&[0, 0, 0]), &DICT),
            LineText {
                text: String::new(),
                confidence: 0.0
            }
        );
        assert_eq!(ctc_text(&[], &DICT).confidence, 0.0);
        // Only spaces: trimmed to nothing, but they were read.
        assert_eq!(ctc_text(&steps(&[SPACE]), &DICT).text, "");
    }

    #[test]
    fn confidence_stays_in_the_unit_range() {
        assert_eq!(ctc_text(&[(1, 7.5)], &DICT).confidence, 1.0);
        assert_eq!(ctc_text(&[(1, -3.0)], &DICT).confidence, 0.0);
        assert_eq!(ctc_text(&[(1, f32::NAN)], &DICT).confidence, 0.0);
    }

    #[test]
    fn the_best_class_is_the_first_maximum() {
        assert_eq!(best_class([0.1f32, 0.7, 0.2].iter()), (1, 0.7));
        assert_eq!(best_class([0.5f32, 0.5].iter()), (0, 0.5));
    }

    #[test]
    fn the_dictionary_must_leave_exactly_blank_and_space() {
        assert!(check_classes(436, 438).is_ok());
        for wrong in [436, 437, 439, 0] {
            let err = check_classes(436, wrong).unwrap_err().to_string();
            assert!(err.contains("436 characters"), "{err}");
        }
    }

    // ── dictionary reader ──

    const YML: &str = "Global:\n  model_name: test_rec\nPreProcess:\n  transform_ops:\n  - DecodeImage:\n      img_mode: BGR\nPostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '0'\n  - A\n  - ''''\n  - '\"'\n  - \\\n  - $\n  - '-'\n  - é\n  - \u{3000}\n";

    #[test]
    fn reads_the_character_list() {
        assert_eq!(
            read_dictionary(YML).unwrap(),
            ['0', 'A', '\'', '"', '\\', '$', '-', 'é', '\u{3000}']
        );
    }

    #[test]
    fn the_list_may_be_followed_by_another_key() {
        let more = format!("{YML}  other_key: 1\nTail:\n  - not a dictionary entry\n");
        assert_eq!(read_dictionary(&more).unwrap().len(), 9);
        let at_top = format!("{YML}Tail:\n  x: 1\n");
        assert_eq!(read_dictionary(&at_top).unwrap().len(), 9);
    }

    #[test]
    fn a_file_cut_off_mid_line_is_refused() {
        for cut in [
            // In the last entry, in its indent, after its dash.
            &YML[..YML.len() - 1],
            "PostProcess:\n  character_dict:\n  - A\n  - B\n  ",
            "PostProcess:\n  character_dict:\n  - A\n  -",
            "PostProcess:\n  character_dict:\n  - A\n  - '",
        ] {
            let err = read_dictionary(cut).unwrap_err().to_string();
            assert!(err.contains("line break"), "{cut:?}: {err}");
        }
        // The same cuts with a line break added are still refused.
        for cut in [
            "PostProcess:\n  character_dict:\n  - A\n  \n",
            "PostProcess:\n  character_dict:\n  - A\n  -\n",
            "PostProcess:\n  character_dict:\n  - A\n  - \n",
            "PostProcess:\n  character_dict:\n  - A\n  - '\n",
            "PostProcess:\n  character_dict:\n  - A\n\n  - B\n",
        ] {
            assert!(read_dictionary(cut).is_err(), "{cut:?}");
        }
    }

    /// A file cut exactly between two entries reads as a shorter list;
    /// the class check then refuses it.
    #[test]
    fn a_file_cut_off_between_entries_fails_the_class_check() {
        let whole = read_dictionary(YML).unwrap();
        let cut_at = YML.rfind("  - é").unwrap();
        let short = read_dictionary(&YML[..cut_at]).unwrap();
        assert_eq!(short.len(), whole.len() - 2);
        let classes = whole.len() + EXTRA_CLASSES;
        assert!(check_classes(whole.len(), classes).is_ok());
        assert!(check_classes(short.len(), classes).is_err());
    }

    #[test]
    fn a_missing_or_repeated_list_is_refused() {
        for bad in [
            "",
            "\n",
            "PostProcess:\n  name: CTCLabelDecode\n",
            // The list is not in PostProcess.
            "Other:\n  character_dict:\n  - A\nPostProcess:\n  name: x\n",
            // Empty list.
            "PostProcess:\n  character_dict:\nTail: 1\n",
            // Two lists, two blocks.
            "PostProcess:\n  character_dict:\n  - A\n  character_dict:\n  - B\n",
            "PostProcess:\n  character_dict:\n  - A\nOther:\n    character_dict:\n",
            "PostProcess:\n  character_dict:\n  - A\nPostProcess:\n  character_dict:\n  - B\n",
            // A flow sequence or a value on the key's line.
            "PostProcess:\n  character_dict: [A, B]\n",
            "PostProcess:\n  character_dict: \n  - A\n",
            // Carriage returns.
            "PostProcess:\r\n  character_dict:\r\n  - A\r\n",
        ] {
            assert!(read_dictionary(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn entries_that_are_not_one_plain_character_are_refused() {
        for entry in [
            // More than one character, or none.
            "AB", "'AB'", "''", // Quoting this reader does not know.
            "\"A\"", "'A", "A'", "'''", "'a'b'",
            // Unquoted characters YAML reads as something else.
            "-", "#", "~", "7", "[", "*", "|", " ", "\t", // A trailing space, a comment.
            "A ", "A # x", // Control characters, quoted or not.
            "\u{7}", "'\u{0}'",
        ] {
            let yml = format!("PostProcess:\n  character_dict:\n  - A\n  - {entry}\n");
            assert!(read_dictionary(&yml).is_err(), "{entry:?}");
        }
        // Their quoted forms are characters like any other.
        for (entry, want) in [("'-'", '-'), ("'#'", '#'), ("'7'", '7'), ("' '", ' ')] {
            let yml = format!("PostProcess:\n  character_dict:\n  - {entry}\n");
            assert_eq!(read_dictionary(&yml).unwrap(), [want], "{entry:?}");
        }
    }

    #[test]
    fn a_line_that_is_neither_an_entry_nor_a_key_ends_in_an_error() {
        for tail in ["    - nested\n", "  -A\n", "- A\n", "   - A\n", "  ...\n"] {
            let yml = format!("PostProcess:\n  character_dict:\n  - A\n{tail}");
            assert!(read_dictionary(&yml).is_err(), "{tail:?}");
        }
    }

    // ── assembling a balloon ──

    fn rect(y: u32) -> LineRect {
        LineRect {
            x: 5,
            y,
            w: 100,
            h: 20,
        }
    }

    fn line(text: &str, confidence: f32) -> LineText {
        LineText {
            text: text.to_owned(),
            confidence,
        }
    }

    #[test]
    fn a_balloon_is_its_lines_joined_with_one_entry_per_line() {
        let rec = assemble(
            &[rect(0), rect(25), rect(50)],
            vec![
                line("SO THESE ARE", 0.9),
                line("THE SPACE-", 0.8),
                line("BUSTERS, EH?", 1.0),
            ],
        );
        assert_eq!(rec.text, "SO THESE ARE\nTHE SPACE-\nBUSTERS, EH?");
        assert!((rec.confidence - 0.9).abs() < 1e-6);
        let lines = rec.words.expect("line entries");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "THE SPACE-");
        assert_eq!(lines[1].line_index, 1);
        assert_eq!(
            (lines[1].xmin, lines[1].ymin, lines[1].xmax, lines[1].ymax),
            (5.0, 25.0, 105.0, 45.0)
        );
    }

    #[test]
    fn an_unread_line_lowers_the_confidence_and_has_no_entry() {
        let rec = assemble(
            &[rect(0), rect(25), rect(50)],
            vec![line("ONE", 0.9), line("", 0.0), line("TWO", 0.9)],
        );
        assert_eq!(rec.text, "ONE\nTWO");
        assert!((rec.confidence - 0.6).abs() < 1e-6);
        let lines = rec.words.expect("line entries");
        let index: Vec<u32> = lines.iter().map(|l| l.line_index).collect();
        assert_eq!(index, [0, 1], "kept lines are numbered consecutively");
        assert_eq!(lines[1].ymin, 50.0);
    }

    #[test]
    fn nothing_read_is_empty_text_without_entries() {
        let rec = assemble(&[], Vec::new());
        assert_eq!((rec.text.as_str(), rec.confidence), ("", 0.0));
        assert!(rec.words.is_none());
        let rec = assemble(&[rect(0)], vec![line("", 0.0)]);
        assert_eq!((rec.text.as_str(), rec.confidence), ("", 0.0));
        assert!(rec.words.is_none());
    }
}
