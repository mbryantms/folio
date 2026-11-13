//! `ocr-bench`: timings and accuracy for OCR engines on comic pages.
//!
//! Without a candidate flag the tool runs the previous detector,
//! `comic-text-detector`'s model in a session of its own: page decode
//! (the server's `decode_limited`) and detection timings, detection
//! counts per page and contact sheets. It never starts the HTTP server,
//! Postgres or Redis, and never downloads: the detector is loaded only
//! when its model is already cached with the expected digest.
//!
//! ```text
//! ocr-bench --pages DIR [--truth DIR] --out DIR --label NAME
//!           [--timing] [--sheets] [--detector-only]
//! ```
//!
//! With neither section flag, both run. Writes `<out>/<label>.json` and
//! `<out>/<label>.md`, and contact sheets under `<out>/<label>-sheets/`.
//! Run the release build, and run it pinned (`taskset`) for the
//! CPU-limited configuration; the tool records the CPUs it was allowed,
//! `OMP_NUM_THREADS` and the load average.
//!
//! `--cand`, `--cand-timing` and `--throughput` (with `--models DIR`)
//! compare candidate engines: see `stage2.rs` and the crate's
//! `README.md`, which also covers the data, `scripts/run.sh` and what
//! the figures leave out.

mod cand;
mod draw;
mod engines;
mod report;
mod score;
mod split;
mod stage2;
mod stats;
mod truth;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use image::DynamicImage;
use serde::Serialize;
use server::util::image_decode::decode_limited;

use crate::cand::CtdOwn;
use crate::engines::{DetBox, PageDetector};
use crate::stats::{Summary, median, summarize, timed};
use crate::truth::{PageFile, RefRegion};

#[derive(Parser, Debug, Clone, Serialize)]
#[command(about = "Benchmark Folio's OCR pipeline (timings + accuracy)")]
struct Args {
    /// Directory of `<book>/pNNN.jpg` page images.
    #[arg(long)]
    pages: PathBuf,
    /// Directory of `<book>-pNNN.json` reference transcriptions.
    #[arg(long)]
    truth: Option<PathBuf>,
    /// Output directory.
    #[arg(long)]
    out: PathBuf,
    /// Name for this run's output files.
    #[arg(long, default_value = "run")]
    label: String,
    /// Timing section.
    #[arg(long)]
    timing: bool,
    /// Contact sheets.
    #[arg(long)]
    sheets: bool,
    /// Timing of decode + detection only (for the thread experiments).
    #[arg(long)]
    detector_only: bool,
    /// Warm repetitions of page decode and detection, per page.
    #[arg(long, default_value_t = 5)]
    warm: usize,
    /// Warm repetitions of each balloon's recognition (--cand-timing).
    #[arg(long, default_value_t = 2)]
    rec_warm: usize,
    /// Only pages whose key contains this.
    #[arg(long)]
    only: Option<String>,
    /// At most this many pages (after --only).
    #[arg(long)]
    max_pages: Option<usize>,
    /// Margin (fraction of the reference box) added around each
    /// reference box for the candidate recognition crops.
    #[arg(long, default_value_t = 0.04)]
    margin: f64,

    // ─── candidate engines (stage 2) ───
    /// Directory holding the candidate ONNX models (never downloaded).
    #[arg(long)]
    models: Option<PathBuf>,
    /// Print each candidate model's inputs/outputs and exit.
    #[arg(long)]
    probe: bool,
    /// Candidate accuracy: recognition alone (A), detection (B), tap (C).
    #[arg(long)]
    cand: bool,
    /// Candidate timings (D, E).
    #[arg(long)]
    cand_timing: bool,
    /// ONNX Runtime intra-op threads for the candidate sessions (0 = default).
    #[arg(long, default_value_t = 4)]
    threads: usize,
    /// Recognisers, comma separated.
    #[arg(
        long,
        default_value = "pp-v5-en,pp-v5-latin,pp-v6-tiny,pp-v6-small,pp-v6-medium"
    )]
    recs: String,
    /// Detectors, comma separated (ctd, ogk-s, ogk-m, ppdetN).
    #[arg(long, default_value = "ctd,ogk-s,ogk-m,ppdet960,ppdet1600")]
    dets: String,
    /// Line splits for the PP recognisers.
    #[arg(
        long,
        default_value = "ppdet,ppdet2x,page-lines,profile,ctd-lines,none"
    )]
    splits: String,
    /// PP recogniser used in the tap combinations and timings.
    #[arg(long, default_value = "pp-v6-small")]
    best_pp: String,
    /// Line split used with it in the timing and throughput runs.
    #[arg(long, default_value = "ppdet")]
    best_split: String,
    /// Line splits tried in the tap simulation (C).
    #[arg(long, default_value = "ppdet,ppdet2x,page-lines")]
    tap_splits: String,
    /// Whole-page throughput: every page once per repeat (decode, detect,
    /// recognise every unit), one worker; run several processes for
    /// parallel workers.
    #[arg(long)]
    throughput: bool,
    /// Passes over the page list in --throughput mode.
    #[arg(long, default_value_t = 1)]
    repeat: usize,
    /// Score threshold for ogkalu's detector.
    #[arg(long, default_value_t = 0.5)]
    ogk_threshold: f32,
}

