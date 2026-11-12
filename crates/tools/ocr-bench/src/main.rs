//! `ocr-bench`: timings and accuracy for Folio's OCR pipeline.
//!
//! Drives `server::ocr` directly (detector singleton, Western recogniser,
//! postprocess); never starts the HTTP server, Postgres or Redis, and
//! never downloads: the detector is loaded only when the pinned model is
//! already cached with the pinned digest.
//!
//! ```text
//! ocr-bench --pages DIR [--truth DIR] --out DIR --label NAME
//!           [--timing] [--accuracy] [--variants] [--sheets] [--detector-only]
//! ```
//!
//! With none of the four section flags, all four run. Writes
//! `<out>/<label>.json` and `<out>/<label>.md`, contact sheets under
//! `<out>/<label>-sheets/` and the worst regions under
//! `<out>/<label>-worst/`. Run the release build, and run it pinned
//! (`taskset`) for the CPU-limited configuration; the tool records the
//! CPUs it was allowed, `OMP_NUM_THREADS` and the load average.

mod cand;
mod draw;
mod engines;
mod report;
mod score;
mod split;
mod stage2;
mod stats;
mod tess;
mod truth;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use clap::Parser;
use image::DynamicImage;
use serde::Serialize;
use server::ocr::postprocess::{self, Cleaned};
use server::ocr::recognizer::{Language, Recognition};
use server::util::image_decode::decode_limited;

use crate::engines::{CropRecognizer, DetBox, FolioDetector, FolioWestern, PageDetector};
use crate::stats::{Summary, median, ms_since, summarize, timed};
use crate::tess::{TessConfig, TessEngine};
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
    /// Accuracy section (needs --truth).
    #[arg(long)]
    accuracy: bool,
    /// Tesseract variant table (needs --truth).
    #[arg(long)]
    variants: bool,
    /// Contact sheets.
    #[arg(long)]
    sheets: bool,
    /// Timing of decode + detection only (for the thread experiments).
    #[arg(long)]
    detector_only: bool,
    /// Warm repetitions of page decode and detection, per page.
    #[arg(long, default_value_t = 5)]
    warm: usize,
    /// Warm repetitions of each region's recognition, per region.
    #[arg(long, default_value_t = 2)]
    rec_warm: usize,
    /// Only pages whose key contains this.
    #[arg(long)]
    only: Option<String>,
    /// At most this many pages (after --only).
    #[arg(long)]
    max_pages: Option<usize>,
    /// Margin (fraction of the reference box) for the variant table and
    /// the worst-regions list.
    #[arg(long, default_value_t = 0.04)]
    variant_margin: f64,
    /// Other language data to compare against `tessdata_best`.
    #[arg(long, default_value = "/usr/share/tessdata")]
    system_tessdata: PathBuf,

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
        default_value = "tess-today,tess-best,pp-v5-en,pp-v5-latin,pp-v6-tiny,pp-v6-small,pp-v6-medium"
    )]
    recs: String,
    /// Detectors, comma separated (ctd, ctd-crate, ogk-s, ogk-m, ppdetN).
    #[arg(long, default_value = "ctd,ogk-s,ogk-m,ppdet960,ppdet1600")]
    dets: String,
    /// Line splits for the PP recognisers.
    #[arg(
        long,
        default_value = "ppdet,ppdet2x,page-lines,profile,ctd-lines,none"
    )]
    splits: String,
    /// Tesseract variant used as "best Tesseract".
    #[arg(long, default_value = "current")]
    best_tess: String,
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

/// Margins tried for recognition alone, as a fraction of the box size.
const MARGINS: [f64; 3] = [0.0, 0.04, 0.08];
/// The web panel's nested-box rule (`web/lib/reader/page-text.ts`).
const NESTED_OVERLAP: f64 = 0.8;
/// Worst regions listed.
const WORST_N: usize = 25;

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
    tesseract_version: String,
    tessdata: String,
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

