//! Candidate comparison: recognition alone (A), detection alone (B), the
//! "tap one balloon" path end to end (C), and timings (D, E).
//!
//! Shares the page list, truth loading, crop geometry and text scoring
//! with the baseline run in `main.rs`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::DynamicImage;
use serde::Serialize;
use server::ocr::recognizer::Recognition;
use server::util::image_decode::decode_limited;

use crate::cand::{CtdOwn, ModelMeta, ModelPaths, Ogkalu, PP_RECS, PaddleDet, PaddleDetPage};
use crate::engines::{CropRecognizer, DetBox, FolioDetector, FolioWestern, PageDetector};
use crate::split::{self, Split};
use crate::stats::{Summary, median, ms_since, summarize, timed};
use crate::tess::{TessConfig, TessEngine};
use crate::truth::{self, PageFile, RefRegion};
use crate::{
    Agg, Args, Grouped, aggregate, area, crop, det_box, grouped, grow, inter, nested_flags,
    score_pair, to_rect,
};

fn status_kb(field: &str) -> Option<u64> {
    crate::read_status_field(field)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[derive(Debug, Clone, Serialize)]
struct LoadInfo {
    name: String,
    load_ms: f64,
    rss_added_mb: f64,
    meta: Option<ModelMeta>,
    note: String,
}

/// Everything loaded for a stage-2 run.
struct Engines {
    recs: Vec<crate::cand::PaddleRec>,
    ppdet: Option<PaddleDet>,
    /// Second PP det session for full pages (same model), so the crop
    /// splitter and the page detector keep separate sessions.
    ppdet_page: Option<PaddleDetPage>,
    ogk: Vec<(String, Ogkalu)>,
    ctd: Option<CtdOwn>,
    ctd_crate: Option<FolioDetector>,
    tess_today: Option<FolioWestern>,
    tess_best: Option<TessEngine>,
    loads: Vec<LoadInfo>,
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_owned())
        .filter(|x| !x.is_empty() && x != "none")
        .collect()
}

fn ctd_model_path() -> anyhow::Result<PathBuf> {
    let pin = server::ocr::models::DETECTOR_FILES[0];
    let root = server::ocr::models::hub_cache_root().map_err(|e| anyhow::anyhow!(e))?;
    let p = pin.snapshot_path(&root);
    if !p.exists() {
        anyhow::bail!(
            "comic-text-detector model not cached at {}; not downloading",
            p.display()
        );
    }
    Ok(p)
}

fn track(
    loads: &mut Vec<LoadInfo>,
    name: &str,
    f: &mut dyn FnMut() -> anyhow::Result<Option<ModelMeta>>,
    note: &str,
) -> anyhow::Result<()> {
    let before = status_kb("VmRSS:").unwrap_or(0);
    let t = Instant::now();
    let meta = f()?;
    let load_ms = ms_since(t);
    let after = status_kb("VmRSS:").unwrap_or(0);
    loads.push(LoadInfo {
        name: name.into(),
        load_ms,
        rss_added_mb: (after as f64 - before as f64) / 1024.0,
        meta,
        note: note.into(),
    });
    Ok(())
}

fn load_engines(args: &Args, rt: &tokio::runtime::Runtime) -> anyhow::Result<Engines> {
    let models = ModelPaths {
        root: args
            .models
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--models is required for the candidate runs"))?,
    };
    let th = args.threads;
    let mut loads = Vec::new();

    let rec_names = list(&args.recs);
    let det_names = list(&args.dets);
    let mut recs = Vec::new();
    for n in &rec_names {
        if n.starts_with("pp-") {
            let mut slot = None;
            track(
                &mut loads,
                n,
                &mut || {
                    let r = crate::cand::PaddleRec::load(n, &models.rec_dir(n), th)?;
                    let m = r.meta.clone();
                    slot = Some(r);
                    Ok(Some(m))
                },
                "session build",
            )?;
            recs.extend(slot);
        }
    }
    let need_ppdet = recs.iter().len() > 0 || det_names.iter().any(|d| d.starts_with("ppdet"));
    let mut ppdet = None;
    if need_ppdet {
        track(
            &mut loads,
            "pp-det (crop splitter)",
            &mut || {
                let d = PaddleDet::load(&models.det_dir(), th)?;
                let m = d.meta.clone();
                ppdet = Some(d);
                Ok(Some(m))
            },
            "session build",
        )?;
    }
    let mut ppdet_page = None;
    if let Some(d) = det_names.iter().find(|d| d.starts_with("ppdet")) {
        let long: u32 = d.trim_start_matches("ppdet").parse().unwrap_or(960);
        track(
            &mut loads,
            "pp-det (page)",
            &mut || {
                let det = PaddleDet::load(&models.det_dir(), th)?;
                let m = det.meta.clone();
                ppdet_page = Some(PaddleDetPage { det, long });
                Ok(Some(m))
            },
            "session build",
        )?;
    }
    let mut ogk = Vec::new();
    for (n, size) in [("ogk-s", "small"), ("ogk-m", "medium")] {
        if det_names.iter().any(|d| d == n) {
            let mut slot = None;
            let res = track(
                &mut loads,
                n,
                &mut || {
                    let o = Ogkalu::load(n, &models.ogkalu(size), th, args.ogk_threshold)?;
                    let m = o.meta.clone();
                    slot = Some(o);
                    Ok(Some(m))
                },
                "session build (int8)",
            );
            if let Err(e) = res {
                loads.push(LoadInfo {
                    name: n.into(),
                    load_ms: 0.0,
                    rss_added_mb: 0.0,
                    meta: None,
                    note: format!("FAILED: {e:#}"),
                });
            }
            if let Some(o) = slot {
                ogk.push((n.to_owned(), o));
            }
        }
    }
    let mut ctd = None;
    if det_names.iter().any(|d| d == "ctd") || rec_names.iter().any(|_| false) {
        let p = ctd_model_path()?;
        track(
            &mut loads,
            "ctd (own session)",
            &mut || {
                let c = CtdOwn::load(&p, th)?;
                let m = c.meta.clone();
                ctd = Some(c);
                Ok(Some(m))
            },
            "session build",
        )?;
    }
    let mut ctd_crate = None;
    if det_names.iter().any(|d| d == "ctd-crate") {
        track(
            &mut loads,
            "ctd-crate (Folio singleton)",
            &mut || {
                let (d, _) = rt.block_on(FolioDetector::load())?;
                ctd_crate = Some(d);
                Ok(None)
            },
            "ensure_pinned + session build, ONNX Runtime default threads",
        )?;
    }
    let mut tess_today = None;
    let mut tess_best = None;
    if rec_names.iter().any(|r| r == "tess-today") {
        track(
            &mut loads,
            "tess-today",
            &mut || {
                let (w, _) = rt.block_on(FolioWestern::load())?;
                tess_today = Some(w);
                Ok(None)
            },
            "Tesseract init",
        )?;
    }
    if rec_names.iter().any(|r| r == "tess-best") {
        let tessdata = crate::tessdata_dir()?;
        let sys = args
            .system_tessdata
            .join("eng.traineddata")
            .exists()
            .then_some(args.system_tessdata.as_path());
        let cfg = TessConfig::variants(&tessdata, sys)
            .into_iter()
            .find(|c| c.name == args.best_tess)
            .ok_or_else(|| anyhow::anyhow!("no Tesseract variant {}", args.best_tess))?;
        let work = args.out.join(format!("{}-work", args.label));
        track(
            &mut loads,
            "tess-best",
            &mut || {
                tess_best = Some(TessEngine::new(cfg.clone(), &work)?);
                Ok(None)
            },
            "Tesseract init",
        )?;
    }
    Ok(Engines {
        recs,
        ppdet,
        ppdet_page,
        ogk,
        ctd,
        ctd_crate,
        tess_today,
        tess_best,
        loads,
    })
}