/// The reader's nested-box rule (`pageTextBoxes` in
/// `web/lib/reader/text-regions.ts`): a box lying this much inside a
/// larger one is dropped, for the page-text panel and for taps alike.
const NESTED_OVERLAP: f64 = 0.8;

#[derive(Debug, Clone, Serialize)]
struct Env {
    profile: &'static str,
    cpu_model: String,
    cpus_allowed: String,
    available_parallelism: usize,
    omp_num_threads: Option<String>,
    loadavg_start: String,
    loadavg_end: String,
    threads_after_detect: Option<u64>,
}

fn read_status_field(field: &str) -> Option<String> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix(field).map(|v| v.trim().to_owned()))
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            l.strip_prefix("model name")
                .map(|v| v.trim_start_matches([' ', '\t', ':']).to_owned())
        })
        .unwrap_or_default()
}

// ─── geometry ───────────────────────────────────────────────────────

/// (x0, y0, x1, y1) in page px → integer crop rect, rounded and clamped
/// like the pipeline's `bbox_to_rect`. `None` if it collapses.
fn to_rect(b: [f64; 4], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let x0 = (b[0].max(0.0).round() as u32).min(w);
    let y0 = (b[1].max(0.0).round() as u32).min(h);
    let x1 = (b[2].max(0.0).round() as u32).min(w);
    let y1 = (b[3].max(0.0).round() as u32).min(h);
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

fn det_box(d: &DetBox) -> [f64; 4] {
    [d.x0, d.y0, d.x1, d.y1].map(f64::from)
}

fn area(b: [f64; 4]) -> f64 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

fn inter(a: [f64; 4], b: [f64; 4]) -> f64 {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// Grow a box by `frac` of its own width/height on each side.
fn grow(b: [f64; 4], frac: f64) -> [f64; 4] {
    let (dw, dh) = ((b[2] - b[0]) * frac, (b[3] - b[1]) * frac);
    [b[0] - dw, b[1] - dh, b[2] + dw, b[3] + dh]
}

/// Indices of boxes nested in another (the panel drops these).
fn nested_flags(boxes: &[DetBox]) -> Vec<bool> {
    let bs: Vec<[f64; 4]> = boxes.iter().map(det_box).collect();
    (0..bs.len())
        .map(|i| {
            let a = area(bs[i]);
            if a <= 0.0 {
                return true;
            }
            (0..bs.len()).any(|j| {
                if i == j {
                    return false;
                }
                let oa = area(bs[j]);
                if oa < a || (oa == a && j > i) {
                    return false;
                }
                inter(bs[i], bs[j]) / a >= NESTED_OVERLAP
            })
        })
        .collect()
}

fn crop(page: &DynamicImage, r: (u32, u32, u32, u32)) -> DynamicImage {
    page.crop_imm(r.0, r.1, r.2, r.3)
}

// ─── result shapes ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
struct StageSamples {
    first: Option<f64>,
    warm: Vec<f64>,
}

impl StageSamples {
    fn push(&mut self, ms: f64) {
        if self.first.is_none() {
            self.first = Some(ms);
        } else {
            self.warm.push(ms);
        }
    }
    fn warm_median(&self) -> Option<f64> {
        if self.warm.is_empty() {
            self.first
        } else {
            Some(median(&self.warm))
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct PageTimings {
    file_read: StageSamples,
    decode: StageSamples,
    detect: StageSamples,
}

#[derive(Debug, Clone, Default, Serialize)]
struct PageResult {
    key: String,
    path: String,
    width: u32,
    height: u32,
    file_bytes: u64,
    boxes: Vec<DetBox>,
    nested: Vec<bool>,
    class_counts: [usize; 2],
    nested_count: usize,
    refs: Option<usize>,
    refs_without_overlap: Option<usize>,
    refs_without_half_cover: Option<usize>,
    timings: Option<PageTimings>,
    sheet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TimingRow {
    stage: String,
    input: String,
    cold_ms: Option<f64>,
    warm: Summary,
}

#[derive(Debug, Clone, Serialize)]
struct Composed {
    name: String,
    definition: String,
    cold_ms: Option<f64>,
    warm: Summary,
}

#[derive(Debug, Clone, Default, Serialize)]
struct Agg {
    n: usize,
    le5: usize,
    le10: usize,
    le25: usize,
    over25: usize,
    missed: usize,
    mean_cer: f64,
    median_cer: f64,
    mean_wer: f64,
    median_wer: f64,
    /// Mean CER over every reference, a missed one counting as 1.0.
    mean_cer_all: f64,
}

fn aggregate<'a>(items: impl Iterator<Item = (Option<f64>, Option<f64>)> + 'a) -> Agg {
    let mut a = Agg::default();
    let (mut cers, mut wers, mut all) = (Vec::new(), Vec::new(), Vec::new());
    for (cer, wer) in items {
        a.n += 1;
        match cer {
            None => {
                a.missed += 1;
                all.push(1.0);
            }
            Some(c) => {
                if c <= 0.05 {
                    a.le5 += 1;
                }
                if c <= 0.10 {
                    a.le10 += 1;
                }
                if c <= 0.25 {
                    a.le25 += 1;
                } else {
                    a.over25 += 1;
                }
                cers.push(c);
                all.push(c.min(1.0));
                wers.push(wer.unwrap_or(1.0));
            }
        }
    }
    let mean = |v: &[f64]| {
        if v.is_empty() {
            f64::NAN
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    a.mean_cer = mean(&cers);
    a.median_cer = if cers.is_empty() {
        f64::NAN
    } else {
        median(&cers)
    };
    a.mean_wer = mean(&wers);
    a.median_wer = if wers.is_empty() {
        f64::NAN
    } else {
        median(&wers)
    };
    a.mean_cer_all = mean(&all);
    a
}

/// The `by_use` group a reader taps: balloons and captions.
const TAP_TARGETS: &str = "tap targets (balloon, caption)";

/// Which use a region kind belongs to.
fn use_group(kind: &str) -> &'static str {
    match kind {
        "balloon" | "caption" => TAP_TARGETS,
        _ => "display lettering (sfx, sign, title, other)",
    }
}

#[derive(Debug, Clone, Serialize)]
struct Grouped {
    overall: Agg,
    /// Tap targets (balloon, caption) apart from display lettering
    /// (sound effects, signs, titles).
    by_use: BTreeMap<String, Agg>,
    by_kind: BTreeMap<String, Agg>,
    by_colour_case: BTreeMap<String, Agg>,
}

fn grouped<T>(
    items: &[T],
    key: impl Fn(&T) -> (Option<f64>, Option<f64>),
    kind: impl Fn(&T) -> &str,
    colour: impl Fn(&T) -> &str,
) -> Grouped {
    let mut by_kind: BTreeMap<String, Vec<&T>> = BTreeMap::new();
    let mut by_colour: BTreeMap<String, Vec<&T>> = BTreeMap::new();
    let mut by_use: BTreeMap<String, Vec<&T>> = BTreeMap::new();
    for it in items {
        by_use
            .entry(use_group(kind(it)).to_owned())
            .or_default()
            .push(it);
        by_kind.entry(kind(it).to_owned()).or_default().push(it);
        by_colour.entry(colour(it).to_owned()).or_default().push(it);
    }
    Grouped {
        overall: aggregate(items.iter().map(&key)),
        by_use: by_use
            .into_iter()
            .map(|(k, v)| (k, aggregate(v.into_iter().map(&key))))
            .collect(),
        by_kind: by_kind
            .into_iter()
            .map(|(k, v)| (k, aggregate(v.into_iter().map(&key))))
            .collect(),
        by_colour_case: by_colour
            .into_iter()
            .map(|(k, v)| (k, aggregate(v.into_iter().map(&key))))
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize)]
struct Results {
    label: String,
    args: Args,
    env: Env,
    detector: String,
    detector_load: Option<engines::DetectorLoad>,
    pages: Vec<PageResult>,
    timing_rows: Vec<TimingRow>,
    composed: Vec<Composed>,
}

// ─── the run ───────────────────────────────────────────────────────

fn score_pair(reference_norm: &str, text: &str) -> (f64, f64) {
    let h = score::normalize(text, false);
    (
        score::cer(reference_norm, &h),
        score::wer(reference_norm, &h),
    )
}

struct Ctx {
    args: Args,
    detector: Option<CtdOwn>,
}

#[expect(clippy::print_stderr, reason = "command-line progress output")]
fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();
    if args.probe {
        let m = args
            .models
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--probe needs --models"))?;
        return stage2::probe(&m, args.threads);
    }
    if args.cand || args.cand_timing || args.throughput {
        std::fs::create_dir_all(&args.out)?;
        let mut pages = truth::list_pages(&args.pages)?;
        if let Some(f) = &args.only {
            pages.retain(|p| p.key.contains(f.as_str()));
        }
        if let Some(n) = args.max_pages {
            pages.truncate(n);
        }
        return stage2::run(&args, &pages);
    }
    if !(args.timing || args.sheets || args.detector_only) {
        args.timing = true;
        args.sheets = true;
    }
    if args.detector_only {
        args.timing = true;
    }
    std::fs::create_dir_all(&args.out)?;
    let loadavg_start = loadavg();

    let (detector, detector_load) = engines::load_ctd(args.threads)?;
    let detector_load = Some(detector_load);

    let mut ctx = Ctx {
        args: args.clone(),
        detector: Some(detector),
    };

    let mut pages = truth::list_pages(&args.pages)?;
    if let Some(f) = &args.only {
        pages.retain(|p| p.key.contains(f.as_str()));
    }
    if let Some(n) = args.max_pages {
        pages.truncate(n);
    }
    if pages.is_empty() {
        anyhow::bail!("no pages under {}", args.pages.display());
    }

    let mut results = Vec::new();
    let mut threads_after_detect = None;
    for (i, pf) in pages.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, pages.len(), pf.key);
        let r = run_page(&mut ctx, pf).with_context(|| format!("page {}", pf.key))?;
        if threads_after_detect.is_none() && ctx.detector.is_some() {
            threads_after_detect = read_status_field("Threads:").and_then(|v| v.parse().ok());
        }
        results.push(r);
    }

    let env = Env {
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        cpu_model: cpu_model(),
        cpus_allowed: read_status_field("Cpus_allowed_list:").unwrap_or_default(),
        available_parallelism: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        omp_num_threads: std::env::var("OMP_NUM_THREADS").ok(),
        loadavg_start,
        loadavg_end: loadavg(),
        threads_after_detect,
    };

    let (timing_rows, composed) = if args.timing {
        summarize_timing(&results, detector_load.as_ref())
    } else {
        (Vec::new(), Vec::new())
    };

    let res = Results {
        label: args.label.clone(),
        args: args.clone(),
        env,
        detector: ctx.detector.as_ref().map(|d| d.name()).unwrap_or_default(),
        detector_load,
        pages: results,
        timing_rows,
        composed,
    };
    let json_path = args.out.join(format!("{}.json", args.label));
    std::fs::write(&json_path, serde_json::to_string_pretty(&res)?)?;
    let md_path = args.out.join(format!("{}.md", args.label));
    std::fs::write(&md_path, report::markdown(&res))?;
    eprintln!("wrote {} and {}", json_path.display(), md_path.display());
    Ok(())
}

fn run_page(ctx: &mut Ctx, pf: &PageFile) -> anyhow::Result<PageResult> {
    let args = ctx.args.clone();
    let mut pr = PageResult {
        key: pf.key.clone(),
        path: pf.path.display().to_string(),
        ..Default::default()
    };
    let mut timings = PageTimings::default();

    // Read + decode (first run, then warm repeats when timing).
    let reps = if args.timing { 1 + args.warm } else { 1 };
    let mut page = None;
    for _ in 0..reps {
        let (bytes, ms) = timed(|| std::fs::read(&pf.path));
        let bytes = bytes?;
        timings.file_read.push(ms);
        pr.file_bytes = bytes.len() as u64;
        let (img, ms) = timed(|| decode_limited(&bytes));
        timings.decode.push(ms);
        page = Some(img?);
    }
    let page = page.expect("decoded at least once");
    pr.width = page.width();
    pr.height = page.height();

    // Detect.
    if let Some(det) = ctx.detector.as_mut() {
        let reps = if args.timing { 1 + args.warm } else { 1 };
        let mut boxes = Vec::new();
        for _ in 0..reps {
            let (b, ms) = timed(|| det.detect(&page));
            timings.detect.push(ms);
            boxes = b?;
        }
        pr.nested = nested_flags(&boxes);
        pr.nested_count = pr.nested.iter().filter(|n| **n).count();
        for b in &boxes {
            pr.class_counts[(b.class as usize).min(1)] += 1;
        }
        pr.boxes = boxes;
    }

    let refs: Option<Vec<RefRegion>> = match &args.truth {
        Some(t) => truth::load_truth(t, &pf.key)?,
        None => None,
    };
    let ref_px: Vec<[f64; 4]> = refs
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|r| r.bbox_px(pr.width, pr.height))
        .collect();
    if refs.is_some() {
        pr.refs = Some(ref_px.len());
        if ctx.detector.is_some() {
            let dets: Vec<[f64; 4]> = pr.boxes.iter().map(det_box).collect();
            pr.refs_without_overlap = Some(
                ref_px
                    .iter()
                    .filter(|r| !dets.iter().any(|d| inter(**r, *d) > 0.0))
                    .count(),
            );
            pr.refs_without_half_cover = Some(
                ref_px
                    .iter()
                    .filter(|r| {
                        let a = area(**r).max(1.0);
                        !dets.iter().any(|d| inter(**r, *d) / a >= 0.5)
                    })
                    .count(),
            );
        }
    }

    if args.sheets && ctx.detector.is_some() {
        let p = args
            .out
            .join(format!("{}-sheets", args.label))
            .join(format!("{}.jpg", pf.key));
        draw::contact_sheet(&page, &pr.boxes, &ref_px, &p)?;
        pr.sheet = Some(p.display().to_string());
    }

    if args.timing {
        pr.timings = Some(timings);
    }
    Ok(pr)
}

// ─── summaries ─────────────────────────────────────────────────────

fn summarize_timing(
    pages: &[PageResult],
    load: Option<&engines::DetectorLoad>,
) -> (Vec<TimingRow>, Vec<Composed>) {
    let ts: Vec<&PageTimings> = pages.iter().filter_map(|p| p.timings.as_ref()).collect();
    let Some(first_page) = ts.first() else {
        return (Vec::new(), Vec::new());
    };
    let dims: Vec<String> = pages
        .iter()
        .map(|p| format!("{}x{}", p.width, p.height))
        .collect();
    let dim_range = {
        let mut d = dims.clone();
        d.sort();
        d.dedup();
        if d.len() <= 3 {
            d.join(", ")
        } else {
            format!("{} sizes, e.g. {}", d.len(), d[0])
        }
    };
    let bytes: Vec<f64> = pages.iter().map(|p| p.file_bytes as f64 / 1024.0).collect();
    let mut rows = Vec::new();
    let all_warm = |f: &dyn Fn(&PageTimings) -> &StageSamples| -> Vec<f64> {
        ts.iter().flat_map(|t| f(t).warm.clone()).collect()
    };
    rows.push(TimingRow {
        stage: "file read".into(),
        input: format!("JPEG, median {:.0} KiB", median(&bytes)),
        cold_ms: first_page.file_read.first,
        warm: summarize(&all_warm(&|t| &t.file_read)),
    });
    rows.push(TimingRow {
        stage: "page decode (decode_limited)".into(),
        input: format!("JPEG -> RGB, {dim_range}"),
        cold_ms: first_page.decode.first,
        warm: summarize(&all_warm(&|t| &t.decode)),
    });
    if let Some(l) = load {
        rows.push(TimingRow {
            stage: "detector model load: hash of cached file".into(),
            input: format!("{:.1} MB ONNX", l.model_bytes as f64 / 1e6),
            cold_ms: Some(l.precheck_hash_ms),
            warm: Summary::default(),
        });
        rows.push(TimingRow {
            stage: "detector model load: ONNX Runtime session build".into(),
            input: "once per process".into(),
            cold_ms: Some(l.session_build_ms),
            warm: Summary::default(),
        });
    }
    if first_page.detect.first.is_some() {
        rows.push(TimingRow {
            stage: "detector inference (incl. 1024x1024 resize, mask)".into(),
            input: format!("{dim_range} -> 1024x1024"),
            cold_ms: first_page.detect.first,
            warm: summarize(&all_warm(&|t| &t.detect)),
        });
    }

    // Composition, from per-page warm medians.
    let mut composed = Vec::new();
    let page_dec: Vec<f64> = ts.iter().filter_map(|t| t.decode.warm_median()).collect();
    let page_det: Vec<f64> = ts.iter().filter_map(|t| t.detect.warm_median()).collect();
    if page_det.len() == page_dec.len() && !page_det.is_empty() {
        let outline: Vec<f64> = page_dec.iter().zip(&page_det).map(|(a, b)| a + b).collect();
        let cold = match (first_page.decode.first, first_page.detect.first, load) {
            (Some(a), Some(b), Some(l)) => Some(a + b + l.precheck_hash_ms + l.session_build_ms),
            _ => None,
        };
        composed.push(Composed {
            name: "(a) outlines appear".into(),
            definition: "decode + detect, one page (cold adds model load)".into(),
            cold_ms: cold,
            warm: summarize(&outline),
        });
    }
    (rows, composed)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
