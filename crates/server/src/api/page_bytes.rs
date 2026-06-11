//! `GET /issues/{id}/pages/{n}` — full page bytes with HTTP Range support (§17.5, §7.2).
//!
//! - ACL-checked via `library_user_access`.
//! - First 16 bytes sniffed for content-type; allowlist enforced.
//! - SVG entries rejected outright (script vector).
//! - 200 with full body, 206 with `Content-Range` for ranges, 416 for invalid.
//! - `If-Range` honored against ETag (`{issue_id}-{page_index}`).
//! - `Cache-Control: private, max-age=3600` (per-user — ACL).
//!
//! The handler uses a synchronous `spawn_blocking` boundary because `Cbz`
//! reads through the `zip` crate's blocking I/O.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, library_user_access};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::{ReaderStream, SyncIoBridge};

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/issues/{id}/pages/{n}", get(serve))
}

pub async fn serve(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((id, n)): AxPath<(String, u32)>,
    headers: HeaderMap,
) -> Response {
    let Ok(Some(row)) = issue::Entity::find_by_id(id.clone()).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !visible(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let page_index = n as usize;
    // Content-derived ETag: `issue.id` is stable across archive rewrites by
    // design, so fold in `content_hash` (which changes when the bytes change) so
    // a cached page is revalidated after an edit / sidecar rewrite (PERF-4).
    // `.min(len)` guards a short hash from a panicking slice.
    let hash = row.content_hash.as_str();
    let etag_value = format!("\"{}-{}\"", &hash[..32.min(hash.len())], page_index);

    // Conditional GET: a matching If-None-Match revalidation returns 304 before
    // we even open the archive (PERF-4).
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|c| c.trim() == etag_value))
    {
        return not_modified(&etag_value);
    }

    let (arc, pread) = match app
        .zip_lru
        .get_or_open_indexed(&row.id, std::path::Path::new(&row.file_path))
    {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %row.id, "zip_lru open failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "archive_unreadable",
                "archive unreadable",
            );
        }
    };

    // Range parsing happens before the blocking task so we can short-circuit
    // 416 / If-Range without paying the blocking-task cost.
    let if_range = headers
        .get(header::IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // If-Range: only honor Range when it matches our ETag.
    let honor_range = if_range.as_deref().map(|v| v == etag_value).unwrap_or(true);

    let arc_clone = arc.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<PageBytes, PageError> {
        let mut cbz = arc_clone.lock().expect("cbz mutex");
        let pages = cbz.pages();
        let entry = pages
            .get(page_index)
            .copied()
            .cloned()
            .ok_or(PageError::NotFound)?;
        // Drop the borrow on `pages` (which borrows `cbz`) before reading.
        let total = entry.uncompressed_size;
        let zip_ordinal = entry.index;

        // Sniff first 16 bytes (or whole entry, whichever is smaller).
        let head_len = total.min(16) as usize;
        let head = if head_len > 0 {
            cbz.read_entry_range(&entry, 0, head_len as u64)
                .map_err(|e| PageError::Read(e.to_string()))?
        } else {
            Vec::new()
        };
        let (mime, ext) = sniff(&head).ok_or(PageError::UnsupportedType)?;

        Ok(PageBytes {
            entry_index_in_pages: page_index,
            zip_ordinal,
            total,
            mime,
            ext,
            cbz_arc: arc_clone.clone(),
        })
    })
    .await;

    let info = match result {
        Ok(Ok(info)) => info,
        Ok(Err(PageError::NotFound)) => {
            return error(StatusCode::NOT_FOUND, "not_found", "page not found");
        }
        Ok(Err(PageError::UnsupportedType)) => {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "page bytes are not a supported image type",
            );
        }
        Ok(Err(PageError::Read(e))) => {
            tracing::warn!(error = %e, "page read failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "page task failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Resolve Range against total.
    let (status, start, len) = if let Some(rh) = range_header.filter(|_| honor_range) {
        match parse_range(&rh, info.total) {
            ParsedRange::Ok { start, len } => (StatusCode::PARTIAL_CONTENT, start, len),
            ParsedRange::Unsatisfiable => return unsatisfiable(info.total),
            ParsedRange::Multiple | ParsedRange::Malformed => {
                // RFC 7233 lets us ignore unparseable Range and return 200.
                (StatusCode::OK, 0, info.total)
            }
        }
    } else {
        (StatusCode::OK, 0, info.total)
    };

    // Headers shared by the streamed (200) and buffered (206) paths.
    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(info.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    hdrs.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    hdrs.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("inline; filename=\"page-{n}.{}\"", info.ext))
            .unwrap_or_else(|_| HeaderValue::from_static("inline")),
    );
    hdrs.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    hdrs.insert(
        header::ETAG,
        HeaderValue::from_str(&etag_value)
            .unwrap_or_else(|_| HeaderValue::from_static("\"unknown\"")),
    );

    // PERF-3 — lock-free fast path for `Stored` (uncompressed) entries, the
    // common case for comic CBZs (images are already compressed). The entry's
    // bytes sit verbatim at a precomputed file offset, so we stream them from a
    // private, per-request file handle with no decompression and *without* the
    // per-issue archive mutex — concurrent reads of one issue no longer
    // serialize, and a slow client holds no lock. Covers both the full-page
    // (200) and Range (206) cases via `take(len)`. Any IO hiccup (or a
    // Deflated / recovery-backed entry, which isn't in the index) falls through
    // to the locked path below. The byte-for-byte equivalence with the locked
    // read is proven in `archive::cbz` tests.
    if let Some(extent) = pread.extent(info.zip_ordinal)
        && extent.length == info.total
        && let Ok(mut file) = tokio::fs::File::open(&row.file_path).await
        && file
            .seek(std::io::SeekFrom::Start(extent.data_start + start))
            .await
            .is_ok()
    {
        hdrs.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
        if status == StatusCode::PARTIAL_CONTENT {
            let end = start + len - 1;
            hdrs.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{}", info.total))
                    .unwrap_or_else(|_| HeaderValue::from_static("bytes 0-0/0")),
            );
        }
        let body = Body::from_stream(ReaderStream::new(file.take(len)));
        return (status, hdrs, body).into_response();
    }

    // Locked fallback for Deflated / recovery-backed entries (or a pread IO
    // failure). Full-page request (200, whole entry): stream the entry straight
    // into the response so memory stays flat regardless of page size (PERF-2).
    // The per-issue zip mutex is held for the stream's duration here; the pread
    // path above avoids it for the common Stored case. Range requests stay
    // buffered — already bounded by the client-requested length.
    if status == StatusCode::OK && start == 0 && len == info.total {
        hdrs.insert(header::CONTENT_LENGTH, HeaderValue::from(info.total));
        let arc_for_read = info.cbz_arc.clone();
        let entry_idx = info.entry_index_in_pages;
        let (writer, reader) = tokio::io::duplex(STREAM_BUF_BYTES);
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let mut sink = SyncIoBridge::new_with_handle(writer, handle);
            let mut cbz = match arc_for_read.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let entry = {
                let pages = cbz.pages();
                match pages.get(entry_idx).copied().cloned() {
                    Some(e) => e,
                    None => return,
                }
            };
            if let Err(e) = cbz.pipe_entry(&entry, &mut sink) {
                // Headers are already sent; we can only truncate the stream.
                tracing::warn!(error = %e, "page stream failed mid-body");
            }
            // Flush + close so the ReaderStream terminates cleanly.
            let _ = std::io::Write::flush(&mut sink);
            let _ = sink.shutdown();
        });
        return (status, hdrs, Body::from_stream(ReaderStream::new(reader))).into_response();
    }

    // Range path: read the (bounded) slice into memory and return it.
    let arc_for_read = info.cbz_arc.clone();
    let entry_idx = info.entry_index_in_pages;
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut cbz = arc_for_read.lock().expect("cbz mutex");
        let pages = cbz.pages();
        let entry = pages
            .get(entry_idx)
            .copied()
            .cloned()
            .ok_or_else(|| "page disappeared".to_string())?;
        cbz.read_entry_range(&entry, start, len)
            .map_err(|e| e.to_string())
    })
    .await;
    let body_bytes = match bytes {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "page byte read failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "page byte task failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    hdrs.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(body_bytes.len() as u64),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        let end = start + body_bytes.len() as u64 - 1;
        hdrs.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", info.total))
                .unwrap_or_else(|_| HeaderValue::from_static("bytes 0-0/0")),
        );
    }
    (status, hdrs, Body::from(body_bytes)).into_response()
}

