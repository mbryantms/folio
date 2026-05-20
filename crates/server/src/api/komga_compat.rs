//! Komga REST compatibility shim — M3 of progress-writeback-2.0.
//!
//! Two endpoints that mimic Komga's progress-sync API surface so
//! OPDS-PSE clients with hardcoded Komga support (Panels iOS/macOS,
//! Mihon / Tachiyomi / Yokai Android via the Komga extension) can
//! sync reading progress to Folio. Together with the OPDS feed
//! fingerprint emitted by `crate::api::opds::komga_compat_author`
//! (M2), Panels detects Folio as Komga and uses its hardcoded
//! `PATCH /api/v1/books/{id}/read-progress` writer.
//!
//! Both endpoints require `compat.opds_panels_mode = "komga"`. When
//! the flag is off, the routes return 404 — Folio's identity stays
//! preserved and the surface vanishes.
//!
//! The wire format mirrors Komga's `BookController.kt` exactly:
//! - `PATCH /api/v1/books/{bookId}/read-progress` body
//!   `{page?: number, completed?: boolean}` → 204
//! - `GET   /api/v1/books/{bookId}` → BookDto with `readProgress: {
//!   page, completed, lastModified, readDate }`
//!
//! `bookId` maps to Folio's `issue.id` (Komga uses string IDs for
//! books; Folio's issue id is already a String). Auth is HTTP Basic
//! with the user's app-password (`read+progress` scope required for
//! PATCH); the existing `CurrentUser` / `RequireProgressScope`
//! extractors do the work.
//!
//! Long-term: this whole module is sunsettable. See
//! `~/.claude/plans/progress-writeback-2.0.md` § "Long-term sunset
//! path" — when a major client ships OPDS Progression 1.0 support
//! (M5 in the same plan), default `compat.opds_panels_mode` stays
//! off, this code becomes ballast, and after a year of zero support
//! tickets we delete the module entirely.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use entity::{issue, progress_record};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};

use crate::api::{error, not_found};
use crate::auth::extractor::{CurrentUser, RequireProgressScope};
use crate::library::access;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/books/{book_id}", get(get_book))
        .route(
            "/api/v1/books/{book_id}/read-progress",
            patch(patch_read_progress),
        )
}

/// Spec-faithful subset of Komga's `BookDto`. Only the fields Panels
/// (and other hardcoded-Komga clients) actually consume. Skipping the
/// rest keeps the response tight — Komga's full DTO is ~50 fields.
#[derive(Debug, Serialize)]
struct BookDto {
    id: String,
    #[serde(rename = "seriesId")]
    series_id: String,
    name: String,
    media: MediaDto,
    #[serde(rename = "readProgress")]
    read_progress: Option<ReadProgressDto>,
}

#[derive(Debug, Serialize)]
struct MediaDto {
    #[serde(rename = "pagesCount")]
    pages_count: i32,
}

#[derive(Debug, Serialize)]
struct ReadProgressDto {
    /// 1-indexed (Komga convention: page=1 is the first page). Folio
    /// stores `last_page` as 0-indexed; add 1 here so Panels reads
    /// what it expects.
    page: i32,
    completed: bool,
    /// RFC 3339 / ISO 8601 timestamp; Panels compares against the
    /// local session's `lastModified` for conflict detection.
    #[serde(rename = "lastModified")]
    last_modified: String,
    /// Same value as `lastModified` for now — Komga distinguishes
    /// "when the row was created" vs "when last touched"; Folio's
    /// progress_record only has the latter.
    #[serde(rename = "readDate")]
    read_date: String,
}

/// `PATCH /api/v1/books/{book_id}/read-progress`.
///
/// Komga's contract: body MUST have at least one of `page` /
/// `completed`. When `completed = true`, mark finished regardless of
/// `page`. When `page` is set, store it (1-indexed in body, store
/// 0-indexed in DB). 204 on success; 422 on missing both fields.
#[derive(Debug, Deserialize)]
struct ReadProgressUpdateDto {
    page: Option<i32>,
    completed: Option<bool>,
}

async fn patch_read_progress(
    State(app): State<AppState>,
    user: RequireProgressScope,
    Path(book_id): Path<String>,
    Json(body): Json<ReadProgressUpdateDto>,
) -> Response {
    if !app.cfg().is_komga_compat() {
        return not_found();
    }
    if body.page.is_none() && body.completed.is_none() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "must provide at least one of `page` / `completed`",
        );
    }
    let issue_row = match issue::Entity::find_by_id(book_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "komga_compat: issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let visible = access::for_user(&app, &user.0).await;
    if !visible.contains(issue_row.library_id) {
        return not_found();
    }

    // Komga's contract: `page` is 1-indexed on the wire; convert to
    // Folio's 0-indexed DB column. `completed = true` short-circuits
    // to "finished at last page"; if a `page` was also given, prefer
    // the explicit page over the inferred last-page derived from
    // total_count (Komga does the same).
    let (db_page, finished_override) = match (body.page, body.completed) {
        (Some(p), Some(true)) => (p.saturating_sub(1).max(0), Some(true)),
        (Some(p), _) => (p.saturating_sub(1).max(0), body.completed),
        (None, Some(true)) => {
            let last = issue_row.page_count.unwrap_or(0).max(0);
            (last.saturating_sub(1).max(0), Some(true))
        }
        // Already rejected above by the both-none guard.
        (None, _) => unreachable!(),
    };
    if let Err(e) = crate::api::progress::upsert_for(
        &app,
        user.0.id,
        &issue_row,
        db_page,
        finished_override,
        Some("komga-compat".into()),
    )
    .await
    {
        tracing::warn!(error = %e, "komga_compat: progress upsert failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /api/v1/books/{book_id}` — return Komga-shaped `BookDto`
/// with the caller's `readProgress` block. Panels uses this on
/// issue open to decide whether the local session is stale.
async fn get_book(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(book_id): Path<String>,
) -> Response {
    if !app.cfg().is_komga_compat() {
        return not_found();
    }
    let issue_row = match issue::Entity::find_by_id(book_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "komga_compat: issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let visible = access::for_user(&app, &user).await;
    if !visible.contains(issue_row.library_id) {
        return not_found();
    }
    let pr = progress_record::Entity::find_by_id((user.id, book_id.clone()))
        .one(&app.db)
        .await
        .ok()
        .flatten();
    let dto = BookDto {
        id: issue_row.id.clone(),
        series_id: issue_row.series_id.to_string(),
        name: issue_row
            .title
            .clone()
            .or_else(|| issue_row.number_raw.clone().map(|n| format!("Issue #{n}")))
            .unwrap_or_else(|| "Issue".to_owned()),
        media: MediaDto {
            pages_count: issue_row.page_count.unwrap_or(0).max(0),
        },
        read_progress: pr.map(|p| {
            let ts = p.updated_at.to_rfc3339();
            ReadProgressDto {
                page: p.last_page + 1,
                completed: p.finished,
                last_modified: ts.clone(),
                read_date: ts,
            }
        }),
    };
    (StatusCode::OK, Json(dto)).into_response()
}
