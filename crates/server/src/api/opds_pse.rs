//! OPDS Page Streaming Extension (PSE) handler — `GET /opds/pse/{issue_id}/{n}`.
//!
//! Authorisation is **out-of-band**: there's no Bearer/Basic/Cookie on
//! this surface. Instead the URL itself carries an HMAC-SHA256 signature
//! (`?u=…&exp=…&sig=…`) issued by `auth::url_signing::issue_query` when
//! the per-entry PSE link is rendered into an acquisition feed. The sig
//! covers `(issue_id, user_id, exp)` so a leaked URL can't be redirected
//! at someone else's account, but it intentionally does **not** cover the
//! page index: clients substitute `{pageNumber}` per the OPDS-PSE spec
//! and one signed window grants access to every page of that issue.
//!
//! Verification chain:
//!   1. parse + verify the URL signature → 401 on tamper/expired/malformed
//!   2. look up `users` by the signed user_id → 401 if gone
//!   3. look up `issues` by id → 404
//!   4. confirm the user still has access to the library → 403 on revoke
//!   5. serve the page bytes (sniffed allowlist, ETag, Range-able)
//!
//! Audit: on the *first* page of an issue (`n == 0`) we record one
//! `opds.pse.access` row. That single row stands in for "this user
//! started streaming this issue at this time" — recording on every page
//! would dominate the audit log on a long read.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, library_user_access, user};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::url_signing::{self, PseUrlError};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/opds/pse/{issue_id}/{n}", get(stream))
}

#[derive(Debug, Deserialize)]
pub struct PseQuery {
    pub u: Option<Uuid>,
    pub exp: Option<u64>,
    pub sig: Option<String>,
}

/// `GET /opds/pse/{issue_id}/{n}` — sig-auth page stream.
pub async fn stream(
    State(app): State<AppState>,
    AxPath((issue_id, n)): AxPath<(String, u32)>,
    Query(q): Query<PseQuery>,
    headers: HeaderMap,
) -> Response {
    let (u, exp, sig) = match (q.u, q.exp, q.sig.as_deref()) {
        (Some(u), Some(exp), Some(sig)) => (u, exp, sig),
        _ => return error(StatusCode::UNAUTHORIZED, "pse_missing_params"),
    };

    if let Err(e) =
        url_signing::verify(&issue_id, u, exp, sig, app.secrets.url_signing_key.as_ref())
    {
        let code = match e {
            PseUrlError::Expired => "pse_expired",
            PseUrlError::BadSig => "pse_bad_sig",
            PseUrlError::Malformed => "pse_malformed",
        };
        return error(StatusCode::UNAUTHORIZED, code);
    }

    // Sig is good — pull the user row to confirm the principal still
    // exists. A revoked / deleted user can't squeeze a stale signed URL
    // through.
    let user_row = match user::Entity::find_by_id(u).one(&app.db).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "user_not_found"),
        Err(e) => {
            tracing::warn!(error = %e, "pse: user lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };
    if user_row.state != "active" {
        return error(StatusCode::UNAUTHORIZED, "user_inactive");
    }

    let issue_row = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => {
            tracing::warn!(error = %e, "pse: issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };

    // Library ACL — admins always pass, non-admins need an explicit grant.
    if user_row.role != "admin" {
        let allowed = library_user_access::Entity::find()
            .filter(library_user_access::Column::UserId.eq(u))
            .filter(library_user_access::Column::LibraryId.eq(issue_row.library_id))
            .one(&app.db)
            .await
            .ok()
            .flatten()
            .is_some();
        if !allowed {
            return error(StatusCode::FORBIDDEN, "library_access_denied");
        }
    }

    // Open + sniff. Mirrors `api::page_bytes::serve` — kept inline rather
    // than refactored into a shared helper because page_bytes also wires
    // up Cache-Control and If-Range against the `CurrentUser` etag, both
    // of which don't apply here verbatim.
    let arc = match app
        .zip_lru
        .get_or_open(&issue_row.id, std::path::Path::new(&issue_row.file_path))
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, issue_id = %issue_row.id, "pse: zip_lru open failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "archive_unreadable");
        }
    };

    let page_index = n as usize;
    let etag_value = format!("\"pse-{}-{}\"", &issue_row.id[..32], page_index);

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let arc_clone = arc.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<PageMeta, PageError> {
        let mut cbz = arc_clone.lock().expect("cbz mutex");
        let pages = cbz.pages();
        let entry = pages
            .get(page_index)
            .copied()
            .cloned()
            .ok_or(PageError::NotFound)?;
        let total = entry.uncompressed_size;
        let head_len = total.min(16) as usize;
        let head = if head_len > 0 {
            cbz.read_entry_range(&entry, 0, head_len as u64)
                .map_err(|e| PageError::Read(e.to_string()))?
        } else {
            Vec::new()
        };
        let (mime, ext) = sniff(&head).ok_or(PageError::UnsupportedType)?;
        Ok(PageMeta {
            total,
            mime,
            ext,
            entry_index: page_index,
        })
    })
    .await;

    let meta = match result {
        Ok(Ok(m)) => m,
        Ok(Err(PageError::NotFound)) => return error(StatusCode::NOT_FOUND, "page_not_found"),
        Ok(Err(PageError::UnsupportedType)) => {
            return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type");
        }
        Ok(Err(PageError::Read(e))) => {
            tracing::warn!(error = %e, "pse: page read failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "pse: page task failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };

    let (status, start, len) = if let Some(rh) = range_header.as_deref() {
        match parse_range(rh, meta.total) {
            ParsedRange::Ok { start, len } => (StatusCode::PARTIAL_CONTENT, start, len),
            ParsedRange::Unsatisfiable => return unsatisfiable(meta.total),
            ParsedRange::Malformed => (StatusCode::OK, 0, meta.total),
        }
    } else {
        (StatusCode::OK, 0, meta.total)
    };

    let arc_for_read = arc.clone();
    let entry_idx = meta.entry_index;
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
            tracing::warn!(error = %e, "pse: byte read failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "pse: byte task failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };

    // Audit only the first page (n == 0). One row stands in for "this
    // user started streaming this issue at this time"; logging every
    // page would saturate the audit log on a long read.
    if n == 0 {
        audit::record(
            &app.db,
            AuditEntry {
                actor_id: u,
                action: "opds.pse.access",
                target_type: Some("issue"),
                target_id: Some(issue_row.id.clone()),
                payload: serde_json::json!({ "file_path": issue_row.file_path }),
                ip: None,
                user_agent: None,
            },
        )
        .await;
    }

    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(meta.mime).unwrap(),
    );
    hdrs.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    hdrs.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("inline; filename=\"page-{n}.{}\"", meta.ext)).unwrap(),
    );
    // PSE clients (Chunky / KOReader) re-fetch pages on every reopen
    // unless the cache is honoured. The signed URL itself is the access
    // gate, so `private, max-age=…` is safe — the URL won't outlive the
    // signature.
    hdrs.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=1800"),
    );
    hdrs.insert(header::ETAG, HeaderValue::from_str(&etag_value).unwrap());
    hdrs.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(body_bytes.len() as u64),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        let end = start + body_bytes.len() as u64 - 1;
        hdrs.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", meta.total)).unwrap(),
        );
    }

    (status, hdrs, Body::from(body_bytes)).into_response()
}

