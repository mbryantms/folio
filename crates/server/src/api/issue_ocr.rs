//! OCR endpoints: `POST /me/issues/{id}/ocr` (text-detection-1.0
//! plan, M3) and `GET /me/issues/{id}/pages/{page}/text-regions`
//! (OCR rework 1.0).
//!
//! The POST loads the page from the issue's archive, decodes to a
//! `DynamicImage`, and hands it off to the OCR pipeline. The
//! response shape is intentionally generous: `text` + `confidence`
//! are the headline fields; `refined_bbox` echoes the detector's
//! bubble outline when one was found, which the reader uses to
//! replace the user's drag rect with a tighter snap-to-bubble
//! marker region.
//!
//! The GET exposes the full-page detector output (percent
//! coordinates) so the reader can render tappable bubble outlines
//! in text mode. It shares the per-page detect cache with the OCR
//! pipeline — one regions fetch makes every subsequent bubble OCR
//! on that page recognize-only.
//!
//! ACL: same as page bytes — visible-to-user library check via
//! [`visible`]. Rate limits: the POST sits on the `OCR` bucket, the
//! GET on the stricter `OCR_DETECT` bucket (a detect-cache miss is
//! the most expensive single operation the server exposes).
//!
//! Concurrency (audit S-23): rate limits are per caller, so the heavy
//! part of both routes (page decode, detection, recognition) also runs
//! under one process-wide semaphore of [`OCR_MAX_CONCURRENT`] permits.
//! Order of work: visibility, the result cache (a hit never takes a
//! permit), the cheap checks (encrypted row, missing file, offline
//! library, page out of range, the entry's size against the per-entry
//! cap: their 404 / 409 / 410 / 503 never wait, and no page bytes are
//! read for them), the models (a first-use download is waited on
//! *without* a permit), then the permit. The page is read and decoded
//! only inside it, and only when the decoded-page cache
//! ([`crate::ocr::page_cache`]) does not already hold it: the balloon-by-
//! balloon requests on one page decode it once. The cache is consulted
//! after every check above, so a user who cannot see the issue gets the
//! same 404 whatever it holds. The permit moves into the blocking read, decode and
//! inference tasks and is released when they finish, not when the
//! request future is dropped (`ocr::pipeline`). A request that waits
//! longer than [`OCR_PERMIT_WAIT`] for one gets 503
//! `service_unavailable` with `Retry-After`, like the thumbnail backlog
//! answer.
//!
//! An issue whose page cannot be read answers by the shared mapping in
//! [`super::page_source`] (410 for a missing or unreadable archive, 404
//! for a refused path or a page past the end, 503 for an I/O error).

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use entity::{issue, series};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Semaphore, SemaphorePermit};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use super::page_source::{PageUnavailable, issue_not_found, no_store};
use crate::auth::CurrentUser;
use crate::library::zip_lru::{CachedReader, lock_reader};
use crate::middleware::rate_limit;
use crate::ocr::cache::{self, CachedDetection};
use crate::ocr::page_cache::{Claim, Lookup, PageKey, SharedPage, decoded_pages};
use crate::ocr::pipeline::{
    OcrInput, OcrOutput, Rect, blocking_with_permit, detect_and_cache_regions, prepare_detector,
    prepare_models, run_ocr,
};
use crate::ocr::recognizer::Language;
use crate::state::AppState;
use server_macros::handler;

/// OCR / detection runs allowed at once, process-wide. A constant, not a
/// setting: each run can hold a full decoded page plus model buffers, and
/// two keep a small host responsive while a reader works through a page.
pub const OCR_MAX_CONCURRENT: usize = 2;

/// How long a request waits for an OCR permit before it is told to retry.
pub const OCR_PERMIT_WAIT: Duration = Duration::from_secs(10);

static OCR_PERMITS: Semaphore = Semaphore::const_new(OCR_MAX_CONCURRENT);

/// The process-wide OCR semaphore, for tests and diagnostics (a test holds
/// its permits to show which requests need one).
pub fn ocr_semaphore() -> &'static Semaphore {
    &OCR_PERMITS
}

/// Seconds a request turned away by a full OCR semaphore should wait.
const OCR_RETRY_AFTER_SECS: &str = "2";

