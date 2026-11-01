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
//!   - Caching (audit OP-7). `Cache-Control: private, max-age=300` plus a
//!     strong `ETag` that is the blake3 digest of the served bytes;
//!     `If-None-Match` answers 304 with no body. See [`THUMB_CACHE_CONTROL`]
//!     for the staleness analysis.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, issue_cover, library};
use sea_orm::EntityTrait;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio_util::io::ReaderStream;

use super::error;
use crate::auth::CurrentUser;
use crate::library::thumbnails::{self, ThumbFormat, ThumbnailQuality, Variant};
use crate::state::AppState;
use server_macros::handler;

const INLINE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Freshness lifetime of a served thumbnail.
///
/// Thumbnail URLs are stable — `/issues/{id}/pages/{n}/thumb` never changes
/// while the bytes behind it can (archive edits, admin force-recreate, a
/// cover-page pick, a library format switch). Until audit OP-7 the route
/// answered `private, no-cache`, so every browse revalidated every cover:
/// one conditional request per card per paint. With `max-age=300` a
/// browser reuses a cover for five minutes and then revalidates against
/// the content-digest [`ETag`](header::ETAG) below, so a regenerated
/// thumbnail is picked up on the first paint after the window closes.
///
/// Staleness window. Every regeneration (archive edit, cover-page pick,
/// force-recreate, format switch) wipes the artifact and writes a new file
/// at the same path through tmp + rename (`library::thumbnails`), so the
/// served bytes — and with them the digest ETag — change exactly when the
/// pixels do. A browser holding the old copy therefore shows it for at
/// most five minutes, then revalidates, misses the old ETag and gets the
/// new bytes. Where it applies:
///
/// - Card grids and the issue page cover (`coverThumbUrl`, no version in
///   the URL): up to five minutes after a cover pick or force-recreate in a
///   tab the service worker does not control. This is the accepted cost.
/// - The reader page strip appends `?v=<last_rewrite_at>`
///   (`withContentVersion`), so an archive edit changes its URLs and the
///   window never applies there; the page-map editor appends
///   `?sw=bypass&t=<open stamp>` for the same reason.
/// - Inside the installed PWA the service worker serves its cached copy
///   first and refetches with `cache: "no-cache"`, which skips `max-age`;
///   a changed cover shows on the next paint, as it did before OP-7.
/// - OPDS clients (Panels, Chunky, KOReader) keep their own cover caches.
///
/// `private` because the route is ACL'd per user: a shared cache must
/// never serve one account's cover to another. `Vary: Cookie,
/// Authorization` goes with it so one browser profile or one OPDS client
/// used by two accounts in turn (parent signs out, age-capped child signs
/// in) keys its cache on the credentials and never reuses the first
/// account's cover for the second.
const THUMB_CACHE_CONTROL: &str = "private, max-age=300";

#[derive(Debug, Deserialize)]
pub struct ThumbQuery {
    /// `cover` (default) or `strip`. Unknown values fall back to `cover`
    /// rather than 400 — the URL is constructed by trusted code, but a
    /// stale client shouldn't break.
    #[serde(default)]
    pub variant: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/issues/{id}/pages/{n}/thumb", get(thumb))
        .route("/issues/{id}/covers/{cover_id}", get(serve_issue_cover))
}

