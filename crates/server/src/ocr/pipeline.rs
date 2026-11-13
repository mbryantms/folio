//! `detect → snap to a region → crop → recognize → clean` orchestration.
//!
//! Callers hand us the **full page** image, the user's rect in
//! page-pixel coordinates, and a content-hash + page key for the
//! detector-result cache. We:
//!
//!  1. Look up the cached detection for `(content_hash, page)`.
//!  2. On miss, run the page detector over the **full page** and store
//!     its boxes in Redis, so every later read on that page skips it.
//!  3. Pick the region whose intersection with the user's rect is
//!     largest; fall back to the user's rect verbatim if none
//!     overlap.
//!  4. Crop to that rect and hand the image to the selected recognizer.
//!     (A Japanese request may read a narrower box: see
//!     [`manga_crop`](super::manga_crop).)
//!  5. Run the raw engine output through
//!     [`postprocess::clean_for`](super::postprocess::clean_for): the
//!     cleanup rules live there, not in the recognizers.
//!
//! Steps 1–2 run only for a request that needs the page's detection
//! ([`uses_detection`]): a `detect: true` request, which also snaps
//! (step 3), and a Japanese request, which looks for the text box inside
//! its region. A Western tap on a region the client already has sends
//! that region's rect and is read as is, without the detector.
//!
//! Which engines do the work is the [`EngineSet`] the caller passes to
//! [`prepare_models`] / [`prepare_detector`]: nothing below reads it from
//! configuration, so a tool can run both sets in one process.
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
//!
//! ## Finished work always reaches the cache
//!
//! [`run_ocr`] and [`detect_and_cache_regions`] run their work in a
//! detached task (`tokio::spawn`) that owns the permit: the blocking
//! inference, then (after the permit is released) the Redis writes of the
//! detection and the recognition. The request future only waits for that
//! task. When the 60 s route timeout or a client abort drops the request,
//! the task carries on, so a detection that outlived its request is
//! cached and the retry is a cache hit instead of a second detection.
//!
//! Abandoned work cannot pile up: the inference runs only inside a task
//! that holds one of the [`OCR_MAX_CONCURRENT`] permits, and a new request
//! must take a permit before it starts any (waiting at most
//! [`OCR_PERMIT_WAIT`], then 503). So at most `OCR_MAX_CONCURRENT`
//! detections or recognitions, abandoned or not, run at any moment. What
//! outlives the permit is one Redis write per finished result, each
//! bounded by [`cache::CACHE_WRITE_TIMEOUT`].
//!
//! [`OCR_MAX_CONCURRENT`]: crate::api::issue_ocr::OCR_MAX_CONCURRENT
//! [`OCR_PERMIT_WAIT`]: crate::api::issue_ocr::OCR_PERMIT_WAIT

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::DynamicImage;
use redis::aio::ConnectionManager;

use super::EngineSet;
use super::bubbles::BubbleDetector;
use super::cache::{self, CachedBbox, CachedDetection};
use super::detector::Detector;
use super::manga_crop::{MANGA_CROP, MangaCrop, manga_crop};
use super::page_cache::SharedPage;
use super::postprocess::{self, Cleaned};
use super::recognizer::{Language, Recognizer, ctc::LineOcr, manga::MangaOcr, western::WesternOcr};
use super::regions;
use crate::api::issue_ocr::OcrResponse;

/// Full-page text detection. The real ones are [`BubbleDetector`] and,
/// with the legacy engine set, [`Detector`]; debug builds let a test
/// stand in for it per content hash ([`test_models`]).
pub trait PageDetector: Send + Sync {
    /// Detect the text regions of a whole page. Blocking.
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection>;
}

impl PageDetector for Detector {
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection> {
        let started = std::time::Instant::now();
        let detection = run_detector_blocking(self, page)?;
        self.note_inference(started.elapsed());
        Ok(detection)
    }
}

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
    pub(crate) fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    pub(crate) fn intersection(&self, other: &Rect) -> Option<Rect> {
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
    /// The decoded page, shared with the decoded-page cache
    /// ([`super::page_cache`]).
    pub page_image: SharedPage,
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
    /// When `true`, run the detector + snap to the detected region that
    /// overlaps the user's rect most. When `false`, the pipeline skips
    /// the detector entirely and the recognizer runs on the user's rect
    /// verbatim — see [`crate::api::issue_ocr::OcrRequest::detect`].
    pub detect: bool,
    /// Result-cache key the finished response is written under
    /// ([`cache::cache_key`]), by the work itself.
    pub result_key: String,
}