/// Take an OCR permit from `permits`, waiting up to `wait`. Over the limit
/// the answer is 503 `service_unavailable` with `Retry-After` (the
/// thumbnail-backlog convention), never a 500.
async fn ocr_permit(
    permits: &Semaphore,
    wait: Duration,
) -> Result<SemaphorePermit<'_>, Box<Response>> {
    let started = std::time::Instant::now();
    let acquired = tokio::time::timeout(wait, permits.acquire()).await;
    metrics::histogram!("folio_ocr_permit_wait_seconds").record(started.elapsed().as_secs_f64());
    match acquired {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) | Err(_) => {
            metrics::counter!("folio_ocr_busy_total").increment(1);
            let mut resp = super::respond(
                StatusCode::SERVICE_UNAVAILABLE,
                shared::error::ApiErrorCode::ServiceUnavailable,
                "OCR is busy; retry shortly",
            );
            resp.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(OCR_RETRY_AFTER_SECS),
            );
            Err(Box::new(no_store(resp)))
        }
    }
}

pub fn routes() -> OpenApiRouter<AppState> {
    // Two sub-routers so each endpoint gets its own rate bucket
    // without the `route_layer` leaking onto the other.
    let ocr = OpenApiRouter::new()
        .routes(routes!(serve))
        .route_layer(rate_limit::OCR.build());
    let detect = OpenApiRouter::new()
        .routes(routes!(text_regions))
        .route_layer(rate_limit::OCR_DETECT.build());
    ocr.merge(detect)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OcrRequest {
    /// 0-based page index, matching the reader's URL convention.
    pub page: u32,
    /// User's drag rectangle in page-pixel coordinates. All four
    /// fields are unsigned pixels; w/h must be > 0 and the rect
    /// must fit inside the decoded page bounds.
    pub region: OcrRegion,
    /// `"western"` or `"manga"`. Omitted → the server resolves a
    /// default: `series.text_language` if set, else `manga` when the
    /// series reads right-to-left, else `western`. The response
    /// echoes the resolved value in `lang`.
    #[serde(default)]
    pub lang: Option<String>,
    /// When `true`, run `comic-text-detector` over the page to snap
    /// the user's drag rect to the tightest enclosing speech-bubble
    /// polygon. When `false` (or omitted — that's the default as of
    /// v0.3.26), the recognizer runs on the user's rect verbatim.
    ///
    /// Why default off: on CPU-constrained hosts the detector costs
    /// ~50 s on the *first* bubble of a page (subsequent bubbles on
    /// the same page hit the polygon cache and run ~200 ms). That
    /// cold-start makes the snap-to-bubble feature unusable in
    /// practice. Operators on fast hardware can flip the request
    /// flag to `true` to opt in; a future server config knob will
    /// let them flip the default. The recognizer-only path
    /// (~200 ms total) matches what the old browser-side
    /// tesseract.js delivered, just better quality (real Tesseract
    /// 5 + tessdata_best, not the in-browser fast variant).
    #[serde(default)]
    pub detect: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, utoipa::ToSchema)]
pub struct OcrRegion {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

// Deserialize is needed so M4's Redis cache can round-trip a stored
// payload back into the response shape; the handler then re-serializes
// it for the client.
#[derive(Debug, Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct OcrResponse {
    /// Recognized text after the postprocess cleanup pass
    /// ([`crate::ocr::postprocess`]).
    pub text: String,
    /// Mean confidence in `[0.0, 1.0]` over the words kept by the
    /// cleanup pass (Tesseract); manga-ocr reports `1.0` (or `0.0`
    /// for an empty result) since the greedy decoder doesn't expose
    /// per-token probabilities.
    pub confidence: f32,
    /// Detector's snap-to-bubble rectangle in page-pixel coords.
    /// `None` when no detector hit overlapped the user's region —
    /// the recognizer ran on the user's rect verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refined_bbox: Option<OcrRegion>,
    /// Recognizer the pipeline actually ran (`"western"` |
    /// `"manga"`) — the request override or the server-side
    /// resolution chain. `default` so result-cache payloads written
    /// before this field existed still deserialize.
    #[serde(default)]
    pub lang: String,
    /// Engine output before cleanup. Present only when cleanup
    /// changed the text — a debugging aid, and the raw material for
    /// golden-fixture authoring when users report junk output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    /// Per-word boxes in page-pixel coords, post-cleanup. `None`
    /// when the engine doesn't expose word data (manga-ocr) or the
    /// iterator yielded nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<OcrWord>>,
}

