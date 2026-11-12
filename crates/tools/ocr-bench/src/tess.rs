//! A configurable Tesseract recogniser for the variant table.
//!
//! [`TessConfig::current`] reproduces Folio's Western recogniser
//! (`server::ocr::recognizer::western`): grey, 3x Lanczos3, Otsu with the
//! midpoint fallback, border-ring polarity fix, 24 px white pad, PSM 6,
//! 300 DPI, `eng` from the build's `tessdata_best`, default dictionaries.
//! Every other config changes one of those. The harness checks at run
//! time that the `current` config gives the same text as the server's own
//! recogniser on every crop, and a unit test checks the preprocessing is
//! byte-identical to `western::preprocess`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{DynamicImage, GrayImage, Luma, imageops::FilterType};
use serde::Serialize;
use server::ocr::recognizer::{Recognition, Word};
use tesseract_rs::{TessPageIteratorLevel, TessPageSegMode, TesseractAPI};

use crate::engines::{CropRecognizer, RecOutput};
use crate::stats::ms_since;

/// Folio's post-upscale white border.
pub(crate) const PAD: u32 = 24;
/// Folio's pinned source resolution.
const SOURCE_PPI: i32 = 300;
/// Folio's upscale factor.
pub(crate) const FOLIO_UPSCALE: u32 = 3;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub(crate) enum Scale {
    /// Integer factor (Folio: 3).
    Factor(u32),
    /// Scale so the estimated capital height lands at this many px.
    CapHeight(u32),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub(crate) enum Psm {
    /// 6, one uniform block (Folio).
    Block,
    /// 7 on each text line Tesseract's layout analysis finds.
    PerLine,
    /// 11, sparse text.
    Sparse,
    /// 13, raw line.
    RawLine,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TessConfig {
    pub name: String,
    pub what_changed: String,
    pub tessdata: PathBuf,
    pub lang: String,
    /// Tesseract OEM: 3 default, 1 LSTM only, 0 legacy only.
    pub oem: i32,
    pub psm: Psm,
    /// Folio's own Otsu step. Off = hand Tesseract the grey image.
    pub binarize: bool,
    /// Tesseract's `thresholding_method` (0 Otsu, 1 Leptonica adaptive
    /// Otsu, 2 Sauvola). `None` leaves the default (0).
    pub thresholding_method: Option<i32>,
    pub scale: Scale,
    /// `load_system_dawg` / `load_freq_dawg`.
    pub dawgs: bool,
    /// Extra page context (source px on each side) taken around the crop
    /// before any processing. Applied by the harness, which owns the page.
    pub context_px: u32,
}

impl TessConfig {
    pub(crate) fn current(tessdata: &Path) -> Self {
        TessConfig {
            name: "current".into(),
            what_changed: "Folio today: grey, 3x, Otsu, polarity, pad 24, PSM 6, tessdata_best eng"
                .into(),
            tessdata: tessdata.to_path_buf(),
            lang: server::ocr::recognizer::western::TESS_LANG.into(),
            oem: 3,
            psm: Psm::Block,
            binarize: true,
            thresholding_method: None,
            scale: Scale::Factor(FOLIO_UPSCALE),
            dawgs: true,
            context_px: 0,
        }
    }

    fn with(&self, name: &str, what: &str, f: impl FnOnce(&mut TessConfig)) -> TessConfig {
        let mut c = self.clone();
        c.name = name.into();
        c.what_changed = what.into();
        f(&mut c);
        c
    }

    /// The variant set: each changes one thing against [`Self::current`].
    /// `system_tessdata` adds the distribution's language data when found.
    pub(crate) fn variants(tessdata: &Path, system_tessdata: Option<&Path>) -> Vec<TessConfig> {
        let cur = Self::current(tessdata);
        let mut v = vec![
            cur.clone(),
            cur.with(
                "grey",
                "no Folio Otsu: grey to Tesseract (its own global Otsu)",
                |c| {
                    c.binarize = false;
                },
            ),
            cur.with(
                "grey+adaptive",
                "grey, thresholding_method=1 (Leptonica adaptive Otsu)",
                |c| {
                    c.binarize = false;
                    c.thresholding_method = Some(1);
                },
            ),
            cur.with(
                "grey+sauvola",
                "grey, thresholding_method=2 (Sauvola)",
                |c| {
                    c.binarize = false;
                    c.thresholding_method = Some(2);
                },
            ),
            cur.with(
                "psm7-lines",
                "PSM 7 on each line from Tesseract's layout pass",
                |c| {
                    c.psm = Psm::PerLine;
                },
            ),
            cur.with("psm11", "PSM 11 (sparse text)", |c| c.psm = Psm::Sparse),
            cur.with("psm13", "PSM 13 (raw line)", |c| c.psm = Psm::RawLine),
            cur.with("no-dawgs", "load_system_dawg=F, load_freq_dawg=F", |c| {
                c.dawgs = false
            }),
            cur.with("scale1x", "no upscale", |c| c.scale = Scale::Factor(1)),
            cur.with("scale2x", "2x upscale", |c| c.scale = Scale::Factor(2)),
            cur.with("cap30", "scale to ~30 px capital height", |c| {
                c.scale = Scale::CapHeight(30);
            }),
            cur.with("cap40", "scale to ~40 px capital height", |c| {
                c.scale = Scale::CapHeight(40);
            }),
            cur.with(
                "context12",
                "12 px page context around the crop before thresholding",
                |c| {
                    c.context_px = 12;
                },
            ),
        ];
        if let Some(sys) = system_tessdata {
            v.push(cur.with(
                "sys-eng-lstm",
                "distribution eng.traineddata (tessdata 4.1.0), OEM 1 LSTM",
                |c| {
                    c.tessdata = sys.to_path_buf();
                    c.oem = 1;
                },
            ));
            v.push(cur.with(
                "sys-eng-legacy",
                "distribution eng.traineddata, OEM 0 legacy engine",
                |c| {
                    c.tessdata = sys.to_path_buf();
                    c.oem = 0;
                },
            ));
        }
        v
    }
}

/// Folio's preprocessing, parameterised. With `factor = 3.0` and
/// `binarize = true` it is byte-identical to `western::preprocess`.
pub(crate) fn prepare(image: &DynamicImage, factor: f64, binarize: bool) -> GrayImage {
    let gray = image.to_luma8();
    let (w, h) = gray.dimensions();
    let nw = ((f64::from(w) * factor).round() as u32).max(1);
    let nh = ((f64::from(h) * factor).round() as u32).max(1);
    let mut img = if nw == w && nh == h {
        gray
    } else {
        image::imageops::resize(&gray, nw, nh, FilterType::Lanczos3)
    };
    let level = imageproc::contrast::otsu_level(&img);
    let level = if level == 0 || level == u8::MAX {
        127
    } else {
        level
    };
    let binarized = |v: u8| if v <= level { 0u8 } else { 255u8 };
    let invert = is_white_on_black(&img, &binarized);
    for px in img.pixels_mut() {
        let v = if binarize {
            binarized(px.0[0])
        } else {
            px.0[0]
        };
        px.0[0] = if invert { 255 - v } else { v };
    }
    let (bw, bh) = img.dimensions();
    let mut padded = GrayImage::from_pixel(bw + 2 * PAD, bh + 2 * PAD, Luma([255u8]));
    image::imageops::replace(&mut padded, &img, i64::from(PAD), i64::from(PAD));
    padded
}

/// Folio's border-ring probe, on the binarised values.
fn is_white_on_black(img: &GrayImage, bin: &impl Fn(u8) -> u8) -> bool {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return false;
    }
    let ring = 2u32.min(w).min(h);
    let (mut black, mut total) = (0u64, 0u64);
    for y in 0..h {
        let on_band = y < ring || y >= h - ring;
        for x in 0..w {
            if on_band || x < ring || x >= w - ring {
                total += 1;
                if bin(img.get_pixel(x, y).0[0]) == 0 {
                    black += 1;
                }
            }
        }
    }
    total > 0 && black * 2 > total
}

pub(crate) struct TessEngine {
    api: TesseractAPI,
    pub cfg: TessConfig,
    /// What Tesseract reports after init, to show a setting took.
    pub effective: serde_json::Value,
}

fn tess_err(what: &str) -> impl Fn(tesseract_rs::TesseractError) -> anyhow::Error + '_ {
    move |e| anyhow::anyhow!("tesseract {what}: {e}")
}

