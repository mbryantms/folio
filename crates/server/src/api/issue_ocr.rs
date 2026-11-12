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
//! GET on the stricter `OCR_DETECT` bucket.
//!
//! Which engines run is [`EngineSet::from_config`], read once per
//! request and passed down: it selects the models, the cleanup rules
//! and the cache keys (the two sets never share an entry).
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
use super::extractors::Validated;
use super::page_source::{PageUnavailable, issue_not_found, no_store};
use crate::auth::CurrentUser;
use crate::library::zip_lru::{CachedReader, lock_reader};
use crate::middleware::rate_limit;
use crate::ocr::EngineSet;
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

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct OcrRequest {
    /// 0-based page index, matching the reader's URL convention.
    #[garde(skip)]
    pub page: u32,
    /// User's drag rectangle in page-pixel coordinates of the decoded
    /// page. All four fields are unsigned pixels; w/h must be > 0 and
    /// the rect must fit inside the decoded page bounds. Send this or
    /// `region_pct`; when both are present `region_pct` is used.
    #[serde(default)]
    #[garde(custom(|r: &Option<OcrRegion>, _: &()| region_given(r, &self.region_pct)))]
    #[schema(nullable = false)]
    pub region: Option<OcrRegion>,
    /// The same rectangle in percent (0–100) of the decoded page, the
    /// reader's marker representation, so the client needs no page
    /// size. Converted after the decode with the decoded width and
    /// height, rounded to whole pixels and clamped to the page; it must
    /// overlap the page. Wins over `region` when both are sent.
    #[serde(default)]
    #[garde(dive, custom(pct_on_page))]
    #[schema(nullable = false)]
    pub region_pct: Option<OcrRegionPct>,
    /// `"western"` or `"manga"`. Omitted → the server resolves a
    /// default: `series.text_language` if set, else `manga` when the
    /// series reads right-to-left, else `western`. The response
    /// echoes the resolved value in `lang`.
    #[serde(default)]
    #[garde(skip)]
    pub lang: Option<String>,
    /// When `true`, run the page detector (or use its cached result
    /// for the page) and snap the rectangle to the detected text region
    /// it overlaps most; the snapped box comes back as `refined_bbox`.
    /// When `false` or omitted (the default), the rectangle is not
    /// snapped: the way to read a region taken from the text-regions
    /// route, which is already a detected box. (A Japanese request then
    /// still reads the text box the detector found inside the region,
    /// when there is exactly one; see `refined_bbox`.)
    #[serde(default)]
    #[garde(skip)]
    pub detect: Option<bool>,
}

/// A region in percent (0–100) of the decoded page.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, garde::Validate, utoipa::ToSchema)]
pub struct OcrRegionPct {
    /// Left edge, percent of page width.
    #[garde(custom(finite))]
    pub x: f64,
    /// Top edge, percent of page height.
    #[garde(custom(finite))]
    pub y: f64,
    /// Width, percent of page width; above 0.
    #[garde(custom(positive))]
    pub w: f64,
    /// Height, percent of page height; above 0.
    #[garde(custom(positive))]
    pub h: f64,
}

/// One of `region` / `region_pct` must be sent.
fn region_given(region: &Option<OcrRegion>, pct: &Option<OcrRegionPct>) -> garde::Result {
    if region.is_none() && pct.is_none() {
        Err(garde::Error::new("region or region_pct is required"))
    } else {
        Ok(())
    }
}

fn finite(v: &f64, _: &()) -> garde::Result {
    if v.is_finite() {
        Ok(())
    } else {
        Err(garde::Error::new("must be a finite number"))
    }
}

fn positive(v: &f64, ctx: &()) -> garde::Result {
    finite(v, ctx)?;
    if *v > 0.0 {
        Ok(())
    } else {
        Err(garde::Error::new("must be above 0"))
    }
}

/// The percent box must overlap the page (0–100 on both axes). Checked
/// only once its numbers are valid; their own rules report the rest.
fn pct_on_page(r: &Option<OcrRegionPct>, _: &()) -> garde::Result {
    let Some(r) = r else { return Ok(()) };
    let valid = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0;
    if valid && (r.x >= 100.0 || r.y >= 100.0 || r.x + r.w <= 0.0 || r.y + r.h <= 0.0) {
        return Err(garde::Error::new("region lies entirely outside the page"));
    }
    Ok(())
}

