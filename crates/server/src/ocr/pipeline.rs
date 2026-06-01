//! `detect → snap polygon → crop → recognize` orchestration.
//!
//! Callers hand us the **full page** image, the user's drag rect in
//! page-pixel coordinates, and a content-hash + page key for the
//! detector-result cache. We:
//!
//!  1. Look up cached detector polygons for `(content_hash, page)`.
//!  2. On miss, run `comic-text-detector` over the **full page** and
//!     store the polygon list in Redis. (Pre-v0.3.25 the detector
//!     ran over a small crop around the user's rect; we moved it to
//!     full-page so subsequent OCRs on different bubbles of the same
//!     page get a cache hit and skip the expensive detector entirely.
//!     The model resizes its input to 1024×1024 internally either
//!     way, so per-call inference cost is unchanged — but the cache
//!     reuse pays for itself within two OCRs on the same page.)
//!  3. Pick the bbox whose intersection with the user's rect is
//!     largest; fall back to the user's rect verbatim if none
//!     overlap.
//!  4. Crop to that rect and hand the image to the selected recognizer.
//!
//! Detector inference + recognize run inside [`tokio::task::spawn_blocking`]
//! so the reactor isn't stalled. The detector + recognizer singletons
//! are held by `&'static` reference, which is `Send + Sync` thanks to
//! the [`Recognizer: Send + Sync`][crate::ocr::recognizer::Recognizer]
//! bound.

use image::DynamicImage;
use redis::aio::ConnectionManager;

use super::cache::{self, CachedBbox};
use super::detector::Detector;
use super::recognizer::{Language, Recognition, Recognizer, manga::MangaOcr, western::WesternOcr};

/// Pixel rectangle in page coordinates. All four fields are
/// unsigned because callers must reject negatives at the API
/// boundary — the pipeline trusts its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let xmax = (self.x + self.w).min(other.x + other.w);
        let ymax = (self.y + self.h).min(other.y + other.h);
        if xmax > x && ymax > y {
            Some(Rect {
                x,
                y,
                w: xmax - x,
                h: ymax - y,
            })
        } else {
            None
        }
    }
}

// `Debug` skipped because `ConnectionManager` doesn't implement it.
// The struct is internal to the pipeline + handler; no Debug consumer
// loses anything by the omission.
pub struct OcrInput {
    pub page_image: DynamicImage,
    pub region: Rect,
    pub language: Language,
    /// BLAKE3 of the issue's on-disk bytes. Drives the detector-result
    /// cache key so a rescan that retags the row invalidates the cache.
    pub content_hash: String,
    /// 0-based page index — second component of the cache key.
    pub page: u32,
    /// Cloneable Redis handle for cache reads/writes. Failures are
    /// swallowed; the pipeline falls back to running the detector
    /// inline.
    pub redis: ConnectionManager,
    /// When `true`, run the detector + snap to the tightest enclosing
    /// bubble polygon. When `false`, the pipeline skips the detector
    /// entirely and the recognizer runs on the user's rect verbatim —
    /// see [`crate::api::issue_ocr::OcrRequest::detect`].
    pub detect: bool,
}

#[derive(Debug)]
pub struct OcrOutput {
    pub recognition: Recognition,
    /// Detector's snap-to-bubble rectangle in page-pixel coordinates,
    /// `None` if no bbox overlapped the user's region above
    /// confidence threshold.
    pub refined_bbox: Option<Rect>,
}

/// Detector confidence threshold below which a candidate bbox is
/// discarded. 0.5 matches dmMaze's default.
const DETECTOR_CONF: f32 = 0.5;
/// Non-max-suppression IoU threshold.
const DETECTOR_NMS: f32 = 0.5;

