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
//!  5. Run the raw engine output through
//!     [`postprocess::clean`](super::postprocess::clean) — junk-token
//!     and confidence filtering live there, not in the recognizers.
//!
//! Detector inference + recognize run inside [`tokio::task::spawn_blocking`]
//! so the reactor isn't stalled. The detector + recognizer singletons
//! are held by `&'static` reference, which is `Send + Sync` thanks to
//! the [`Recognizer: Send + Sync`][crate::ocr::recognizer::Recognizer]
//! bound.
//!
//! ## Models first, then the permit, held by the work
//!
//! The handler (`api::issue_ocr`) waits for the models
//! ([`prepare_models`] / [`prepare_detector`]: the first call waits on
//! the verified download and builds the sessions) *before* it takes an OCR
//! permit, so a first-use download never holds the permits. The permit is
//! then handed to the blocking work itself ([`blocking_with_permit`],
//! [`run_ocr`], [`detect_and_cache_regions`]): it is released when that
//! work finishes, not when the request future is dropped. A client that
//! fires and aborts requests cannot start more concurrent decodes or
//! inferences than there are permits.

use image::DynamicImage;
use redis::aio::ConnectionManager;

use super::cache::{self, CachedBbox, CachedDetection};
use super::detector::Detector;
use super::postprocess::{self, Cleaned};
use super::recognizer::{Language, Recognizer, manga::MangaOcr, western::WesternOcr};

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
    /// Post-processed recognition. Word bboxes (when present) are in
    /// **page**-pixel coordinates — the crop offset is already added
    /// back so API consumers don't need the crop origin.
    pub recognition: Cleaned,
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

/// The initialised singletons one OCR run uses.
pub struct OcrModels {
    detector: Option<&'static Detector>,
    recognizer: &'static dyn Recognizer,
}

/// Wait for the models a run needs: the recognizer for `language`, and the
/// detector when `detect` is set. The first call per model waits on its
/// verified download (`ocr::models`) and builds the session; later calls
/// return at once. Call this before taking an OCR permit.
pub async fn prepare_models(language: Language, detect: bool) -> anyhow::Result<OcrModels> {
    // Only initialize the detector singleton when we're actually
    // going to use it. The first-call cost includes a HF download +
    // ort session build — skipping that on the no-detect path means
    // operators with detection disabled never pay it.
    let detector = if detect {
        Some(Detector::shared().await?)
    } else {
        None
    };
    let recognizer: &'static dyn Recognizer = match language {
        Language::Western => WesternOcr::shared().await?,
        Language::Manga => MangaOcr::shared().await?,
    };
    Ok(OcrModels {
        detector,
        recognizer,
    })
}

/// [`prepare_models`] for the detector alone (the text-regions route).
pub async fn prepare_detector() -> anyhow::Result<&'static Detector> {
    Detector::shared().await
}

/// Run `f` on the blocking pool with `permit` moved into the task: the
/// permit is released when `f` has finished, even when the caller's future
/// was dropped first. Returns `f`'s result and the permit, so a caller can
/// carry the permit on to the next stage.
pub async fn blocking_with_permit<P, T, F>(permit: P, f: F) -> anyhow::Result<(T, P)>
where
    P: Send + 'static,
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || (f(), permit))
        .await
        .map_err(|e| anyhow::anyhow!("blocking task failed: {e}"))
}