/// Print every candidate model's input/output metadata.
#[expect(clippy::print_stdout, reason = "probe output for the operator")]
pub(crate) fn probe(models: &Path, threads: usize) -> anyhow::Result<()> {
    let mp = ModelPaths {
        root: models.to_path_buf(),
    };
    let mut paths: Vec<PathBuf> = PP_RECS
        .iter()
        .map(|n| mp.rec_dir(n).join("inference.onnx"))
        .collect();
    paths.push(mp.det_dir().join("inference.onnx"));
    paths.push(mp.ogkalu("small"));
    paths.push(mp.ogkalu("medium"));
    paths.push(ctd_model_path()?);
    for p in paths {
        match crate::cand::load_session(&p, threads) {
            Ok((_, m)) => println!("{}", serde_json::to_string_pretty(&m)?),
            Err(e) => println!("{}: FAILED {e:#}", p.display()),
        }
    }
    for n in PP_RECS {
        let d = crate::cand::read_dict(&mp.rec_dir(n).join("inference.yml"))?;
        println!(
            "{n}: dictionary {} entries; first {:?}; last {:?}",
            d.len(),
            &d[..5.min(d.len())],
            d.last()
        );
    }
    Ok(())
}

// ─── units: what one detector box set means as "a balloon" ──────────

/// The boxes a detector offers as tap targets / balloon units.
fn units(det: &str, unit: &str, boxes: &[DetBox]) -> Vec<[f64; 4]> {
    match (det, unit) {
        ("ctd" | "ctd-crate", "panel") => {
            let nested = nested_flags(boxes);
            boxes
                .iter()
                .zip(nested)
                .filter(|(_, n)| !n)
                .map(|(b, _)| det_box(b))
                .collect()
        }
        ("ctd" | "ctd-crate", _) => boxes.iter().map(det_box).collect(),
        (_, "text_bubble") => boxes
            .iter()
            .filter(|b| b.class == 1 || b.class == 2)
            .map(det_box)
            .collect(),
        (_, "bubble") => boxes
            .iter()
            .filter(|b| b.class == 0 || b.class == 2)
            .map(det_box)
            .collect(),
        (_, "lines") => boxes.iter().map(det_box).collect(),
        (_, "blocks") => blocks(&boxes.iter().map(det_box).collect::<Vec<_>>())
            .into_iter()
            .map(|(b, _)| b)
            .collect(),
        _ => boxes.iter().map(det_box).collect(),
    }
}

/// Group text lines into blocks: two lines join when they overlap
/// horizontally and the vertical gap is under one line height. Returns
/// each block's box and its member line indices (reading order).
fn blocks(lines: &[[f64; 4]]) -> Vec<([f64; 4], Vec<usize>)> {
    let n = lines.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (lines[i], lines[j]);
            let h = (a[3] - a[1]).min(b[3] - b[1]);
            let xo = a[2].min(b[2]) - a[0].max(b[0]);
            let gap = (a[1].max(b[1]) - a[3].min(b[3])).max(0.0);
            if xo > 0.0 && gap < h {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups
        .into_values()
        .map(|mut idx| {
            idx.sort_by(|&a, &b| lines[a][1].total_cmp(&lines[b][1]));
            let mut b = lines[idx[0]];
            for &i in &idx {
                b = [
                    b[0].min(lines[i][0]),
                    b[1].min(lines[i][1]),
                    b[2].max(lines[i][2]),
                    b[3].max(lines[i][3]),
                ];
            }
            (b, idx)
        })
        .collect()
}

// ─── B: detection classification ─────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
struct DetCounts {
    refs: usize,
    found: usize,
    split: usize,
    merged: usize,
    partial: usize,
    missed: usize,
    boxes: usize,
    boxes_matching_nothing: usize,
}

/// Per reference region, against a set of unit boxes:
/// - found: one box covers at least 60% of the reference box and covers
///   no other reference by 50% or more;
/// - merged: the box covering it also covers another reference by 50%+;
/// - split: no single box covers 60%, but boxes lying mostly (50%+ of
///   their area) inside the reference, grown by 10%, cover 60% together;
/// - partial: something overlaps but none of the above;
/// - missed: no box overlaps the reference box at all.
///
/// A box "matches nothing" when it does not overlap any reference box
/// grown by 10%. Reference boxes are approximate.
fn classify(refs: &[[f64; 4]], units: &[[f64; 4]], c: &mut DetCounts) {
    c.refs += refs.len();
    c.boxes += units.len();
    for (ri, r) in refs.iter().enumerate() {
        let ra = area(*r).max(1.0);
        let g = grow(*r, 0.1);
        let cover = |u: &[f64; 4]| inter(*r, *u) / ra;
        if !units.iter().any(|u| inter(*r, *u) > 0.0) {
            c.missed += 1;
            continue;
        }
        if let Some(best) = units.iter().max_by(|a, b| cover(a).total_cmp(&cover(b)))
            && cover(best) >= 0.6
        {
            let other = refs
                .iter()
                .enumerate()
                .any(|(oi, o)| oi != ri && inter(*o, *best) / area(*o).max(1.0) >= 0.5);
            if other {
                c.merged += 1;
            } else {
                c.found += 1;
            }
            continue;
        }
        let parts: f64 = units
            .iter()
            .filter(|u| inter(g, **u) / area(**u).max(1.0) >= 0.5)
            .map(cover)
            .sum();
        if parts >= 0.6 {
            c.split += 1;
        } else {
            c.partial += 1;
        }
    }
    c.boxes_matching_nothing += units
        .iter()
        .filter(|u| !refs.iter().any(|r| inter(grow(*r, 0.1), **u) > 0.0))
        .count();
}

// ─── records ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct Rec {
    page: String,
    ref_id: String,
    kind: String,
    colour_case: String,
    reference: String,
    engine: String,
    raw: String,
    clean: String,
    cer_raw: Option<f64>,
    cer_clean: Option<f64>,
    wer_raw: Option<f64>,
    wer_clean: Option<f64>,
    lines: usize,
    ms: f64,
    crop_px: (u32, u32, u32, u32),
    note: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct PageOut {
    key: String,
    width: u32,
    height: u32,
    det_ms: BTreeMap<String, f64>,
    det_boxes: BTreeMap<String, Vec<DetBox>>,
    split_ms: BTreeMap<String, Vec<f64>>,
    /// Per split: (crops where it found no lines and used the whole crop, crops).
    split_fallbacks: BTreeMap<String, (usize, usize)>,
    ctd_line_fallbacks: usize,
    ctd_line_uses: usize,
    recog: Vec<Rec>,
    taps: Vec<Rec>,
}

#[expect(clippy::too_many_arguments, reason = "one record, many fields")]
fn rec_record(
    page: &str,
    r: &RefRegion,
    reference: &str,
    engine: String,
    rec: &Recognition,
    lines: usize,
    ms: f64,
    rect: (u32, u32, u32, u32),
    note: String,
) -> Rec {
    let (cl, _) = crate::clean(rec.clone());
    let (cr, wr) = score_pair(reference, &rec.text);
    let (cc, wc) = score_pair(reference, &cl.text);
    Rec {
        page: page.into(),
        ref_id: r.id.clone(),
        kind: r.kind.clone(),
        colour_case: r.colour_case.clone(),
        reference: crate::score::display(reference),
        engine,
        raw: rec.text.clone(),
        clean: cl.text,
        cer_raw: Some(cr),
        cer_clean: Some(cc),
        wer_raw: Some(wr),
        wer_clean: Some(wc),
        lines,
        ms,
        crop_px: rect,
        note,
    }
}

fn missed_record(page: &str, r: &RefRegion, reference: &str, engine: String, note: &str) -> Rec {
    Rec {
        page: page.into(),
        ref_id: r.id.clone(),
        kind: r.kind.clone(),
        colour_case: r.colour_case.clone(),
        reference: crate::score::display(reference),
        engine,
        raw: String::new(),
        clean: String::new(),
        cer_raw: None,
        cer_clean: None,
        wer_raw: None,
        wer_clean: None,
        lines: 0,
        ms: 0.0,
        crop_px: (0, 0, 0, 0),
        note: note.into(),
    }
}

/// Recognise a crop with a PP recogniser after splitting it into lines.
fn pp_balloon(
    rec: &mut crate::cand::PaddleRec,
    crop_img: &DynamicImage,
    rects: &[split::Rect],
    batched: bool,
) -> anyhow::Result<(Recognition, usize)> {
    let lines = split::cut(crop_img, rects);
    let outs = if batched {
        rec.run(&lines)?
    } else {
        let mut v = Vec::new();
        for l in &lines {
            v.extend(rec.run(std::slice::from_ref(l))?);
        }
        v
    };
    let texts: Vec<&str> = outs
        .iter()
        .map(|(t, _)| t.as_str())
        .filter(|t| !t.is_empty())
        .collect();
    let conf = if outs.is_empty() {
        0.0
    } else {
        outs.iter().map(|(_, c)| *c).sum::<f32>() / outs.len() as f32
    };
    Ok((
        Recognition {
            text: texts.join("\n"),
            confidence: conf,
            words: None,
        },
        lines.len(),
    ))
}

/// Line rects for a crop under each split. `ctd_lines` are the page's
/// comic-text-detector class-1 boxes (page px).
fn split_rects(
    s: Split,
    eng: &mut Engines,
    crop_img: &DynamicImage,
    rect: (u32, u32, u32, u32),
    ctd_page_lines: &[[f64; 4]],
    pp_page_lines: &[[f64; 4]],
) -> anyhow::Result<(Vec<split::Rect>, bool)> {
    let local = |lines: &[[f64; 4]]| -> Vec<[f32; 4]> {
        lines
            .iter()
            .map(|b| {
                [
                    (b[0] - f64::from(rect.0)) as f32,
                    (b[1] - f64::from(rect.1)) as f32,
                    (b[2] - f64::from(rect.0)) as f32,
                    (b[3] - f64::from(rect.1)) as f32,
                ]
            })
            .collect()
    };
    Ok(match s {
        Split::PpDet => {
            split::ppdet_lines(eng.ppdet.as_mut().expect("pp det loaded"), crop_img, None)?
        }
        Split::PpDet2x => split::ppdet_lines(
            eng.ppdet.as_mut().expect("pp det loaded"),
            crop_img,
            Some(2.0),
        )?,
        Split::Profile => (split::profile_lines(crop_img), false),
        Split::None => (split::none_lines(crop_img), false),
        Split::CtdLines => split::ctd_lines(&local(ctd_page_lines), crop_img),
        Split::PageLines => split::ctd_lines(&local(pp_page_lines), crop_img),
    })
}

// ─── the accuracy run (A, B, C) ──────────────────────────────────────

fn run_detectors(
    eng: &mut Engines,
    page: &DynamicImage,
    names: &[String],
    out: &mut PageOut,
) -> anyhow::Result<()> {
    for d in names {
        let res: Option<(anyhow::Result<Vec<DetBox>>, f64)> = match d.as_str() {
            "ctd" => eng.ctd.as_mut().map(|c| timed(|| c.detect(page))),
            "ctd-crate" => eng.ctd_crate.as_mut().map(|c| timed(|| c.detect(page))),
            "ogk-s" | "ogk-m" => eng
                .ogk
                .iter_mut()
                .find(|(n, _)| n == d)
                .map(|(_, o)| timed(|| o.detect(page))),
            x if x.starts_with("ppdet") => {
                let long: u32 = x.trim_start_matches("ppdet").parse().unwrap_or(960);
                eng.ppdet_page.as_mut().map(|p| {
                    p.long = long;
                    timed(|| p.detect(page))
                })
            }
            _ => None,
        };
        if let Some((boxes, ms)) = res {
            out.det_ms.insert(d.clone(), ms);
            out.det_boxes.insert(d.clone(), boxes?);
        }
    }
    Ok(())
}

/// The unit sets section B and C use, per detector.
fn unit_names(det: &str) -> &'static [&'static str] {
    match det {
        "ctd" | "ctd-crate" => &["panel", "all"],
        "ogk-s" | "ogk-m" => &["text_bubble", "bubble"],
        _ => &["lines", "blocks"],
    }
}

