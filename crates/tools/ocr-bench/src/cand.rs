//! Candidate engines on ONNX Runtime: PP-OCR line recognisers, the
//! PP-OCRv5 DB text-line detector, ogkalu's RT-DETR comic text and bubble
//! detector, and comic-text-detector's own model run through a session we
//! build ourselves (so its thread count can be set).
//!
//! Everything runs on the `ort` (2.0.0-rc.10, ONNX Runtime 1.22) the
//! server already links. Model files are read from a local directory and
//! never fetched. Each session is built once and reused; the intra-op
//! thread count is a parameter (0 = ONNX Runtime's default).

use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, GrayImage, Luma, imageops::FilterType};
use ndarray::{Array, Array4, ArrayViewD, IxDyn};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use serde::Serialize;

use crate::engines::{DetBox, PageDetector};

/// Input and output names, types and shapes, as the session reports them.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ModelMeta {
    pub path: String,
    pub bytes: u64,
    pub intra_threads: usize,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub load_ms: f64,
}

pub(crate) fn load_session(
    path: &Path,
    intra_threads: usize,
) -> anyhow::Result<(Session, ModelMeta)> {
    let t = std::time::Instant::now();
    // The server's environment (one per process) and its session options:
    // every graph optimization (`All`, which was `Level3` before `ort`
    // rc.15), intra-op spinning off, `intra_threads` (0 = ONNX Runtime's
    // default count, for the `timefree` step).
    let env = server::ocr::session::environment()?;
    let mut b = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::All)
        .and_then(|b| b.with_intra_op_spinning(false))
        .map_err(ort::Error::<()>::from)?;
    if intra_threads > 0 {
        b = b
            .with_intra_threads(intra_threads)
            .map_err(ort::Error::<()>::from)?;
    }
    // ONNX Runtime's per-op profile (a Chrome trace JSON per session),
    // for finding the op a slow model spends its time in.
    if let Some(dir) = std::env::var_os("OCR_BENCH_ORT_PROFILE") {
        let stem = path
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let file = path.file_stem().unwrap_or_default().to_string_lossy();
        b = b
            .with_profiling(Path::new(&dir).join(format!("{stem}-{file}")))
            .map_err(ort::Error::<()>::from)?;
    }
    let s = b
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let load_ms = crate::stats::ms_since(t);
    let meta = ModelMeta {
        path: path.display().to_string(),
        bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        intra_threads,
        inputs: s
            .inputs()
            .iter()
            .map(|i| format!("{}: {:?}", i.name(), i.dtype()))
            .collect(),
        outputs: s
            .outputs()
            .iter()
            .map(|o| format!("{}: {:?}", o.name(), o.dtype()))
            .collect(),
        load_ms,
    };
    Ok((s, meta))
}

fn first_output_f32(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> anyhow::Result<Array<f32, IxDyn>> {
    let v: ArrayViewD<'_, f32> = outputs[name].try_extract_array::<f32>()?;
    Ok(v.to_owned())
}

// ─── PP-OCR recognition ──────────────────────────────────────────────

/// The character list from `inference.yml` (`PostProcess.character_dict`).
/// A tiny reader for that one block: YAML scalars in single quotes
/// (`''` = `'`), double quotes (JSON-style escapes) or plain.
pub(crate) fn read_dict(yml: &Path) -> anyhow::Result<Vec<String>> {
    let raw = std::fs::read_to_string(yml)?;
    let mut out = Vec::new();
    let mut inside = false;
    for line in raw.lines() {
        if !inside {
            if line.trim_end() == "  character_dict:" {
                inside = true;
            }
            continue;
        }
        let Some(item) = line.strip_prefix("  - ") else {
            break;
        };
        let v = if let Some(s) = item.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            s.replace("''", "'")
        } else if item.starts_with('"') {
            serde_json::from_str::<String>(item)
                .map_err(|e| anyhow::anyhow!("dictionary entry {item:?}: {e}"))?
        } else {
            item.to_owned()
        };
        out.push(v);
    }
    if out.is_empty() {
        anyhow::bail!("no character_dict in {}", yml.display());
    }
    Ok(out)
}