impl OcrRegionPct {
    /// Whole-pixel rect on a `page_w × page_h` page: edges rounded, then
    /// clamped to the page (an edge overshoot is cut, not refused).
    /// `None` when nothing of it is left on the page.
    pub fn to_pixels(self, page_w: u32, page_h: u32) -> Option<OcrRegion> {
        let edge = |pct: f64, size: u32| -> u32 {
            (pct / 100.0 * f64::from(size))
                .round()
                .clamp(0.0, f64::from(size)) as u32
        };
        let (x0, x1) = (edge(self.x, page_w), edge(self.x + self.w, page_w));
        let (y0, y1) = (edge(self.y, page_h), edge(self.y + self.h, page_h));
        (x1 > x0 && y1 > y0).then_some(OcrRegion {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
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
    /// Confidence in `[0.0, 1.0]`. Western text: the mean over the
    /// text lines read, each line scored by the mean probability of its
    /// characters (a line that could not be read counts as 0).
    /// manga-ocr reports `1.0` (or `0.0` for an empty result) since its
    /// greedy decoder doesn't expose per-token probabilities.
    pub confidence: f32,
    /// The detected box that was read, in page-pixel coords, when it
    /// is not the region as sent: the region a `detect: true` request
    /// was snapped to, or, for Japanese, the text box inside the region.
    /// Absent when the recognizer read the user's rect verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refined_bbox: Option<OcrRegion>,
    /// Recognizer the pipeline actually ran (`"western"` |
    /// `"manga"`) — the request override or the server-side
    /// resolution chain. `default` so result-cache payloads written
    /// before this field existed still deserialize.
    #[serde(default)]
    pub lang: String,
    /// Engine output before cleanup. Present only when cleanup
    /// changed the text (for Western text: the lines as read, one per
    /// line, where `text` has them joined) — a debugging aid, and the
    /// raw material for golden-fixture authoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    /// The pieces the text was read in, in reading order, with their
    /// boxes in page-pixel coords. Western text: one entry per text
    /// line (the recognizer reads whole lines, so an entry's `text`
    /// holds several words). Absent for manga-ocr, which returns one
    /// string, and when nothing was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<OcrWord>>,
    /// Decoded page width in pixels, the space `refined_bbox` and
    /// `words` are in (and `region_pct` was converted with). Always
    /// sent; a cached payload without it is malformed (a miss).
    pub page_w: u32,
    /// Decoded page height in pixels.
    pub page_h: u32,
}

/// One piece of the recognized text (a text line; see
/// `OcrResponse.words`): its text, confidence and page-pixel box.
#[derive(Debug, Deserialize, Serialize, Clone, utoipa::ToSchema)]
pub struct OcrWord {
    pub text: String,
    /// `[0.0, 1.0]`: the recognizer's score for this piece.
    pub confidence: f32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Detected text regions for one page, in percent-of-page
/// coordinates (0–100 floats — the reader's native marker
/// representation, consumable by its SVG overlay without math).
/// The set is final: one box per region to read, duplicates already
/// removed. A box inside a larger one is a region of its own.
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
    /// What the detector took the region for: 0 = a balloon, 2 = text
    /// outside balloons (a caption on the art, a sound effect). Every
    /// region is a box to read as a whole. (A server running the
    /// previous engines, `COMIC_OCR_LEGACY_ENGINES`, sends that
    /// detector's classes: 0 = text block, 1 = text line.)
    pub class: u32,
}

impl OcrResponse {
    /// The response for one finished pipeline run (`lang` is the
    /// recognizer that ran).
    pub(crate) fn from_output(out: OcrOutput, lang: &str, (page_w, page_h): (u32, u32)) -> Self {
        OcrResponse {
            page_w,
            page_h,
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

impl TextRegionsView {
    /// A page's detection as the client sees it: the page's regions for
    /// the engine set ([`crate::ocr::regions::regions`]: the region
    /// classes, clamped to the page, the set's duplicates removed), in
    /// percent. The client draws and reads exactly this set.
    pub(crate) fn from_detection(d: CachedDetection, engines: EngineSet) -> Self {
        let (pw, ph) = (d.page_w as f32, d.page_h as f32);
        let regions = if pw <= 0.0 || ph <= 0.0 {
            Vec::new()
        } else {
            crate::ocr::regions::regions(engines, &d.bboxes, d.page_w, d.page_h)
                .iter()
                .map(|b| TextRegionView {
                    x: b.xmin / pw * 100.0,
                    y: b.ymin / ph * 100.0,
                    w: (b.xmax - b.xmin) / pw * 100.0,
                    h: (b.ymax - b.ymin) / ph * 100.0,
                    confidence: b.confidence,
                    class: b.class,
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
        (status = 422, description = "invalid region or lang (`invalid_region`, `invalid_lang`), or a `region_pct` / missing region failing validation (`validation`, with field details)"),
        (status = 500, description = "ocr pipeline failure"),
        (status = 503, description = "OCR busy (`service_unavailable`), the library's folder is offline (`library_offline`), or the file could not be read; carries Retry-After"),
    )
)]
#[handler]
pub async fn serve(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<String>,
    Validated(req): Validated<OcrRequest>,
) -> Response {
    // ─── Validate ────────────────────────────────────────────────
    // `region_pct` (checked by garde on extraction) wins over `region`;
    // the pixel `region` keeps its own checks.
    let pixel_region = match (&req.region_pct, req.region) {
        (Some(_), _) => None,
        (None, Some(r)) => {
            if r.w == 0 || r.h == 0 {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_region",
                    "region w/h must be > 0",
                );
            }
            Some(r)
        }
        // Refused by `region_given` on extraction; an answer, not a panic.
        (None, None) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_region",
                "region or region_pct is required",
            );
        }
    };
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
    // Default off (see [`OcrRequest::detect`]): a region from the
    // text-regions route is read as sent. Clients opt in to snapping
    // for a rectangle the user drew.
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
    //
    // A `region_pct` request finds its pixel rect through the stored
    // page size (below), still before any page work; only when that size
    // is not stored does its lookup wait for the decode. Both kinds key
    // on the pixel rect, so they share entries.
    let lang_str = match language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    // The keys carry the engine fingerprint (`ocr::cache`), computed once
    // per process and engine set, off the async runtime.
    let engines = EngineSet::from_config(&app.cfg());
    cache::warm_fingerprints_for(engines).await;
    let result_key = |r: &OcrRegion| {
        let region_hash = cache::region_hash(r.x, r.y, r.w, r.h);
        cache::cache_key_for(
            engines,
            &row.content_hash,
            req.page,
            lang_str,
            detect,
            &region_hash,
        )
    };
    if let Some(r) = &pixel_region
        && let Some(hit) = cache::get(&app.jobs.redis, &result_key(r)).await
    {
        return Json(hit).into_response();
    }
    // A `region_pct` request finds its pixel rect through the stored
    // decoded size (written on every OCR decode), so a repeat answers
    // here too: no models, no permit, no decode. The cached payload must
    // carry the same size, or it is not used. A miss on either key goes
    // on to the decode, which is the authority on the size.
    let size_key = cache::page_size_key(&row.content_hash, req.page);
    // The result key the early lookup already read (and missed), so the
    // lookup after the decode does not read it a second time.
    let mut early_key: Option<String> = None;
    let stored_size = match &req.region_pct {
        Some(pct) => {
            let stored = cache::get_page_size(&app.jobs.redis, &size_key).await;
            if let Some(size) = stored
                && let Some(r) = pct.to_pixels(size.0, size.1)
            {
                let key = result_key(&r);
                if let Some(hit) = cache::get_sized(&app.jobs.redis, &key, size).await {
                    return Json(hit).into_response();
                }
                early_key = Some(key);
            }
            stored
        }
        None => None,
    };

    // ─── Cheap checks (no permit, no page bytes) ─────────────────
    let located = match locate_page(&app, &row, req.page).await {
        Ok(l) => l,
        Err(resp) => return *resp,
    };
    // ─── Models (a first-use download waits here, without a permit) ─
    let models = match prepare_models(engines, &row.content_hash, language, detect).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "ocr: model preparation failed");
            return ocr_failure(&e, "ocr pipeline failed");
        }
    };
    // ─── Decode (under the OCR permit, held by the work) ─────────
    let page_key = PageKey::new(&row.content_hash, req.page);
    let (decoded, permit) =
        match page_image_holding(located, page_key, app.jobs.redis.clone()).await {
            Ok(pair) => pair,
            Err(resp) => return *resp,
        };
    let (page_w, page_h) = (decoded.width(), decoded.height());
    // The decode wins over a stored size that disagrees with it (or one
    // that is missing because the page came from the decoded-page cache).
    if req.region_pct.is_some() && stored_size != Some((page_w, page_h)) {
        if let Some(stored) = stored_size {
            tracing::warn!(
                issue_id = %row.id,
                page = req.page,
                stored_w = stored.0,
                stored_h = stored.1,
                page_w,
                page_h,
                "ocr: stored page size differs from the decode; replacing it"
            );
        }
        cache::put_page_size(&app.jobs.redis, &size_key, (page_w, page_h)).await;
    }
    let region = match (pixel_region, &req.region_pct) {
        (Some(r), _) => {
            if r.x.saturating_add(r.w) > page_w || r.y.saturating_add(r.h) > page_h {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_region",
                    "region extends outside page bounds",
                );
            }
            r
        }
        (None, Some(pct)) => match pct.to_pixels(page_w, page_h) {
            Some(r) => r,
            // Overlaps the page in percent but rounds to no whole pixel.
            None => {
                return super::respond_with_field_errors(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    shared::error::ApiErrorCode::Validation,
                    "region_pct: covers no whole pixel of the page",
                    vec![shared::error::FieldError {
                        field: "region_pct".to_owned(),
                        message: "covers no whole pixel of the page".to_owned(),
                    }],
                );
            }
        },
        // Refused before the ACL; kept as an answer, not a panic.
        (None, None) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_region",
                "region or region_pct is required",
            );
        }
    };
    let key = result_key(&region);
    // A percent request looks its result up here only when the early
    // lookup did not already read this key (no stored size, or one that
    // differed from the decode), and only accepts a payload computed on a
    // page of this decoded size: anything else is a miss, the pipeline
    // runs, and the entry is overwritten.
    if pixel_region.is_none()
        && early_key.as_deref() != Some(key.as_str())
        && let Some(hit) = cache::get_sized(&app.jobs.redis, &key, (page_w, page_h)).await
    {
        return Json(hit).into_response();
    }

    // ─── Run OCR ─────────────────────────────────────────────────
    let input = OcrInput {
        page_image: decoded,
        region: Rect {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
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
    let engines = EngineSet::from_config(&app.cfg());
    cache::warm_fingerprints_for(engines).await;
    let key = cache::detect_cache_key_for(engines, &row.content_hash, page);
    if let Some(hit) = cache::get_detect(&app.jobs.redis, &key).await {
        return Json(TextRegionsView::from_detection(hit, engines)).into_response();
    }

    // ─── Cheap checks (no permit, no page bytes) ─────────────────
    let located = match locate_page(&app, &row, page).await {
        Ok(l) => l,
        Err(resp) => return *resp,
    };
    // ─── Detector (a first-use download waits here, no permit) ───
    let detector = match prepare_detector(engines, &row.content_hash).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "ocr: detector preparation failed");
            return ocr_failure(&e, "text-region detection failed");
        }
    };
    // ─── Decode + detect (under the OCR permit, held by the work) ─
    let page_key = PageKey::new(&row.content_hash, page);
    let (decoded, permit) =
        match page_image_holding(located, page_key, app.jobs.redis.clone()).await {
            Ok(pair) => pair,
            Err(resp) => return *resp,
        };
    match detect_and_cache_regions(
        app.jobs.redis.clone(),
        engines,
        &row.content_hash,
        page,
        decoded,
        detector,
        permit,
    )
    .await
    {
        Ok(detection) => Json(TextRegionsView::from_detection(detection, engines)).into_response(),
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
/// side by side count the reads of their own issue. Only hashes a test
/// registered stand-ins for (`ocr::pipeline::test_models`) are counted,
/// so a debug server reading a real library does not grow it.
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
    redis: redis::aio::ConnectionManager,
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
                return read_and_decode_holding(located, permit, claim, redis).await;
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
/// the box via a decompression bomb. The decoded size is written to the
/// page-size cache from the same task (`cache::put_page_size`), so it is
/// stored even when the request is gone.
async fn read_and_decode_holding<P: Send + 'static>(
    located: LocatedPage,
    permit: P,
    claim: Claim,
    redis: redis::aio::ConnectionManager,
) -> Result<(SharedPage, P), Box<Response>> {
    let LocatedPage { reader, entry } = located;
    let work = blocking_with_permit(permit, move || {
        OCR_PAGE_READS.fetch_add(1, Ordering::Relaxed);
        #[cfg(debug_assertions)]
        if crate::ocr::pipeline::test_models::is_installed(claim.key().content_hash())
            && let Ok(mut by_hash) = OCR_PAGE_READS_BY_HASH.lock()
        {
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
        let size_key = cache::page_size_key(claim.key().content_hash(), claim.key().page());
        let size = (page.width(), page.height());
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move { cache::put_page_size(&redis, &size_key, size).await });
            }
            Err(_) => tracing::debug!(
                key = %size_key,
                "ocr: no runtime handle in the decode; page size not stored"
            ),
        }
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
    use garde::Validate;

    fn pct(x: f64, y: f64, w: f64, h: f64) -> OcrRegionPct {
        OcrRegionPct { x, y, w, h }
    }

    #[test]
    fn region_pct_rules() {
        for bad in [
            pct(f64::NAN, 0.0, 5.0, 5.0),
            pct(0.0, f64::INFINITY, 5.0, 5.0),
            pct(0.0, 0.0, f64::NEG_INFINITY, 5.0),
            pct(0.0, 0.0, 5.0, 0.0),
            pct(0.0, 0.0, -1.0, 5.0),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert!(pct(0.0, 0.0, 5.0, 5.0).validate().is_ok());
        assert!(pct_on_page(&Some(pct(100.0, 0.0, 5.0, 5.0)), &()).is_err());
        assert!(pct_on_page(&Some(pct(-5.0, 0.0, 5.0, 5.0)), &()).is_err());
        assert!(pct_on_page(&Some(pct(-5.0, 0.0, 6.0, 5.0)), &()).is_ok());
        assert!(pct_on_page(&None, &()).is_ok());
    }

    #[test]
    fn region_pct_converts_rounds_and_clamps() {
        let r = pct(10.0, 10.0, 25.0, 30.0).to_pixels(200, 100).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (20, 10, 50, 30));
        let r = pct(90.0, 80.0, 20.0, 30.0).to_pixels(200, 100).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (180, 80, 20, 20));
        let r = pct(-5.0, -5.0, 10.0, 10.0).to_pixels(200, 100).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 10, 5));
        // Less than half a pixel: nothing left.
        assert!(pct(50.0, 50.0, 0.1, 0.1).to_pixels(200, 100).is_none());
    }

    /// Clients get the regions of a page: with the current engines the
    /// balloons and the free text, not the text boxes inside balloons
    /// the detection also holds; with the legacy engines every box.
    #[test]
    fn text_regions_are_the_region_classes_of_the_engine_set() {
        use crate::ocr::cache::CachedBbox;
        // Three boxes side by side, one of each class index.
        let detection = || CachedDetection {
            page_w: 1000,
            page_h: 2000,
            bboxes: [(0, 100.0), (1, 350.0), (2, 600.0)]
                .into_iter()
                .map(|(class, x)| CachedBbox {
                    xmin: x,
                    ymin: 200.0,
                    xmax: x + 200.0,
                    ymax: 600.0,
                    confidence: 0.9,
                    class,
                })
                .collect(),
        };
        let classes = |engines| -> Vec<u32> {
            TextRegionsView::from_detection(detection(), engines)
                .regions
                .iter()
                .map(|r| r.class)
                .collect()
        };
        assert_eq!(classes(EngineSet::Current), [0, 2]);
        assert_eq!(classes(EngineSet::Legacy), [0, 1, 2]);
        // Percent of the page, as before.
        let view = TextRegionsView::from_detection(detection(), EngineSet::Current);
        assert_eq!((view.page_w, view.page_h), (1000, 2000));
        let r = &view.regions[0];
        assert!((r.x - 10.0).abs() < 1e-4 && (r.y - 10.0).abs() < 1e-4);
        assert!((r.w - 20.0).abs() < 1e-4 && (r.h - 20.0).abs() < 1e-4);
    }

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
