//! Chapter breaks in a reading list (plan `linked-reading-lists-1.0`,
//! PR 3a).
//!
//! A break starts a new chapter at an issue, with an optional name, so a
//! long list can be read in pieces without linking an event. Breaks are
//! part of the list's shape: everyone who can read the list sees them, and
//! only someone who can manage it can change them (the owner of a personal
//! list, an admin for a shared one, audited as `admin.cbl_list.break.*`).
//! They come back in the links view (`GET /me/cbl-lists/{id}/links`:
//! `breaks`, and `chapters[].break_id`).
//!
//! - `POST   /me/cbl-lists/{id}/breaks` — start a chapter at an entry.
//! - `PATCH  /me/cbl-lists/{id}/breaks/{break_id}` — rename (or unname).
//! - `DELETE /me/cbl-lists/{id}/breaks/{break_id}`.

use axum::{
    Extension,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::reading_list_break;
use entity::reading_list_link::AnchorStatus;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, ModelTrait, PaginatorTrait,
    QueryFilter,
};
use serde::Deserialize;
use server_macros::handler;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_links::{
    ReadingListLinksView, anchor_of_entry, field_error, internal, view_response,
};
use super::cbl_lists::{audit_admin_list_write, ensure_can_manage, fetch_list};
use super::error;
use super::extractors::Validated;
use crate::auth::CurrentUser;
use crate::middleware::RequestContext;
use crate::state::AppState;

/// Most chapter breaks one list may hold (keeps the links GET bounded).
pub const MAX_BREAKS_PER_LIST: u64 = 500;
/// Longest chapter name, in characters.
pub const MAX_LABEL_CHARS: usize = 120;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_break))
        .routes(routes!(update_break, delete_break))
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateBreakReq {
    /// The entry the new chapter starts at. Give this or `position`.
    #[serde(default)]
    #[garde(skip)]
    pub entry_id: Option<Uuid>,
    /// The 0-based position the chapter starts at, for naming a chapter
    /// that already exists (the list's first, or the one after an event)
    /// without knowing its first entry's id.
    #[serde(default)]
    #[garde(range(min = 0))]
    pub position: Option<i32>,
    /// The chapter's name; omitted or blank = named after its first issue.
    #[serde(default)]
    #[garde(custom(label_fits))]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct UpdateBreakReq {
    /// The new name; `null` or blank = named after its first issue.
    #[serde(default)]
    #[garde(custom(label_fits))]
    pub label: Option<String>,
}

fn label_fits(value: &Option<String>, _: &()) -> garde::Result {
    match value {
        Some(v) if v.trim().chars().count() > MAX_LABEL_CHARS => Err(garde::Error::new(format!(
            "a chapter name can be at most {MAX_LABEL_CHARS} characters"
        ))),
        _ => Ok(()),
    }
}

/// Trimmed label; blank means none.
fn clean(label: Option<String>) -> Option<String> {
    label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty())
}

async fn fetch_break(
    app: &AppState,
    list_id: Uuid,
    break_id: Uuid,
) -> Result<reading_list_break::Model, Response> {
    reading_list_break::Entity::find_by_id(break_id)
        .filter(reading_list_break::Column::ListId.eq(list_id))
        .one(&app.db)
        .await
        .map_err(|_| internal())?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "not_found",
                "chapter break not found",
            )
        })
}