/// A PP-OCR CTC line recogniser (v5 / v6 exports).
pub(crate) struct PaddleRec {
    pub name: String,
    session: Session,
    input: String,
    output: String,
    dict: Vec<String>,
    pub meta: ModelMeta,
    /// Set from the first output: whether the class count includes the
    /// trailing space class.
    space_class: Option<bool>,
    pub dict_len: usize,
    pub classes_seen: Option<usize>,
}

pub(crate) const REC_H: u32 = 48;
/// Minimum batch width, from the yml's 3x48x320.
const REC_MIN_W: u32 = 320;

impl PaddleRec {
    pub(crate) fn load(name: &str, dir: &Path, threads: usize) -> anyhow::Result<Self> {
        let dict = read_dict(&dir.join("inference.yml"))?;
        let (session, meta) = load_session(&dir.join("inference.onnx"), threads)?;
        let input = session.inputs()[0].name().to_owned();
        let output = session.outputs()[0].name().to_owned();
        Ok(PaddleRec {
            name: name.into(),
            session,
            input,
            output,
            dict_len: dict.len(),
            dict,
            meta,
            space_class: None,
            classes_seen: None,
        })
    }

    /// Preprocess a batch: BGR, resize to height 48 keeping aspect, width
    /// capped at the batch width (48 x max(320/48, widest ratio)),
    /// normalise (x/255 - 0.5)/0.5, right-pad with zeros. This is
    /// PaddleOCR's `resize_norm_img` for the batched predictor.
    fn prep(lines: &[DynamicImage]) -> Array4<f32> {
        let max_ratio = lines
            .iter()
            .map(|l| f64::from(l.width()) / f64::from(l.height().max(1)))
            .fold(f64::from(REC_MIN_W) / f64::from(REC_H), f64::max);
        let batch_w = (f64::from(REC_H) * max_ratio) as u32;
        let mut arr = Array4::<f32>::zeros((lines.len(), 3, REC_H as usize, batch_w as usize));
        for (n, l) in lines.iter().enumerate() {
            let ratio = f64::from(l.width()) / f64::from(l.height().max(1));
            let w = ((f64::from(REC_H) * ratio).ceil() as u32).clamp(1, batch_w);
            let rgb = l.resize_exact(w, REC_H, FilterType::Triangle).to_rgb8();
            for (x, y, p) in rgb.enumerate_pixels() {
                let [r, g, b] = p.0;
                for (c, v) in [b, g, r].into_iter().enumerate() {
                    arr[[n, c, y as usize, x as usize]] = (f32::from(v) / 255.0 - 0.5) / 0.5;
                }
            }
        }
        arr
    }

    /// Recognise line images in one batch. Returns (text, confidence) per
    /// line: greedy CTC (argmax, collapse repeats, drop blank 0), the
    /// confidence is the mean of the kept maxima.
    pub(crate) fn run(&mut self, lines: &[DynamicImage]) -> anyhow::Result<Vec<(String, f32)>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let x = Self::prep(lines);
        let outputs = self
            .session
            .run(ort::inputs![self.input.as_str() => TensorRef::from_array_view(x.view())?])?;
        let y = first_output_f32(&outputs, &self.output)?;
        drop(outputs);
        let shape = y.shape().to_vec();
        if shape.len() != 3 || shape[0] != lines.len() {
            anyhow::bail!("{}: unexpected output shape {shape:?}", self.name);
        }
        let classes = shape[2];
        let space = match self.space_class {
            Some(s) => s,
            None => {
                let s = if classes == self.dict.len() + 2 {
                    true
                } else if classes == self.dict.len() + 1 {
                    false
                } else {
                    anyhow::bail!(
                        "{}: {classes} output classes for a {}-entry dictionary",
                        self.name,
                        self.dict.len()
                    );
                };
                self.space_class = Some(s);
                self.classes_seen = Some(classes);
                s
            }
        };
        let mut out = Vec::with_capacity(lines.len());
        for n in 0..lines.len() {
            let mut text = String::new();
            let mut confs = Vec::new();
            let mut prev = usize::MAX;
            for t in 0..shape[1] {
                let (mut best, mut best_v) = (0usize, f32::MIN);
                for c in 0..classes {
                    let v = y[[n, t, c]];
                    if v > best_v {
                        best_v = v;
                        best = c;
                    }
                }
                if best != 0 && best != prev {
                    if space && best == classes - 1 {
                        text.push(' ');
                    } else if let Some(ch) = self.dict.get(best - 1) {
                        text.push_str(ch);
                    }
                    confs.push(best_v);
                }
                prev = best;
            }
            let conf = if confs.is_empty() {
                0.0
            } else {
                confs.iter().sum::<f32>() / confs.len() as f32
            };
            out.push((text.trim().to_owned(), conf));
        }
        Ok(out)
    }
}

