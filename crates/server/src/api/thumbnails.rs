//! `/issues/{id}/pages/{n}/thumb[?variant=cover|strip]` — page thumbnails.
//!
//! M2 (thumbnail pipeline):
//!   - Variant-aware. `?variant=cover` (default) serves the 600 px cover
//!     used by issue / series cards. `?variant=strip` serves the 160 px
//!     thumb consumed by the reader page-strip overlay.
//!   - Disk-first. Cover jobs pre-generate the grid artwork; page-strip
//!     thumbnails are generated lazily and then completed by a deduped
//!     issue-level catchup job.
//!   - Inline fallback. On miss (freshly-added issue, mid-scan reader,
//!     race between scan and reader), the handler generates the missing
//!     thumb on-demand AND enqueues a strip catchup job so the rest of the
//!     page map lands in the background.
//!   - Bounded concurrency. Inline gen is wrapped in a semaphore
//!     (`thumb_inline_parallel`) so a 30-page strip burst can't saturate
//!     the encoder pool. Beyond the cap, requests wait briefly; a wait
//!     longer than 2s yields 503 with `Retry-After: 1` so the browser's
//!     lazy-load doesn't pile up.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, library, library_user_access};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::time::Duration;
use tokio_util::io::ReaderStream;

use crate::auth::CurrentUser;
use crate::library::thumbnails::{self, ThumbFormat, ThumbnailQuality, Variant};
use crate::state::AppState;

const INLINE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
pub struct ThumbQuery {
    /// `cover` (default) or `strip`. Unknown values fall back to `cover`
    /// rather than 400 — the URL is constructed by trusted code, but a
    /// stale client shouldn't break.
    #[serde(default)]
    pub variant: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/issues/{id}/pages/{n}/thumb", get(thumb))
}