/// One recognized word: cleaned text + confidence + page-pixel box.
#[derive(Debug, Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct OcrWord {
    pub text: String,
    /// `[0.0, 1.0]`, from Tesseract's per-word score.
    pub confidence: f32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Detected text regions for one page, in percent-of-page
/// coordinates (0–100 floats — the reader's native marker
/// representation, consumable by its SVG overlay without math).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TextRegionsView {
    /// Decoded page width in pixels. Doubles as a `naturalSize`
    /// fallback for clients that haven't measured the `<img>` yet.
    pub page_w: u32,
    /// Decoded page height in pixels.
    pub page_h: u32,
    pub regions: Vec<TextRegionView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TextRegionView {
    /// Left edge, percent of page width (0–100).
    pub x: f32,
    /// Top edge, percent of page height (0–100).
    pub y: f32,
    /// Width, percent of page width (0–100).
    pub w: f32,
    /// Height, percent of page height (0–100).
    pub h: f32,
    /// Detector confidence, `[0.0, 1.0]`.
    pub confidence: f32,
    /// Detector class index (0 = text block, 1 = text line).
    pub class: u32,
}

impl OcrResponse {
    /// The response for one finished pipeline run (`lang` is the
    /// recognizer that ran).
    pub(crate) fn from_output(out: OcrOutput, lang: &str) -> Self {
        OcrResponse {
            text: out.recognition.text,
            confidence: out.recognition.confidence,
            refined_bbox: out.refined_bbox.map(|r| OcrRegion {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            }),
            lang: lang.to_owned(),
            raw_text: out.recognition.raw_text,
            words: out.recognition.words.map(|ws| {
                ws.into_iter()
                    .map(|w| OcrWord {
                        text: w.text,
                        confidence: w.confidence,
                        x: w.xmin.max(0.0).round() as u32,
                        y: w.ymin.max(0.0).round() as u32,
                        w: (w.xmax - w.xmin).max(0.0).round() as u32,
                        h: (w.ymax - w.ymin).max(0.0).round() as u32,
                    })
                    .collect()
            }),
        }
    }
}

impl From<CachedDetection> for TextRegionsView {
    fn from(d: CachedDetection) -> Self {
        let (pw, ph) = (d.page_w as f32, d.page_h as f32);
        let regions = if pw <= 0.0 || ph <= 0.0 {
            Vec::new()
        } else {
            d.bboxes
                .iter()
                .filter_map(|b| {
                    let xmin = b.xmin.clamp(0.0, pw);
                    let ymin = b.ymin.clamp(0.0, ph);
                    let xmax = b.xmax.clamp(0.0, pw);
                    let ymax = b.ymax.clamp(0.0, ph);
                    if xmax <= xmin || ymax <= ymin {
                        return None;
                    }
                    Some(TextRegionView {
                        x: xmin / pw * 100.0,
                        y: ymin / ph * 100.0,
                        w: (xmax - xmin) / pw * 100.0,
                        h: (ymax - ymin) / ph * 100.0,
                        confidence: b.confidence,
                        class: b.class,
                    })
                })
                .collect()
        };
        TextRegionsView {
            page_w: d.page_w,
            page_h: d.page_h,
            regions,
        }
    }
}

