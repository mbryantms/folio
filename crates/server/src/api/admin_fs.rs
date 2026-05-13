//! `GET /admin/fs/list` — list immediate-child directories under a path
//! inside `COMIC_LIBRARY_PATH`.
//!
//! Used by the Admin → New Library dialog so operators don't have to
//! remember or translate Docker-mapped paths by hand. The picker starts
//! at the library root (the value of `COMIC_LIBRARY_PATH` — typically
//! `/library` inside the prod container) and refuses to traverse above
//! it: every request canonicalises the requested path and confirms it
//! is still a descendant of the canonicalised root, so symlinks pointing
//! outside the root are also rejected. Combined with `RequireAdmin`,
//! the surface can't be used to enumerate the container's `/etc` or
//! `/home`.
//!
//! Files (non-directories) are filtered out — the picker only selects
//! folders. Dotfiles (`.git`, `.DS_Store`, …) are hidden so the picker
//! list isn't dominated by housekeeping noise.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/fs/list", get(list))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListQuery {
    /// Path to list. Omit (or pass empty) to list the root. The server
    /// will reject anything that doesn't canonicalise to a descendant of
    /// `COMIC_LIBRARY_PATH`.
    pub path: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListResp {
    /// Canonical absolute path of the listed directory. The picker uses
    /// this for the breadcrumb and as the value to drill back up from.
    pub path: String,
    /// Canonical absolute path of the configured library root. The
    /// picker uses this to know when "up" should be disabled.
    pub root: String,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
}

#[utoipa::path(
    get,
    path = "/admin/fs/list",
    params(ListQuery),
    responses(
        (status = 200, body = ListResp),
        (status = 400, description = "path validation failed"),
        (status = 403, description = "admin only, or path is outside library root"),
        (status = 404, description = "path does not exist"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListQuery>,
) -> Response {
    let configured_root = &app.cfg.library_path;
    // Canonicalising the configured root in addition to the request path
    // means a symlinked library root works as long as the requested path
    // resolves under the same canonical target. We surface a distinct
    // error code when the root itself can't be resolved so the picker UI
    // can prompt the operator to fix `COMIC_LIBRARY_PATH` — generic 500
    // looks like a server bug, but the typical cause is a fresh-clone
    // dev environment (the gitignored `./fixtures/library` doesn't
    // exist yet) or a Docker volume that wasn't mounted.
    let root = match tokio::fs::canonicalize(configured_root).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %configured_root.display(),
                "fs/list: configured root not accessible",
            );
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "library_root_missing",
                &format!(
                    "Configured library root '{}' is not accessible on the server. \
                     Create the directory or update COMIC_LIBRARY_PATH.",
                    configured_root.display(),
                ),
            );
        }
    };
    let requested = match q.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => root.clone(),
    };
    // Reject paths with `..` segments before canonicalising. A canonical
    // `starts_with` check still catches the case, but rejecting early
    // gives a cleaner error code and avoids touching the filesystem at
    // all for obviously-malicious inputs.
    if requested
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "path may not contain '..'",
        );
    }
    let canonical = match tokio::fs::canonicalize(&requested).await {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return error(StatusCode::NOT_FOUND, "not_found", "path does not exist");
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %requested.display(), "fs/list: canonicalize failed");
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "path could not be resolved",
            );
        }
    };
    if !canonical.starts_with(&root) {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "path is outside the configured library root",
        );
    }
    let mut rd = match tokio::fs::read_dir(&canonical).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, path = %canonical.display(), "fs/list: read_dir failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "could not read directory",
            );
        }
    };
    let mut entries: Vec<DirEntry> = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(entry)) => {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir {
                    continue;
                }
                entries.push(DirEntry {
                    name,
                    path: entry.path().to_string_lossy().into_owned(),
                });
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, path = %canonical.display(), "fs/list: next_entry failed");
                break;
            }
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Json(ListResp {
        path: canonical.to_string_lossy().into_owned(),
        root: root.to_string_lossy().into_owned(),
        entries,
    })
    .into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_parent_dir_segment() {
        // The handler short-circuits any path containing a `..` segment.
        // We can verify the predicate cheaply here without spinning the
        // full app.
        let p = std::path::Path::new("/library/../etc");
        assert!(
            p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        );
    }
}