/// Driver: returns the recognized text + a refined bubble outline.
///
/// `models` come from [`prepare_models`] (the async download and session
/// build cannot run inside `spawn_blocking`). `permit` is held by the
/// blocking task until it finishes (see the module docs).
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
pub async fn run_ocr<P: Send + 'static>(
    input: OcrInput,
    models: OcrModels,
    permit: P,
) -> anyhow::Result<OcrOutput> {
    // Cancellation (OCR-1 / PERF-9): a `spawn_blocking` task keeps running
    // detached if this future is dropped (client disconnected / aborted). The
    // drop guard flips this flag, and the blocking body checks it after the
    // detector run — bailing before the expensive recognize step so the
    // recognizer singleton is freed for the next request instead of being
    // pinned for the full pipeline on work nobody is waiting for.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = CancelGuard(cancel.clone());

    let lang_label = match input.language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    let start = std::time::Instant::now();
    let OcrModels {
        detector,
        recognizer,
    } = models;
    // A model the caller did not prepare for this request is not used.
    let detector = if input.detect { detector } else { None };
    if input.detect && detector.is_none() {
        return Err(anyhow::anyhow!("detector requested but not prepared"));
    }

    // ─── Detect (with per-page cache) ────────────────────────────
    // When `detect` is off we skip the cache lookup + detector run
    // entirely. The blocking task receives `None` for both and falls
    // through to "OCR the user's rect verbatim".
    let (detect_key, cached_detection) = if input.detect {
        let key = cache::detect_cache_key(&input.content_hash, input.page);
        let cached = cache::get_detect(&input.redis, &key).await;
        (Some(key), cached)
    } else {
        (None, None)
    };

    let redis_for_put = input.redis.clone();
    let detect_key_for_put = detect_key.clone();
    let cancel_for_task = cancel.clone();
    let ((output, detection_to_cache), _permit) = blocking_with_permit(permit, move || {
        run_blocking(
            detector,
            recognizer,
            input,
            lang_label,
            cached_detection,
            &cancel_for_task,
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))
    .and_then(|(r, p)| r.map(|v| (v, p)))?;

    if let (Some(key), Some(detection)) = (detect_key_for_put, detection_to_cache) {
        cache::put_detect(&redis_for_put, &key, &detection).await;
    }

    metrics::histogram!("folio_ocr_pipeline_seconds", "lang" => lang_label)
        .record(start.elapsed().as_secs_f64());
    Ok(output)
}

/// Sets its flag when dropped — used by [`run_ocr`] to signal the detached
/// blocking task that the request was abandoned.
struct CancelGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Blocking body of [`run_ocr`]: runs the detector (if not cached),
/// picks the bbox overlapping the user's rect, crops, and recognizes.
/// Returns `(output, detection_to_cache)` — `detection_to_cache` is
/// `Some` only when the detector ran inline (cache miss), telling
/// the async caller to schedule a Redis PUT.
fn run_blocking(
    detector: Option<&Detector>,
    recognizer: &dyn Recognizer,
    input: OcrInput,
    lang_label: &'static str,
    cached_detection: Option<CachedDetection>,
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<(OcrOutput, Option<CachedDetection>)> {
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
    //   - detect=true + cache miss → run detector, return output to cache
    let (refined_page, detection_to_cache) = match (detector, cached_detection) {
        (Some(_), Some(detection)) => (
            pick_bbox_page(&detection.bboxes, &clamped, page_w, page_h),
            None,
        ),
        (Some(det), None) => {
            let detection = run_detector_blocking(det, &input.page_image)?;
            let picked = pick_bbox_page(&detection.bboxes, &clamped, page_w, page_h);
            (picked, Some(detection))
        }
        (None, _) => (None, None),
    };

    // The detector run above can take seconds; if the request was abandoned in
    // the meantime, bail before recognize (which also holds the recognizer
    // singleton's lock). The returned Err is discarded — the caller's future is
    // already gone.
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(anyhow::anyhow!("ocr request cancelled before recognize"));
    }

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

    let mut cleaned = postprocess::clean(recognition, input.language);
    // Word bboxes come back in crop coordinates; shift them into
    // page space here, where the crop origin is still known.
    if let Some(words) = cleaned.words.as_mut() {
        let origin = refined_bbox.unwrap_or(clamped);
        let (ox, oy) = (origin.x as f32, origin.y as f32);
        for w in words {
            w.xmin += ox;
            w.xmax += ox;
            w.ymin += oy;
            w.ymax += oy;
        }
    }

    Ok((
        OcrOutput {
            recognition: cleaned,
            refined_bbox,
        },
        detection_to_cache,
    ))
}

/// Run one full-page detector inference. Blocking — call from
/// inside `spawn_blocking`. The mutex serializes concurrent callers
/// through the process-wide ONNX session.
fn run_detector_blocking(
    det: &Detector,
    page_image: &DynamicImage,
) -> anyhow::Result<CachedDetection> {
    let det_out = {
        let mut d = det.lock()?;
        d.inference(page_image, DETECTOR_CONF, DETECTOR_NMS)?
    };
    Ok(CachedDetection {
        page_w: page_image.width(),
        page_h: page_image.height(),
        bboxes: det_out
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
            .collect(),
    })
}

/// Full-page detection for the text-regions endpoint: run the
/// detector over an owned page image and store the result under the
/// page's detect-cache key — the same entry [`run_ocr`] reads, so a
/// regions fetch makes every subsequent OCR on that page
/// recognize-only.
///
/// Callers are expected to have checked [`cache::get_detect`] first;
/// the lookup happening before this call is what lets the hit path
/// skip the archive load + page decode entirely.
pub async fn detect_and_cache_regions<P: Send + 'static>(
    redis: ConnectionManager,
    content_hash: &str,
    page: u32,
    page_image: DynamicImage,
    detector: &'static Detector,
    permit: P,
) -> anyhow::Result<CachedDetection> {
    let start = std::time::Instant::now();
    let (detection, _permit) =
        blocking_with_permit(permit, move || run_detector_blocking(detector, &page_image))
            .await
            .map_err(|e| anyhow::anyhow!("detect task panicked: {e}"))?;
    let detection = detection?;
    metrics::histogram!("folio_ocr_detect_seconds").record(start.elapsed().as_secs_f64());
    let key = cache::detect_cache_key(content_hash, page);
    cache::put_detect(&redis, &key, &detection).await;
    Ok(detection)
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

    /// Dropping the caller's future does not release the permit while the
    /// blocking work is still running; it is released when the work ends.
    #[tokio::test]
    async fn the_permit_lives_as_long_as_the_blocking_work() {
        use std::sync::Arc;
        use std::time::Duration;
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = sem.clone().acquire_owned().await.unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let work = blocking_with_permit(permit, move || {
            started_tx.send(()).unwrap();
            rx.recv().unwrap();
        });
        // Poll until the work has started, then abandon the future.
        let abandoned = tokio::time::timeout(Duration::from_millis(50), work).await;
        assert!(abandoned.is_err(), "still running");
        started_rx.recv().unwrap();
        assert_eq!(sem.available_permits(), 0, "held by the running work");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(sem.available_permits(), 0, "still held after the drop");
        tx.send(()).unwrap();
        for _ in 0..200 {
            if sem.available_permits() == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("permit not released when the work finished");
    }
}
