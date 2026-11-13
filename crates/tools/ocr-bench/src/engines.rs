//! The two seams other engines plug into later, and Folio's engines
//! behind them.
//!
//! A detector takes a page and returns boxes; a recogniser takes a crop
//! and returns text and a confidence. Timing, scoring, contact sheets and
//! reports are shared and know nothing else about the engine. To add an
//! engine, implement one trait and register it in `main.rs`.

use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;

use image::DynamicImage;
use serde::Serialize;
use server::ocr::models::{self, PinnedFile};
use server::ocr::recognizer::{Recognition, Recognizer, western::WesternOcr};
use sha2::{Digest, Sha256};

use crate::cand::CtdOwn;
use crate::stats::ms_since;

/// One detected box in page pixels.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub confidence: f32,
    /// Engine-specific class index (for `comic-text-detector`, the API
    /// documents 0 = text block, 1 = text line; unverified).
    pub class: u32,
}

pub(crate) trait PageDetector {
    fn name(&self) -> String;
    fn detect(&mut self, page: &DynamicImage) -> anyhow::Result<Vec<DetBox>>;
}

/// A recogniser's answer, plus an optional breakdown of its own stages
/// (name, ms) for the timing tables.
pub(crate) struct RecOutput {
    pub rec: Recognition,
    pub stages: Vec<(&'static str, f64)>,
}

pub(crate) trait CropRecognizer {
    fn name(&self) -> String;
    fn recognize(&mut self, crop: &DynamicImage) -> anyhow::Result<RecOutput>;
}

/// The `comic-text-detector` model the server ran before the engine
/// change: the baseline detector. The server no longer pins or fetches
/// it; the tool reads it from the hub cache where an earlier server (or
/// `docs/dev/ocr.md`'s pre-staging) left it, and refuses a file that is
/// not this one.
pub(crate) const CTD_MODEL: PinnedFile = PinnedFile {
    repo: "mayocream/comic-text-detector-onnx",
    revision: "a5d67ec772adef819ef5b0e7aa701fcf4c8bf74a",
    file: "comic-text-detector.onnx",
    sha256: "1a86ace74961413cbd650002e7bb4dcec4980ffa21b2f19b86933372071d718f",
    size: 94_669_756,
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetectorLoad {
    /// Our own read + SHA-256 of the cached model, before anything else.
    pub precheck_hash_ms: f64,
    /// ONNX Runtime session build.
    pub session_build_ms: f64,
    pub model_path: String,
    pub model_bytes: u64,
}

/// [`CTD_MODEL`] in the hub cache, checked against its digest. Never
/// downloads: a missing or different file is an error.
pub(crate) fn ctd_model_path() -> anyhow::Result<(PathBuf, u64, f64)> {
    let root = models::hub_cache_root().map_err(|e| anyhow::anyhow!(e))?;
    let path = CTD_MODEL.snapshot_path(&root);
    let t = Instant::now();
    let mut f = std::fs::File::open(&path).map_err(|e| {
        anyhow::anyhow!(
            "comic-text-detector model not cached at {} ({e}); not downloading",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut bytes = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hasher.update(&buf[..n]);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != CTD_MODEL.sha256 {
        anyhow::bail!(
            "refusing to load: cached comic-text-detector model digest {digest} != pinned {}",
            CTD_MODEL.sha256
        );
    }
    Ok((path, bytes, ms_since(t)))
}

/// The baseline detector: `comic-text-detector`'s model in a session the
/// tool builds ([`CtdOwn`], `threads` intra-op threads, 0 = ONNX
/// Runtime's default).
pub(crate) fn load_ctd(threads: usize) -> anyhow::Result<(CtdOwn, DetectorLoad)> {
    let (path, bytes, precheck_hash_ms) = ctd_model_path()?;
    let t = Instant::now();
    let det = CtdOwn::load(&path, threads)?;
    Ok((
        det,
        DetectorLoad {
            precheck_hash_ms,
            session_build_ms: ms_since(t),
            model_path: path.display().to_string(),
            model_bytes: bytes,
        },
    ))
}

/// Folio's Western recogniser, exactly as the pipeline calls it (the
/// `detect: false` path: crop in, `WesternOcr::recognize`).
pub(crate) struct FolioWestern {
    ocr: &'static WesternOcr,
}

impl FolioWestern {
    /// Returns the recogniser and its init time in ms.
    pub(crate) async fn load() -> anyhow::Result<(Self, f64)> {
        let t = Instant::now();
        let ocr = WesternOcr::shared().await?;
        Ok((FolioWestern { ocr }, ms_since(t)))
    }
}

impl CropRecognizer for FolioWestern {
    fn name(&self) -> String {
        "folio-western (Tesseract via server::ocr::recognizer::western)".into()
    }

    fn recognize(&mut self, crop: &DynamicImage) -> anyhow::Result<RecOutput> {
        Ok(RecOutput {
            rec: self.ocr.recognize(crop)?,
            stages: Vec::new(),
        })
    }
}