#[derive(Debug)]
pub struct OcrOutput {
    /// Post-processed recognition. Word bboxes (when present) are in
    /// **page**-pixel coordinates — the crop offset is already added
    /// back so API consumers don't need the crop origin.
    pub recognition: Cleaned,
    /// The detector box that was read, in page-pixel coordinates, when
    /// one decided the crop ([`choose_crop`]); `None` when the user's
    /// region was read as sent.
    pub refined_bbox: Option<Rect>,
}

/// Legacy detector (`comic-text-detector`): confidence threshold below
/// which a candidate bbox is discarded. 0.5 matches dmMaze's default.
const DETECTOR_CONF: f32 = 0.5;
/// Legacy detector: non-max-suppression IoU threshold.
const DETECTOR_NMS: f32 = 0.5;

/// The initialised singletons one OCR run uses, and the engine set they
/// belong to (it picks the cleanup rules and the detect-cache key).
pub struct OcrModels {
    engines: EngineSet,
    detector: Option<&'static dyn PageDetector>,
    recognizer: &'static dyn Recognizer,
}

/// Whether a run needs the page's detection: to snap the user's rect to
/// a region (`detect`), or, with the current engines, to look for the
/// text box of a Japanese request ([`MangaCrop::TextInRegion`]).
pub fn uses_detection(
    engines: EngineSet,
    language: Language,
    detect: bool,
    manga: MangaCrop,
) -> bool {
    detect
        || (engines == EngineSet::Current
            && language == Language::Manga
            && manga == MangaCrop::TextInRegion)
}

/// Wait for the models a run needs: the recognizer for `language`, and the
/// detector when the run needs the page's detection ([`uses_detection`]),
/// both of the `engines` set. The first call per model waits on its
/// verified download (`ocr::models`) and builds the session; later calls
/// return at once. Call this before taking an OCR permit. `content_hash`
/// only selects a test stand-in in debug builds ([`test_models`]).
pub async fn prepare_models(
    engines: EngineSet,
    content_hash: &str,
    language: Language,
    detect: bool,
) -> anyhow::Result<OcrModels> {
    #[cfg(debug_assertions)]
    let stub = test_models::get(content_hash);
    #[cfg(not(debug_assertions))]
    let _ = content_hash;
    // Only initialize the detector singleton when we're actually
    // going to use it. The first-call cost includes a HF download +
    // ort session build — skipping that on the no-detect path means
    // operators with detection disabled never pay it.
    let detect = uses_detection(engines, language, detect, MANGA_CROP);
    let detector: Option<&'static dyn PageDetector> = if detect {
        #[cfg(debug_assertions)]
        if let Some(d) = stub.and_then(|s| s.detector) {
            return Ok(OcrModels {
                engines,
                detector: Some(d),
                recognizer: match stub.and_then(|s| s.recognizer) {
                    Some(r) => r,
                    None => real_recognizer(engines, language).await?,
                },
            });
        }
        Some(real_detector(engines).await?)
    } else {
        None
    };
    #[cfg(debug_assertions)]
    if let Some(r) = stub.and_then(|s| s.recognizer) {
        return Ok(OcrModels {
            engines,
            detector,
            recognizer: r,
        });
    }
    Ok(OcrModels {
        engines,
        detector,
        recognizer: real_recognizer(engines, language).await?,
    })
}

/// The page detector of an engine set. Pages of every language use it.
async fn real_detector(engines: EngineSet) -> anyhow::Result<&'static dyn PageDetector> {
    Ok(match engines {
        EngineSet::Current => BubbleDetector::shared().await?,
        EngineSet::Legacy => Detector::shared().await?,
    })
}

/// The recognizer for a language. Japanese is the `manga-ocr` model with
/// either engine set.
async fn real_recognizer(
    engines: EngineSet,
    language: Language,
) -> anyhow::Result<&'static dyn Recognizer> {
    Ok(match (language, engines) {
        (Language::Western, EngineSet::Current) => LineOcr::shared().await?,
        (Language::Western, EngineSet::Legacy) => WesternOcr::shared().await?,
        (Language::Manga, _) => MangaOcr::shared().await?,
    })
}

/// [`prepare_models`] for the detector alone (the text-regions route).
pub async fn prepare_detector(
    engines: EngineSet,
    content_hash: &str,
) -> anyhow::Result<&'static dyn PageDetector> {
    #[cfg(debug_assertions)]
    if let Some(d) = test_models::get(content_hash).and_then(|s| s.detector) {
        return Ok(d);
    }
    #[cfg(not(debug_assertions))]
    let _ = content_hash;
    real_detector(engines).await
}

