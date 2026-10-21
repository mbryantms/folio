//! Per-user, issue-level closing note.
//!
//! `PUT /series/{series_slug}/issues/{issue_slug}/note` with
//! `{ "body": "…" | null }`. A `null`, empty, or whitespace-only body
//! clears the note (deletes the row) so the client has one call for
//! "save" and "clear", mirroring the rating endpoint next door.
//!
//! Unlike a page note (`markers.kind = 'note'`), this note has no page:
//! it is written about the whole issue from the end-of-issue card's
//! Wrap up pane and read back inline on `GET /series/{s}/issues/{i}` as
//! `user_note`.

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::user_issue_note;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::api::extractors::Validated;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

/// Upper bound on a note body, in characters. Generous for a paragraph
/// or three; stops a runaway paste from landing a megabyte in the row.
pub const MAX_NOTE_CHARS: usize = 20_000;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(set_issue_note))
}

/// Body for the note endpoint. `null` / blank clears the note.
#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct SetIssueNoteReq {
    #[garde(inner(length(chars, max = MAX_NOTE_CHARS)))]
    pub body: Option<String>,
}

/// The note as stored. `body` is `None` after a clear; `updated_at` is
/// RFC 3339 and `None` alongside it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueNoteView {
    pub body: Option<String>,
    pub updated_at: Option<String>,
}

#[utoipa::path(
    operation_id = "issue_notes_set_issue_note",
    put,
    path = "/series/{series_slug}/issues/{issue_slug}/note",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = SetIssueNoteReq,
    responses(
        (status = 200, body = IssueNoteView),
        (status = 404, description = "issue not found"),
        (status = 422, description = "validation error"),
    )
)]
#[handler]
pub async fn set_issue_note(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Validated(req): Validated<SetIssueNoteReq>,
) -> impl IntoResponse {
    let issue_row =
        match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };
    if !crate::library::access::issue_visible(&app, &user, &issue_row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // Blank means clear — the Wrap up textarea sends whatever it holds.
    let body = req
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let Some(body) = body else {
        if let Err(e) = user_issue_note::Entity::delete_many()
            .filter(user_issue_note::Column::UserId.eq(user.id))
            .filter(user_issue_note::Column::IssueId.eq(&issue_row.id))
            .exec(&app.db)
            .await
        {
            tracing::error!(error = %e, "issue note clear failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        return Json(IssueNoteView {
            body: None,
            updated_at: None,
        })
        .into_response();
    };

    let now = chrono::Utc::now().fixed_offset();
    let existing = user_issue_note::Entity::find_by_id((user.id, issue_row.id.clone()))
        .one(&app.db)
        .await;
    let result = match existing {
        Ok(Some(row)) => {
            let mut am: user_issue_note::ActiveModel = row.into();
            am.body = Set(body);
            am.updated_at = Set(now);
            am.update(&app.db).await
        }
        Ok(None) => {
            user_issue_note::ActiveModel {
                user_id: Set(user.id),
                issue_id: Set(issue_row.id.clone()),
                body: Set(body),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&app.db)
            .await
        }
        Err(e) => {
            tracing::error!(error = %e, "issue note lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    match result {
        Ok(m) => Json(IssueNoteView {
            body: Some(m.body),
            updated_at: Some(m.updated_at.to_rfc3339()),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "issue note upsert failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// The calling user's closing note on `issue_id`, for inlining on the
/// issue detail view. Read failures degrade to `None` — the detail page
/// must not 500 over a side-table hiccup.
pub(crate) async fn lookup_user_note(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_id: &str,
) -> Option<String> {
    user_issue_note::Entity::find_by_id((user_id, issue_id.to_owned()))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|m| m.body)
}