impl TessEngine {
    /// `work_dir` holds the small config file used to turn dictionaries off.
    pub(crate) fn new(cfg: TessConfig, work_dir: &Path) -> anyhow::Result<Self> {
        let api = TesseractAPI::new();
        let datapath = cfg
            .tessdata
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-UTF-8 tessdata path"))?
            .to_owned();
        if cfg.dawgs {
            if cfg.oem == 3 {
                api.init(&cfg.tessdata, &cfg.lang)
                    .map_err(tess_err("init"))?;
            } else {
                api.init_2(&datapath, &cfg.lang, cfg.oem)
                    .map_err(tess_err("init_2"))?;
            }
        } else {
            // Dictionary loading is an init-only parameter: it has to come
            // in through a config file read during init.
            std::fs::create_dir_all(work_dir)?;
            let conf = work_dir.join("no-dawgs.config");
            std::fs::write(&conf, "load_system_dawg F\nload_freq_dawg F\n")?;
            let conf = conf
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 work dir"))?
                .to_owned();
            api.init_1(&datapath, &cfg.lang, cfg.oem, &[conf.as_str()])
                .map_err(tess_err("init_1"))?;
        }
        if let Some(m) = cfg.thresholding_method {
            api.set_variable("thresholding_method", &m.to_string())
                .map_err(tess_err("set thresholding_method"))?;
        }
        let effective = serde_json::json!({
            "load_system_dawg": api.get_bool_variable("load_system_dawg").ok(),
            "load_freq_dawg": api.get_bool_variable("load_freq_dawg").ok(),
            "thresholding_method": api.get_int_variable("thresholding_method").ok(),
            "tessedit_ocr_engine_mode": api.get_int_variable("tessedit_ocr_engine_mode").ok(),
            "version": TesseractAPI::version(),
        });
        Ok(TessEngine {
            api,
            cfg,
            effective,
        })
    }