fn accuracy_page(
    args: &Args,
    eng: &mut Engines,
    pf: &PageFile,
    refs: &[RefRegion],
) -> anyhow::Result<PageOut> {
    let bytes = std::fs::read(&pf.path)?;
    let page = decode_limited(&bytes)?;
    let (pw, ph) = (page.width(), page.height());
    let mut out = PageOut {
        key: pf.key.clone(),
        width: pw,
        height: ph,
        ..Default::default()
    };
    let det_names = list(&args.dets);
    run_detectors(eng, &page, &det_names, &mut out)?;
    let ctd_page_lines: Vec<[f64; 4]> = out
        .det_boxes
        .get("ctd")
        .map(|b| b.iter().filter(|x| x.class == 1).map(det_box).collect())
        .unwrap_or_default();
    let pp_page_lines: Vec<[f64; 4]> = out
        .det_boxes
        .get("ppdet960")
        .map(|b| b.iter().map(det_box).collect())
        .unwrap_or_default();
    let ref_px: Vec<[f64; 4]> = refs.iter().map(|r| r.bbox_px(pw, ph)).collect();
    let refs_norm: Vec<String> = refs
        .iter()
        .map(|r| crate::score::normalize(&r.text, true))
        .collect();

    // A: recognition alone on the reference box + margin.
    let splits: Vec<Split> = Split::ALL
        .into_iter()
        .filter(|s| list(&args.splits).iter().any(|n| n == s.name()))
        .collect();
    // Two crops per reference: its rough box + margin, and ("@tight") the
    // ogkalu-medium text box that covers it best, when one covers at
    // least 30% of it and lies mostly (50%+) inside it grown by 25%: a
    // tight crop as a detector would give, without the rough box's art.
    let tight_src: Vec<[f64; 4]> = out
        .det_boxes
        .get("ogk-m")
        .map(|b| {
            b.iter()
                .filter(|x| x.class == 1 || x.class == 2)
                .map(det_box)
                .collect()
        })
        .unwrap_or_default();
    let mut jobs: Vec<(usize, split::Rect, &'static str)> = Vec::new();
    for (ri, rp) in ref_px.iter().enumerate() {
        if let Some(rect) = to_rect(grow(*rp, args.variant_margin), pw, ph) {
            jobs.push((ri, rect, ""));
        }
        let g = grow(*rp, 0.25);
        let ra = area(*rp).max(1.0);
        let best = tight_src
            .iter()
            .filter(|b| inter(g, **b) / area(**b).max(1.0) >= 0.5)
            .max_by(|a, b| inter(*rp, **a).total_cmp(&inter(*rp, **b)))
            .filter(|b| inter(*rp, **b) / ra >= 0.3);
        if let Some(rect) = best.and_then(|b| to_rect(*b, pw, ph)) {
            jobs.push((ri, rect, "@tight"));
        }
    }
    for (ri, rect, sfx) in jobs {
        let r = &refs[ri];
        let c = crop(&page, rect);
        if let Some(t) = eng.tess_today.as_mut() {
            let (o, ms) = timed(|| t.recognize(&c));
            out.recog.push(rec_record(
                &pf.key,
                r,
                &refs_norm[ri],
                format!("tess-today{sfx}"),
                &o?.rec,
                0,
                ms,
                rect,
                String::new(),
            ));
        }
        if let Some(t) = eng.tess_best.as_mut() {
            let (o, ms) = timed(|| t.recognize(&c));
            out.recog.push(rec_record(
                &pf.key,
                r,
                &refs_norm[ri],
                format!("tess-best ({}){sfx}", t.cfg.name),
                &o?.rec,
                0,
                ms,
                rect,
                String::new(),
            ));
        }
        for &s in &splits {
            let ((rects, fell_back), ms) = {
                let t = Instant::now();
                let v = split_rects(s, eng, &c, rect, &ctd_page_lines, &pp_page_lines)?;
                (v, ms_since(t))
            };
            out.split_ms.entry(s.name().into()).or_default().push(ms);
            let fb = out.split_fallbacks.entry(s.name().into()).or_insert((0, 0));
            fb.1 += 1;
            fb.0 += usize::from(fell_back);
            if s == Split::CtdLines {
                out.ctd_line_uses += 1;
                out.ctd_line_fallbacks += usize::from(fell_back);
            }
            let note = if fell_back {
                "no line boxes: whole crop".to_owned()
            } else {
                String::new()
            };
            for rec in eng.recs.iter_mut() {
                let t = Instant::now();
                let (recog, n) = pp_balloon(rec, &c, &rects, true)?;
                let ms_r = ms_since(t);
                out.recog.push(rec_record(
                    &pf.key,
                    r,
                    &refs_norm[ri],
                    format!("{}/{}{sfx}", rec.name, s.name()),
                    &recog,
                    n,
                    ms_r,
                    rect,
                    note.clone(),
                ));
            }
        }
    }

    // C: tap one balloon at the centre of each reference box.
    taps(
        args,
        eng,
        &page,
        &out.det_boxes.clone(),
        refs,
        &ref_px,
        &refs_norm,
        &mut out,
    )?;
    Ok(out)
}

fn contains(b: [f64; 4], p: (f64, f64)) -> bool {
    p.0 >= b[0] && p.0 <= b[2] && p.1 >= b[1] && p.1 <= b[3]
}

#[expect(
    clippy::too_many_arguments,
    reason = "page context for the tap simulation"
)]
fn taps(
    args: &Args,
    eng: &mut Engines,
    page: &DynamicImage,
    det_boxes: &BTreeMap<String, Vec<DetBox>>,
    refs: &[RefRegion],
    ref_px: &[[f64; 4]],
    refs_norm: &[String],
    out: &mut PageOut,
) -> anyhow::Result<()> {
    let (pw, ph) = (page.width(), page.height());
    let best_split = Split::ALL
        .into_iter()
        .find(|s| s.name() == args.best_split)
        .unwrap_or(Split::Profile);
    let best_pp = eng.recs.iter().position(|r| r.name == args.best_pp);
    let ctd_page_lines: Vec<[f64; 4]> = det_boxes
        .get("ctd")
        .map(|b| b.iter().filter(|x| x.class == 1).map(det_box).collect())
        .unwrap_or_default();
    let pp_page_lines: Vec<[f64; 4]> = det_boxes
        .get("ppdet960")
        .map(|b| b.iter().map(det_box).collect())
        .unwrap_or_default();
    let tap_splits: Vec<Split> = Split::ALL
        .into_iter()
        .filter(|s| list(&args.tap_splits).iter().any(|n| n == s.name()))
        .collect();
    let _ = best_split;
    // (combo name, detector, unit, recogniser, split)
    let mut combos: Vec<(String, &str, &str, &str, Split)> = Vec::new();
    for d in det_boxes.keys() {
        let d = d.as_str();
        match d {
            "ctd" | "ctd-crate" => {
                if eng.tess_today.is_some() {
                    combos.push((
                        format!("{d}(panel) + tess-today"),
                        d,
                        "panel",
                        "tess-today",
                        Split::None,
                    ));
                }
                for &sp in &tap_splits {
                    combos.push((
                        format!("{d}(panel) + {}/{}", args.best_pp, sp.name()),
                        d,
                        "panel",
                        "pp",
                        sp,
                    ));
                }
            }
            "ogk-s" | "ogk-m" => {
                for u in ["text_bubble", "bubble"] {
                    for &sp in &tap_splits {
                        combos.push((
                            format!("{d}({u}) + {}/{}", args.best_pp, sp.name()),
                            d,
                            u,
                            "pp",
                            sp,
                        ));
                    }
                }
                if eng.tess_today.is_some() {
                    combos.push((
                        format!("{d}(text_bubble) + tess-today"),
                        d,
                        "text_bubble",
                        "tess-today",
                        Split::None,
                    ));
                }
            }
            _ => combos.push((
                format!("{d}(blocks) + {} on its own lines", args.best_pp),
                d,
                "blocks",
                "pp-lines",
                Split::None,
            )),
        }
    }
    for (name, d, unit, recn, sp) in combos {
        let boxes = &det_boxes[d];
        let line_boxes: Vec<[f64; 4]> = boxes.iter().map(det_box).collect();
        let blks = if unit == "blocks" {
            blocks(&line_boxes)
        } else {
            Vec::new()
        };
        let us = units(d, unit, boxes);
        for (ri, r) in refs.iter().enumerate() {
            let rb = ref_px[ri];
            let p = ((rb[0] + rb[2]) / 2.0, (rb[1] + rb[3]) / 2.0);
            // The reader picks the tapped box; among several, the smallest
            // (ogkalu / PP) or the enclosing panel box (comic-text-detector).
            let pick = us
                .iter()
                .enumerate()
                .filter(|(_, u)| contains(**u, p))
                .min_by(|a, b| area(*a.1).total_cmp(&area(*b.1)))
                .map(|(i, u)| (i, *u));
            let Some((ui, u)) = pick else {
                out.taps.push(missed_record(
                    &out.key,
                    r,
                    &refs_norm[ri],
                    name.clone(),
                    "no box under the tap",
                ));
                continue;
            };
            let Some(rect) = to_rect(u, pw, ph) else {
                out.taps.push(missed_record(
                    &out.key,
                    r,
                    &refs_norm[ri],
                    name.clone(),
                    "empty box",
                ));
                continue;
            };
            let c = crop(page, rect);
            let t = Instant::now();
            let (recog, n) = match recn {
                "tess-today" => (
                    eng.tess_today.as_mut().expect("loaded").recognize(&c)?.rec,
                    0,
                ),
                "pp" => {
                    let Some(bi) = best_pp else { continue };
                    let (rects, _) =
                        split_rects(sp, eng, &c, rect, &ctd_page_lines, &pp_page_lines)?;
                    pp_balloon(&mut eng.recs[bi], &c, &rects, true)?
                }
                _ => {
                    let Some(bi) = best_pp else { continue };
                    let members = &blks[ui].1;
                    let rects: Vec<split::Rect> = members
                        .iter()
                        .filter_map(|&li| {
                            let l = line_boxes[li];
                            to_rect(
                                [
                                    l[0] - f64::from(rect.0),
                                    l[1] - f64::from(rect.1),
                                    l[2] - f64::from(rect.0),
                                    l[3] - f64::from(rect.1),
                                ],
                                rect.2,
                                rect.3,
                            )
                        })
                        .collect();
                    pp_balloon(&mut eng.recs[bi], &c, &rects, true)?
                }
            };
            let ms = ms_since(t);
            out.taps.push(rec_record(
                &out.key,
                r,
                &refs_norm[ri],
                name.clone(),
                &recog,
                n,
                ms,
                rect,
                String::new(),
            ));
        }
    }
    Ok(())
}