// ─── PP-OCRv5 DB text-line detection ─────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub(crate) enum DetResize {
    /// Long side to this many px (rounded to a multiple of 32).
    Long(u32),
    /// Keep the size (rounded to 32, short side at least 64): for crops.
    Keep,
    /// Scale by this factor (rounded to 32, long side capped at 1600).
    Scale(f64),
}

pub(crate) struct PaddleDet {
    session: Session,
    input: String,
    output: String,
    pub meta: ModelMeta,
    /// From the yml: thresh, box_thresh, unclip_ratio.
    pub thresh: f32,
    pub box_thresh: f32,
    pub unclip: f32,
}

/// One text line in source pixels, with the DB box score.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct LineBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub score: f32,
}

fn yml_number(raw: &str, key: &str) -> Option<f32> {
    raw.lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix(&format!("{key}:"))
                .map(|v| v.trim().to_owned())
        })
        .and_then(|v| v.parse().ok())
}

impl PaddleDet {
    pub(crate) fn load(dir: &Path, threads: usize) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(dir.join("inference.yml"))?;
        let (session, meta) = load_session(&dir.join("inference.onnx"), threads)?;
        Ok(PaddleDet {
            input: session.inputs()[0].name().to_owned(),
            output: session.outputs()[0].name().to_owned(),
            session,
            meta,
            thresh: yml_number(&raw, "thresh").unwrap_or(0.3),
            box_thresh: yml_number(&raw, "box_thresh").unwrap_or(0.6),
            unclip: yml_number(&raw, "unclip_ratio").unwrap_or(1.5),
        })
    }

    /// DB detection. Preprocess as the yml says (BGR, ImageNet mean/std
    /// applied in that channel order, CHW). Postprocess: threshold the
    /// probability map, 8-connected components, axis-aligned box per
    /// component, box score = mean probability inside the box, drop below
    /// `box_thresh` or under 3 px, unclip each side by
    /// area * ratio / perimeter (the closed form of the polygon offset
    /// for a rectangle). Rotated boxes are not produced (simplification).
    pub(crate) fn detect_lines(
        &mut self,
        img: &DynamicImage,
        resize: DetResize,
    ) -> anyhow::Result<Vec<LineBox>> {
        let (w, h) = img.dimensions();
        let round32 = |v: f64| ((v / 32.0).round() as u32).max(1) * 32;
        let (nw, nh) = match resize {
            DetResize::Long(l) => {
                let s = f64::from(l) / f64::from(w.max(h));
                (round32(f64::from(w) * s), round32(f64::from(h) * s))
            }
            DetResize::Keep => {
                let s = (64.0 / f64::from(w.min(h).max(1))).max(1.0);
                (round32(f64::from(w) * s), round32(f64::from(h) * s))
            }
            DetResize::Scale(f) => {
                let s = f.min(1600.0 / f64::from(w.max(h).max(1)));
                (round32(f64::from(w) * s), round32(f64::from(h) * s))
            }
        };
        let rgb = img.resize_exact(nw, nh, FilterType::Triangle).to_rgb8();
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let mut x = Array4::<f32>::zeros((1, 3, nh as usize, nw as usize));
        for (px, py, p) in rgb.enumerate_pixels() {
            let [r, g, b] = p.0;
            for (c, v) in [b, g, r].into_iter().enumerate() {
                x[[0, c, py as usize, px as usize]] = (f32::from(v) / 255.0 - mean[c]) / std[c];
            }
        }
        let outputs = self
            .session
            .run(ort::inputs![self.input.as_str() => TensorRef::from_array_view(x.view())?])?;
        let y = first_output_f32(&outputs, &self.output)?;
        drop(outputs);
        let shape = y.shape().to_vec();
        if shape.len() != 4 {
            anyhow::bail!("PP det: unexpected output shape {shape:?}");
        }
        let (oh, ow) = (shape[2], shape[3]);
        let mut bin = GrayImage::new(ow as u32, oh as u32);
        for yy in 0..oh {
            for xx in 0..ow {
                if y[[0, 0, yy, xx]] > self.thresh {
                    bin.put_pixel(xx as u32, yy as u32, Luma([255]));
                }
            }
        }
        let labels = imageproc::region_labelling::connected_components(
            &bin,
            imageproc::region_labelling::Connectivity::Eight,
            Luma([0u8]),
        );
        let mut bounds: std::collections::HashMap<u32, [usize; 4]> =
            std::collections::HashMap::new();
        for (xx, yy, l) in labels.enumerate_pixels() {
            if l.0[0] == 0 {
                continue;
            }
            let e = bounds.entry(l.0[0]).or_insert([
                xx as usize,
                yy as usize,
                xx as usize,
                yy as usize,
            ]);
            e[0] = e[0].min(xx as usize);
            e[1] = e[1].min(yy as usize);
            e[2] = e[2].max(xx as usize);
            e[3] = e[3].max(yy as usize);
        }
        let (sx, sy) = (f64::from(w) / ow as f64, f64::from(h) / oh as f64);
        let mut out = Vec::new();
        for b in bounds.values().take(1000) {
            let (bw, bh) = ((b[2] - b[0] + 1) as f32, (b[3] - b[1] + 1) as f32);
            if bw.min(bh) < 3.0 {
                continue;
            }
            let mut sum = 0.0f32;
            for yy in b[1]..=b[3] {
                for xx in b[0]..=b[2] {
                    sum += y[[0, 0, yy, xx]];
                }
            }
            let score = sum / (bw * bh);
            if score < self.box_thresh {
                continue;
            }
            let d = bw * bh * self.unclip / (2.0 * (bw + bh));
            let (x0, y0) = (b[0] as f32 - d, b[1] as f32 - d);
            let (x1, y1) = (b[2] as f32 + 1.0 + d, b[3] as f32 + 1.0 + d);
            if (x1 - x0).min(y1 - y0) < 5.0 {
                continue;
            }
            out.push(LineBox {
                x0: (f64::from(x0) * sx).clamp(0.0, f64::from(w)) as f32,
                y0: (f64::from(y0) * sy).clamp(0.0, f64::from(h)) as f32,
                x1: (f64::from(x1) * sx).clamp(0.0, f64::from(w)) as f32,
                y1: (f64::from(y1) * sy).clamp(0.0, f64::from(h)) as f32,
                score,
            });
        }
        Ok(out)
    }
}