/// Debug-build stand-ins for the models, per content hash, so the
/// integration tests can drive the routes (timing, cancellation, caching)
/// without the real detector and recognizers, which download hundreds of
/// MB on first use. Keyed by content hash so tests running side by side
/// do not see each other's stand-ins. Not compiled into release builds.
#[cfg(debug_assertions)]
pub mod test_models {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use super::PageDetector;
    use crate::ocr::recognizer::Recognizer;

    /// What to use instead of the real models for one content hash; a
    /// `None` field keeps the real model.
    #[derive(Clone, Copy, Default)]
    pub struct Stub {
        pub detector: Option<&'static dyn PageDetector>,
        pub recognizer: Option<&'static dyn Recognizer>,
    }

    static STUBS: LazyLock<Mutex<HashMap<String, Stub>>> = LazyLock::new(Default::default);

    /// Use `stub` for every request on an issue with this content hash.
    pub fn install(content_hash: &str, stub: Stub) {
        STUBS
            .lock()
            .expect("test model lock")
            .insert(content_hash.to_owned(), stub);
    }

    /// Whether a test installed stand-ins for this content hash.
    pub fn is_installed(content_hash: &str) -> bool {
        get(content_hash).is_some()
    }

    pub(super) fn get(content_hash: &str) -> Option<Stub> {
        STUBS
            .lock()
            .expect("test model lock")
            .get(content_hash)
            .copied()
    }
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

/// Driver: runs the OCR work and returns the response the handler sends.
///
/// `models` come from [`prepare_models`] (the async download and session
/// build cannot run inside `spawn_blocking`). `permit` is held by the
/// blocking work until it finishes (see the module docs). The work runs in
/// a detached task that also writes the detect cache (when the detector
/// ran) and the result cache (under [`OcrInput::result_key`]); both writes
/// happen even when this future is dropped first.
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
) -> anyhow::Result<OcrResponse> {
    // Cancellation (OCR-1 / PERF-9): the work below keeps running when
    // this future is dropped (client disconnected / aborted / route
    // timeout). The drop guard flips this flag, and the blocking body
    // checks it after the detector run — skipping the recognize step so
    // the recognizer singleton is freed for the next request instead of
    // being pinned for work nobody is waiting for. The detection itself
    // is still cached.
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelGuard(cancel.clone());

    let lang_label = match input.language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    let start = std::time::Instant::now();
    let OcrModels {
        engines,
        detector,
        recognizer,
    } = models;
    // A model the caller did not prepare for this request is not used.
    let wants_detection = uses_detection(engines, input.language, input.detect, MANGA_CROP);
    let detector = if wants_detection { detector } else { None };
    if wants_detection && detector.is_none() {
        return Err(anyhow::anyhow!("detector requested but not prepared"));
    }

    // ─── Detect (with per-page cache) ────────────────────────────
    // When the run needs no detection we skip the cache lookup +
    // detector run entirely. The blocking task receives `None` for both
    // and falls through to "OCR the user's rect verbatim".
    let (detect_key, cached_detection) = if wants_detection {
        let key = cache::detect_cache_key_for(engines, &input.content_hash, input.page);
        let cached = cache::get_detect(&input.redis, &key).await;
        (Some(key), cached)
    } else {
        (None, None)
    };

    let redis = input.redis.clone();
    let result_key = input.result_key.clone();
    let page_size = (input.page_image.width(), input.page_image.height());
    let gone = cancel.clone();
    let work = tokio::spawn(async move {
        let result = async move {
            let ((detection_to_cache, output), permit) = blocking_with_permit(permit, move || {
                run_blocking(
                    engines,
                    detector,
                    recognizer,
                    input,
                    lang_label,
                    cached_detection,
                    &cancel,
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))?;
            // The permit covers the inference only; the cache writes below do
            // not hold it.
            drop(permit);
            if let (Some(key), Some(detection)) = (detect_key, detection_to_cache) {
                cache::put_detect(&redis, &key, &detection).await;
            }
            let response = OcrResponse::from_output(output?, lang_label, page_size);
            // Best-effort: a failed write only means the next call reruns.
            cache::put(&redis, &result_key, &response).await;
            metrics::histogram!("folio_ocr_pipeline_seconds", "lang" => lang_label)
                .record(start.elapsed().as_secs_f64());
            Ok(response)
        }
        .await;
        log_if_unheard(&result, &gone, "ocr");
        result
    });
    work.await
        .map_err(|e| anyhow::anyhow!("ocr task failed: {e}"))?
}

/// The error [`recognize_blocking`] returns when it skips recognition because
/// the request was abandoned (not a failure, so not logged).
#[derive(Debug)]
struct CancelledBeforeRecognize;

impl std::fmt::Display for CancelledBeforeRecognize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ocr request cancelled before recognize")
    }
}