// ─── D: timings ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
struct TimingOut {
    decode: Vec<f64>,
    detect: BTreeMap<String, (Option<f64>, Vec<f64>)>,
    /// Per recogniser: per-balloon (batched) ms, per-balloon with one run
    /// per line ms, per-line ms (single runs), split ms.
    balloon_batched: BTreeMap<String, Vec<f64>>,
    balloon_single: BTreeMap<String, Vec<f64>>,
    per_line: BTreeMap<String, Vec<f64>>,
    first_balloon: BTreeMap<String, f64>,
    split_profile: Vec<f64>,
    /// Whole page per combo: decode + detect + every unit, measured.
    whole_page: BTreeMap<String, Vec<f64>>,
    units_per_page: BTreeMap<String, Vec<usize>>,
    tap_fresh: BTreeMap<String, Vec<f64>>,
    tap_detected: BTreeMap<String, Vec<f64>>,
}

fn timing_page(
    args: &Args,
    eng: &mut Engines,
    pf: &PageFile,
    refs: Option<&[RefRegion]>,
    t: &mut TimingOut,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(&pf.path)?;
    let mut page = None;
    for _ in 0..=args.warm {
        let (p, ms) = timed(|| decode_limited(&bytes));
        t.decode.push(ms);
        page = Some(p?);
    }
    let page = page.expect("decoded");
    let (pw, ph) = (page.width(), page.height());
    let det_names = list(&args.dets);
    let mut boxes_by: BTreeMap<String, Vec<DetBox>> = BTreeMap::new();
    for d in &det_names {
        let mut first = None;
        let mut warm = Vec::new();
        let mut last = Vec::new();
        let reps = if d == "ctd-crate" {
            1 + args.warm.min(2)
        } else {
            1 + args.warm
        };
        for _ in 0..reps {
            let mut po = PageOut::default();
            run_detectors(eng, &page, std::slice::from_ref(d), &mut po)?;
            let Some(ms) = po.det_ms.get(d).copied() else {
                break;
            };
            if first.is_none() {
                first = Some(ms);
            } else {
                warm.push(ms);
            }
            last = po.det_boxes.remove(d).unwrap_or_default();
        }
        let e = t.detect.entry(d.clone()).or_default();
        if e.0.is_none() {
            e.0 = first;
        }
        e.1.extend(warm);
        boxes_by.insert(d.clone(), last);
    }

    // Balloons to time: the reference boxes when there is a reference,
    // else comic-text-detector's panel boxes.
    let balloons: Vec<(u32, u32, u32, u32)> = match refs {
        Some(r) => r
            .iter()
            .filter_map(|x| to_rect(grow(x.bbox_px(pw, ph), args.variant_margin), pw, ph))
            .collect(),
        None => boxes_by
            .values()
            .next()
            .map(|b| {
                b.iter()
                    .filter_map(|x| to_rect(det_box(x), pw, ph))
                    .collect()
            })
            .unwrap_or_default(),
    };
    for &rect in &balloons {
        let c = crop(&page, rect);
        let (rects, ms) = timed(|| split::profile_lines(&c));
        t.split_profile.push(ms);
        for _ in 0..=args.rec_warm {
            for rec in eng.recs.iter_mut() {
                let (r, ms) = timed(|| pp_balloon(rec, &c, &rects, true));
                r?;
                if t.first_balloon.contains_key(&rec.name) {
                    t.balloon_batched
                        .entry(rec.name.clone())
                        .or_default()
                        .push(ms);
                } else {
                    t.first_balloon.insert(rec.name.clone(), ms);
                }
                let lines = split::cut(&c, &rects);
                let mut total = 0.0;
                for l in &lines {
                    let (r, ms) = timed(|| rec.run(std::slice::from_ref(l)));
                    r?;
                    total += ms;
                    t.per_line.entry(rec.name.clone()).or_default().push(ms);
                }
                t.balloon_single
                    .entry(rec.name.clone())
                    .or_default()
                    .push(total);
            }
            if let Some(tt) = eng.tess_today.as_mut() {
                let (r, ms) = timed(|| tt.recognize(&c));
                r?;
                if t.first_balloon.contains_key("tess-today") {
                    t.balloon_batched
                        .entry("tess-today".into())
                        .or_default()
                        .push(ms);
                } else {
                    t.first_balloon.insert("tess-today".into(), ms);
                }
            }
            if let Some(tb) = eng.tess_best.as_mut() {
                let (r, ms) = timed(|| tb.recognize(&c));
                r?;
                t.balloon_batched
                    .entry("tess-best".into())
                    .or_default()
                    .push(ms);
            }
        }
    }

    // Whole page and tap, per combo, measured end to end on this page.
    let best_split = Split::ALL
        .into_iter()
        .find(|s| s.name() == args.best_split)
        .unwrap_or(Split::Profile);
    let best_pp = eng.recs.iter().position(|r| r.name == args.best_pp);
    for d in &det_names {
        let unit = match d.as_str() {
            "ctd" | "ctd-crate" => "panel",
            "ogk-s" | "ogk-m" => "text_bubble",
            _ => "blocks",
        };
        let mut recogs: Vec<&str> = Vec::new();
        if best_pp.is_some() {
            recogs.push("pp");
        }
        if eng.tess_today.is_some() && (d.starts_with("ctd")) {
            recogs.push("tess-today");
        }
        for recn in recogs {
            let combo = format!(
                "{d}({unit}) + {}",
                if recn == "pp" {
                    format!(
                        "{}/{}",
                        args.best_pp,
                        if unit == "blocks" {
                            "own lines"
                        } else {
                            best_split.name()
                        }
                    )
                } else {
                    recn.to_owned()
                }
            );
            let t0 = Instant::now();
            let p2 = decode_limited(&bytes)?;
            let decode_ms = ms_since(t0);
            let mut po = PageOut::default();
            run_detectors(eng, &p2, std::slice::from_ref(d), &mut po)?;
            let det_ms = po.det_ms.get(d).copied().unwrap_or(0.0);
            let boxes = po.det_boxes.remove(d).unwrap_or_default();
            let line_boxes: Vec<[f64; 4]> = boxes.iter().map(det_box).collect();
            let blks = if unit == "blocks" {
                blocks(&line_boxes)
            } else {
                Vec::new()
            };
            let us = units(d, unit, &boxes);
            let ctd_lines: Vec<[f64; 4]> =
                boxes.iter().filter(|x| x.class == 1).map(det_box).collect();
            let mut unit_ms = Vec::new();
            for (ui, u) in us.iter().enumerate() {
                let Some(rect) = to_rect(*u, pw, ph) else {
                    continue;
                };
                let tu = Instant::now();
                let c = crop(&p2, rect);
                match recn {
                    "tess-today" => {
                        eng.tess_today.as_mut().expect("loaded").recognize(&c)?;
                    }
                    _ => {
                        let bi = best_pp.expect("checked");
                        let rects = if unit == "blocks" {
                            blks[ui]
                                .1
                                .iter()
                                .filter_map(|&li| {
                                    let l = line_boxes[li];
                                    to_rect(
                                        [
                                            l[0] - f64::from(rect.0),
                                            l[1] - f64::from(rect.1),
                                            l[2] - f64::from(rect.0),
                                            l[3] - f64::from(rect.1),
                                        ],
                                        rect.2,
                                        rect.3,
                                    )
                                })
                                .collect()
                        } else {
                            split_rects(best_split, eng, &c, rect, &ctd_lines, &[])?.0
                        };
                        pp_balloon(&mut eng.recs[bi], &c, &rects, true)?;
                    }
                }
                unit_ms.push(ms_since(tu));
            }
            let total = ms_since(t0);
            t.whole_page.entry(combo.clone()).or_default().push(total);
            t.units_per_page
                .entry(combo.clone())
                .or_default()
                .push(us.len());
            for u in &unit_ms {
                t.tap_fresh
                    .entry(combo.clone())
                    .or_default()
                    .push(decode_ms + det_ms + u);
                t.tap_detected.entry(combo.clone()).or_default().push(*u);
            }
        }
    }
    Ok(())
}

