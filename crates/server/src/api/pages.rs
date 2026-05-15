//! Multi-page rails M2 — page CRUD.
//!
//! Pages are user-owned named containers of pinned saved-view rails.
//! Every user has exactly one `is_system = true` row named "Home"
//! (slug `home`) auto-created by the M1 migration and reachable at `/`;
//! custom pages live at `/pages/{slug}` and can be renamed, reordered,
//! or deleted. Pin ownership is stored on `user_view_pin.page_id`
//! scoped by `(user_id, page_id, view_id)` — adding/removing rails
//! happens through the existing `/me/saved-views/{id}/pin` surface
//! (page-aware as of M3).
//!
//! Sidebar integration (auto-insert into `user_sidebar_entries` on
//! create, label sourced from the system page's name) lands in M4 with
//! the rest of the layout-resolver work.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post},
};
use chrono::Utc;
use entity::{user_page, user_sidebar_entry, user_view_pin};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::{NotSet, Set, Unchanged},
    ColumnTrait, ConnectionTrait, EntityTrait, ModelTrait, PaginatorTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::api::sidebar_layout::KIND_PAGE;

use crate::auth::CurrentUser;
use crate::slug::allocate_user_page_slug;
use crate::state::AppState;

/// Soft cap on custom pages per user (excludes the system Home row).
/// Enforced in the API; not a DB constraint.
const MAX_PAGES_PER_USER: u64 = 20;
/// Trim-then-char-count limit. Generous enough for descriptive names
/// ("Marvel — Ultimate Universe") while keeping the sidebar legible.
const MAX_NAME_LEN: usize = 80;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/pages", get(list).post(create))
        .route("/me/pages/reorder", post(reorder))
        .route("/me/pages/{id}", patch(update).delete(delete_one))
        .route("/me/pages/{id}/sidebar", post(set_sidebar))
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PageView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub is_system: bool,
    pub position: i32,
    /// Count of `user_view_pin` rows where `pinned = true` for this
    /// (user, page) pair. Lets the sidebar render a badge or the page
    /// picker show "n / 12" without fetching every pin row.
    pub pin_count: i64,
    /// Optional free-form description rendered under the title. `None`
    /// (or absent) hides the descriptor row.
    pub description: Option<String>,
    /// Whether this page appears in the sidebar nav. Computed from
    /// `user_sidebar_entries` — missing override means visible (the
    /// default). System pages always show via the builtin Home entry
    /// regardless of this flag.
    pub show_in_sidebar: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreatePageReq {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdatePageReq {
    #[serde(default)]
    pub name: Option<String>,
    /// Description string. Omitting (or sending null) leaves the
    /// existing value alone; sending an empty (or whitespace-only)
    /// string clears it. Avoids the `Option<Option<T>>` trick — serde
    /// can't reliably distinguish absent from explicit null without a
    /// custom deserializer, so the "clear via empty string" convention
    /// is the contract.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetSidebarQuery {
    /// `true` (default) shows the page in the sidebar; `false` hides it.
    /// Idempotent — repeated calls with the same value are no-ops.
    #[serde(default)]
    pub show: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReorderPagesReq {
    pub page_ids: Vec<Uuid>,
}

// ───── helpers ─────

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    let body = serde_json::json!({ "error": { "code": code, "message": message } });
    (status, Json(body)).into_response()
}

async fn fetch_owned<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    id: Uuid,
) -> Result<user_page::Model, axum::response::Response> {
    match user_page::Entity::find_by_id(id).one(db).await {
        Ok(Some(p)) if p.user_id == user_id => Ok(p),
        // Existence-leak guard: other-user rows look identical to missing.
        Ok(_) => Err(error(StatusCode::NOT_FOUND, "not_found", "page not found")),
        Err(e) => {
            tracing::error!(error = %e, "pages: fetch failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

async fn pin_count_for<C: ConnectionTrait>(db: &C, user_id: Uuid, page_id: Uuid) -> i64 {
    user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user_id))
        .filter(user_view_pin::Column::PageId.eq(page_id))
        .filter(user_view_pin::Column::Pinned.eq(true))
        .count(db)
        .await
        .unwrap_or(0) as i64
}

fn to_view(m: &user_page::Model, pin_count: i64, show_in_sidebar: bool) -> PageView {
    PageView {
        id: m.id.to_string(),
        name: m.name.clone(),
        slug: m.slug.clone(),
        is_system: m.is_system,
        position: m.position,
        pin_count,
        description: m.description.clone(),
        show_in_sidebar,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// Look up the user's `kind='page'` sidebar overrides keyed by ref_id.
/// Missing entries default to visible (the server's pages section in
/// `compute_layout` emits every page; only an explicit override hides
/// one).
async fn page_sidebar_overrides<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<HashMap<String, bool>, sea_orm::DbErr> {
    let rows = user_sidebar_entry::Entity::find()
        .filter(user_sidebar_entry::Column::UserId.eq(user_id))
        .filter(user_sidebar_entry::Column::Kind.eq(KIND_PAGE))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.ref_id, r.visible)).collect())
}

fn show_in_sidebar_for(model: &user_page::Model, overrides: &HashMap<String, bool>) -> bool {
    // System pages: visible via the builtin Home entry, not the kind='page'
    // override. Report visible regardless so the UI's toggle stays
    // meaningful (it just won't be exposed for system pages).
    if model.is_system {
        return true;
    }
    overrides
        .get(&model.id.to_string())
        .copied()
        .unwrap_or(true)
}

// ───── handlers ─────

#[utoipa::path(
    get,
    path = "/me/pages",
    responses((status = 200, body = Vec<PageView>))
)]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    // Resolve / lazy-create the system page so a brand-new user sees
    // Home in the list even before any pin/sidebar interaction has
    // landed the row.
    if let Err(e) = crate::pages::system_page_id(&app.db, user.id).await {
        tracing::error!(error = %e, "pages: system page resolve failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    let rows = match user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .order_by_asc(user_page::Column::Position)
        .order_by_asc(user_page::Column::CreatedAt)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "pages: list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let overrides = page_sidebar_overrides(&app.db, user.id)
        .await
        .unwrap_or_default();
    // Per-page COUNT in a small loop — N is bounded at MAX_PAGES_PER_USER + 1.
    let mut items: Vec<PageView> = Vec::with_capacity(rows.len());
    for row in rows {
        let pc = pin_count_for(&app.db, user.id, row.id).await;
        let visible = show_in_sidebar_for(&row, &overrides);
        items.push(to_view(&row, pc, visible));
    }
    Json(items).into_response()
}

#[utoipa::path(
    post,
    path = "/me/pages",
    request_body = CreatePageReq,
    responses((status = 201, body = PageView))
)]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreatePageReq>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() {
        return error(StatusCode::BAD_REQUEST, "validation", "name required");
    }
    if name.chars().count() > MAX_NAME_LEN {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "name must be 80 chars or fewer",
        );
    }
    // Ensure the system Home exists before counting — otherwise a fresh
    // user creating their first custom page would undercount and could
    // briefly exceed the documented cap if the system row materializes
    // mid-loop later.
    if let Err(e) = crate::pages::system_page_id(&app.db, user.id).await {
        tracing::error!(error = %e, "pages: system page resolve failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    // Cap applies to custom pages only.
    let custom_count = user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .filter(user_page::Column::IsSystem.eq(false))
        .count(&app.db)
        .await
        .unwrap_or(0);
    if custom_count >= MAX_PAGES_PER_USER {
        return error(
            StatusCode::CONFLICT,
            "page_cap_reached",
            "delete a page to add another",
        );
    }
    let slug = match allocate_user_page_slug(&app.db, user.id, name, None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "pages: slug allocate failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // Append after the user's current max so the new page lands at the
    // end of their sidebar order.
    let max_pos = user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .order_by_desc(user_page::Column::Position)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|p| p.position)
        .unwrap_or(0);
    let id = Uuid::now_v7();
    let row = match (user_page::ActiveModel {
        id: Set(id),
        user_id: Set(user.id),
        name: Set(name.to_owned()),
        slug: Set(slug),
        is_system: Set(false),
        position: Set(max_pos + 1),
        description: Set(None),
        created_at: NotSet,
        updated_at: NotSet,
    })
    .insert(&app.db)
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "pages: insert failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // New pages default to visible — no override row exists yet, so
    // `show_in_sidebar_for` correctly reports `true`.
    (StatusCode::CREATED, Json(to_view(&row, 0, true))).into_response()
}

#[utoipa::path(
    patch,
    path = "/me/pages/{id}",
    params(("id" = String, Path,)),
    request_body = UpdatePageReq,
    responses((status = 200, body = PageView))
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdatePageReq>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let mut am: user_page::ActiveModel = row.clone().into();
    let mut changed = false;
    if let Some(name) = req.name.as_ref() {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return error(StatusCode::BAD_REQUEST, "validation", "name required");
        }
        if trimmed.chars().count() > MAX_NAME_LEN {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "name must be 80 chars or fewer",
            );
        }
        if trimmed != row.name {
            am.name = Set(trimmed.to_owned());
            changed = true;
            // System page keeps its `home` slug regardless of name —
            // the route `/` always resolves to it, and the slug is the
            // user-visible URL token only for custom pages.
            if !row.is_system {
                let new_slug = match allocate_user_page_slug(&app.db, user.id, trimmed, Some(id))
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "pages: slug realloc failed");
                        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                    }
                };
                if new_slug != row.slug {
                    am.slug = Set(new_slug);
                }
            }
        }
    }
    if let Some(desc) = req.description.as_ref() {
        let trimmed = desc.trim();
        let normalized: Option<String> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        if normalized != row.description {
            am.description = Set(normalized);
            changed = true;
        }
    }
    if changed {
        am.updated_at = Set(Utc::now().fixed_offset());
    }
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "pages: update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let pc = pin_count_for(&app.db, user.id, updated.id).await;
    let overrides = page_sidebar_overrides(&app.db, user.id)
        .await
        .unwrap_or_default();
    let visible = show_in_sidebar_for(&updated, &overrides);
    Json(to_view(&updated, pc, visible)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/pages/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if row.is_system {
        return error(
            StatusCode::CONFLICT,
            "system_page",
            "the home page cannot be deleted",
        );
    }
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "pages: delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/pages/reorder",
    request_body = ReorderPagesReq,
    responses((status = 204))
)]
pub async fn reorder(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ReorderPagesReq>,
) -> impl IntoResponse {
    let owned = match user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "pages: reorder lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let owned_ids: std::collections::HashSet<Uuid> = owned.iter().map(|p| p.id).collect();
    let req_ids: std::collections::HashSet<Uuid> = req.page_ids.iter().copied().collect();
    if req_ids.len() != req.page_ids.len() {
        return error(StatusCode::BAD_REQUEST, "validation", "duplicate page id");
    }
    if req_ids != owned_ids {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "page_ids must list every owned page exactly once",
        );
    }
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Two-pass to dodge transient unique collisions while positions
    // overlap mid-rewrite; same idiom as saved_views::reorder.
    for (i, page_id) in req.page_ids.iter().enumerate() {
        let am = user_page::ActiveModel {
            id: Unchanged(*page_id),
            position: Set(10_000 + i as i32),
            updated_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        if let Err(e) = am.update(&txn).await {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "pages: reorder pass1 failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    for (i, page_id) in req.page_ids.iter().enumerate() {
        let am = user_page::ActiveModel {
            id: Unchanged(*page_id),
            position: Set(i as i32),
            updated_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        if let Err(e) = am.update(&txn).await {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "pages: reorder pass2 failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "pages: reorder commit failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/pages/{id}/sidebar",
    params(
        ("id" = String, Path,),
        ("show" = Option<bool>, Query,),
    ),
    responses((status = 204))
)]
pub async fn set_sidebar(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<SetSidebarQuery>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if row.is_system {
        // Hiding the system page would orphan `/`; the builtin Home
        // entry has its own toggle in /settings/navigation. Reject
        // rather than silently lying about state.
        return error(
            StatusCode::CONFLICT,
            "system_page",
            "the home page is controlled via the Home builtin in sidebar settings",
        );
    }
    let show = q.show.unwrap_or(true);

    // Upsert the override row for this page. The
    // `user_sidebar_entries` composite PK on (user_id, kind, ref_id)
    // lets the existing row be loaded + updated; missing → insert.
    let existing = user_sidebar_entry::Entity::find()
        .filter(user_sidebar_entry::Column::UserId.eq(user.id))
        .filter(user_sidebar_entry::Column::Kind.eq(KIND_PAGE))
        .filter(user_sidebar_entry::Column::RefId.eq(id.to_string()))
        .one(&app.db)
        .await
        .unwrap_or(None);
    let result = match existing {
        Some(model) => {
            // Avoid a write when the desired state already matches.
            if model.visible == show {
                return StatusCode::NO_CONTENT.into_response();
            }
            let mut am: user_sidebar_entry::ActiveModel = model.into();
            am.visible = Set(show);
            am.update(&app.db).await.map(|_| ())
        }
        None => {
            // No override exists → only insert one when the user is
            // overriding the visible default (which would be a no-op
            // here, so we'd never insert visible=true rows that match
            // the default). Hide → insert; show → no-op.
            if show {
                return StatusCode::NO_CONTENT.into_response();
            }
            let am = user_sidebar_entry::ActiveModel {
                user_id: Set(user.id),
                kind: Set(KIND_PAGE.into()),
                ref_id: Set(id.to_string()),
                visible: Set(false),
                // Position is reassigned wholesale by /me/sidebar-layout
                // PATCH and only matters when the user has explicit
                // overrides for the whole list. Park at 0 as a sentinel
                // for "no explicit order" — compute_layout merges in
                // the default position from the section walk.
                position: Set(0),
                label: Set(None),
            };
            am.insert(&app.db).await.map(|_| ())
        }
    };
    if let Err(e) = result {
        tracing::error!(error = %e, "pages: sidebar toggle failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}