/// Reading order for line boxes: rows top to bottom (two boxes share a
/// row when their vertical centres are within half the shorter height),
/// then left to right.
pub(crate) fn reading_order(boxes: &mut Vec<[f32; 4]>) {
    boxes.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut rows: Vec<Vec<[f32; 4]>> = Vec::new();
    for b in boxes.drain(..) {
        let cy = (b[1] + b[3]) / 2.0;
        let h = b[3] - b[1];
        if let Some(row) = rows.last_mut() {
            let r = row[0];
            let rcy = (r[1] + r[3]) / 2.0;
            if (cy - rcy).abs() < 0.5 * h.min(r[3] - r[1]) {
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

/// PP-OCR det as a page detector: each text line is a box (class 0).
pub(crate) struct PaddleDetPage {
    pub det: PaddleDet,
    pub long: u32,
}

impl PageDetector for PaddleDetPage {
    fn name(&self) -> String {
        format!("PP-OCRv5 mobile det, long side {}", self.long)
    }
    fn detect(&mut self, page: &DynamicImage) -> anyhow::Result<Vec<DetBox>> {
        Ok(self
            .det
            .detect_lines(page, DetResize::Long(self.long))?
            .into_iter()
            .map(|b| DetBox {
                x0: b.x0,
                y0: b.y0,
                x1: b.x1,
                y1: b.y1,
                confidence: b.score,
                class: 0,
            })
            .collect())
    }
}

// ─── ogkalu comic text and bubble detector (RT-DETR-v2) ──────────────

pub(crate) struct Ogkalu {
    pub name: String,
    session: Session,
    pub meta: ModelMeta,
    pub threshold: f32,
    /// `true` for the transformers export (pixel_values -> logits,
    /// pred_boxes); `false` for the original RT-DETR export (images +
    /// orig_target_sizes -> labels, boxes, scores).
    hf: bool,
}

pub(crate) const OGK_SIZE: u32 = 640;

impl Ogkalu {
    pub(crate) fn load(
        name: &str,
        path: &Path,
        threads: usize,
        threshold: f32,
    ) -> anyhow::Result<Self> {
        let (session, meta) = load_session(path, threads)?;
        let names: Vec<&str> = session
            .inputs()
            .iter()
            .map(ort::value::Outlet::name)
            .collect();
        let hf = if names.contains(&"pixel_values") {
            true
        } else if names.contains(&"images") {
            false
        } else {
            anyhow::bail!("{name}: unrecognised inputs {names:?}");
        };
        Ok(Ogkalu {
            name: name.into(),
            session,
            meta,
            threshold,
            hf,
        })
    }

    /// Resize to 640x640 (squash, bilinear), RGB / 255, no normalisation
    /// (`preprocessor_config.json`).
    fn prep(page: &DynamicImage) -> Array4<f32> {
        let rgb = page
            .resize_exact(OGK_SIZE, OGK_SIZE, FilterType::Triangle)
            .to_rgb8();
        let mut x = Array4::<f32>::zeros((1, 3, OGK_SIZE as usize, OGK_SIZE as usize));
        for (px, py, p) in rgb.enumerate_pixels() {
            for c in 0..3 {
                x[[0, c, py as usize, px as usize]] = f32::from(p.0[c]) / 255.0;
            }
        }
        x
    }
}

impl PageDetector for Ogkalu {
    fn name(&self) -> String {
        format!("ogkalu {} (threshold {})", self.name, self.threshold)
    }

    /// Classes: 0 bubble, 1 text_bubble, 2 text_free.
    fn detect(&mut self, page: &DynamicImage) -> anyhow::Result<Vec<DetBox>> {
        let (w, h) = page.dimensions();
        let x = Self::prep(page);
        let mut out = Vec::new();
        if self.hf {
            let outputs = self
                .session
                .run(ort::inputs!["pixel_values" => TensorRef::from_array_view(x.view())?])?;
            let logits = first_output_f32(&outputs, "logits")?;
            let boxes = first_output_f32(&outputs, "pred_boxes")?;
            drop(outputs);
            let q = logits.shape()[1];
            let nc = logits.shape()[2];
            for i in 0..q {
                for c in 0..nc {
                    let s = 1.0 / (1.0 + (-logits[[0, i, c]]).exp());
                    if s < self.threshold {
                        continue;
                    }
                    let (cx, cy, bw, bh) = (
                        boxes[[0, i, 0]],
                        boxes[[0, i, 1]],
                        boxes[[0, i, 2]],
                        boxes[[0, i, 3]],
                    );
                    out.push(DetBox {
                        x0: (cx - bw / 2.0) * w as f32,
                        y0: (cy - bh / 2.0) * h as f32,
                        x1: (cx + bw / 2.0) * w as f32,
                        y1: (cy + bh / 2.0) * h as f32,
                        confidence: s,
                        class: c as u32,
                    });
                }
            }
        } else {
            let sizes = ndarray::arr2(&[[i64::from(w), i64::from(h)]]);
            let outputs = self.session.run(ort::inputs![
                "images" => TensorRef::from_array_view(x.view())?,
                "orig_target_sizes" => TensorRef::from_array_view(sizes.view())?,
            ])?;
            let labels: Array<i64, IxDyn> =
                outputs["labels"].try_extract_array::<i64>()?.to_owned();
            let boxes = first_output_f32(&outputs, "boxes")?;
            let scores = first_output_f32(&outputs, "scores")?;
            drop(outputs);
            for i in 0..scores.shape()[1] {
                let s = scores[[0, i]];
                if s < self.threshold {
                    continue;
                }
                out.push(DetBox {
                    x0: boxes[[0, i, 0]],
                    y0: boxes[[0, i, 1]],
                    x1: boxes[[0, i, 2]],
                    y1: boxes[[0, i, 3]],
                    confidence: s,
                    class: labels[[0, i]] as u32,
                });
            }
        }
        for b in &mut out {
            b.x0 = b.x0.clamp(0.0, w as f32);
            b.x1 = b.x1.clamp(0.0, w as f32);
            b.y0 = b.y0.clamp(0.0, h as f32);
            b.y1 = b.y1.clamp(0.0, h as f32);
        }
        Ok(out)
    }
}

// ─── comic-text-detector through our own session ─────────────────────
//
// Licence: the `CtdOwn` code below (to the end of its `PageDetector`
// impl) is derived from the `comic-text-detector` crate 0.5.1 (koharu,
// <https://github.com/mayocream/koharu>, by Mayo Takanashi), whose
// licence is the GNU General Public License version 3. That code is
// licensed under the GNU General Public License version 3, as the crate
// is, and is combined with the rest of Folio (AGPL-3.0-or-later) as
// section 13 of GPLv3 and of AGPLv3 permit; see `LICENSE-THIRD-PARTY.md`.
// Modified by the Folio project, November 2026: run on a session the
// tool builds, without the full-page mask.

/// comic-text-detector's model with the crate's own pre/postprocessing
/// (1024x1024 squash, CatmullRom, RGB/255; `blk` rows above confidence
/// 0.5, class by comparing columns 5 and 6, per-class NMS at IoU 0.5),
/// minus the full-page mask the pipeline throws away, and with a
/// settable intra-op thread count.
pub(crate) struct CtdOwn {
    session: Session,
    pub meta: ModelMeta,
}

impl CtdOwn {
    pub(crate) fn load(path: &Path, threads: usize) -> anyhow::Result<Self> {
        let (session, meta) = load_session(path, threads)?;
        Ok(CtdOwn { session, meta })
    }
}

fn iou(a: &DetBox, b: &DetBox) -> f32 {
    let w = a.x1.min(b.x1) - a.x0.max(b.x0);
    let h = a.y1.min(b.y1) - a.y0.max(b.y0);
    if w <= 0.0 || h <= 0.0 {
        return 0.0;
    }
    let i = w * h;
    let u = (a.x1 - a.x0) * (a.y1 - a.y0) + (b.x1 - b.x0) * (b.y1 - b.y0) - i;
    if u <= 0.0 { 0.0 } else { i / u }
}

impl PageDetector for CtdOwn {
    fn name(&self) -> String {
        format!(
            "comic-text-detector model, own session (intra threads {}), no mask",
            self.meta.intra_threads
        )
    }

    fn detect(&mut self, page: &DynamicImage) -> anyhow::Result<Vec<DetBox>> {
        let (w, h) = page.dimensions();
        let (wr, hr) = (w as f32 / 1024.0, h as f32 / 1024.0);
        let img = page.resize_exact(1024, 1024, FilterType::CatmullRom);
        let mut x = Array4::<f32>::zeros((1, 3, 1024, 1024));
        for (px, py, p) in img.pixels() {
            let [r, g, b, _] = p.0;
            x[[0, 0, py as usize, px as usize]] = f32::from(r) / 255.0;
            x[[0, 1, py as usize, px as usize]] = f32::from(g) / 255.0;
            x[[0, 2, py as usize, px as usize]] = f32::from(b) / 255.0;
        }
        let outputs = self
            .session
            .run(ort::inputs!["images" => TensorRef::from_array_view(x.view())?])?;
        let blk = first_output_f32(&outputs, "blk")?;
        drop(outputs);
        let mut per_class: [Vec<DetBox>; 2] = [Vec::new(), Vec::new()];
        for i in 0..blk.shape()[1] {
            let conf = blk[[0, i, 4]];
            if conf < 0.5 {
                continue;
            }
            let class = usize::from(blk[[0, i, 5]] < blk[[0, i, 6]]);
            let (cx, cy, bw, bh) = (
                blk[[0, i, 0]] * wr,
                blk[[0, i, 1]] * hr,
                blk[[0, i, 2]] * wr,
                blk[[0, i, 3]] * hr,
            );
            per_class[class].push(DetBox {
                x0: cx - bw / 2.0,
                y0: cy - bh / 2.0,
                x1: cx + bw / 2.0,
                y1: cy + bh / 2.0,
                confidence: conf,
                class: class as u32,
            });
        }
        let mut out = Vec::new();
        for mut boxes in per_class {
            boxes.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
            let mut kept: Vec<DetBox> = Vec::new();
            for b in boxes {
                if kept.iter().all(|k| iou(k, &b) <= 0.5) {
                    kept.push(b);
                }
            }
            out.extend(kept);
        }
        Ok(out)
    }
}

/// Where the candidate models live under the models directory.
pub(crate) struct ModelPaths {
    pub root: PathBuf,
}

impl ModelPaths {
    pub(crate) fn rec_dir(&self, name: &str) -> PathBuf {
        let d = match name {
            "pp-v5-en" => "PaddlePaddle__en_PP-OCRv5_mobile_rec_onnx",
            "pp-v5-latin" => "PaddlePaddle__latin_PP-OCRv5_mobile_rec_onnx",
            "pp-v6-tiny" => "PaddlePaddle__PP-OCRv6_tiny_rec_onnx",
            "pp-v6-small" => "PaddlePaddle__PP-OCRv6_small_rec_onnx",
            "pp-v6-medium" => "PaddlePaddle__PP-OCRv6_medium_rec_onnx",
            other => other,
        };
        self.root.join(d)
    }
    pub(crate) fn det_dir(&self) -> PathBuf {
        self.root.join("PaddlePaddle__PP-OCRv5_mobile_det_onnx")
    }
    pub(crate) fn ogkalu(&self, size: &str) -> PathBuf {
        let f = if size == "small" {
            "detector-v4-s_int8.onnx"
        } else {
            "detector_int8.onnx"
        };
        self.root
            .join("ogkalu__comic-text-and-bubble-detector")
            .join(f)
    }
}

pub(crate) const PP_RECS: [&str; 5] = [
    "pp-v5-en",
    "pp-v5-latin",
    "pp-v6-tiny",
    "pp-v6-small",
    "pp-v6-medium",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_scalars() {
        let dir = std::env::temp_dir().join(format!("ocr-bench-dict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("inference.yml");
        std::fs::write(
            &p,
            "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '0'\n  - A\n  - ''''\n  - \"\\\\\"\n  - \\\n  - ' '\nOther: 1\n",
        )
        .unwrap();
        let d = read_dict(&p).unwrap();
        assert_eq!(d, vec!["0", "A", "'", "\\", "\\", " "]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reading_order_rows_then_columns() {
        let mut b = vec![
            [100.0, 52.0, 200.0, 80.0],
            [0.0, 0.0, 90.0, 30.0],
            [0.0, 50.0, 90.0, 80.0],
            [100.0, 2.0, 200.0, 28.0],
        ];
        reading_order(&mut b);
        assert_eq!(b[0][0], 0.0);
        assert_eq!(b[1][0], 100.0);
        assert_eq!(b[2], [0.0, 50.0, 90.0, 80.0]);
    }
}