// ─── output ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct Stage2 {
    label: String,
    env: crate::Env,
    threads: usize,
    loads: Vec<LoadInfo>,
    rec_classes: Vec<(String, usize, Option<usize>)>,
    vm_hwm_mb: Option<f64>,
    pages: Vec<PageOut>,
    a_recognition: BTreeMap<String, Grouped>,
    a_recognition_raw: BTreeMap<String, Agg>,
    a_time: BTreeMap<String, Summary>,
    split_time: BTreeMap<String, Summary>,
    ctd_line_fallbacks: (usize, usize),
    b_detection: BTreeMap<String, DetCounts>,
    det_time: BTreeMap<String, Summary>,
    c_taps: BTreeMap<String, Grouped>,
    c_taps_raw: BTreeMap<String, Agg>,
    timing: Option<TimingOut>,
    timing_summary: BTreeMap<String, (Option<f64>, Summary)>,
    worst: BTreeMap<String, Vec<WorstRow>>,
}

#[derive(Debug, Clone, Serialize)]
struct WorstRow {
    page: String,
    ref_id: String,
    cer: f64,
    reference: String,
    produced: String,
    image: String,
}

fn rec_metric(r: &Rec, clean: bool) -> (Option<f64>, Option<f64>) {
    if clean {
        (r.cer_clean, r.wer_clean)
    } else {
        (r.cer_raw, r.wer_raw)
    }
}