impl std::error::Error for CancelledBeforeRecognize {}

/// Logs, once at warn, an error from detached OCR work whose request is gone:
/// nobody else receives it. Skipping recognition because the request was gone
/// is not a failure and is not logged.
fn log_if_unheard<T>(result: &anyhow::Result<T>, gone: &AtomicBool, work: &str) {
    if let Err(e) = result
        && gone.load(Ordering::Relaxed)
        && !e.is::<CancelledBeforeRecognize>()
    {
        tracing::warn!(error = %e, work, "ocr: detached work failed after the request was gone");
    }
}

/// The longest panic message logged from a caught recognizer panic.
const PANIC_MESSAGE_MAX_CHARS: usize = 200;

/// The message of a caught panic when its payload is a string, cut to
/// [`PANIC_MESSAGE_MAX_CHARS`].
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    message.chars().take(PANIC_MESSAGE_MAX_CHARS).collect()
}

/// Sets its flag when dropped — used by [`run_ocr`] to signal the detached
/// blocking task that the request was abandoned.
struct CancelGuard(Arc<AtomicBool>);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Blocking body of [`run_ocr`]: runs the detector (if not cached),
/// picks the bbox overlapping the user's rect, crops, and recognizes.
/// Returns `(detection_to_cache, output)` — `detection_to_cache` is
/// `Some` whenever the detector ran inline (cache miss) and succeeded,
/// even when the recognition then failed or was skipped because the
/// request was gone, so the caller always caches a finished detection.
fn run_blocking(
    engines: EngineSet,
    detector: Option<&dyn PageDetector>,
    recognizer: &dyn Recognizer,
    input: OcrInput,
    lang_label: &'static str,
    cached_detection: Option<CachedDetection>,
    cancel: &AtomicBool,
) -> (Option<CachedDetection>, anyhow::Result<OcrOutput>) {
    match recognize_blocking(
        engines,
        detector,
        recognizer,
        input,
        lang_label,
        cached_detection,
        cancel,
    ) {
        Ok((detection, output)) => (detection, output),
        Err(e) => (None, Err(e)),
    }
}

/// [`run_blocking`] with the detector's own failures as the outer error.
#[allow(clippy::type_complexity)]
fn recognize_blocking(
    engines: EngineSet,
    detector: Option<&dyn PageDetector>,
    recognizer: &dyn Recognizer,
    input: OcrInput,
    lang_label: &'static str,
    cached_detection: Option<CachedDetection>,
    cancel: &AtomicBool,
) -> anyhow::Result<(Option<CachedDetection>, anyhow::Result<OcrOutput>)> {
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
    //   - no detection needed → no detector handle; skip to recognize
    //   - needed + cache hit → use the cached boxes
    //   - needed + cache miss → run detector, return output to cache
    let (detection, ran_detector) = match (detector, cached_detection) {
        (Some(_), Some(detection)) => (Some(detection), false),
        (Some(det), None) => {
            let detection = det.detect(&input.page_image)?;
            if cancel.load(Ordering::Relaxed) {
                metrics::counter!("folio_ocr_detect_abandoned_total").increment(1);
            }
            (Some(detection), true)
        }
        (None, _) => (None, false),
    };
    let crop = choose_crop(
        engines,
        input.language,
        input.detect,
        MANGA_CROP,
        clamped,
        detection.as_ref().map_or(&[][..], |d| &d.bboxes[..]),
        (page_w, page_h),
    );
    let detection_to_cache = if ran_detector { detection } else { None };

    // The detector run above can take seconds; if the request was abandoned in
    // the meantime, skip recognize (which also holds the recognizer
    // singleton's lock). The detection is still handed back for the cache.
    if cancel.load(Ordering::Relaxed) {
        return Ok((
            detection_to_cache,
            Err(anyhow::Error::new(CancelledBeforeRecognize)),
        ));
    }

    // ─── Crop, recognize ─────────────────────────────────────────
    let Crop { read, refined_bbox } = crop;
    let final_img = input.page_image.crop_imm(read.x, read.y, read.w, read.h);

    let recognize_start = std::time::Instant::now();
    // A failed recognition (an error, or a panic from the engine) must not
    // lose a detection that already finished: it is handed back for the
    // cache either way. A panic is caught here for that reason only; an
    // engine mutex that was held at that moment is poisoned by it, as
    // before (Tesseract and manga-ocr hold theirs for the whole call, the
    // sessions built here for the inference only).
    let recognized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recognizer.recognize(&final_img)
    }));
    let recognition = match recognized {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Ok((detection_to_cache, Err(e))),
        Err(payload) => {
            tracing::error!(
                lang = lang_label,
                panic = %panic_message(payload.as_ref()),
                "ocr: the recognizer panicked; an engine whose lock was held is unusable until restart"
            );
            metrics::counter!("folio_ocr_recognizer_panics_total", "model" => lang_label)
                .increment(1);
            return Ok((
                detection_to_cache,
                Err(anyhow::anyhow!("the recognizer panicked")),
            ));
        }
    };
    metrics::histogram!("folio_ocr_recognize_seconds", "lang" => lang_label)
        .record(recognize_start.elapsed().as_secs_f64());

    let mut cleaned = postprocess::clean_for(recognition, input.language, engines);
    // Word (or line) boxes come back in crop coordinates; shift them
    // into page space here, where the crop origin is still known.
    if let Some(words) = cleaned.words.as_mut() {
        let (ox, oy) = (read.x as f32, read.y as f32);
        for w in words {
            w.xmin += ox;
            w.xmax += ox;
            w.ymin += oy;
            w.ymax += oy;
        }
    }

    Ok((
        detection_to_cache,
        Ok(OcrOutput {
            recognition: cleaned,
            refined_bbox,
        }),
    ))
}

