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

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, library_user_access, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::CurrentUser;
use crate::middleware::rate_limit;
use crate::ocr::cache::{self, CachedDetection};
use crate::ocr::pipeline::{OcrInput, Rect, detect_and_cache_regions, run_ocr};
use crate::ocr::recognizer::Language;
use crate::state::AppState;
use server_macros::handler;

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
        (status = 404, description = "issue not found"),
        (status = 500, description = "ocr pipeline failure"),
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
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !visible(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
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
    let key = cache::cache_key(&row.content_hash, req.page, lang_str, detect, &region_hash);
    if let Some(hit) = cache::get(&app.jobs.redis, &key).await {
        return Json(hit).into_response();
    }

    // ─── Load + decode page ──────────────────────────────────────
    let decoded = match load_page_image(&app, &row, req.page).await {
        Ok(img) => img,
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
    };
    match run_ocr(input).await {
        Ok(out) => {
            let response = OcrResponse {
                text: out.recognition.text,
                confidence: out.recognition.confidence,
                refined_bbox: out.refined_bbox.map(|r| OcrRegion {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                }),
                lang: lang_str.to_owned(),
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
            };
            // Best-effort cache write — failures don't fail the
            // request; the next call will simply rerun the pipeline.
            cache::put(&app.jobs.redis, &key, &response).await;
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: pipeline failure");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ocr_failed",
                "ocr pipeline failed",
            )
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
        (status = 415, description = "page is not a decodable image"),
        (status = 500, description = "detector failure"),
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
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !visible(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // ─── Cache lookup ────────────────────────────────────────────
    // Checked before the archive is touched: a hit skips page load
    // and decode entirely, making warm requests a Redis round-trip.
    let key = cache::detect_cache_key(&row.content_hash, page);
    if let Some(hit) = cache::get_detect(&app.jobs.redis, &key).await {
        return Json(TextRegionsView::from(hit)).into_response();
    }

    // ─── Decode + detect ─────────────────────────────────────────
    let decoded = match load_page_image(&app, &row, page).await {
        Ok(img) => img,
        Err(resp) => return *resp,
    };
    match detect_and_cache_regions(app.jobs.redis.clone(), &row.content_hash, page, decoded).await {
        Ok(detection) => Json(TextRegionsView::from(detection)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "ocr: text-region detection failed");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ocr_failed",
                "text-region detection failed",
            )
        }
    }
}

/// Load + decode one page image from the issue's archive. Failures
/// come back as ready-to-return canonical-envelope responses so the
/// OCR POST and the text-regions GET share one error mapping.
/// Boxed because the error path is cold and `Response` is large.
async fn load_page_image(
    app: &AppState,
    row: &issue::Model,
    page: u32,
) -> Result<image::DynamicImage, Box<Response>> {
    let arc = match app
        .zip_lru
        .get_or_open(&row.id, std::path::Path::new(&row.file_path))
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %row.id, "ocr: zip_lru open failed");
            return Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "archive_unreadable",
                "archive unreadable",
            )));
        }
    };
    let page_index = page as usize;
    let arc_clone = arc.clone();
    let bytes_result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut cbz = arc_clone
            .lock()
            .map_err(|_| "cbz mutex poisoned".to_string())?;
        let pages = cbz.pages();
        let entry = pages
            .get(page_index)
            .copied()
            .cloned()
            .ok_or_else(|| "page not found".to_string())?;
        let total = entry.uncompressed_size;
        cbz.read_entry_range(&entry, 0, total)
            .map_err(|e| e.to_string())
    })
    .await;
    let bytes = match bytes_result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) if e == "page not found" => {
            return Err(Box::new(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "page not found",
            )));
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ocr: page read failed");
            return Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            )));
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: page read task panicked");
            return Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            )));
        }
    };
    // `decode_limited` (security M0) caps decoded dimensions so a
    // malicious archive can't OOM the box via a decompression bomb.
    match tokio::task::spawn_blocking(move || crate::util::image_decode::decode_limited(&bytes))
        .await
    {
        Ok(Ok(img)) => Ok(img),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ocr: page decode failed");
            Err(Box::new(error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "decode_failed",
                "page is not a decodable image",
            )))
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: decode task panicked");
            Err(Box::new(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            )))
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

async fn visible(app: &AppState, user: &CurrentUser, lib_id: uuid::Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}