#[derive(Debug, Clone, Serialize)]
struct Throughput {
    label: String,
    env: crate::Env,
    threads: usize,
    detector: String,
    recogniser: String,
    split: String,
    pages: usize,
    page_ms: Vec<f64>,
    page_summary: Summary,
    units: usize,
    wall_start_unix_ms: u128,
    wall_end_unix_ms: u128,
    pages_per_minute: f64,
    vm_hwm_mb: Option<f64>,
    loads: Vec<LoadInfo>,
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// One worker: every page `repeat` times, decode + detect + recognise
/// every unit, serially. Parallel workers are separate processes, each
/// with its own sessions; the wall-clock bounds let a caller add them up.
fn run_throughput(
    args: &Args,
    rt: &tokio::runtime::Runtime,
    pages: &[PageFile],
) -> anyhow::Result<()> {
    let loadavg_start = crate::loadavg();
    let mut eng = load_engines(args, rt)?;
    let d = list(&args.dets)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("--dets needs one detector"))?;
    let unit = match d.as_str() {
        "ctd" | "ctd-crate" => "panel",
        "ogk-s" | "ogk-m" => "text_bubble",
        _ => "blocks",
    };
    let best_split = Split::ALL
        .into_iter()
        .find(|s| s.name() == args.best_split)
        .unwrap_or(Split::Profile);
    let pp = eng.recs.iter().position(|r| r.name == args.best_pp);
    let use_tess = pp.is_none();
    let bytes: Vec<Vec<u8>> = pages
        .iter()
        .map(|p| std::fs::read(&p.path))
        .collect::<Result<_, _>>()?;
    let start = unix_ms();
    let mut page_ms = Vec::new();
    let mut n_units = 0;
    for _ in 0..args.repeat {
        for b in &bytes {
            let t0 = Instant::now();
            let page = decode_limited(b)?;
            let (pw, ph) = (page.width(), page.height());
            let mut po = PageOut::default();
            run_detectors(&mut eng, &page, std::slice::from_ref(&d), &mut po)?;
            let boxes = po.det_boxes.remove(&d).unwrap_or_default();
            let line_boxes: Vec<[f64; 4]> = boxes.iter().map(det_box).collect();
            let blks = if unit == "blocks" {
                blocks(&line_boxes)
            } else {
                Vec::new()
            };
            let us = units(&d, unit, &boxes);
            let ctd_lines: Vec<[f64; 4]> =
                boxes.iter().filter(|x| x.class == 1).map(det_box).collect();
            for (ui, u) in us.iter().enumerate() {
                let Some(rect) = to_rect(*u, pw, ph) else {
                    continue;
                };
                let c = crop(&page, rect);
                n_units += 1;
                if use_tess {
                    eng.tess_today
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("no recogniser loaded"))?
                        .recognize(&c)?;
                    continue;
                }
                let rects = if unit == "blocks" {
                    blks[ui]
                        .1
                        .iter()
                        .filter_map(|&li| {
                            let l = line_boxes[li];
                            to_rect(
                                [
                                    l[0] - f64::from(rect.0),
                                    l[1] - f64::from(rect.1),
                                    l[2] - f64::from(rect.0),
                                    l[3] - f64::from(rect.1),
                                ],
                                rect.2,
                                rect.3,
                            )
                        })
                        .collect()
                } else {
                    split_rects(best_split, &mut eng, &c, rect, &ctd_lines, &[])?.0
                };
                let bi = pp.expect("checked");
                pp_balloon(&mut eng.recs[bi], &c, &rects, true)?;
            }
            page_ms.push(ms_since(t0));
        }
    }
    let end = unix_ms();
    let env = crate::Env {
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        cpu_model: crate::cpu_model(),
        cpus_allowed: crate::read_status_field("Cpus_allowed_list:").unwrap_or_default(),
        available_parallelism: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        omp_num_threads: std::env::var("OMP_NUM_THREADS").ok(),
        loadavg_start,
        loadavg_end: crate::loadavg(),
        threads_after_detect: crate::read_status_field("Threads:").and_then(|v| v.parse().ok()),
        tesseract_version: tesseract_rs::TesseractAPI::version(),
        tessdata: String::new(),
    };
    let wall = (end - start) as f64;
    let res = Throughput {
        label: args.label.clone(),
        env,
        threads: args.threads,
        detector: d.clone(),
        recogniser: if use_tess {
            "tess-today".into()
        } else {
            args.best_pp.clone()
        },
        split: if unit == "blocks" {
            "own lines".into()
        } else {
            best_split.name().into()
        },
        pages: page_ms.len(),
        page_summary: summarize(&page_ms),
        page_ms,
        units: n_units,
        wall_start_unix_ms: start,
        wall_end_unix_ms: end,
        pages_per_minute: if wall > 0.0 {
            60000.0 * (bytes.len() * args.repeat) as f64 / wall
        } else {
            0.0
        },
        vm_hwm_mb: status_kb("VmHWM:").map(|k| k as f64 / 1024.0),
        loads: eng.loads.clone(),
    };
    std::fs::write(
        args.out.join(format!("{}.json", args.label)),
        serde_json::to_string_pretty(&res)?,
    )?;
    Ok(())
}