/// Run one full-page inference of the legacy detector. Blocking — call
/// from inside `spawn_blocking`. The mutex serializes concurrent callers
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
/// page's detect-cache key for `engines` (the set `detector` belongs
/// to) — the same entry [`run_ocr`] reads, so a regions fetch makes
/// every subsequent OCR on that page recognize-only.
///
/// The detection and the cache write run in a detached task that owns
/// `permit` (see the module docs): a detection that finishes after this
/// future was dropped is still cached, and counted in
/// `folio_ocr_detect_abandoned_total`.
///
/// Callers are expected to have checked [`cache::get_detect`] first;
/// the lookup happening before this call is what lets the hit path
/// skip the archive load + page decode entirely.
pub async fn detect_and_cache_regions<P: Send + 'static>(
    redis: ConnectionManager,
    engines: EngineSet,
    content_hash: &str,
    page: u32,
    page_image: SharedPage,
    detector: &'static dyn PageDetector,
    permit: P,
) -> anyhow::Result<CachedDetection> {
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelGuard(cancel.clone());
    let key = cache::detect_cache_key_for(engines, content_hash, page);
    let gone = cancel.clone();
    let work = tokio::spawn(async move {
        let result = async move {
            let start = std::time::Instant::now();
            let (detection, permit) = blocking_with_permit(permit, move || {
                let detection = detector.detect(&page_image);
                if detection.is_ok() && cancel.load(Ordering::Relaxed) {
                    metrics::counter!("folio_ocr_detect_abandoned_total").increment(1);
                }
                detection
            })
            .await
            .map_err(|e| anyhow::anyhow!("detect task panicked: {e}"))?;
            drop(permit);
            let detection = detection?;
            metrics::histogram!("folio_ocr_detect_seconds").record(start.elapsed().as_secs_f64());
            cache::put_detect(&redis, &key, &detection).await;
            Ok(detection)
        }
        .await;
        log_if_unheard(&result, &gone, "page detection");
        result
    });
    work.await
        .map_err(|e| anyhow::anyhow!("detect task failed: {e}"))?
}

/// The rectangle of the page one run reads, and what the response says
/// about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Crop {
    /// The rectangle handed to the recognizer.
    pub read: Rect,
    /// `read`, when a detector box decided it (a snap, or the text box
    /// of a Japanese request); `None` when the user's region was read
    /// as sent. Becomes the response's `refined_bbox`, which therefore
    /// always describes the box that was read.
    pub refined_bbox: Option<Rect>,
}

/// Decide what to read. `user` is the request's region clamped to the
/// page, `boxes` the page's detection (empty when the run has none).
///
/// 1. With `detect`, the snap candidate that overlaps `user` most
///    replaces it ([`pick_bbox_page`]); with no overlap `user` stays.
/// 2. A Japanese request with the current engines then goes through
///    [`manga_crop`], which narrows a rectangle that is one of the page's
///    regions (a tap, a snap) to the text box inside it; any other
///    rectangle is read as sent (`manga` chooses; the legacy engines
///    read the region as they always did).
pub(crate) fn choose_crop(
    engines: EngineSet,
    language: Language,
    detect: bool,
    manga: MangaCrop,
    user: Rect,
    boxes: &[CachedBbox],
    (page_w, page_h): (u32, u32),
) -> Crop {
    let snapped = if detect {
        pick_bbox_page(engines, boxes, &user, page_w, page_h)
    } else {
        None
    };
    let region = snapped.unwrap_or(user);
    let read = match (language, engines) {
        (Language::Manga, EngineSet::Current) => manga_crop(manga, region, boxes, page_w, page_h),
        (Language::Manga, EngineSet::Legacy) | (Language::Western, _) => region,
    };
    Crop {
        read,
        refined_bbox: (snapped.is_some() || read != region).then_some(read),
    }
}