/// Driver: returns the recognized text + a refined bubble outline.
///
/// Looks up both singletons before entering the blocking task: the
/// `OnceCell`s wrap the async HF download, which we can't run inside
/// `spawn_blocking`. The heavy CPU work then runs once both are
/// warm.
///
/// Records two Prometheus histograms (units: seconds, matching
/// `folio_reader_next_up_latency_seconds` and friends):
///
/// - `folio_ocr_pipeline_seconds` — wall-clock for the whole call,
///   tagged `lang="western"|"manga"`. Spans detector + recognize.
/// - `folio_ocr_recognize_seconds` — recognize-only time. Tagged
///   the same way.
///
/// Use the gap between them to spot detector-bound vs recognizer-
/// bound deployments.
pub async fn run_ocr(input: OcrInput) -> anyhow::Result<OcrOutput> {
    let lang_label = match input.language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    let start = std::time::Instant::now();
    // Only initialize the detector singleton when we're actually
    // going to use it. The first-call cost includes a HF download +
    // ort session build — skipping that on the no-detect path means
    // operators with detection disabled never pay it.
    let detector = if input.detect {
        Some(Detector::shared().await?)
    } else {
        None
    };
    let recognizer: &'static dyn Recognizer = match input.language {
        Language::Western => WesternOcr::shared().await?,
        Language::Manga => MangaOcr::shared().await?,
    };

    // ─── Detect (with per-page cache) ────────────────────────────
    // When `detect` is off we skip the cache lookup + detector run
    // entirely. The blocking task receives `None` for both and falls
    // through to "OCR the user's rect verbatim".
    let (detect_key, cached_bboxes) = if input.detect {
        let key = cache::detect_cache_key(&input.content_hash, input.page);
        let cached = cache::get_detect(&input.redis, &key).await;
        (Some(key), cached)
    } else {
        (None, None)
    };

    let redis_for_put = input.redis.clone();
    let detect_key_for_put = detect_key.clone();
    let (output, bboxes_to_cache) = tokio::task::spawn_blocking(move || {
        run_blocking(detector, recognizer, input, lang_label, cached_bboxes)
    })
    .await
    .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))??;

    if let (Some(key), Some(bboxes)) = (detect_key_for_put, bboxes_to_cache) {
        cache::put_detect(&redis_for_put, &key, &bboxes).await;
    }

    metrics::histogram!("folio_ocr_pipeline_seconds", "lang" => lang_label)
        .record(start.elapsed().as_secs_f64());
    Ok(output)
}

/// Blocking body of [`run_ocr`]: runs the detector (if not cached),
/// picks the bbox overlapping the user's rect, crops, and recognizes.
/// Returns `(output, bboxes_to_cache)` — `bboxes_to_cache` is `Some`
/// only when the detector ran inline (cache miss), telling the async
/// caller to schedule a Redis PUT.
fn run_blocking(
    detector: Option<&Detector>,
    recognizer: &dyn Recognizer,
    input: OcrInput,
    lang_label: &'static str,
    cached_bboxes: Option<Vec<CachedBbox>>,
) -> anyhow::Result<(OcrOutput, Option<Vec<CachedBbox>>)> {
    let (page_w, page_h) = (input.page_image.width(), input.page_image.height());
    // Clamp the user's region into page bounds defensively — the
    // handler already validated this, but we don't want a panic in
    // `crop_imm` if a future caller skipped validation.
    let clamped = Rect {
        x: input.region.x.min(page_w.saturating_sub(1)),
        y: input.region.y.min(page_h.saturating_sub(1)),
        w: input.region.w.min(page_w - input.region.x.min(page_w)),
        h: input.region.h.min(page_h - input.region.y.min(page_h)),
    };
    if clamped.w == 0 || clamped.h == 0 {
        return Err(anyhow::anyhow!("region collapsed to zero area"));
    }

    // ─── Detect on the full page (or use cache, or skip) ─────────
    // Three cases:
    //   - detect=false → no detector handle; skip directly to recognize
    //   - detect=true + cache hit → use cached bboxes
    //   - detect=true + cache miss → run detector, return bboxes to cache
    let (refined_page, bboxes_to_cache) = match (detector, cached_bboxes) {
        (Some(_), Some(bboxes)) => (pick_bbox_page(&bboxes, &clamped, page_w, page_h), None),
        (Some(det), None) => {
            let det_out = {
                let mut d = det.lock()?;
                d.inference(&input.page_image, DETECTOR_CONF, DETECTOR_NMS)?
            };
            let bboxes: Vec<CachedBbox> = det_out
                .bboxes
                .iter()
                .map(|b| CachedBbox {
                    xmin: b.xmin,
                    ymin: b.ymin,
                    xmax: b.xmax,
                    ymax: b.ymax,
                    confidence: b.confidence,
                    class: b.class as u32,
                })
                .collect();
            let picked = pick_bbox_page(&bboxes, &clamped, page_w, page_h);
            (picked, Some(bboxes))
        }
        (None, _) => (None, None),
    };

    // ─── Pick crop, recognize ────────────────────────────────────
    let (final_img, refined_bbox) = if let Some(r) = refined_page {
        (input.page_image.crop_imm(r.x, r.y, r.w, r.h), Some(r))
    } else {
        (
            input
                .page_image
                .crop_imm(clamped.x, clamped.y, clamped.w, clamped.h),
            None,
        )
    };

    let recognize_start = std::time::Instant::now();
    let recognition = recognizer.recognize(&final_img)?;
    metrics::histogram!("folio_ocr_recognize_seconds", "lang" => lang_label)
        .record(recognize_start.elapsed().as_secs_f64());

    Ok((
        OcrOutput {
            recognition,
            refined_bbox,
        },
        bboxes_to_cache,
    ))
}