#[utoipa::path(
    operation_id = "cbl_breaks_create",
    post,
    path = "/me/cbl-lists/{id}/breaks",
    params(("id" = String, Path,)),
    request_body = CreateBreakReq,
    responses(
        (status = 201, body = ReadingListLinksView),
        (status = 404, description = "list not found"),
        (status = 409, description = "a chapter already starts at that issue"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn create_break(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
    Validated(req): Validated<CreateBreakReq>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&list, &user).await {
        return r;
    }
    let entry_id = match (req.entry_id, req.position) {
        (Some(id), _) => id,
        (None, Some(position)) => {
            let found = entity::cbl_entry::Entity::find()
                .filter(entity::cbl_entry::Column::CblListId.eq(list.id))
                .filter(entity::cbl_entry::Column::Position.eq(position))
                .one(&app.db)
                .await;
            match found {
                Ok(Some(e)) => e.id,
                Ok(None) => return field_error("position", "no entry at that position"),
                Err(_) => return internal(),
            }
        }
        (None, None) => return field_error("entry_id", "say which issue the chapter starts at"),
    };
    let (key, position) = match anchor_of_entry(&app, list.id, entry_id).await {
        Ok(Some(v)) => v,
        Ok(None) => return field_error("entry_id", "entry not in this list"),
        Err(_) => return internal(),
    };
    let label = clean(req.label);
    let checks = async {
        let earliest = entity::cbl_entry::Entity::find()
            .filter(entity::cbl_entry::Column::CblListId.eq(list.id))
            .filter(entity::cbl_entry::Column::Position.lt(position))
            .count(&app.db)
            .await?;
        let taken = reading_list_break::Entity::find()
            .filter(reading_list_break::Column::ListId.eq(list.id))
            .filter(reading_list_break::Column::AnchorPositionHint.eq(position))
            .filter(reading_list_break::Column::AnchorStatus.eq(AnchorStatus::Placed))
            .count(&app.db)
            .await?
            > 0;
        let total = reading_list_break::Entity::find()
            .filter(reading_list_break::Column::ListId.eq(list.id))
            .count(&app.db)
            .await?;
        Ok::<_, sea_orm::DbErr>((earliest == 0, taken, total))
    };
    let (is_first, taken, total) = match checks.await {
        Ok(v) => v,
        Err(_) => return internal(),
    };
    // The first chapter already starts at the first issue: a break there
    // only carries its name.
    if is_first && label.is_none() {
        return field_error("entry_id", "the list's first chapter already starts there");
    }
    if taken {
        return error(
            StatusCode::CONFLICT,
            "conflict",
            "a chapter already starts at that issue",
        );
    }
    if total >= MAX_BREAKS_PER_LIST {
        return field_error(
            "entry_id",
            &format!("a list can hold at most {MAX_BREAKS_PER_LIST} chapter breaks"),
        );
    }
    let now = Utc::now().fixed_offset();
    let row = reading_list_break::ActiveModel {
        id: Set(Uuid::now_v7()),
        list_id: Set(list.id),
        label: Set(label.clone()),
        anchor_cv_issue_id: Set(key.cv_issue_id),
        anchor_series: Set(key.series),
        anchor_number: Set(key.number),
        anchor_volume: Set(key.volume),
        anchor_year: Set(key.year),
        anchor_position_hint: Set(Some(position)),
        anchor_status: Set(AnchorStatus::Placed),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let row = match row.insert(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "cbl breaks: insert failed");
            return internal();
        }
    };
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &list,
        "admin.cbl_list.break.create",
        serde_json::json!({ "break_id": row.id.to_string(), "label": label }),
    )
    .await;
    view_response(&app, &user, &list, StatusCode::CREATED).await
}

#[utoipa::path(
    operation_id = "cbl_breaks_update",
    patch,
    path = "/me/cbl-lists/{id}/breaks/{break_id}",
    params(("id" = String, Path,), ("break_id" = String, Path,)),
    request_body = UpdateBreakReq,
    responses(
        (status = 200, body = ReadingListLinksView),
        (status = 404, description = "list or chapter break not found"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn update_break(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((id, break_id)): AxPath<(Uuid, Uuid)>,
    Validated(req): Validated<UpdateBreakReq>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&list, &user).await {
        return r;
    }
    let row = match fetch_break(&app, list.id, break_id).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let label = clean(req.label);
    let mut am: reading_list_break::ActiveModel = row.into();
    am.label = Set(label.clone());
    am.updated_at = Set(Utc::now().fixed_offset());
    if am.update(&app.db).await.is_err() {
        return internal();
    }
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &list,
        "admin.cbl_list.break.update",
        serde_json::json!({ "break_id": break_id.to_string(), "label": label }),
    )
    .await;
    view_response(&app, &user, &list, StatusCode::OK).await
}

#[utoipa::path(
    operation_id = "cbl_breaks_delete",
    delete,
    path = "/me/cbl-lists/{id}/breaks/{break_id}",
    params(("id" = String, Path,), ("break_id" = String, Path,)),
    responses(
        (status = 204),
        (status = 404, description = "list or chapter break not found"),
    )
)]
#[handler]
pub async fn delete_break(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((id, break_id)): AxPath<(Uuid, Uuid)>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&list, &user).await {
        return r;
    }
    let row = match fetch_break(&app, list.id, break_id).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if row.delete(&app.db).await.is_err() {
        return internal();
    }
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &list,
        "admin.cbl_list.break.delete",
        serde_json::json!({ "break_id": break_id.to_string() }),
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}