    fn set_psm(&self, psm: TessPageSegMode) -> anyhow::Result<()> {
        self.api
            .set_page_seg_mode(psm)
            .map_err(tess_err("set_page_seg_mode"))
    }

    fn set_image(&self, img: &GrayImage) -> anyhow::Result<()> {
        let (w, h) = img.dimensions();
        self.api
            .set_image(img.as_raw(), w as i32, h as i32, 1, w as i32)
            .map_err(tess_err("set_image"))?;
        self.api
            .set_source_resolution(SOURCE_PPI)
            .map_err(tess_err("set_source_resolution"))
    }

    /// Text lines from Tesseract's PSM 6 layout pass over a prepared image
    /// (left, top, right, bottom in prepared-image pixels).
    fn layout_lines(&self, prepared: &GrayImage) -> anyhow::Result<Vec<(i32, i32, i32, i32)>> {
        self.set_psm(TessPageSegMode::PSM_SINGLE_BLOCK)?;
        self.set_image(prepared)?;
        let Ok(it) = self.api.analyse_layout() else {
            return Ok(Vec::new());
        };
        let mut lines = Vec::new();
        loop {
            if let Ok(b) = it.bounding_box(TessPageIteratorLevel::RIL_TEXTLINE) {
                lines.push(b);
            }
            match it.next(TessPageIteratorLevel::RIL_TEXTLINE) {
                Ok(true) => continue,
                _ => break,
            }
        }
        Ok(lines)
    }