/// Same resolution as the server's recogniser.
fn tessdata_dir() -> anyhow::Result<PathBuf> {
    if let Ok(d) = std::env::var("TESSDATA_PREFIX") {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var("HOME").context("HOME unset")?;
    Ok(PathBuf::from(home).join(".tesseract-rs").join("tessdata"))
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
struct RegionTiming {
    box_index: usize,
    crop_px: (u32, u32),
    crop: StageSamples,
    /// `WesternOcr::recognize`, the production call.
    recognize_total: StageSamples,
    /// `western::preprocess` on its own.
    preprocess: StageSamples,
    /// The Tesseract call on the prepared image (bench engine, Folio's
    /// settings): set_image, recognize, word walk.
    tesseract: StageSamples,
    postprocess: StageSamples,
}

#[derive(Debug, Clone, Default, Serialize)]
struct PageTimings {
    file_read: StageSamples,
    decode: StageSamples,
    detect: StageSamples,
    regions: Vec<RegionTiming>,
}

#[derive(Debug, Clone, Serialize)]
struct Produced {
    box_index: usize,
    class: u32,
    nested: bool,
    raw: String,
    clean: String,
    confidence: f32,
    ms: f64,
}

#[derive(Debug, Clone, Serialize)]
struct RefOutcome {
    page: String,
    ref_id: String,
    kind: String,
    colour_case: String,
    reference: String,
    /// Index into the produced list it was matched to (the first part).
    matched: Option<usize>,
    /// Produced regions joined for it (more than one: many-to-one).
    parts: usize,
    produced: Option<String>,
    cer: Option<f64>,
    wer: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct E2e {
    set: &'static str,
    outcomes: Vec<RefOutcome>,
    unmatched_nonempty: Vec<String>,
    unmatched_empty: usize,
}

#[derive(Debug, Clone, Serialize)]
struct RegionRec {
    page: String,
    ref_id: String,
    kind: String,
    colour_case: String,
    reference: String,
    engine: String,
    margin: f64,
    crop_px: (u32, u32, u32, u32),
    raw: String,
    clean: String,
    cer_raw: f64,
    cer_clean: f64,
    wer_raw: f64,
    wer_clean: f64,
    confidence: f32,
    ms: f64,
    /// The recogniser's own stage breakdown (name, ms), when it gives one.
    stages: Vec<(String, f64)>,
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
    e2e: Vec<E2e>,
    produced: Vec<Produced>,
    rec_alone: Vec<RegionRec>,
    variant_recs: Vec<RegionRec>,
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

/// Which use a region kind belongs to.
fn use_group(kind: &str) -> &'static str {
    match kind {
        "balloon" | "caption" => "tap targets (balloon, caption)",
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
struct E2eSummary {
    set: &'static str,
    refs: Grouped,
    produced_total: usize,
    unmatched_nonempty: usize,
    unmatched_empty: usize,
}

#[derive(Debug, Clone, Serialize)]
struct MarginSummary {
    margin: f64,
    clean: Grouped,
    raw: Agg,
    time: Summary,
}

#[derive(Debug, Clone, Serialize)]
struct VariantSummary {
    name: String,
    what_changed: String,
    effective: serde_json::Value,
    clean: Agg,
    raw: Agg,
    time: Summary,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Worst {
    rank: usize,
    page: String,
    ref_id: String,
    kind: String,
    colour_case: String,
    reference: String,
    produced: String,
    produced_raw: String,
    cer: f64,
    wer: f64,
    crop_image: String,
    prepared_image: String,
}

#[derive(Debug, Clone, Serialize)]
struct Results {
    label: String,
    args: Args,
    env: Env,
    detector: String,
    recognizer: String,
    detector_load: Option<engines::DetectorLoad>,
    recognizer_init_ms: Option<f64>,
    bench_tesseract_init_ms: Option<f64>,
    pages: Vec<PageResult>,
    timing_rows: Vec<TimingRow>,
    composed: Vec<Composed>,
    e2e: Vec<E2eSummary>,
    margins: Vec<MarginSummary>,
    variants: Vec<VariantSummary>,
    /// Crops where the bench's `current` Tesseract config and the server's
    /// recogniser returned the same raw text, of those compared.
    current_matches_server: (usize, usize),
    worst: Vec<Worst>,
    notes: Vec<String>,
}

// ─── the run ───────────────────────────────────────────────────────

fn clean(rec: Recognition) -> (Cleaned, f64) {
    timed(|| postprocess::clean(rec, Language::Western))
}

fn score_pair(reference_norm: &str, text: &str) -> (f64, f64) {
    let h = score::normalize(text, false);
    (
        score::cer(reference_norm, &h),
        score::wer(reference_norm, &h),
    )
}

struct Ctx {
    args: Args,
    detector: Option<FolioDetector>,
    western: Option<FolioWestern>,
    split: Option<TessEngine>,
    variants: Vec<(TessConfig, anyhow::Result<TessEngine>)>,
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
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut pages = truth::list_pages(&args.pages)?;
        if let Some(f) = &args.only {
            pages.retain(|p| p.key.contains(f.as_str()));
        }
        if let Some(n) = args.max_pages {
            pages.truncate(n);
        }
        return stage2::run(&args, &rt, &pages);
    }
    if !(args.timing || args.accuracy || args.variants || args.sheets || args.detector_only) {
        args.timing = true;
        args.accuracy = true;
        args.variants = true;
        args.sheets = true;
    }
    if args.detector_only {
        args.timing = true;
    }
    if (args.accuracy || args.variants) && args.truth.is_none() {
        args.accuracy = false;
        args.variants = false;
    }
    std::fs::create_dir_all(&args.out)?;
    let loadavg_start = loadavg();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let tessdata = tessdata_dir()?;
    let mut notes = Vec::new();
    let need_detector = args.timing || args.accuracy || args.sheets;
    let need_rec = !args.detector_only && (args.timing || args.accuracy || args.variants);

    let (detector, detector_load) = if need_detector {
        let (d, l) = rt.block_on(FolioDetector::load())?;
        (Some(d), Some(l))
    } else {
        (None, None)
    };
    let (western, recognizer_init_ms) = if need_rec {
        let (w, ms) = rt.block_on(FolioWestern::load())?;
        (Some(w), Some(ms))
    } else {
        (None, None)
    };
    let (split, bench_tesseract_init_ms) = if need_rec {
        let (e, ms) = timed(|| TessEngine::new(TessConfig::current(&tessdata), &args.out));
        (Some(e?), Some(ms))
    } else {
        (None, None)
    };
    let variants = if args.variants {
        let sys = args
            .system_tessdata
            .join("eng.traineddata")
            .exists()
            .then_some(args.system_tessdata.as_path());
        if sys.is_none() {
            notes.push(format!(
                "no eng.traineddata under {}; only tessdata_best compared",
                args.system_tessdata.display()
            ));
        }
        let work = args.out.join(format!("{}-work", args.label));
        TessConfig::variants(&tessdata, sys)
            .into_iter()
            .map(|c| {
                let e = TessEngine::new(c.clone(), &work);
                (c, e)
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut ctx = Ctx {
        args: args.clone(),
        detector,
        western,
        split,
        variants,
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
        tesseract_version: tesseract_rs::TesseractAPI::version(),
        tessdata: tessdata.display().to_string(),
    };

    let (timing_rows, composed) = if args.timing {
        summarize_timing(&results, detector_load.as_ref(), recognizer_init_ms)
    } else {
        (Vec::new(), Vec::new())
    };
    let e2e = if args.accuracy {
        summarize_e2e(&results)
    } else {
        Vec::new()
    };
    let margins = if args.accuracy {
        summarize_margins(&results)
    } else {
        Vec::new()
    };
    let (variants, current_matches_server) = if args.variants {
        summarize_variants(&ctx, &results)
    } else {
        (Vec::new(), (0, 0))
    };
    let worst = if args.accuracy {
        write_worst(&args, &results)?
    } else {
        Vec::new()
    };

    let res = Results {
        label: args.label.clone(),
        args: args.clone(),
        env,
        detector: ctx.detector.as_ref().map(|d| d.name()).unwrap_or_default(),
        recognizer: ctx.western.as_ref().map(|w| w.name()).unwrap_or_default(),
        detector_load,
        recognizer_init_ms,
        bench_tesseract_init_ms,
        pages: results,
        timing_rows,
        composed,
        e2e,
        margins,
        variants,
        current_matches_server,
        worst,
        notes,
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

    // Per-region recognition timing over the panel set (what "Show page
    // text" reads: non-nested boxes, `detect: false`).
    if args.timing
        && !args.detector_only
        && let (Some(western), Some(split)) = (ctx.western.as_mut(), ctx.split.as_ref())
    {
        for (bi, b) in pr.boxes.iter().enumerate() {
            if pr.nested[bi] {
                continue;
            }
            let Some(rect) = to_rect(det_box(b), pr.width, pr.height) else {
                continue;
            };
            let mut rt = RegionTiming {
                box_index: bi,
                crop_px: (rect.2, rect.3),
                ..Default::default()
            };
            for _ in 0..=args.rec_warm {
                let (c, ms) = timed(|| crop(&page, rect));
                rt.crop.push(ms);
                let (out, ms) = timed(|| western.recognize(&c));
                rt.recognize_total.push(ms);
                let out = out?;
                let (_, ms) = clean(out.rec);
                rt.postprocess.push(ms);
                let (prepared, ms) = timed(|| server::ocr::recognizer::western::preprocess(&c));
                rt.preprocess.push(ms);
                let (r, ms) = timed(|| split.recognize_prepared_block(&prepared));
                r?;
                rt.tesseract.push(ms);
            }
            timings.regions.push(rt);
        }
    }

    if args.accuracy
        && let Some(refs) = refs.as_ref()
    {
        accuracy_page(ctx, &mut pr, &page, refs, &ref_px)?;
    }
    if args.variants
        && let Some(refs) = refs.as_ref()
    {
        variants_page(ctx, &mut pr, &page, refs, &ref_px)?;
    }

    if args.timing {
        pr.timings = Some(timings);
    }
    Ok(pr)
}

fn accuracy_page(
    ctx: &mut Ctx,
    pr: &mut PageResult,
    page: &DynamicImage,
    refs: &[RefRegion],
    ref_px: &[[f64; 4]],
) -> anyhow::Result<()> {
    let western = ctx
        .western
        .as_mut()
        .expect("recogniser loaded for accuracy");
    // End to end: recognise every detected box.
    for (bi, b) in pr.boxes.iter().enumerate() {
        let Some(rect) = to_rect(det_box(b), pr.width, pr.height) else {
            continue;
        };
        let c = crop(page, rect);
        let t = Instant::now();
        let out = western.recognize(&c)?;
        let raw = out.rec.text.clone();
        let (cl, _) = clean(out.rec);
        pr.produced.push(Produced {
            box_index: bi,
            class: b.class,
            nested: pr.nested[bi],
            raw,
            clean: cl.text,
            confidence: cl.confidence,
            ms: ms_since(t),
        });
    }
    let refs_norm: Vec<String> = refs
        .iter()
        .map(|r| score::normalize(&r.text, true))
        .collect();
    for set in ["all-boxes", "panel-set"] {
        let cand: Vec<usize> = (0..pr.produced.len())
            .filter(|i| set == "all-boxes" || !pr.produced[*i].nested)
            .collect();
        pr.e2e
            .push(match_page(pr, refs, &refs_norm, ref_px, &cand, set));
    }

    // Recognition alone at each margin.
    for &m in &MARGINS {
        for (ri, r) in refs.iter().enumerate() {
            let Some(rect) = to_rect(grow(ref_px[ri], m), pr.width, pr.height) else {
                continue;
            };
            let c = crop(page, rect);
            let t = Instant::now();
            let out = western.recognize(&c)?;
            let raw = out.rec.text.clone();
            let (cl, _) = clean(out.rec);
            let ms = ms_since(t);
            let (cer_raw, wer_raw) = score_pair(&refs_norm[ri], &raw);
            let (cer_clean, wer_clean) = score_pair(&refs_norm[ri], &cl.text);
            pr.rec_alone.push(RegionRec {
                page: pr.key.clone(),
                ref_id: r.id.clone(),
                kind: r.kind.clone(),
                colour_case: r.colour_case.clone(),
                reference: score::display(&refs_norm[ri]),
                engine: "folio-western".into(),
                margin: m,
                crop_px: rect,
                raw,
                clean: cl.text,
                cer_raw,
                cer_clean,
                wer_raw,
                wer_clean,
                confidence: cl.confidence,
                ms,
                stages: Vec::new(),
            });
        }
    }
    Ok(())
}

/// One-to-one matching of reference regions to produced regions by text,
/// with candidates restricted to the reference's area of the page and
/// the box overlap breaking ties.
fn match_page(
    pr: &PageResult,
    refs: &[RefRegion],
    refs_norm: &[String],
    ref_px: &[[f64; 4]],
    cand: &[usize],
    set: &'static str,
) -> E2e {
    let (pw, ph) = (f64::from(pr.width), f64::from(pr.height));
    let cost: Vec<Vec<f64>> = refs
        .iter()
        .enumerate()
        .map(|(ri, _)| {
            let r = ref_px[ri];
            let g = grow(r, 0.5);
            let near = [
                g[0] - 0.02 * pw,
                g[1] - 0.02 * ph,
                g[2] + 0.02 * pw,
                g[3] + 0.02 * ph,
            ];
            cand.iter()
                .map(|&pi| {
                    let p = &pr.produced[pi];
                    let b = det_box(&pr.boxes[p.box_index]);
                    if inter(near, b) <= 0.0 {
                        return score::FORBIDDEN;
                    }
                    let (c, _) = score_pair(&refs_norm[ri], &p.clean);
                    let overlap = inter(r, b) / area(r).max(1.0);
                    c.min(1.0) + 0.001 * (1.0 - overlap)
                })
                .collect()
        })
        .collect();
    let assignment = if cand.is_empty() {
        vec![None; refs.len()]
    } else {
        score::assign(&cost)
    };
    let mut used = vec![false; cand.len()];
    let mut outcomes = Vec::new();
    for (ri, r) in refs.iter().enumerate() {
        let mut o = RefOutcome {
            page: pr.key.clone(),
            ref_id: r.id.clone(),
            kind: r.kind.clone(),
            colour_case: r.colour_case.clone(),
            reference: score::display(&refs_norm[ri]),
            matched: None,
            parts: 0,
            produced: None,
            cer: None,
            wer: None,
        };
        if let Some(ci) = assignment[ri] {
            let p = &pr.produced[cand[ci]];
            let (c, w) = score_pair(&refs_norm[ri], &p.clean);
            if c < 1.0 {
                used[ci] = true;
                o.parts = 1;
                o.matched = Some(cand[ci]);
                o.produced = Some(p.clean.clone());
                o.cer = Some(c);
                o.wer = Some(w);
            }
        }
        outcomes.push(o);
    }
    // Many-to-one: a reference drawn as one unit (stacked signs, a screen)
    // may be several produced boxes. Greedily add unused produced regions
    // whose centre lies inside the reference box (grown by 10%) while the
    // joined text (in reading order) lowers the error.
    for (ri, o) in outcomes.iter_mut().enumerate() {
        let g = grow(ref_px[ri], 0.1);
        let mut parts: Vec<usize> = o
            .matched
            .and_then(|pi| cand.iter().position(|&c| c == pi))
            .into_iter()
            .collect();
        let mut best = o.cer.unwrap_or(1.0);
        loop {
            let mut improved = None;
            for (ci, &pi) in cand.iter().enumerate() {
                if used[ci] || parts.contains(&ci) {
                    continue;
                }
                let b = det_box(&pr.boxes[pr.produced[pi].box_index]);
                let c = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                if c.0 < g[0] || c.0 > g[2] || c.1 < g[1] || c.1 > g[3] {
                    continue;
                }
                let mut trial = parts.clone();
                trial.push(ci);
                let text = joined(pr, cand, &trial);
                let (cer, _) = score_pair(&refs_norm[ri], &text);
                if cer < best && improved.is_none_or(|(_, e)| cer < e) {
                    improved = Some((ci, cer));
                }
            }
            let Some((ci, cer)) = improved else { break };
            parts.push(ci);
            best = cer;
        }
        if parts.len() > 1 || (o.matched.is_none() && !parts.is_empty()) {
            for &ci in &parts {
                used[ci] = true;
            }
            let text = joined(pr, cand, &parts);
            let (c, w) = score_pair(&refs_norm[ri], &text);
            o.matched = Some(cand[parts[0]]);
            o.parts = parts.len();
            o.produced = Some(text);
            o.cer = Some(c);
            o.wer = Some(w);
        }
    }
    let mut unmatched_nonempty = Vec::new();
    let mut unmatched_empty = 0;
    for (ci, &pi) in cand.iter().enumerate() {
        if used[ci] {
            continue;
        }
        let t = pr.produced[pi].clean.trim();
        if t.is_empty() {
            unmatched_empty += 1;
        } else {
            unmatched_nonempty.push(t.to_owned());
        }
    }
    E2e {
        set,
        outcomes,
        unmatched_nonempty,
        unmatched_empty,
    }
}

/// Clean texts of the given candidate positions, in reading order.
fn joined(pr: &PageResult, cand: &[usize], parts: &[usize]) -> String {
    let mut ps: Vec<usize> = parts.iter().map(|&ci| cand[ci]).collect();
    ps.sort_by(|&a, &b| {
        let (ba, bb) = (
            det_box(&pr.boxes[pr.produced[a].box_index]),
            det_box(&pr.boxes[pr.produced[b].box_index]),
        );
        ba[1].total_cmp(&bb[1]).then(ba[0].total_cmp(&bb[0]))
    });
    ps.iter()
        .map(|&p| pr.produced[p].clean.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn variants_page(
    ctx: &mut Ctx,
    pr: &mut PageResult,
    page: &DynamicImage,
    refs: &[RefRegion],
    ref_px: &[[f64; 4]],
) -> anyhow::Result<()> {
    let m = ctx.args.variant_margin;
    for (ri, r) in refs.iter().enumerate() {
        let reference = score::normalize(&r.text, true);
        let grown = grow(ref_px[ri], m);
        for (cfg, engine) in ctx.variants.iter_mut() {
            let Ok(engine) = engine else { continue };
            let c = cfg.context_px as f64;
            let b = [grown[0] - c, grown[1] - c, grown[2] + c, grown[3] + c];
            let Some(rect) = to_rect(b, pr.width, pr.height) else {
                continue;
            };
            let cr = crop(page, rect);
            let t = Instant::now();
            let out = engine.recognize(&cr)?;
            let stages = out
                .stages
                .iter()
                .map(|(n, m)| ((*n).to_owned(), *m))
                .collect();
            let raw = out.rec.text.clone();
            let (cl, _) = clean(out.rec);
            let ms = ms_since(t);
            let (cer_raw, wer_raw) = score_pair(&reference, &raw);
            let (cer_clean, wer_clean) = score_pair(&reference, &cl.text);
            pr.variant_recs.push(RegionRec {
                page: pr.key.clone(),
                ref_id: r.id.clone(),
                kind: r.kind.clone(),
                colour_case: r.colour_case.clone(),
                reference: score::display(&reference),
                engine: cfg.name.clone(),
                margin: m,
                crop_px: rect,
                raw,
                clean: cl.text,
                cer_raw,
                cer_clean,
                wer_raw,
                wer_clean,
                confidence: cl.confidence,
                ms,
                stages,
            });
        }
    }
    Ok(())
}

// ─── summaries ─────────────────────────────────────────────────────

fn summarize_timing(
    pages: &[PageResult],
    load: Option<&engines::DetectorLoad>,
    rec_init: Option<f64>,
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
            stage: "detector model load: ensure_pinned (hash of cached file)".into(),
            input: format!("{:.1} MB ONNX", l.model_bytes as f64 / 1e6),
            cold_ms: Some(l.ensure_pinned_ms),
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
    if let Some(ms) = rec_init {
        rows.push(TimingRow {
            stage: "recogniser init (Tesseract, eng tessdata_best)".into(),
            input: "once per process".into(),
            cold_ms: Some(ms),
            warm: Summary::default(),
        });
    }
    let regions: Vec<&RegionTiming> = ts.iter().flat_map(|t| t.regions.iter()).collect();
    if let Some(first_region) = regions.first() {
        let sizes: Vec<f64> = regions
            .iter()
            .map(|r| f64::from(r.crop_px.0) * f64::from(r.crop_px.1) / 1000.0)
            .collect();
        let input = format!(
            "{} region crops, median {:.0} kpx (panel set)",
            regions.len(),
            median(&sizes)
        );
        let rw = |f: &dyn Fn(&RegionTiming) -> &StageSamples| -> Vec<f64> {
            regions.iter().flat_map(|r| f(r).warm.clone()).collect()
        };
        for (name, f) in [
            (
                "region crop",
                (|r: &RegionTiming| &r.crop) as fn(&RegionTiming) -> &StageSamples,
            ),
            ("recognition: Folio preprocessing", |r| &r.preprocess),
            ("recognition: Tesseract call", |r| &r.tesseract),
            ("recognition total (WesternOcr::recognize)", |r| {
                &r.recognize_total
            }),
            ("postprocess (clean)", |r| &r.postprocess),
        ] {
            rows.push(TimingRow {
                stage: name.into(),
                input: input.clone(),
                cold_ms: f(first_region).first,
                warm: summarize(&rw(&f)),
            });
        }
    }

    // Composition, from per-page warm medians.
    let mut composed = Vec::new();
    let page_dec: Vec<f64> = ts.iter().filter_map(|t| t.decode.warm_median()).collect();
    let page_det: Vec<f64> = ts.iter().filter_map(|t| t.detect.warm_median()).collect();
    if page_det.len() == page_dec.len() && !page_det.is_empty() {
        let outline: Vec<f64> = page_dec.iter().zip(&page_det).map(|(a, b)| a + b).collect();
        let cold = match (first_page.decode.first, first_page.detect.first, load) {
            (Some(a), Some(b), Some(l)) => Some(a + b + l.ensure_pinned_ms + l.session_build_ms),
            _ => None,
        };
        composed.push(Composed {
            name: "(a) outlines appear".into(),
            definition: "decode + detect, one page (cold adds model load)".into(),
            cold_ms: cold,
            warm: summarize(&outline),
        });
    }
    let region_one = |r: &RegionTiming| {
        r.crop.warm_median().unwrap_or(0.0)
            + r.recognize_total.warm_median().unwrap_or(0.0)
            + r.postprocess.warm_median().unwrap_or(0.0)
    };
    if !regions.is_empty() {
        let mut with_decode = Vec::new();
        let mut without = Vec::new();
        for t in &ts {
            let d = t.decode.warm_median().unwrap_or(0.0);
            for r in &t.regions {
                with_decode.push(d + region_one(r));
                without.push(region_one(r));
            }
        }
        let cold_r = first_page.regions.first().map(|r| {
            r.crop.first.unwrap_or(0.0)
                + r.recognize_total.first.unwrap_or(0.0)
                + r.postprocess.first.unwrap_or(0.0)
        });
        composed.push(Composed {
            name: "(b) tap one balloon, page decoded per request (today)".into(),
            definition: "decode + crop + recognise + clean, per region".into(),
            cold_ms: cold_r
                .map(|r| r + first_page.decode.first.unwrap_or(0.0) + rec_init.unwrap_or(0.0)),
            warm: summarize(&with_decode),
        });
        composed.push(Composed {
            name: "(b') tap one balloon, page already decoded".into(),
            definition: "crop + recognise + clean, per region".into(),
            cold_ms: cold_r.map(|r| r + rec_init.unwrap_or(0.0)),
            warm: summarize(&without),
        });
        let mut today = Vec::new();
        let mut single = Vec::new();
        for t in &ts {
            let d = t.decode.warm_median().unwrap_or(0.0);
            let det = t.detect.warm_median().unwrap_or(0.0);
            let rs: f64 = t.regions.iter().map(region_one).sum();
            let n = t.regions.len() as f64;
            today.push(d + det + n * d + rs);
            single.push(d + det + rs);
        }
        composed.push(Composed {
            name: "(c) read the whole page, serial, decode per region (today)".into(),
            definition: "decode + detect + sum over panel-set regions of (decode + recognise)"
                .into(),
            cold_ms: None,
            warm: summarize(&today),
        });
        composed.push(Composed {
            name: "(c') read the whole page, serial, one decode".into(),
            definition: "decode + detect + sum over panel-set regions of recognise".into(),
            cold_ms: None,
            warm: summarize(&single),
        });
    }
    (rows, composed)
}

fn summarize_e2e(pages: &[PageResult]) -> Vec<E2eSummary> {
    ["all-boxes", "panel-set"]
        .into_iter()
        .map(|set| {
            let es: Vec<&E2e> = pages
                .iter()
                .flat_map(|p| p.e2e.iter().filter(|e| e.set == set))
                .collect();
            let outcomes: Vec<RefOutcome> = es.iter().flat_map(|e| e.outcomes.clone()).collect();
            let produced_total: usize = pages
                .iter()
                .filter(|p| p.refs.is_some())
                .map(|p| {
                    p.produced
                        .iter()
                        .filter(|x| set == "all-boxes" || !x.nested)
                        .count()
                })
                .sum();
            E2eSummary {
                set,
                refs: grouped(
                    &outcomes,
                    |o| (o.cer, o.wer),
                    |o| o.kind.as_str(),
                    |o| o.colour_case.as_str(),
                ),
                produced_total,
                unmatched_nonempty: es.iter().map(|e| e.unmatched_nonempty.len()).sum(),
                unmatched_empty: es.iter().map(|e| e.unmatched_empty).sum(),
            }
        })
        .collect()
}

fn summarize_margins(pages: &[PageResult]) -> Vec<MarginSummary> {
    MARGINS
        .iter()
        .map(|&m| {
            let recs: Vec<RegionRec> = pages
                .iter()
                .flat_map(|p| p.rec_alone.iter().filter(|r| r.margin == m).cloned())
                .collect();
            MarginSummary {
                margin: m,
                clean: grouped(
                    &recs,
                    |r| (Some(r.cer_clean), Some(r.wer_clean)),
                    |r| r.kind.as_str(),
                    |r| r.colour_case.as_str(),
                ),
                raw: aggregate(recs.iter().map(|r| (Some(r.cer_raw), Some(r.wer_raw)))),
                time: summarize(&recs.iter().map(|r| r.ms).collect::<Vec<_>>()),
            }
        })
        .collect()
}

fn summarize_variants(ctx: &Ctx, pages: &[PageResult]) -> (Vec<VariantSummary>, (usize, usize)) {
    let out = ctx
        .variants
        .iter()
        .map(|(cfg, e)| {
            let recs: Vec<&RegionRec> = pages
                .iter()
                .flat_map(|p| p.variant_recs.iter().filter(|r| r.engine == cfg.name))
                .collect();
            VariantSummary {
                name: cfg.name.clone(),
                what_changed: cfg.what_changed.clone(),
                effective: e.as_ref().map(|e| e.effective.clone()).unwrap_or_default(),
                clean: aggregate(recs.iter().map(|r| (Some(r.cer_clean), Some(r.wer_clean)))),
                raw: aggregate(recs.iter().map(|r| (Some(r.cer_raw), Some(r.wer_raw)))),
                time: summarize(&recs.iter().map(|r| r.ms).collect::<Vec<_>>()),
                error: e.as_ref().err().map(|e| format!("{e:#}")),
            }
        })
        .collect();
    // Does the bench's `current` config reproduce the server recogniser?
    let m = ctx.args.variant_margin;
    let mut same = (0, 0);
    for p in pages {
        for v in p.variant_recs.iter().filter(|r| r.engine == "current") {
            if let Some(s) = p
                .rec_alone
                .iter()
                .find(|r| r.ref_id == v.ref_id && (r.margin - m).abs() < 1e-9)
            {
                same.1 += 1;
                if s.raw == v.raw {
                    same.0 += 1;
                }
            }
        }
    }
    (out, same)
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

fn write_worst(args: &Args, pages: &[PageResult]) -> anyhow::Result<Vec<Worst>> {
    let m = args.variant_margin;
    let mut recs: Vec<(&PageResult, &RegionRec)> = pages
        .iter()
        .flat_map(|p| {
            p.rec_alone
                .iter()
                .filter(move |r| (r.margin - m).abs() < 1e-9)
                .map(move |r| (p, r))
        })
        .collect();
    recs.sort_by(|a, b| {
        b.1.cer_clean
            .total_cmp(&a.1.cer_clean)
            .then(b.1.reference.len().cmp(&a.1.reference.len()))
    });
    let dir = args.out.join(format!("{}-worst", args.label));
    std::fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for (rank, (p, r)) in recs.into_iter().take(WORST_N).enumerate() {
        let bytes = std::fs::read(&p.path)?;
        let page = decode_limited(&bytes)?;
        let c = crop(&page, r.crop_px);
        let base = format!(
            "{:02}-{}-{}",
            rank + 1,
            sanitize(&r.page),
            sanitize(&r.ref_id)
        );
        let crop_path = dir.join(format!("{base}.png"));
        let prep_path = dir.join(format!("{base}-prepared.png"));
        c.save(&crop_path)?;
        server::ocr::recognizer::western::preprocess(&c).save(&prep_path)?;
        out.push(Worst {
            rank: rank + 1,
            page: r.page.clone(),
            ref_id: r.ref_id.clone(),
            kind: r.kind.clone(),
            colour_case: r.colour_case.clone(),
            reference: r.reference.clone(),
            produced: r.clean.clone(),
            produced_raw: r.raw.clone(),
            cer: r.cer_clean,
            wer: r.wer_clean,
            crop_image: crop_path.display().to_string(),
            prepared_image: prep_path.display().to_string(),
        });
    }
    Ok(out)
}
