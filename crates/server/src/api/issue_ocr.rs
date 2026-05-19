//! `POST /me/issues/{id}/ocr` (text-detection-1.0 plan, M3).
//!
//! Loads the page from the issue's archive, decodes to a
//! `DynamicImage`, and hands it off to the OCR pipeline. The
//! response shape is intentionally generous: `text` + `confidence`
//! are the headline fields; `refined_bbox` echoes the detector's
//! bubble outline when one was found, which the reader uses to
//! replace the user's drag rect with a tighter snap-to-bubble
//! marker region.
//!
//! ACL: same as page bytes — visible-to-user library check via
//! [`fetch_visible_issue`]. No rate-limit yet (lands with M4).

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use entity::{issue, library_user_access};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use super::error;
use crate::auth::CurrentUser;
use crate::middleware::rate_limit;
use crate::ocr::cache;
use crate::ocr::pipeline::{OcrInput, Rect, run_ocr};
use crate::ocr::recognizer::Language;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/me/issues/{id}/ocr",
        post(serve).route_layer(rate_limit::OCR.build()),
    )
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OcrRequest {
    /// 0-based page index, matching the reader's URL convention.
    pub page: u32,
    /// User's drag rectangle in page-pixel coordinates. All four
    /// fields are unsigned pixels; w/h must be > 0 and the rect
    /// must fit inside the decoded page bounds.
    pub region: OcrRegion,
    /// `"western"` (default) or `"manga"`. Phase 2 will read
    /// `series.text_language` as the default; for now the client
    /// sends an explicit hint.
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
    /// Recognized text, trimmed.
    pub text: String,
    /// Mean confidence in `[0.0, 1.0]`. Tesseract surfaces a real
    /// per-page score; manga-ocr reports `1.0` (or `0.0` for an
    /// empty result) since the greedy decoder doesn't expose
    /// per-token probabilities.
    pub confidence: f32,
    /// Detector's snap-to-bubble rectangle in page-pixel coords.
    /// `None` when no detector hit overlapped the user's region —
    /// the recognizer ran on the user's rect verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refined_bbox: Option<OcrRegion>,
}

#[utoipa::path(
    post,
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
pub async fn serve(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<String>,
    Json(req): Json<OcrRequest>,
) -> Response {
    // ─── Validate ────────────────────────────────────────────────
    if req.region.w == 0 || req.region.h == 0 {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_region",
            "region w/h must be > 0",
        );
    }
    let language = match req.lang.as_deref().unwrap_or("western") {
        "western" => Language::Western,
        "manga" => Language::Manga,
        other => {
            return error(
                StatusCode::BAD_REQUEST,
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
    let arc = match app
        .zip_lru
        .get_or_open(&row.id, std::path::Path::new(&row.file_path))
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %row.id, "ocr: zip_lru open failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "archive_unreadable",
                "archive unreadable",
            );
        }
    };
    let page_index = req.page as usize;
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
            return error(StatusCode::NOT_FOUND, "not_found", "page not found");
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ocr: page read failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: page read task panicked");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let decoded = match tokio::task::spawn_blocking(move || image::load_from_memory(&bytes)).await {
        Ok(Ok(img)) => img,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ocr: page decode failed");
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "decode_failed",
                "page is not a decodable image",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "ocr: decode task panicked");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let (page_w, page_h) = (decoded.width(), decoded.height());
    if req.region.x.saturating_add(req.region.w) > page_w
        || req.region.y.saturating_add(req.region.h) > page_h
    {
        return error(
            StatusCode::BAD_REQUEST,
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