    /// Set the image, recognise, and walk words (Folio's word walk).
    /// `offset` and `factor` map prepared coordinates back to the crop.
    fn recognize_image(
        &self,
        img: &GrayImage,
        psm: TessPageSegMode,
        line_base: u32,
        offset: (i32, i32),
        factor: f64,
    ) -> anyhow::Result<Recognition> {
        self.set_psm(psm)?;
        self.set_image(img)?;
        self.api.recognize().map_err(tess_err("recognize"))?;
        let unmap = |v: i32, o: i32| ((v + o - PAD as i32).max(0) as f64 / factor) as f32;
        let mut words: Vec<Word> = Vec::new();
        if let Ok(it) = self.api.get_iterator() {
            let mut line_index = line_base;
            let mut prev_line: Option<(i32, i32, i32, i32)> = None;
            loop {
                if let Ok((text, l, t, r, b, conf)) = it.get_current_word() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let lb = it
                            .get_bounding_box(TessPageIteratorLevel::RIL_TEXTLINE)
                            .ok();
                        if let (Some(p), Some(c)) = (prev_line, lb)
                            && p != c
                        {
                            line_index += 1;
                        }
                        if lb.is_some() {
                            prev_line = lb;
                        }
                        words.push(Word {
                            text: trimmed.to_owned(),
                            confidence: conf.clamp(0.0, 100.0) / 100.0,
                            xmin: unmap(l, offset.0),
                            ymin: unmap(t, offset.1),
                            xmax: unmap(r, offset.0),
                            ymax: unmap(b, offset.1),
                            line_index,
                        });
                    }
                }
                match it.next(TessPageIteratorLevel::RIL_WORD) {
                    Ok(true) => continue,
                    _ => break,
                }
            }
        }
        let confidence = self
            .api
            .mean_text_conf()
            .map(|c| (c.clamp(0, 100) as f32) / 100.0)
            .unwrap_or(0.0);
        if words.is_empty() {
            let text = self
                .api
                .get_utf8_text()
                .map(|t| t.trim().to_owned())
                .unwrap_or_default();
            return Ok(Recognition {
                text,
                confidence,
                words: None,
            });
        }
        Ok(Recognition {
            text: join_words(&words),
            confidence,
            words: Some(words),
        })
    }

    /// Recognise an already prepared image the way Folio does (PSM 6).
    /// Used for the preprocessing / Tesseract timing split.
    pub(crate) fn recognize_prepared_block(
        &self,
        prepared: &GrayImage,
    ) -> anyhow::Result<Recognition> {
        self.recognize_image(
            prepared,
            TessPageSegMode::PSM_SINGLE_BLOCK,
            0,
            (0, 0),
            f64::from(FOLIO_UPSCALE),
        )
    }

    /// Estimated capital height in crop pixels: median text-line height
    /// from a layout pass over Folio's 3x preparation (comic lettering is
    /// upper case, so a line box is about one capital high).
    fn estimate_cap_height(&self, crop: &DynamicImage) -> anyhow::Result<Option<f64>> {
        let prepared = prepare(crop, f64::from(FOLIO_UPSCALE), true);
        let mut hs: Vec<f64> = self
            .layout_lines(&prepared)?
            .iter()
            .map(|(_, t, _, b)| f64::from(b - t) / f64::from(FOLIO_UPSCALE))
            .filter(|h| *h > 2.0)
            .collect();
        if hs.is_empty() {
            return Ok(None);
        }
        hs.sort_by(f64::total_cmp);
        Ok(Some(hs[hs.len() / 2]))
    }
}

fn join_words(words: &[Word]) -> String {
    let mut out = String::new();
    let mut prev = words.first().map(|w| w.line_index).unwrap_or(0);
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(if w.line_index != prev { '\n' } else { ' ' });
        }
        out.push_str(&w.text);
        prev = w.line_index;
    }
    out
}

impl CropRecognizer for TessEngine {
    fn name(&self) -> String {
        format!("tesseract:{}", self.cfg.name)
    }