/// Pick the box a drawn rectangle snaps to: the candidate with the
/// largest intersection area against the user's rect. Both are in
/// page-pixel coordinates. Candidates with zero overlap or that round
/// out to an empty rect are discarded.
///
/// - **Current engines**: the candidates are the page's regions
///   ([`regions::regions`]: balloons and free text, near-duplicates
///   removed), and among candidates with the same overlap the smallest
///   box wins (a rectangle drawn wholly inside a balloon that itself
///   lies in a larger region snaps to the balloon).
/// - **Legacy engines**: exactly what the release before the engine
///   change did. The candidates are every box of the detection, blocks
///   and lines alike, and among candidates with the same overlap the
///   last in detector order wins.
fn pick_bbox_page(
    engines: EngineSet,
    bboxes: &[CachedBbox],
    user: &Rect,
    page_w: u32,
    page_h: u32,
) -> Option<Rect> {
    let regions;
    let candidates: &[CachedBbox] = match engines {
        EngineSet::Current => {
            regions = regions::regions(engines, bboxes, page_w, page_h);
            &regions
        }
        EngineSet::Legacy => bboxes,
    };
    let overlapping = candidates.iter().filter_map(|b| {
        let r = bbox_to_rect(b, page_w, page_h)?;
        let overlap = r.intersection(user)?.area();
        Some((overlap, r))
    });
    match engines {
        EngineSet::Current => overlapping.max_by(|(overlap_a, a), (overlap_b, b)| {
            overlap_a.cmp(overlap_b).then(b.area().cmp(&a.area()))
        }),
        EngineSet::Legacy => overlapping.max_by_key(|(overlap, _)| *overlap),
    }
    .map(|(_, r)| r)
}