/// Duplex buffer for the streaming page path. Bounds the per-request memory the
/// blocking reader can run ahead of a slow client to this size (vs. the whole
/// page); a comic page is typically well under this, so the reader usually
/// finishes and releases the zip lock without blocking on backpressure.
const STREAM_BUF_BYTES: usize = 256 * 1024;

/// 304 response carrying just the validators (no body), for a matched
/// If-None-Match revalidation.
fn not_modified(etag: &str) -> Response {
    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    hdrs.insert(
        header::ETAG,
        HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static("\"unknown\"")),
    );
    (StatusCode::NOT_MODIFIED, hdrs).into_response()
}

struct PageBytes {
    entry_index_in_pages: usize,
    /// Zip ordinal (`ArchiveEntry::index`) — the key into the `PreadIndex` for
    /// the lock-free Stored read path (PERF-3).
    zip_ordinal: usize,
    total: u64,
    mime: &'static str,
    ext: &'static str,
    cbz_arc: std::sync::Arc<std::sync::Mutex<archive::cbz::Cbz>>,
}

#[derive(Debug)]
enum PageError {
    NotFound,
    UnsupportedType,
    Read(String),
}

/// Sniff first ≤ 16 bytes against the §17.5 allowlist.
/// Returns `None` (→ 415) for SVG / unknown / disallowed types.
fn sniff(head: &[u8]) -> Option<(&'static str, &'static str)> {
    // SVG explicitly rejected — script vector. Catch both `<?xml … <svg` and bare `<svg`.
    if head_starts_with_svg(head) {
        return None;
    }
    // JPEG: FF D8 FF
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("image/jpeg", "jpg"));
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if head.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(("image/png", "png"));
    }
    // GIF: "GIF87a" or "GIF89a"
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    // WebP: "RIFF....WEBP"
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some(("image/webp", "webp"));
    }
    // AVIF: "....ftypavif" or "....ftypavis" at offset 4
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some(("image/avif", "avif"));
        }
    }
    // JXL: codestream "FF 0A" or container "00 00 00 0C 4A 58 4C 20 0D 0A 87 0A"
    if head.starts_with(&[0xFF, 0x0A]) {
        return Some(("image/jxl", "jxl"));
    }
    if head.len() >= 12
        && head[0..12]
            == [
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
            ]
    {
        return Some(("image/jxl", "jxl"));
    }
    None
}