    fn recognize(&mut self, crop: &DynamicImage) -> anyhow::Result<RecOutput> {
        let mut stages = Vec::new();
        let factor = match self.cfg.scale {
            Scale::Factor(f) => f64::from(f),
            Scale::CapHeight(target) => {
                let t = Instant::now();
                let cap = self.estimate_cap_height(crop)?;
                stages.push(("cap-height layout pass", ms_since(t)));
                match cap {
                    Some(c) => (f64::from(target) / c).clamp(0.5, 8.0),
                    None => f64::from(FOLIO_UPSCALE),
                }
            }
        };
        let t = Instant::now();
        let prepared = prepare(crop, factor, self.cfg.binarize);
        stages.push(("preprocess", ms_since(t)));
        let t = Instant::now();
        let rec = match self.cfg.psm {
            Psm::Block => self.recognize_image(
                &prepared,
                TessPageSegMode::PSM_SINGLE_BLOCK,
                0,
                (0, 0),
                factor,
            )?,
            Psm::Sparse => self.recognize_image(
                &prepared,
                TessPageSegMode::PSM_SPARSE_TEXT,
                0,
                (0, 0),
                factor,
            )?,
            Psm::RawLine => {
                self.recognize_image(&prepared, TessPageSegMode::PSM_RAW_LINE, 0, (0, 0), factor)?
            }
            Psm::PerLine => self.recognize_per_line(&prepared, factor)?,
        };
        stages.push(("tesseract", ms_since(t)));
        Ok(RecOutput { rec, stages })
    }
}

impl TessEngine {
    fn recognize_per_line(&self, prepared: &GrayImage, factor: f64) -> anyhow::Result<Recognition> {
        let lines = self.layout_lines(prepared)?;
        if lines.is_empty() {
            return self.recognize_image(
                prepared,
                TessPageSegMode::PSM_SINGLE_BLOCK,
                0,
                (0, 0),
                factor,
            );
        }
        let (pw, ph) = prepared.dimensions();
        let mut all_words = Vec::new();
        let mut texts = Vec::new();
        let mut confs = Vec::new();
        for (i, (l, t, r, b)) in lines.into_iter().enumerate() {
            let m = 6;
            let x0 = (l - m).max(0) as u32;
            let y0 = (t - m).max(0) as u32;
            let x1 = ((r + m).max(0) as u32).min(pw);
            let y1 = ((b + m).max(0) as u32).min(ph);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let line = image::imageops::crop_imm(prepared, x0, y0, x1 - x0, y1 - y0).to_image();
            let mut padded = GrayImage::from_pixel(
                line.width() + 2 * PAD,
                line.height() + 2 * PAD,
                Luma([255u8]),
            );
            image::imageops::replace(&mut padded, &line, i64::from(PAD), i64::from(PAD));
            let offset = (x0 as i32 - PAD as i32, y0 as i32 - PAD as i32);
            let rec = self.recognize_image(
                &padded,
                TessPageSegMode::PSM_SINGLE_LINE,
                i as u32,
                offset,
                factor,
            )?;
            if !rec.text.trim().is_empty() {
                texts.push(rec.text.clone());
                confs.push(rec.confidence);
            }
            if let Some(ws) = rec.words {
                all_words.extend(ws.into_iter().map(|mut w| {
                    w.line_index = i as u32;
                    w
                }));
            }
        }
        let confidence = if confs.is_empty() {
            0.0
        } else {
            confs.iter().sum::<f32>() / confs.len() as f32
        };
        Ok(Recognition {
            text: texts.join("\n"),
            confidence,
            words: (!all_words.is_empty()).then_some(all_words),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bench's parameterised preparation at Folio's settings is
    /// byte-identical to the server's.
    #[test]
    fn current_prepare_matches_server_preprocess() {
        let mut img = GrayImage::from_pixel(60, 40, Luma([230]));
        for y in 10..30 {
            for x in 5..55 {
                if (x / 3 + y / 4) % 2 == 0 {
                    img.put_pixel(x, y, Luma([((x * 7 + y * 3) % 90) as u8]));
                }
            }
        }
        let dynimg = DynamicImage::ImageLuma8(img.clone());
        let ours = prepare(&dynimg, 3.0, true);
        let server = server::ocr::recognizer::western::preprocess(&dynimg);
        assert_eq!(ours.dimensions(), server.dimensions());
        assert_eq!(ours.as_raw(), server.as_raw());

        // And on a dark crop that the polarity probe inverts.
        let dark = DynamicImage::ImageLuma8(GrayImage::from_fn(40, 40, |x, y| {
            Luma([if (18..22).contains(&x) && (10..30).contains(&y) {
                240
            } else {
                15
            }])
        }));
        assert_eq!(
            prepare(&dark, 3.0, true).as_raw(),
            server::ocr::recognizer::western::preprocess(&dark).as_raw()
        );
    }
}