#[utoipa::path(
    operation_id = "issue_ocr_serve",    post,
    path = "/me/issues/{id}/ocr",
    params(("id" = String, Path, description = "issue UUID")),
    request_body = OcrRequest,
    responses(
        (status = 200, body = OcrResponse),
        (status = 400, description = "invalid request body"),
        (status = 404, description = "issue or page not found"),
        (status = 409, description = "the issue's file is a CBR / CB7 that must be converted to CBZ first (`archive_needs_conversion`)"),
        (status = 410, description = "the issue's file is missing (`archive_missing`) or not a readable archive (`archive_unreadable`)"),
        (status = 500, description = "ocr pipeline failure"),
        (status = 503, description = "OCR busy (`service_unavailable`), the library's folder is offline (`library_offline`), or the file could not be read; carries Retry-After"),
    )
)]
#[handler]
pub async fn serve(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<String>,
    Json(req): Json<OcrRequest>,
) -> Response {
    // ─── Validate ────────────────────────────────────────────────
    if req.region.w == 0 || req.region.h == 0 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_region",
            "region w/h must be > 0",
        );
    }
    let lang_override = match req.lang.as_deref() {
        None => None,
        Some("western") => Some(Language::Western),
        Some("manga") => Some(Language::Manga),
        Some(other) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_lang",
                &format!("unknown lang {other:?}; expected western|manga"),
            );
        }
    };

    // ─── ACL ─────────────────────────────────────────────────────
    let Ok(Some(row)) = issue::Entity::find_by_id(id.clone()).one(&app.db).await else {
        return issue_not_found();
    };
    if !crate::library::access::issue_visible(&app, &user, &row).await {
        return issue_not_found();
    }

    // ─── Resolve language ────────────────────────────────────────
    // Explicit request override wins; otherwise consult the series.
    // The resolved value feeds the cache key, so western and manga
    // results for the same region never collide.
    let language = match lang_override {
        Some(l) => l,
        None => resolve_series_language(&app, row.series_id).await,
    };

    // ─── Resolve detect mode ─────────────────────────────────────
    // Default off: see [`OcrRequest::detect`] for the rationale —
    // the detector cold-start is too slow on most operator hosts to
    // run by default. Clients opt in explicitly.
    let detect = req.detect.unwrap_or(false);

    // ─── Cache lookup (M4) ───────────────────────────────────────
    // Key includes `content_hash` rather than `issue_id` so a rescan
    // that retags the same row with different bytes invalidates
    // every entry for free — old keys age out via [`cache::CACHE_TTL`].
    // It also includes the `detect` flag so snap-to-bubble results
    // and raw-rect results cache independently (their recognized
    // text differs).
    //
    // Region bounds aren't validated yet (that needs the decoded
    // page size) but anything we hand back from the cache passed
    // bounds at write time and the content_hash binding makes that
    // promise stick.
    let region_hash = cache::region_hash(req.region.x, req.region.y, req.region.w, req.region.h);
    let lang_str = match language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    // The keys carry the engine fingerprint (`ocr::cache`), computed once
    // per process off the async runtime.
    cache::warm_fingerprints().await;
    let key = cache::cache_key(&row.content_hash, req.page, lang_str, detect, &region_hash);
    if let Some(hit) = cache::get(&app.jobs.redis, &key).await {
        return Json(hit).into_response();
    }

    // ─── Cheap checks (no permit, no page bytes) ─────────────────
    let located = match locate_page(&app, &row, req.page).await {
        Ok(l) => l,
        Err(resp) => return *resp,
    };
    // ─── Models (a first-use download waits here, without a permit) ─
    let models = match prepare_models(&row.content_hash, language, detect).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "ocr: model preparation failed");
            return ocr_failure(&e, "ocr pipeline failed");
        }
    };
    // ─── Decode (under the OCR permit, held by the work) ─────────
    let page_key = PageKey::new(&row.content_hash, req.page);
    let (decoded, permit) = match page_image_holding(located, page_key).await {
        Ok(pair) => pair,
        Err(resp) => return *resp,
    };
    let (page_w, page_h) = (decoded.width(), decoded.height());
    if req.region.x.saturating_add(req.region.w) > page_w
        || req.region.y.saturating_add(req.region.h) > page_h
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_region",
            "region extends outside page bounds",
        );
    }

    // ─── Run OCR ─────────────────────────────────────────────────
    let input = OcrInput {
        page_image: decoded,
        region: Rect {
            x: req.region.x,
            y: req.region.y,
            w: req.region.w,
            h: req.region.h,
        },
        language,
        content_hash: row.content_hash.clone(),
        page: req.page,
        redis: app.jobs.redis.clone(),
        detect,
        // The work writes the result cache itself, so a result that
        // finishes after this request is gone is still cached.
        result_key: key,
    };
    match run_ocr(input, models, permit).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "ocr: pipeline failure");
            ocr_failure(&e, "ocr pipeline failed")
        }
    }
}