#[handler]
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
    if !crate::library::access::issue_visible(&app, &user, &row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // G9: a small-cover (`@sm`) request degrades to the full cover when the
    // variant hasn't been generated yet — new covers emit it inline and the
    // backfill derives it for the back-catalogue, so until then the browser
    // just gets the full cover for both `srcset` steps. This avoids paying an
    // inline archive decode to produce a 300px image (the full cover is the
    // one every other caller needs anyway).
    let variant = if variant == Variant::CoverSmall
        && thumbnails::find_existing_cover_small(&app.cfg().data_path, &row.id).is_none()
    {
        Variant::Cover
    } else {
        variant
    };

    // The cover variants render the issue's cover page — ComicInfo's
    // FrontCover marker or an admin's pick (`cover_page_index`) — not
    // page 0, so the inline path and the post-scan worker agree.
    let page_index = if matches!(variant, Variant::Cover | Variant::CoverSmall) {
        usize::try_from(row.cover_page_index.max(0)).unwrap_or(0)
    } else {
        n as usize
    };
    let cache_key = thumb_cache_key(&row.id, variant, page_index);

    // Hot path: any file on disk (in any known format) → stream it. The
    // library may have switched format since the last regen, but reads are
    // format-agnostic so old thumbs keep serving until force-recreate.
    if let Some(cached) = app.cached_thumb_path(&cache_key) {
        if tokio::fs::try_exists(&cached).await.unwrap_or(false) {
            touch_for_lru(&app, &cached);
            return serve_file(&cached, &headers).await;
        }
        app.uncache_thumb_path(&cache_key);
    }
    if let Some(existing) =
        thumbnails::find_existing_variant(&app.cfg().data_path, &row.id, variant, page_index)
    {
        app.cache_thumb_path(cache_key.clone(), existing.clone());
        touch_for_lru(&app, &existing);
        return serve_file(&existing, &headers).await;
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

    // An admin-chosen cover ("Use as cover") renders instead of the
    // archive page, exactly as the post-scan worker does it. A decode
    // failure falls through to the archive page.
    if matches!(variant, Variant::Cover | Variant::CoverSmall)
        && let Some(bytes) =
            thumbnails::chosen_cover_bytes(&app.db, &app.cfg().data_path, &row.id).await
    {
        let data_dir = app.cfg().data_path.clone();
        let id_clone = row.id.clone();
        let front_side = thumbnails::resolve_front_cover_side(&app.db, &row).await;
        let r = tokio::task::spawn_blocking(move || {
            thumbnails::encode_chosen_cover(
                &data_dir, &id_clone, page_index, &bytes, variant, format, quality, front_side,
            )
        })
        .await;
        match r {
            Ok(Ok(path)) => {
                app.cache_thumb_path(cache_key, path.clone());
                return serve_file(&path, &headers).await;
            }
            Ok(Err(e)) => {
                tracing::warn!(issue_id = %row.id, error = %e, "chosen cover thumb failed; using the archive page");
            }
            Err(e) => {
                tracing::error!(error = %e, "chosen cover thumb task failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    }

    let arc = match app
        .zip_lru
        .get_or_open(&row.id, std::path::Path::new(&row.file_path))
        .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "zip_lru open failed for thumb");
            return error(StatusCode::NOT_FOUND, "not_found", "thumbnail unavailable");
        }
    };
    let data_dir = app.cfg().data_path.clone();
    let id_clone = row.id.clone();
    // Cover variants crop a wraparound page to its front half; the side
    // follows the issue's reading direction (same resolver as the
    // post-scan worker, so inline and background output are identical).
    let front_side = if matches!(variant, Variant::Cover | Variant::CoverSmall) {
        thumbnails::resolve_front_cover_side(&app.db, &row).await
    } else {
        thumbnails::FrontCoverSide::default()
    };
    let r = tokio::task::spawn_blocking(move || {
        let mut cbz = arc.lock().expect("cbz mutex");
        thumbnails::generate_with_quality(
            &data_dir, &mut *cbz, &id_clone, variant, page_index, format, quality, front_side,
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

    app.cache_thumb_path(cache_key, path.clone());
    // A fresh write may push the tree over the optional byte budget
    // (WP-3.8); throttled + no-op when the budget is off.
    let cfg = app.cfg();
    thumbnails::spawn_budget_sweep_if_needed(cfg.data_path.clone(), cfg.thumbs_budget_bytes());
    serve_file(&path, &headers).await
}

/// Keep a served thumbnail's mtime fresh so the byte-budget sweep evicts
/// least-recently-*used* files, not least-recently-generated ones. Only
/// when a budget is configured — with it off (the default) the hot path
/// stays syscall-free.
fn touch_for_lru(app: &AppState, path: &std::path::Path) {
    if app.cfg().thumbs_budget_mb == 0 {
        return;
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || thumbnails::touch_if_stale(&path));
}

fn thumb_cache_key(issue_id: &str, variant: Variant, page_index: usize) -> String {
    let variant = match variant {
        Variant::Cover => "cover",
        Variant::CoverSmall => "cover_small",
        Variant::Strip => "strip",
    };
    format!("{issue_id}:{variant}:{page_index}")
}

/// Strong validator for a thumbnail: the blake3 digest of the bytes about
/// to be served, quoted per RFC 9110. Bytes-only on purpose — never the
/// file's mtime (a `cp -p` restore or a backup replay keeps mtime while the
/// pixels differ) and never the pipeline constant `THUMBNAIL_VERSION` (a
/// regen under the same version must still miss). Identical bytes rewritten
/// by a no-op regeneration keep matching, so the client is spared the
/// transfer.
fn thumb_etag(bytes: &[u8]) -> String {
    format!("\"{}\"", blake3::hash(bytes).to_hex())
}

fn cache_headers(etag: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(THUMB_CACHE_CONTROL),
    );
    headers.insert(
        header::VARY,
        HeaderValue::from_static("Cookie, Authorization"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static("\"unknown\"")),
    );
    headers
}

/// `If-None-Match` uses the weak comparison (RFC 9110 §13.1.2): a `W/`
/// prefix a proxy added on the way out still matches the strong validator.
fn if_none_match_hits(req_headers: &HeaderMap, etag: &str) -> bool {
    req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').map(str::trim).any(|candidate| {
                candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
        })
}

async fn serve_file(path: &Path, req_headers: &HeaderMap) -> Response {
    // Thumbnails are small (a 600 px cover is tens of KB, a strip thumb a
    // few KB), so the whole artifact is read before the response: the ETag
    // is a digest of the bytes and has to be known before the status line.
    // A revalidation therefore costs one small read + a blake3 pass (well
    // under a millisecond) and sends no body.
    let Ok(bytes) = tokio::fs::read(path).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "thumbnail unavailable");
    };
    let etag = thumb_etag(&bytes);
    if if_none_match_hits(req_headers, &etag) {
        return (StatusCode::NOT_MODIFIED, cache_headers(&etag)).into_response();
    }
    let mut headers = cache_headers(&etag);

    // Content-Type follows the on-disk extension so old thumbs in the
    // previous format keep serving with the right MIME after a switch.
    let mime = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(ThumbFormat::from_ext)
        .map(ThumbFormat::mime)
        .unwrap_or("image/webp");
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    (StatusCode::OK, headers, Body::from(bytes)).into_response()
}