/// Pick the cached bbox with the largest intersection area against
/// the user's rect. Both are in page-pixel coordinates. Discards
/// candidates with zero overlap or that round out to an empty rect.
fn pick_bbox_page(bboxes: &[CachedBbox], user: &Rect, page_w: u32, page_h: u32) -> Option<Rect> {
    bboxes
        .iter()
        .filter_map(|b| {
            let r = bbox_to_rect(b, page_w, page_h)?;
            let overlap = r.intersection(user)?.area();
            Some((overlap, r))
        })
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, r)| r)
}

fn bbox_to_rect(b: &CachedBbox, w: u32, h: u32) -> Option<Rect> {
    let xmin = b.xmin.max(0.0).round() as i64;
    let ymin = b.ymin.max(0.0).round() as i64;
    let xmax = b.xmax.max(0.0).round() as i64;
    let ymax = b.ymax.max(0.0).round() as i64;
    if xmax <= xmin || ymax <= ymin {
        return None;
    }
    let rx = (xmin as u32).min(w);
    let ry = (ymin as u32).min(h);
    let rxmax = (xmax as u32).min(w);
    let rymax = (ymax as u32).min(h);
    if rxmax <= rx || rymax <= ry {
        return None;
    }
    Some(Rect {
        x: rx,
        y: ry,
        w: rxmax - rx,
        h: rymax - ry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(xmin: f32, ymin: f32, xmax: f32, ymax: f32, conf: f32) -> CachedBbox {
        CachedBbox {
            xmin,
            ymin,
            xmax,
            ymax,
            confidence: conf,
            class: 0,
        }
    }

    #[test]
    fn intersection_of_disjoint_rects_is_none() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let b = Rect {
            x: 100,
            y: 100,
            w: 10,
            h: 10,
        };
        assert!(a.intersection(&b).is_none());
    }

    #[test]
    fn intersection_of_overlapping_rects() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 20,
        };
        let b = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        let i = a.intersection(&b).expect("rects overlap");
        assert_eq!(
            i,
            Rect {
                x: 10,
                y: 10,
                w: 10,
                h: 10
            }
        );
    }

    #[test]
    fn pick_bbox_page_returns_largest_overlap() {
        let user = Rect {
            x: 100,
            y: 100,
            w: 100,
            h: 100,
        };
        let bboxes = vec![
            bbox(99.0, 99.0, 110.0, 110.0, 0.9),    // tiny overlap
            bbox(120.0, 110.0, 250.0, 230.0, 0.7),  // big overlap → chosen
            bbox(500.0, 500.0, 600.0, 600.0, 0.95), // disjoint → discarded
        ];
        let chosen = pick_bbox_page(&bboxes, &user, 1000, 1000).expect("pick a bbox");
        assert_eq!(chosen.x, 120);
        assert_eq!(chosen.y, 110);
    }

    #[test]
    fn pick_bbox_page_returns_none_for_no_overlap() {
        let user = Rect {
            x: 0,
            y: 0,
            w: 50,
            h: 50,
        };
        let bboxes = vec![bbox(500.0, 500.0, 600.0, 600.0, 0.9)];
        assert!(pick_bbox_page(&bboxes, &user, 1000, 1000).is_none());
    }

    #[test]
    fn bbox_to_rect_clamps_to_canvas() {
        // Detector bbox extends past canvas edges; we clamp to canvas.
        let b = bbox(-10.0, -5.0, 250.0, 250.0, 0.9);
        let r = bbox_to_rect(&b, 200, 200).expect("rect");
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 0);
        assert_eq!(r.x + r.w, 200);
        assert_eq!(r.y + r.h, 200);
    }

    #[test]
    fn bbox_to_rect_rejects_collapsed_rect() {
        let b = bbox(100.0, 100.0, 50.0, 50.0, 0.9); // xmax < xmin
        assert!(bbox_to_rect(&b, 200, 200).is_none());
    }
}