#[expect(clippy::print_stderr, reason = "command-line progress output")]
pub(crate) fn run(
    args: &Args,
    rt: &tokio::runtime::Runtime,
    pages: &[PageFile],
) -> anyhow::Result<()> {
    if args.throughput {
        return run_throughput(args, rt, pages);
    }
    let loadavg_start = crate::loadavg();
    let mut eng = load_engines(args, rt)?;
    let mut outs = Vec::new();
    let mut timing = TimingOut::default();
    for (i, pf) in pages.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, pages.len(), pf.key);
        let refs = match &args.truth {
            Some(t) => truth::load_truth(t, &pf.key)?,
            None => None,
        };
        if args.cand
            && let Some(r) = refs.as_deref()
        {
            outs.push(accuracy_page(args, &mut eng, pf, r)?);
        }
        if args.cand_timing {
            timing_page(args, &mut eng, pf, refs.as_deref(), &mut timing)?;
        }
    }

    let mut groups: BTreeMap<String, Vec<Rec>> = BTreeMap::new();
    for p in &outs {
        for r in &p.recog {
            groups.entry(r.engine.clone()).or_default().push(r.clone());
        }
    }
    let clean_for = |engine: &str| engine.starts_with("tess");
    let a_recognition = groups
        .iter()
        .map(|(k, v)| {
            let c = clean_for(k);
            (
                k.clone(),
                grouped(
                    v,
                    |r| rec_metric(r, c),
                    |r| r.kind.as_str(),
                    |r| r.colour_case.as_str(),
                ),
            )
        })
        .collect();
    let a_recognition_raw = groups
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                aggregate(v.iter().map(|r| rec_metric(r, !clean_for(k)))),
            )
        })
        .collect();
    let a_time = groups
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                summarize(&v.iter().map(|r| r.ms).collect::<Vec<_>>()),
            )
        })
        .collect();
    let mut split_all: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for p in &outs {
        for (k, v) in &p.split_ms {
            split_all.entry(k.clone()).or_default().extend(v);
        }
    }
    let split_time = split_all
        .iter()
        .map(|(k, v)| (k.clone(), summarize(v)))
        .collect();

    // B.
    let mut b_detection: BTreeMap<String, DetCounts> = BTreeMap::new();
    let mut det_all: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for p in &outs {
        let refs =
            truth::load_truth(args.truth.as_ref().expect("truth"), &p.key)?.unwrap_or_default();
        let ref_px: Vec<[f64; 4]> = refs.iter().map(|r| r.bbox_px(p.width, p.height)).collect();
        for (d, boxes) in &p.det_boxes {
            det_all.entry(d.clone()).or_default().push(p.det_ms[d]);
            for u in unit_names(d) {
                let us = units(d, u, boxes);
                classify(
                    &ref_px,
                    &us,
                    b_detection.entry(format!("{d}:{u}")).or_default(),
                );
            }
        }
    }
    let det_time = det_all
        .iter()
        .map(|(k, v)| (k.clone(), summarize(v)))
        .collect();

    // C.
    let mut tap_groups: BTreeMap<String, Vec<Rec>> = BTreeMap::new();
    for p in &outs {
        for r in &p.taps {
            tap_groups
                .entry(r.engine.clone())
                .or_default()
                .push(r.clone());
        }
    }
    let c_taps = tap_groups
        .iter()
        .map(|(k, v)| {
            let c = k.contains("tess");
            (
                k.clone(),
                grouped(
                    v,
                    |r| rec_metric(r, c),
                    |r| r.kind.as_str(),
                    |r| r.colour_case.as_str(),
                ),
            )
        })
        .collect();
    let c_taps_raw = tap_groups
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                aggregate(v.iter().map(|r| rec_metric(r, !k.contains("tess")))),
            )
        })
        .collect();

    // Worst 25 per recogniser (best split for PP, both Tesseract paths).
    let mut worst = BTreeMap::new();
    if args.cand {
        let dir = args.out.join(format!("{}-worst", args.label));
        for (k, v) in &groups {
            let keep = k.starts_with("tess")
                || k.ends_with(&format!("/{}", args.best_split))
                || k.ends_with(&format!("/{}@tight", args.best_split));
            if !keep {
                continue;
            }
            let c = clean_for(k);
            let mut v: Vec<&Rec> = v.iter().collect();
            v.sort_by(|a, b| {
                rec_metric(b, c)
                    .0
                    .unwrap_or(1.0)
                    .total_cmp(&rec_metric(a, c).0.unwrap_or(1.0))
            });
            let sub = dir.join(crate::sanitize(k));
            std::fs::create_dir_all(&sub)?;
            let mut rows = Vec::new();
            let mut index = String::new();
            for (i, r) in v.into_iter().take(25).enumerate() {
                let pf = pages.iter().find(|p| p.key == r.page).expect("page");
                let img = decode_limited(&std::fs::read(&pf.path)?)?;
                let base = format!(
                    "{:02}-{}-{}",
                    i + 1,
                    crate::sanitize(&r.page),
                    crate::sanitize(&r.ref_id)
                );
                let ip = sub.join(format!("{base}.png"));
                crop(&img, r.crop_px).save(&ip)?;
                let produced = if c { r.clean.clone() } else { r.raw.clone() };
                std::fs::write(
                    sub.join(format!("{base}.txt")),
                    format!(
                        "reference: {}\nengine ({k}): {}\nCER: {:.3}\n",
                        r.reference,
                        produced.replace('\n', " / "),
                        rec_metric(r, c).0.unwrap_or(1.0)
                    ),
                )?;
                let _ = writeln!(
                    index,
                    "{base}\tCER {:.3}\tREF {}\tGOT {}",
                    rec_metric(r, c).0.unwrap_or(1.0),
                    r.reference,
                    produced.replace('\n', " / ")
                );
                rows.push(WorstRow {
                    page: r.page.clone(),
                    ref_id: r.ref_id.clone(),
                    cer: rec_metric(r, c).0.unwrap_or(1.0),
                    reference: r.reference.clone(),
                    produced,
                    image: ip.display().to_string(),
                });
            }
            std::fs::write(sub.join("index.tsv"), index)?;
            worst.insert(k.clone(), rows);
        }
    }

    let timing_summary = if args.cand_timing {
        summarize_timing(&timing)
    } else {
        BTreeMap::new()
    };
    let env = crate::Env {
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        cpu_model: crate::cpu_model(),
        cpus_allowed: crate::read_status_field("Cpus_allowed_list:").unwrap_or_default(),
        available_parallelism: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        omp_num_threads: std::env::var("OMP_NUM_THREADS").ok(),
        loadavg_start,
        loadavg_end: crate::loadavg(),
        threads_after_detect: crate::read_status_field("Threads:").and_then(|v| v.parse().ok()),
        tesseract_version: tesseract_rs::TesseractAPI::version(),
        tessdata: crate::tessdata_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    };
    let res = Stage2 {
        label: args.label.clone(),
        env,
        threads: args.threads,
        loads: eng.loads.clone(),
        rec_classes: eng
            .recs
            .iter()
            .map(|r| (r.name.clone(), r.dict_len, r.classes_seen))
            .collect(),
        vm_hwm_mb: status_kb("VmHWM:").map(|k| k as f64 / 1024.0),
        pages: outs,
        a_recognition,
        a_recognition_raw,
        a_time,
        split_time,
        ctd_line_fallbacks: (0, 0),
        b_detection,
        det_time,
        c_taps,
        c_taps_raw,
        timing: args.cand_timing.then_some(timing),
        timing_summary,
        worst,
    };
    let mut res = res;
    res.ctd_line_fallbacks = (
        res.pages.iter().map(|p| p.ctd_line_fallbacks).sum(),
        res.pages.iter().map(|p| p.ctd_line_uses).sum(),
    );
    let json = args.out.join(format!("{}.json", args.label));
    std::fs::write(&json, serde_json::to_string_pretty(&res)?)?;
    let md = args.out.join(format!("{}.md", args.label));
    std::fs::write(&md, markdown(&res, args))?;
    eprintln!("wrote {} and {}", json.display(), md.display());
    Ok(())
}

fn summarize_timing(t: &TimingOut) -> BTreeMap<String, (Option<f64>, Summary)> {
    let mut m = BTreeMap::new();
    m.insert(
        "page decode".into(),
        (
            t.decode.first().copied(),
            summarize(t.decode.get(1..).unwrap_or_default()),
        ),
    );
    for (d, (first, warm)) in &t.detect {
        m.insert(format!("detect: {d}"), (*first, summarize(warm)));
    }
    m.insert("split: profile".into(), (None, summarize(&t.split_profile)));
    for (k, v) in &t.balloon_batched {
        m.insert(
            format!("balloon (lines batched): {k}"),
            (t.first_balloon.get(k).copied(), summarize(v)),
        );
    }
    for (k, v) in &t.balloon_single {
        m.insert(
            format!("balloon (one run per line): {k}"),
            (None, summarize(v)),
        );
    }
    for (k, v) in &t.per_line {
        m.insert(format!("per line (single run): {k}"), (None, summarize(v)));
    }
    for (k, v) in &t.tap_fresh {
        m.insert(format!("tap, page never seen: {k}"), (None, summarize(v)));
    }
    for (k, v) in &t.tap_detected {
        m.insert(
            format!("tap, page already detected: {k}"),
            (None, summarize(v)),
        );
    }
    for (k, v) in &t.whole_page {
        m.insert(format!("whole page: {k}"), (None, summarize(v)));
    }
    let _ = median;
    m
}