/// Stream a locally-stored `issue_cover` artifact (primary or variant)
/// by its row id. Covers downloaded by the apply path live at
/// `issue_cover.local_path`; this is the surface the gallery renders
/// instead of hotlinking the provider CDN. 404 when the row has no
/// local artifact (caller falls back to `source_url`) or the requesting
/// user can't see the issue's library.
#[handler]
pub async fn serve_issue_cover(
    State(app): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    AxPath((id, cover_id)): AxPath<(String, uuid::Uuid)>,
) -> Response {
    let Ok(Some(cover)) = issue_cover::Entity::find_by_id(cover_id).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "cover not found");
    };
    // The path's issue id must match the row — blocks cross-issue id
    // swaps and keeps the URL self-describing.
    if cover.issue_id != id {
        return error(StatusCode::NOT_FOUND, "not_found", "cover not found");
    }
    if cover.local_path.is_empty() {
        // No on-disk artifact (metadata-only hotlink row) — the caller
        // should render `source_url` directly.
        return error(
            StatusCode::NOT_FOUND,
            "not_found",
            "cover not stored locally",
        );
    }
    let Ok(Some(issue_row)) = issue::Entity::find_by_id(cover.issue_id.clone())
        .one(&app.db)
        .await
    else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !crate::library::access::issue_visible(&app, &user, &issue_row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "cover not found");
    }
    let path = app.cfg().data_path.join(&cover.local_path);
    serve_cover_file(&path, &headers, cover_id).await
}

async fn serve_cover_file(
    path: &std::path::Path,
    req_headers: &HeaderMap,
    cover_id: uuid::Uuid,
) -> Response {
    // Cover bytes are immutable per row id (a re-apply mints a new
    // `cover_id`), so we can cache hard.
    let etag = format!("\"cover-{cover_id}\"");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"unknown\"")),
    );
    if req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|candidate| candidate.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return error(StatusCode::NOT_FOUND, "not_found", "cover unavailable"),
    };
    let len = file.metadata().await.map(|m| m.len()).ok();
    // SE-6 (WP-6.3): the MIME comes from the stored bytes, never the file
    // extension (which, before the write-side sniff, was derived from the
    // provider URL). A legacy file that isn't an image is not served at all.
    let mime = match sniff_cover_file(&mut file).await {
        Some(kind) => kind.mime(),
        None => {
            tracing::warn!(path = %path.display(), "stored cover is not a recognised image; refusing to serve");
            return error(StatusCode::NOT_FOUND, "not_found", "cover unavailable");
        }
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(l) = len {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(l));
    }
    let stream = ReaderStream::new(file);
    (StatusCode::OK, headers, Body::from_stream(stream)).into_response()
}

/// Sniff the leading bytes of a stored cover, then rewind so the whole file
/// streams. `None` for a short read error or a non-image.
async fn sniff_cover_file(file: &mut tokio::fs::File) -> Option<archive::image_sniff::ImageKind> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut head = [0u8; archive::image_sniff::SNIFF_LEN];
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]).await {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    file.seek(std::io::SeekFrom::Start(0)).await.ok()?;
    archive::image_sniff::sniff(&head[..filled])
}