/// A detector box as a whole-pixel rectangle on a `w × h` page (rounded,
/// clamped); `None` when nothing of it is left.
pub(crate) fn bbox_to_rect(b: &CachedBbox, w: u32, h: u32) -> Option<Rect> {
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

    fn classed(class: u32, xmin: f32, ymin: f32, xmax: f32, ymax: f32) -> CachedBbox {
        CachedBbox {
            class,
            ..bbox(xmin, ymin, xmax, ymax, 0.9)
        }
    }

    const fn rect(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    /// A page with one balloon (class 0), its text (class 1) and a
    /// caption outside balloons (class 2).
    fn page_boxes() -> Vec<CachedBbox> {
        vec![
            classed(0, 100.0, 100.0, 300.0, 400.0),
            classed(1, 140.0, 130.0, 260.0, 370.0),
            classed(2, 500.0, 100.0, 800.0, 160.0),
        ]
    }

    const PAGE_SIZE: (u32, u32) = (1000, 1500);
    const BALLOON: Rect = rect(100, 100, 200, 300);
    const BALLOON_TEXT: Rect = rect(140, 130, 120, 240);

    /// A drawn rectangle snaps to a region (a balloon or free text) of
    /// the current engines, never to the text box inside a balloon, even
    /// when that overlaps it more.
    #[test]
    fn a_snap_takes_a_region_not_the_text_inside_a_balloon() {
        // Inside the balloon's text box entirely.
        let user = rect(150, 150, 60, 60);
        let boxes = page_boxes();
        assert_eq!(
            pick_bbox_page(EngineSet::Current, &boxes, &user, 1000, 1500),
            Some(BALLOON)
        );
        // On the caption.
        assert_eq!(
            pick_bbox_page(
                EngineSet::Current,
                &boxes,
                &rect(600, 110, 50, 20),
                1000,
                1500
            ),
            Some(rect(500, 100, 300, 60))
        );
        // Only the text box under it: nothing to snap to.
        let text_only = [classed(1, 140.0, 130.0, 260.0, 370.0)];
        assert_eq!(
            pick_bbox_page(EngineSet::Current, &text_only, &user, 1000, 1500),
            None
        );
        // The legacy detector's classes (0 block, 1 line) are all
        // regions, as before: the larger overlap wins whatever the class.
        let legacy = [
            classed(0, 100.0, 100.0, 300.0, 400.0),
            classed(1, 100.0, 100.0, 320.0, 420.0),
        ];
        assert_eq!(
            pick_bbox_page(
                EngineSet::Legacy,
                &legacy,
                &rect(250, 350, 100, 100),
                1000,
                1500
            ),
            Some(rect(100, 100, 220, 320))
        );
    }

    /// A rectangle drawn wholly inside a balloon that lies in a larger
    /// region overlaps both in full: with the current engines the snap
    /// takes the smaller box, whichever was detected first.
    #[test]
    fn a_snap_tie_goes_to_the_smaller_box() {
        let caption = classed(2, 50.0, 50.0, 900.0, 700.0);
        let balloon = classed(0, 100.0, 100.0, 300.0, 400.0);
        let user = rect(150, 150, 60, 60);
        for boxes in [
            [caption.clone(), balloon.clone()],
            [balloon.clone(), caption.clone()],
        ] {
            assert_eq!(
                pick_bbox_page(EngineSet::Current, &boxes, &user, 1000, 1500),
                Some(BALLOON)
            );
        }
        // A larger overlap still beats a smaller box.
        let reaching_out = rect(250, 150, 100, 60);
        assert_eq!(
            pick_bbox_page(
                EngineSet::Current,
                &[caption, balloon],
                &reaching_out,
                1000,
                1500
            ),
            Some(rect(50, 50, 850, 650))
        );
    }

    /// The legacy engines keep the tie rule of the release before the
    /// engine change: among boxes with the same overlap, the last in
    /// detector order, whatever its size.
    #[test]
    fn a_legacy_snap_tie_goes_to_the_last_box_in_detector_order() {
        let block = classed(0, 50.0, 50.0, 900.0, 700.0);
        let line = classed(1, 100.0, 100.0, 300.0, 400.0);
        let user = rect(150, 150, 60, 60);
        assert_eq!(
            pick_bbox_page(
                EngineSet::Legacy,
                &[block.clone(), line.clone()],
                &user,
                1000,
                1500
            ),
            Some(BALLOON),
            "the line, detected last"
        );
        assert_eq!(
            pick_bbox_page(EngineSet::Legacy, &[line, block], &user, 1000, 1500),
            Some(rect(50, 50, 850, 650)),
            "the block, detected last, although it is the larger box"
        );
    }

    /// With the current engines the snap sees the page's regions: of two
    /// boxes around the same region only the one that was kept. The
    /// legacy engines snap to any box of the detection, as before.
    #[test]
    fn a_snap_uses_the_de_duplicated_regions_of_the_current_engines() {
        let as_text = CachedBbox {
            confidence: 0.80,
            ..classed(2, 399.0, 381.0, 746.0, 427.0)
        };
        let as_balloon = CachedBbox {
            confidence: 0.85,
            ..classed(0, 400.0, 380.0, 747.0, 428.0)
        };
        let boxes = [as_balloon, as_text];
        // Inside both: without the de-duplication the tie would go to
        // the smaller, lower-scored box.
        let user = rect(500, 390, 50, 20);
        assert_eq!(
            pick_bbox_page(EngineSet::Current, &boxes, &user, 1000, 1500),
            Some(rect(400, 380, 347, 48))
        );
        // No de-duplication for the legacy engines: the last of the two.
        assert_eq!(
            pick_bbox_page(EngineSet::Legacy, &boxes, &user, 1000, 1500),
            Some(rect(399, 381, 347, 46))
        );
    }

    #[test]
    fn a_run_needs_the_detection_to_snap_or_to_find_japanese_text() {
        use EngineSet::{Current, Legacy};
        use Language::{Manga, Western};
        use MangaCrop::{Region, TextInRegion};
        // `detect` always does.
        for engines in [Current, Legacy] {
            for language in [Western, Manga] {
                for manga in [Region, TextInRegion] {
                    assert!(uses_detection(engines, language, true, manga));
                }
            }
        }
        // Without it, only a Japanese request of the current engines in
        // the text-box mode.
        assert!(uses_detection(Current, Manga, false, TextInRegion));
        assert!(!uses_detection(Current, Manga, false, Region));
        assert!(!uses_detection(Current, Western, false, TextInRegion));
        assert!(!uses_detection(Legacy, Manga, false, TextInRegion));
        // As the server is configured: a Japanese tap needs the page's
        // detection (to find the balloon's text box), a Western one
        // reads without it.
        assert!(uses_detection(Current, Manga, false, MANGA_CROP));
        assert!(!uses_detection(Current, Western, false, MANGA_CROP));
        assert!(!uses_detection(Legacy, Manga, false, MANGA_CROP));
    }

    fn crop_of(
        engines: EngineSet,
        language: Language,
        detect: bool,
        manga: MangaCrop,
        user: Rect,
    ) -> Crop {
        choose_crop(
            engines,
            language,
            detect,
            manga,
            user,
            &page_boxes(),
            PAGE_SIZE,
        )
    }

    /// Western text and the region mode: the user's rect as sent, or the
    /// region it snapped to; `refined_bbox` only for the snap.
    #[test]
    fn a_region_is_read_as_sent_or_as_snapped() {
        let drawn = rect(150, 150, 60, 60);
        for (language, manga) in [
            (Language::Western, MangaCrop::Region),
            (Language::Western, MangaCrop::TextInRegion),
            (Language::Manga, MangaCrop::Region),
        ] {
            let sent = crop_of(EngineSet::Current, language, false, manga, drawn);
            assert_eq!((sent.read, sent.refined_bbox), (drawn, None));
            let snapped = crop_of(EngineSet::Current, language, true, manga, drawn);
            assert_eq!(
                (snapped.read, snapped.refined_bbox),
                (BALLOON, Some(BALLOON))
            );
            // A tap sends the region itself: read as sent, nothing refined.
            let tap = crop_of(EngineSet::Current, language, false, manga, BALLOON);
            assert_eq!((tap.read, tap.refined_bbox), (BALLOON, None));
        }
        // Nothing under the rectangle: it is read as drawn.
        let nowhere = rect(900, 1400, 50, 50);
        let missed = crop_of(
            EngineSet::Current,
            Language::Western,
            true,
            MangaCrop::Region,
            nowhere,
        );
        assert_eq!((missed.read, missed.refined_bbox), (nowhere, None));
    }

    /// The text-box mode for Japanese: the balloon's text box is read,
    /// and `refined_bbox` is that box, whether the balloon was tapped or
    /// snapped to. With no clear text box the region is read.
    #[test]
    fn a_japanese_request_in_text_box_mode_reads_the_text_box_and_says_so() {
        let mode = MangaCrop::TextInRegion;
        let tap = crop_of(EngineSet::Current, Language::Manga, false, mode, BALLOON);
        assert_eq!(
            (tap.read, tap.refined_bbox),
            (BALLOON_TEXT, Some(BALLOON_TEXT))
        );
        let snapped = crop_of(
            EngineSet::Current,
            Language::Manga,
            true,
            mode,
            rect(150, 150, 60, 60),
        );
        assert_eq!(
            (snapped.read, snapped.refined_bbox),
            (BALLOON_TEXT, Some(BALLOON_TEXT))
        );
        // A rectangle drawn around the text that is not a region of the
        // page (no snap) is what the user enclosed: read as sent.
        let drawn = rect(130, 120, 140, 260);
        let sent = crop_of(EngineSet::Current, Language::Manga, false, mode, drawn);
        assert_eq!((sent.read, sent.refined_bbox), (drawn, None));
        // The caption has no text box in it: read whole, nothing refined
        // for a tap, the caption for a snap.
        let caption = rect(500, 100, 300, 60);
        let tap = crop_of(EngineSet::Current, Language::Manga, false, mode, caption);
        assert_eq!((tap.read, tap.refined_bbox), (caption, None));
        let snapped = crop_of(
            EngineSet::Current,
            Language::Manga,
            true,
            mode,
            rect(600, 110, 50, 20),
        );
        assert_eq!(
            (snapped.read, snapped.refined_bbox),
            (caption, Some(caption))
        );
    }

    /// The legacy engines read Japanese as they always did: the mode
    /// does not apply, and every class of that detector is a region.
    #[test]
    fn the_legacy_engines_ignore_the_japanese_crop_mode() {
        for manga in [MangaCrop::Region, MangaCrop::TextInRegion] {
            let tap = crop_of(EngineSet::Legacy, Language::Manga, false, manga, BALLOON);
            assert_eq!((tap.read, tap.refined_bbox), (BALLOON, None));
            // Snapping takes any box of the legacy detector: the drawn
            // rectangle lies in the block and in its line, and of two
            // full overlaps the one detected last wins (here the line).
            let snapped = crop_of(
                EngineSet::Legacy,
                Language::Manga,
                true,
                manga,
                rect(150, 150, 60, 60),
            );
            assert_eq!(
                (snapped.read, snapped.refined_bbox),
                (BALLOON_TEXT, Some(BALLOON_TEXT))
            );
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
        let chosen =
            pick_bbox_page(EngineSet::Current, &bboxes, &user, 1000, 1000).expect("pick a bbox");
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
        assert!(pick_bbox_page(EngineSet::Current, &bboxes, &user, 1000, 1000).is_none());
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
