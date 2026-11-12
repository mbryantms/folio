//! The two seams other engines plug into later, and Folio's engines
//! behind them.
//!
//! A detector takes a page and returns boxes; a recogniser takes a crop
//! and returns text and a confidence. Timing, scoring, contact sheets and
//! reports are shared and know nothing else about the engine. To add an
//! engine, implement one trait and register it in `main.rs`.

use std::io::Read;
use std::time::Instant;

use image::DynamicImage;
use serde::Serialize;
use server::ocr::detector::Detector;
use server::ocr::models;
use server::ocr::pipeline::PageDetector as ServerPageDetector;
use server::ocr::recognizer::{Recognition, Recognizer, western::WesternOcr};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetectorLoad {
    /// Our own read + SHA-256 of the cached model, before anything else,
    /// to refuse a run that would make the server code download.
    pub precheck_hash_ms: f64,
    /// `models::ensure_pinned` (hashes the cached file once per process).
    pub ensure_pinned_ms: f64,
    /// `Detector::shared` after the pin: ONNX Runtime session build.
    pub session_build_ms: f64,
    pub model_path: String,
    pub model_bytes: u64,
}

/// `comic-text-detector` through Folio's singleton.
pub(crate) struct FolioDetector {
    det: &'static Detector,
}

impl FolioDetector {
    /// Load the detector. Refuses (instead of letting the server code
    /// fetch anything) unless the pinned model is already in the hub
    /// cache with the pinned digest and `refs/main` names the pinned
    /// commit.
    pub(crate) async fn load() -> anyhow::Result<(Self, DetectorLoad)> {
        let pin = models::DETECTOR_FILES[0];
        let root = models::hub_cache_root().map_err(|e| anyhow::anyhow!(e))?;
        let path = pin.snapshot_path(&root);
        let refs_main = root
            .join(format!("models--{}", pin.repo.replace('/', "--")))
            .join("refs")
            .join("main");
        let rev = std::fs::read_to_string(&refs_main).unwrap_or_default();
        if rev.trim() != pin.revision {
            anyhow::bail!(
                "refusing to load: {} does not name the pinned commit {} (the server code would rewrite it); not downloading",
                refs_main.display(),
                pin.revision
            );
        }
        let t = Instant::now();
        let mut f = std::fs::File::open(&path).map_err(|e| {
            anyhow::anyhow!(
                "refusing to load: pinned detector model not cached at {} ({e}); the server code would download it",
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
        if digest != pin.sha256 {
            anyhow::bail!(
                "refusing to load: cached detector model digest {digest} != pinned {}; the server code would delete and re-download it",
                pin.sha256
            );
        }
        let precheck_hash_ms = ms_since(t);

        let t = Instant::now();
        models::ensure_pinned(models::DETECTOR_FILES).await?;
        let ensure_pinned_ms = ms_since(t);
        let t = Instant::now();
        let det = Detector::shared().await?;
        let session_build_ms = ms_since(t);
        Ok((
            FolioDetector { det },
            DetectorLoad {
                precheck_hash_ms,
                ensure_pinned_ms,
                session_build_ms,
                model_path: path.display().to_string(),
                model_bytes: bytes,
            },
        ))
    }
}

impl PageDetector for FolioDetector {
    fn name(&self) -> String {
        "comic-text-detector 0.5.1 (server::ocr::pipeline full-page detection)".into()
    }

    /// The server's own full-page detection
    /// (`server::ocr::pipeline::PageDetector` on the singleton): the call
    /// both OCR routes make on a detect-cache miss, with the pipeline's
    /// thresholds, returning the boxes it would cache.
    fn detect(&mut self, page: &DynamicImage) -> anyhow::Result<Vec<DetBox>> {
        let detection = ServerPageDetector::detect(self.det, page)?;
        Ok(detection
            .bboxes
            .iter()
            .map(|b| DetBox {
                x0: b.xmin,
                y0: b.ymin,
                x1: b.xmax,
                y1: b.ymax,
                confidence: b.confidence,
                class: b.class,
            })
            .collect())
    }
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