struct PageMeta {
    total: u64,
    mime: &'static str,
    ext: &'static str,
    entry_index: usize,
}

#[derive(Debug)]
enum PageError {
    NotFound,
    UnsupportedType,
    Read(String),
}

/// Same magic-byte allowlist as `api::page_bytes::sniff`. SVG is
/// rejected to remove the script vector — duplicated here rather than
/// re-exported to keep the two surfaces independently auditable.
fn sniff(head: &[u8]) -> Option<(&'static str, &'static str)> {
    let s = std::str::from_utf8(head).unwrap_or("").trim_start();
    if s.starts_with("<svg") || s.starts_with("<?xml") {
        return None;
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("image/jpeg", "jpg"));
    }
    if head.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(("image/png", "png"));
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some(("image/webp", "webp"));
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some(("image/avif", "avif"));
        }
    }
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

enum ParsedRange {
    Ok { start: u64, len: u64 },
    Unsatisfiable,
    Malformed,
}

fn parse_range(header: &str, total: u64) -> ParsedRange {
    let body = match header.strip_prefix("bytes=") {
        Some(s) => s.trim(),
        None => return ParsedRange::Malformed,
    };
    if body.contains(',') {
        return ParsedRange::Malformed;
    }
    let (a, b) = match body.split_once('-') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => return ParsedRange::Malformed,
    };
    if a.is_empty() {
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
        HeaderValue::from_str(&format!("bytes */{total}")).unwrap(),
    );
    hdrs.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::RANGE_NOT_SATISFIABLE, hdrs, Body::empty()).into_response()
}

fn error(status: StatusCode, code: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({"error": {"code": code, "message": code}})),
    )
        .into_response()
}