pub async fn thumb(
    State(app): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    AxPath((id, n)): AxPath<(String, u32)>,
    Query(q): Query<ThumbQuery>,
) -> Response {
    // Default variant is conditional so pre-M2 callers (the un-paramed
    // legacy URL) still get sensible behavior: page 0 → cover, anything
    // else → strip. Callers that explicitly pass `?variant=` get exactly
    // what they ask for.
    let variant = match q.variant.as_deref().and_then(Variant::parse) {
        Some(v) => v,
        None if n == 0 => Variant::Cover,
        None => Variant::Strip,
    };

    let Ok(Some(row)) = issue::Entity::find_by_id(id.clone()).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !visible(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let page_index = n as usize;
    let cache_key = thumb_cache_key(&row.id, variant, page_index);

    // Hot path: any file on disk (in any known format) → stream it. The
    // library may have switched format since the last regen, but reads are
    // format-agnostic so old thumbs keep serving until force-recreate.
    if let Some(cached) = app.cached_thumb_path(&cache_key).await {
        if tokio::fs::try_exists(&cached).await.unwrap_or(false) {
            return serve_file(&cached, &headers, &row.id, page_index, variant).await;
        }
        app.uncache_thumb_path(&cache_key).await;
    }
    if let Some(existing) =
        thumbnails::find_existing_variant(&app.cfg.data_path, &row.id, variant, page_index)
    {
        app.cache_thumb_path(cache_key.clone(), existing.clone())
            .await;
        return serve_file(&existing, &headers, &row.id, page_index, variant).await;
    }

    // Cold path: generate inline under a global semaphore. Strip misses enqueue
    // a deduped issue-level catchup below.
    let permit = tokio::time::timeout(
        INLINE_WAIT_TIMEOUT,
        app.thumb_inline_semaphore.clone().acquire_owned(),
    )
    .await;
    let _permit = match permit {
        Ok(Ok(p)) => p,
        Ok(Err(_)) => {
            // Semaphore closed — server is shutting down.
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "service shutting down",
            );
        }
        Err(_) => {
            // Wait timeout — back the caller off so the lazy-loader doesn't
            // pile up. The post-scan worker will catch up.
            let mut headers = HeaderMap::new();
            headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                headers,
                axum::Json(serde_json::json!({
                    "error": {
                        "code": "thumb.busy",
                        "message": "thumbnail generation backlog; retry shortly"
                    }
                })),
            )
                .into_response();
        }
    };

    // Resolve the library's configured format for the inline write. If
    // the library was deleted out from under us, or the value is somehow
    // garbage, fall back to the default (webp) so the request still
    // succeeds. The catchup job below uses the same format via the
    // post-scan worker's library lookup.
    let (format, quality) = match library::Entity::find_by_id(row.library_id)
        .one(&app.db)
        .await
    {
        Ok(Some(lib)) => (
            ThumbFormat::parse(&lib.thumbnail_format).unwrap_or_default(),
            ThumbnailQuality::new(lib.thumbnail_cover_quality, lib.thumbnail_page_quality),
        ),
        _ => (ThumbFormat::default(), ThumbnailQuality::default()),
    };

    let arc = match app
        .zip_lru
        .get_or_open(&row.id, std::path::Path::new(&row.file_path))
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "zip_lru open failed for thumb");
            return error(StatusCode::NOT_FOUND, "not_found", "thumbnail unavailable");
        }
    };
    let data_dir = app.cfg.data_path.clone();
    let id_clone = row.id.clone();
    let r = tokio::task::spawn_blocking(move || {
        let mut cbz = arc.lock().expect("cbz mutex");
        thumbnails::generate_with_quality(
            &data_dir, &mut *cbz, &id_clone, variant, page_index, format, quality,
        )
    })
    .await;
    let path = match r {
        Ok(Ok(p)) => p,
        Ok(Err(thumbnails::ThumbError::PageOutOfRange)) => {
            return error(StatusCode::NOT_FOUND, "not_found", "page not found");
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "lazy thumb gen failed");
            return error(StatusCode::NOT_FOUND, "not_found", "thumbnail unavailable");
        }
        Err(e) => {
            tracing::error!(error = %e, "thumb gen task failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Background catchup: enqueue one deduped strip job so the rest of the
    // page map doesn't have to take the inline path next time. Best-effort
    // — apalis push failures are non-fatal here.
    if matches!(variant, Variant::Strip) {
        let _ = crate::jobs::post_scan::enqueue_thumb_job(
            &app,
            crate::jobs::post_scan::ThumbsJob::strip(row.id.clone()),
        )
        .await;
    }

    app.cache_thumb_path(cache_key, path.clone()).await;
    serve_file(&path, &headers, &row.id, page_index, variant).await
}

fn thumb_cache_key(issue_id: &str, variant: Variant, page_index: usize) -> String {
    let variant = match variant {
        Variant::Cover => "cover",
        Variant::Strip => "strip",
    };
    format!("{issue_id}:{variant}:{page_index}")
}

async fn serve_file(
    path: &std::path::Path,
    req_headers: &HeaderMap,
    issue_id: &str,
    page_index: usize,
    variant: Variant,
) -> Response {
    let variant_tag = match variant {
        Variant::Cover => "c",
        Variant::Strip => "s",
    };
    // ETag includes variant + page index + thumbnail version so changing
    // any of them invalidates correctly.
    let etag = format!(
        "\"{}-{}-{}-v{}\"",
        &issue_id[..32.min(issue_id.len())],
        variant_tag,
        page_index,
        thumbnails::THUMBNAIL_VERSION
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());

    if req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|candidate| candidate.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }

    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return error(StatusCode::NOT_FOUND, "not_found", "thumbnail unavailable"),
    };
    let len = file.metadata().await.map(|m| m.len()).ok();

    // Content-Type follows the on-disk extension so old thumbs in the
    // previous format keep serving with the right MIME after a switch.
    let mime = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(ThumbFormat::from_ext)
        .map(ThumbFormat::mime)
        .unwrap_or("image/webp");
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    if let Some(l) = len {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(l));
    }
    let stream = ReaderStream::new(file);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
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

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