#[utoipa::path(
    operation_id = "issue_page_text_regions",    get,
    path = "/me/issues/{id}/pages/{page}/text-regions",
    params(
        ("id" = String, Path, description = "issue UUID"),
        ("page" = u32, Path, description = "0-based page index"),
    ),
    responses(
        (status = 200, body = TextRegionsView),
        (status = 404, description = "issue or page not found"),
        (status = 409, description = "the issue's file is a CBR / CB7 that must be converted to CBZ first (`archive_needs_conversion`)"),
        (status = 410, description = "the issue's file is missing (`archive_missing`) or not a readable archive (`archive_unreadable`)"),
        (status = 415, description = "page is not a decodable image"),
        (status = 500, description = "detector failure"),
        (status = 503, description = "OCR busy (`service_unavailable`), the library's folder is offline (`library_offline`), or the file could not be read; carries Retry-After"),
    )
)]
#[handler]
pub async fn text_regions(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((id, page)): AxPath<(String, u32)>,
) -> Response {
    // ─── ACL ─────────────────────────────────────────────────────
    let Ok(Some(row)) = issue::Entity::find_by_id(id.clone()).one(&app.db).await else {
        return issue_not_found();
    };
    if !crate::library::access::issue_visible(&app, &user, &row).await {
        return issue_not_found();
    }

    // ─── Cache lookup ────────────────────────────────────────────
    // Checked before the archive is touched: a hit skips page load
    // and decode entirely, making warm requests a Redis round-trip.
    cache::warm_fingerprints().await;
    let key = cache::detect_cache_key(&row.content_hash, page);
    if let Some(hit) = cache::get_detect(&app.jobs.redis, &key).await {
        return Json(TextRegionsView::from(hit)).into_response();
    }

    // ─── Cheap checks (no permit, no page bytes) ─────────────────
    let located = match locate_page(&app, &row, page).await {
        Ok(l) => l,
        Err(resp) => return *resp,
    };
    // ─── Detector (a first-use download waits here, no permit) ───
    let detector = match prepare_detector(&row.content_hash).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "ocr: detector preparation failed");
            return ocr_failure(&e, "text-region detection failed");
        }
    };
    // ─── Decode + detect (under the OCR permit, held by the work) ─
    let page_key = PageKey::new(&row.content_hash, page);
    let (decoded, permit) = match page_image_holding(located, page_key).await {
        Ok(pair) => pair,
        Err(resp) => return *resp,
    };
    match detect_and_cache_regions(
        app.jobs.redis.clone(),
        &row.content_hash,
        page,
        decoded,
        detector,
        permit,
    )
    .await
    {
        Ok(detection) => Json(TextRegionsView::from(detection)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "ocr: text-region detection failed");
            ocr_failure(&e, "text-region detection failed")
        }
    }
}

/// 500 for a failed OCR run. A model file that could not be fetched or
/// failed its digest check (`ocr::models`) gets its own code so the
/// client can say so; the file, repository and digests are in the server
/// log, not the response.
fn ocr_failure(e: &anyhow::Error, generic: &str) -> axum::response::Response {
    let unverified = e.chain().any(|c| {
        c.downcast_ref::<crate::ocr::models::ModelVerifyError>()
            .is_some()
    });
    if unverified {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ocr_model_unverified",
            "an OCR model could not be downloaded or failed its integrity check; see the server log",
        )
    } else {
        error(StatusCode::INTERNAL_SERVER_ERROR, "ocr_failed", generic)
    }
}

/// One page of an issue's archive, found but not read.
struct LocatedPage {
    reader: Arc<Mutex<CachedReader>>,
    entry: archive::ArchiveEntry,
}

/// Page entries read for OCR so far (each one inside a permit). For tests
/// and diagnostics: a test holding every permit checks no page is read.
static OCR_PAGE_READS: AtomicU64 = AtomicU64::new(0);

/// Pages read for OCR since start (see [`OCR_PAGE_READS`]).
pub fn ocr_page_reads() -> u64 {
    OCR_PAGE_READS.load(Ordering::Relaxed)
}