fn pct(x: f64) -> String {
    if x.is_finite() {
        format!("{:.1}%", x * 100.0)
    } else {
        "–".into()
    }
}

fn fms(x: f64) -> String {
    if !x.is_finite() || x == 0.0 {
        "–".into()
    } else if x >= 1000.0 {
        format!("{:.2} s", x / 1000.0)
    } else if x >= 10.0 {
        format!("{x:.0} ms")
    } else {
        format!("{x:.1} ms")
    }
}

fn agg_line(o: &mut String, name: &str, a: &Agg, t: Option<&Summary>) {
    let _ = writeln!(
        o,
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        name.replace('|', "\\|"),
        a.n,
        a.le5,
        a.le10,
        a.le25,
        a.missed,
        pct(a.mean_cer),
        pct(a.median_cer),
        pct(a.mean_wer),
        pct(a.mean_cer_all),
        t.map(|s| fms(s.median_ms)).unwrap_or_default(),
        t.map(|s| fms(s.p90_ms)).unwrap_or_default(),
    );
}

const AGG_HEAD: &str = "| engine | n | CER<=5% | <=10% | <=25% | missed | mean CER | median CER | mean WER | mean CER, missed=1 | median time | p90 time |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|";

fn markdown(r: &Stage2, args: &Args) -> String {
    let mut o = String::new();
    let e = &r.env;
    let _ = writeln!(
        o,
        "# OCR bench, candidates: {}\n\n- Build profile **{}**; CPU {}; CPUs allowed `{}`; ONNX Runtime intra-op threads for candidate sessions: {} (0 = ORT default)\n- Load average start `{}`, end `{}`; peak RSS (VmHWM) {:.0} MB\n- Pages with a reference: {}\n- Recognition crops: reference box + {:.0}% margin. Tesseract rows are scored after Folio's postprocess (what the reader shows); PP rows on the raw text (Folio's cleanup was written for Tesseract debris); the other score is in the JSON.\n",
        r.label,
        e.profile,
        e.cpu_model,
        e.cpus_allowed,
        r.threads,
        e.loadavg_start,
        e.loadavg_end,
        r.vm_hwm_mb.unwrap_or(f64::NAN),
        r.pages.len(),
        args.variant_margin * 100.0
    );
    let _ = writeln!(
        o,
        "## Model loads\n\n| model | load | RSS added | note |\n|---|---:|---:|---|"
    );
    for l in &r.loads {
        let _ = writeln!(
            o,
            "| {} | {} | {:.0} MB | {} |",
            l.name,
            fms(l.load_ms),
            l.rss_added_mb,
            l.note
        );
    }
    if !r.a_recognition.is_empty() {
        let _ = writeln!(o, "\n## A. Recognition alone\n\n{AGG_HEAD}");
        for (k, g) in &r.a_recognition {
            agg_line(&mut o, k, &g.overall, r.a_time.get(k));
        }
        let _ = writeln!(
            o,
            "\nCrops where a split found no lines and the whole crop was read as one line: {}\n\nSplit time per crop: {}\n",
            {
                let mut fb: BTreeMap<String, (usize, usize)> = BTreeMap::new();
                for p in &r.pages {
                    for (k, v) in &p.split_fallbacks {
                        let e = fb.entry(k.clone()).or_default();
                        e.0 += v.0;
                        e.1 += v.1;
                    }
                }
                fb.iter()
                    .map(|(k, v)| format!("{k} {} of {}", v.0, v.1))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
            r.split_time
                .iter()
                .map(|(k, s)| format!("{k} median {} p90 {}", fms(s.median_ms), fms(s.p90_ms)))
                .collect::<Vec<_>>()
                .join("; ")
        );
        let _ = writeln!(o, "### A by kind and colour/case (mean CER, n)\n");
        let mut cols: Vec<String> = Vec::new();
        for g in r.a_recognition.values() {
            for k in g.by_kind.keys() {
                let c = format!("kind: {k}");
                if !cols.contains(&c) {
                    cols.push(c);
                }
            }
            for k in g.by_colour_case.keys() {
                let c = format!("colour: {k}");
                if !cols.contains(&c) {
                    cols.push(c);
                }
            }
        }
        let _ = writeln!(
            o,
            "| engine | {} |\n|---|{}",
            cols.join(" | "),
            "---:|".repeat(cols.len())
        );
        for (k, g) in &r.a_recognition {
            let cells: Vec<String> = cols
                .iter()
                .map(|c| {
                    let a = if let Some(x) = c.strip_prefix("kind: ") {
                        g.by_kind.get(x)
                    } else {
                        g.by_colour_case.get(c.trim_start_matches("colour: "))
                    };
                    a.map(|a| format!("{} ({})", pct(a.mean_cer), a.n))
                        .unwrap_or_else(|| "–".into())
                })
                .collect();
            let _ = writeln!(o, "| {} | {} |", k, cells.join(" | "));
        }
    }
    if !r.b_detection.is_empty() {
        let _ = writeln!(
            o,
            "\n## B. Detection alone\n\n| detector:unit | refs | found | split | merged | partial | missed | boxes | boxes matching no reference | detect median | p90 |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
        );
        for (k, c) in &r.b_detection {
            let d = k.split(':').next().unwrap_or("");
            let t = r.det_time.get(d);
            let _ = writeln!(
                o,
                "| {k} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                c.refs,
                c.found,
                c.split,
                c.merged,
                c.partial,
                c.missed,
                c.boxes,
                c.boxes_matching_nothing,
                t.map(|s| fms(s.median_ms)).unwrap_or_default(),
                t.map(|s| fms(s.p90_ms)).unwrap_or_default()
            );
        }
    }
    if !r.c_taps.is_empty() {
        let _ = writeln!(
            o,
            "\n## C. Tap one balloon (tap at the reference box centre)\n\n{AGG_HEAD}"
        );
        for (k, g) in &r.c_taps {
            agg_line(&mut o, k, &g.overall, None);
        }
    }
    if !r.timing_summary.is_empty() {
        let _ = writeln!(
            o,
            "\n## D. Timings\n\n| what | cold | median | p90 | n |\n|---|---:|---:|---:|---:|"
        );
        for (k, (cold, s)) in &r.timing_summary {
            let _ = writeln!(
                o,
                "| {k} | {} | {} | {} | {} |",
                cold.map(fms).unwrap_or_else(|| "–".into()),
                fms(s.median_ms),
                fms(s.p90_ms),
                s.n
            );
        }
        if let Some(t) = &r.timing {
            let _ = writeln!(
                o,
                "\n## E. Whole-page throughput (this configuration)\n\n| combo | median page | pages per minute | median units per page |\n|---|---:|---:|---:|"
            );
            for (k, v) in &t.whole_page {
                let s = summarize(v);
                let u: Vec<f64> = t
                    .units_per_page
                    .get(k)
                    .map(|x| x.iter().map(|n| *n as f64).collect())
                    .unwrap_or_default();
                let _ = writeln!(
                    o,
                    "| {k} | {} | {:.1} | {:.0} |",
                    fms(s.median_ms),
                    60000.0 / s.median_ms,
                    median(&u)
                );
            }
        }
    }
    if !r.worst.is_empty() {
        let _ = writeln!(o, "\n## Worst 25 per engine\n");
        for (k, rows) in &r.worst {
            let dir = rows
                .first()
                .map(|w| {
                    Path::new(&w.image)
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let _ = writeln!(o, "- {k}: `{dir}` (index.tsv, one PNG + TXT per region)");
        }
    }
    o
}