fn head_starts_with_svg(head: &[u8]) -> bool {
    let s = std::str::from_utf8(head).unwrap_or("").trim_start();
    s.starts_with("<svg") || s.starts_with("<?xml")
}

enum ParsedRange {
    Ok { start: u64, len: u64 },
    Unsatisfiable,
    Multiple,
    Malformed,
}

/// Parse `Range: bytes=N-M` / `bytes=N-` / `bytes=-N`. Single-range only.
fn parse_range(header: &str, total: u64) -> ParsedRange {
    let body = match header.strip_prefix("bytes=") {
        Some(s) => s.trim(),
        None => return ParsedRange::Malformed,
    };
    if body.contains(',') {
        return ParsedRange::Multiple;
    }
    let (a, b) = match body.split_once('-') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => return ParsedRange::Malformed,
    };
    if a.is_empty() {
        // suffix: bytes=-N — last N bytes
        let n: u64 = match b.parse() {
            Ok(n) if n > 0 => n,
            _ => return ParsedRange::Malformed,
        };
        if total == 0 {
            return ParsedRange::Unsatisfiable;
        }
        let len = n.min(total);
        let start = total - len;
        return ParsedRange::Ok { start, len };
    }
    let start: u64 = match a.parse() {
        Ok(n) => n,
        Err(_) => return ParsedRange::Malformed,
    };
    if start >= total {
        return ParsedRange::Unsatisfiable;
    }
    let end: u64 = if b.is_empty() {
        total - 1
    } else {
        match b.parse() {
            Ok(n) => n,
            Err(_) => return ParsedRange::Malformed,
        }
    };
    if end < start {
        return ParsedRange::Malformed;
    }
    let end = end.min(total - 1);
    ParsedRange::Ok {
        start,
        len: end - start + 1,
    }
}

fn unsatisfiable(total: u64) -> Response {
    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{total}"))
            .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
    );
    hdrs.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    (
        StatusCode::RANGE_NOT_SATISFIABLE,
        hdrs,
        axum::Json(shared::error::ApiError::new(
            shared::error::ApiErrorCode::RangeNotSatisfiable,
            "requested range invalid",
        )),
    )
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_jpeg() {
        let head = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert_eq!(sniff(&head), Some(("image/jpeg", "jpg")));
    }

    #[test]
    fn sniff_png() {
        let head = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        assert_eq!(sniff(&head), Some(("image/png", "png")));
    }

    #[test]
    fn sniff_webp() {
        let head = b"RIFF\x00\x00\x00\x00WEBP";
        assert_eq!(sniff(head), Some(("image/webp", "webp")));
    }

    #[test]
    fn sniff_svg_rejected() {
        assert!(sniff(b"<svg xmlns=").is_none());
        assert!(sniff(b"<?xml versi").is_none());
    }

    #[test]
    fn sniff_unknown_rejected() {
        assert!(sniff(b"plain text!").is_none());
    }

    #[test]
    fn parse_range_full() {
        match parse_range("bytes=0-99", 100) {
            ParsedRange::Ok { start, len } => {
                assert_eq!(start, 0);
                assert_eq!(len, 100);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn parse_range_open_end() {
        match parse_range("bytes=50-", 100) {
            ParsedRange::Ok { start, len } => {
                assert_eq!(start, 50);
                assert_eq!(len, 50);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn parse_range_suffix() {
        match parse_range("bytes=-10", 100) {
            ParsedRange::Ok { start, len } => {
                assert_eq!(start, 90);
                assert_eq!(len, 10);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn parse_range_unsatisfiable() {
        assert!(matches!(
            parse_range("bytes=100-200", 100),
            ParsedRange::Unsatisfiable
        ));
        assert!(matches!(
            parse_range("bytes=200-300", 100),
            ParsedRange::Unsatisfiable
        ));
    }

    #[test]
    fn parse_range_caps_end() {
        match parse_range("bytes=50-9999", 100) {
            ParsedRange::Ok { start, len } => {
                assert_eq!(start, 50);
                assert_eq!(len, 50);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn parse_range_multiple() {
        assert!(matches!(
            parse_range("bytes=0-9,20-29", 100),
            ParsedRange::Multiple
        ));
    }
}