/// Page reads per content hash, debug builds only: lets tests running
/// side by side count the reads of their own issue.
#[cfg(debug_assertions)]
static OCR_PAGE_READS_BY_HASH: std::sync::LazyLock<Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::LazyLock::new(Default::default);

/// Pages read (and decoded) for OCR since start for one content hash.
/// Debug builds only; for tests.
#[cfg(debug_assertions)]
pub fn ocr_page_reads_for(content_hash: &str) -> u64 {
    OCR_PAGE_READS_BY_HASH
        .lock()
        .map(|m| m.get(content_hash).copied().unwrap_or(0))
        .unwrap_or(0)
}

/// The cheap checks that run before any OCR permit is taken, without
/// reading the page: encrypted row, refused / missing file, offline
/// library, unreadable archive, page out of range, and the entry's listed
/// size against the archive's per-entry cap. Failures come back as
/// ready-to-return canonical-envelope responses (the shared `page_source`
/// mapping) so the OCR POST and the text-regions GET answer alike. Boxed
/// because the error path is cold and `Response` is large.
async fn locate_page(
    app: &AppState,
    row: &issue::Model,
    page: u32,
) -> Result<LocatedPage, Box<Response>> {
    // Same rules as the page routes (`library::no_follow`). The reader is
    // built from this handle (or reused only for the same version of the
    // same file).
    PageUnavailable::check_row(row).map_err(|why| Box::new(why.response()))?;
    let archive_file = match crate::library::no_follow::open_issue_archive(app, row).await {
        Ok(f) => f,
        Err(refusal) => {
            return Err(Box::new(
                PageUnavailable::for_refusal(app, row, &refusal)
                    .await
                    .response(),
            ));
        }
    };
    let reader = match app.zip_lru.get_or_open(&row.id, archive_file).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %row.id, "ocr: zip_lru open failed");
            return Err(Box::new(PageUnavailable::from_archive_error(&e).response()));
        }
    };
    let max_entry_bytes = app.cfg().archive_limits().max_entry_bytes;
    let page_index = page as usize;
    let for_task = reader.clone();
    // The page list is in memory; the lock is taken off the runtime only
    // because a page stream may hold it for a while.
    let found = tokio::task::spawn_blocking(
        move || -> Result<Option<archive::ArchiveEntry>, archive::ArchiveError> {
            let cbz = lock_reader(&for_task)?;
            Ok(cbz.pages().get(page_index).copied().cloned())
        },
    )
    .await;
    let entry = match found {
        Ok(Ok(Some(entry))) => entry,
        Ok(Ok(None)) => {
            return Err(Box::new(no_store(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "page not found",
            ))));
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ocr: page lookup failed");
            return Err(Box::new(PageUnavailable::from_archive_error(&e).response()));
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: page lookup task failed");
            return Err(Box::new(PageUnavailable::Internal.response()));
        }
    };
    if entry.uncompressed_size > max_entry_bytes {
        let e = archive::ArchiveError::CapExceeded(archive::Cap::new(
            archive::SafetyCheck::EntrySize,
            "entry exceeded max_entry_bytes",
        ));
        return Err(Box::new(PageUnavailable::from_archive_error(&e).response()));
    }
    Ok(LocatedPage { reader, entry })
}

/// Why [`read_and_decode_holding`] produced no image.
enum ReadDecodeError {
    Read(archive::ArchiveError),
    Decode(String),
}

/// Take an OCR permit, then the decoded page: from the decoded-page cache,
/// by waiting for a decode of the same page already running, or by reading
/// and decoding it ([`read_and_decode_holding`]). The permit comes first so
/// the pages in use are bounded by the permits, and a waiter only ever
/// waits on a decode running under another permit. Callers run every
/// visibility and archive check before this; `key` comes from the issue
/// row, never from the request.
async fn page_image_holding(
    located: LocatedPage,
    key: PageKey,
) -> Result<(SharedPage, SemaphorePermit<'static>), Box<Response>> {
    let permit = ocr_permit(&OCR_PERMITS, OCR_PERMIT_WAIT).await?;
    let pages = decoded_pages();
    loop {
        match pages.lookup_or_claim(&key) {
            Lookup::Hit(page) => return Ok((page, permit)),
            // Woken when that decode is stored or failed; look again.
            Lookup::Wait(mut done) => {
                let _ = done.wait_for(|stored| *stored).await;
            }
            Lookup::Claimed(claim) => {
                return read_and_decode_holding(located, permit, claim).await;
            }
        }
    }
}

/// Read the located page and decode it on the blocking pool with the OCR
/// `permit` held by the task (released when the work ends, even if the
/// request is dropped first), store it under `claim` from inside that task,
/// and hand the permit back for the next stage. The page bytes exist only
/// inside the permit. A read error found here answers by the same
/// `page_source` mapping as the cheap checks (the claim is dropped, so a
/// request waiting for this page tries itself). `decode_limited`
/// (security M0) caps decoded dimensions so a malicious archive can't OOM
/// the box via a decompression bomb.
async fn read_and_decode_holding<P: Send + 'static>(
    located: LocatedPage,
    permit: P,
    claim: Claim,
) -> Result<(SharedPage, P), Box<Response>> {
    let LocatedPage { reader, entry } = located;
    let work = blocking_with_permit(permit, move || {
        OCR_PAGE_READS.fetch_add(1, Ordering::Relaxed);
        #[cfg(debug_assertions)]
        if let Ok(mut by_hash) = OCR_PAGE_READS_BY_HASH.lock() {
            *by_hash
                .entry(claim.key().content_hash().to_owned())
                .or_default() += 1;
        }
        let started = std::time::Instant::now();
        let bytes = {
            let mut cbz = lock_reader(&reader).map_err(ReadDecodeError::Read)?;
            cbz.read_entry_range(&entry, 0, entry.uncompressed_size)
                .map_err(ReadDecodeError::Read)?
        };
        let page = Arc::new(
            crate::util::image_decode::decode_limited(&bytes)
                .map_err(|e| ReadDecodeError::Decode(e.to_string()))?,
        );
        metrics::histogram!("folio_ocr_page_decode_seconds")
            .record(started.elapsed().as_secs_f64());
        claim.fulfil(Arc::clone(&page));
        Ok(page)
    })
    .await;
    match work {
        Ok((Ok(img), permit)) => Ok((img, permit)),
        Ok((Err(ReadDecodeError::Read(e)), _)) => {
            tracing::warn!(error = %e, "ocr: page read failed");
            Err(Box::new(PageUnavailable::from_archive_error(&e).response()))
        }
        Ok((Err(ReadDecodeError::Decode(e)), _)) => {
            tracing::warn!(error = %e, "ocr: page decode failed");
            Err(Box::new(error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "decode_failed",
                "page is not a decodable image",
            )))
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: read / decode task failed");
            Err(Box::new(PageUnavailable::Internal.response()))
        }
    }
}

/// Default-language resolution for requests without an explicit
/// `lang`: `series.text_language` → `reading_direction == "rtl"` ⇒
/// manga → western. Lookup failures fall back to western — never
/// fail an OCR request over a metadata read.
async fn resolve_series_language(app: &AppState, series_id: uuid::Uuid) -> Language {
    let Ok(Some(series)) = series::Entity::find_by_id(series_id).one(&app.db).await else {
        return Language::Western;
    };
    match series.text_language.as_deref() {
        Some("manga") => Language::Manga,
        Some("western") => Language::Western,
        _ => {
            if series.reading_direction.as_deref() == Some("rtl") {
                Language::Manga
            } else {
                Language::Western
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over the limit a request waits, then gets 503 + Retry-After, not a
    /// 500; a permit freed in time is taken.
    #[tokio::test]
    async fn ocr_permits_cap_concurrency_and_answer_busy() {
        let permits = Semaphore::new(1);
        let held = ocr_permit(&permits, Duration::from_millis(10))
            .await
            .expect("first permit");
        let busy = ocr_permit(&permits, Duration::from_millis(20))
            .await
            .expect_err("over the limit");
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(busy.headers()[header::RETRY_AFTER], "2");
        drop(held);
        assert!(
            ocr_permit(&permits, Duration::from_millis(20))
                .await
                .is_ok()
        );
    }
}
