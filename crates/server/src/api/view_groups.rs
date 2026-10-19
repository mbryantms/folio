//! Views-index groups — per-user named buckets for organizing saved
//! views on Settings → Views.
//!
//! A heavy CBL user ends up with dozens of reading lists in one flat
//! grid. Groups let them drag those lists into categories ("Marvel
//! events", "Finished", …) that are renamed, reordered, and collapsed
//! independently of the lists themselves. Groups are scoped to one index
//! tab (`scope` ∈ `filter` / `cbl` / `collection`) and are purely an
//! organizational overlay: membership never changes what a view *is*,
//! where it's pinned, or whether it shows in the sidebar.
//!
//! Surface:
//!
//!   - `GET    /me/view-groups?scope=`      — groups + ordered member ids,
//!     plus the hand-arranged order of the ungrouped remainder
//!   - `POST   /me/view-groups`             — create (appends to the end)
//!   - `PATCH  /me/view-groups/{id}`        — rename / collapse
//!   - `DELETE /me/view-groups/{id}`        — delete; members become ungrouped
//!   - `PUT    /me/view-groups/layout`      — wholesale order + membership
//!     replace for one scope. This is what drag-and-drop commits to: one
//!     request carries the new group order, each group's member list, and
//!     the ungrouped order, so a cross-group move never leaves a
//!     half-applied state.
//!
//! Views not listed by any group are "ungrouped". Their order inside that
//! section is persisted too — as membership rows with a NULL `group_id` —
//! so a user who arranges the remainder by hand finds it where they left
//! it; views without any row trail the arranged ones. Membership rows for
//! a view that stops being visible to the caller (deleted view) cascade
//! away at the DB level.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use entity::{saved_view, saved_view_group, saved_view_group_item};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::{NotSet, Set, Unchanged},
    ColumnTrait, ConnectionTrait, EntityTrait, ModelTrait, PaginatorTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::extractors::Validated;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

/// Soft cap on groups per `(user, scope)`. Enforced in the API; not a DB
/// constraint.
const MAX_GROUPS_PER_SCOPE: u64 = 50;
const MAX_NAME_LEN: usize = 80;

pub const SCOPE_FILTER: &str = "filter";
pub const SCOPE_CBL: &str = "cbl";
pub const SCOPE_COLLECTION: &str = "collection";
const SCOPES: &[&str] = &[SCOPE_FILTER, SCOPE_CBL, SCOPE_COLLECTION];

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(create))
        .routes(routes!(replace_layout))
        .routes(routes!(update))
        .routes(routes!(delete_one))
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ViewGroupView {
    pub id: String,
    /// `'filter'` | `'cbl'` | `'collection'` — the Views tab this group
    /// belongs to.
    pub scope: String,
    pub name: String,
    /// Ordinal within the scope (dense 0..N after any layout write).
    pub position: i32,
    /// Whether the index renders the group collapsed.
    pub collapsed: bool,
    /// Member saved-view ids in display order.
    pub view_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ViewGroupListView {
    pub scope: String,
    pub groups: Vec<ViewGroupView>,
    /// Hand-arranged order of the ungrouped section. Ungrouped views
    /// missing from this list have no pinned position and render after
    /// these, in the client's sort order.
    pub ungrouped_ids: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListQuery {
    /// Required tab scope (`filter` / `cbl` / `collection`).
    pub scope: String,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateViewGroupReq {
    #[garde(custom(valid_scope))]
    pub scope: String,
    #[garde(custom(non_empty_after_trim_80))]
    pub name: String,
    /// Optional initial members. Each must be a view visible to the
    /// caller; a view already in another group of the same scope is moved.
    #[serde(default)]
    #[garde(skip)]
    pub view_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct UpdateViewGroupReq {
    #[serde(default)]
    #[garde(inner(custom(non_empty_after_trim_80)))]
    pub name: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub collapsed: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LayoutGroupReq {
    pub id: Uuid,
    /// Member view ids in display order. A view listed under no group is
    /// ungrouped after the write.
    pub view_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReplaceLayoutReq {
    pub scope: String,
    /// Every group in the scope, exactly once, in display order.
    pub groups: Vec<LayoutGroupReq>,
    /// Ungrouped views in display order. Optional and may be partial:
    /// ungrouped views not listed simply have no pinned position.
    #[serde(default)]
    pub ungrouped: Vec<Uuid>,
}

fn valid_scope(value: &str, _: &()) -> garde::Result {
    if SCOPES.contains(&value) {
        Ok(())
    } else {
        Err(garde::Error::new(
            "scope must be filter, cbl, or collection",
        ))
    }
}

fn non_empty_after_trim_80(value: &str, _: &()) -> garde::Result {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(garde::Error::new("name required"));
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(garde::Error::new("name must be 80 chars or fewer"));
    }
    Ok(())
}

// ───── helpers ─────

fn internal() -> axum::response::Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn invalid_scope_response() -> axum::response::Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation",
        "scope must be filter, cbl, or collection",
    )
}

async fn fetch_owned<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    id: Uuid,
) -> Result<saved_view_group::Model, axum::response::Response> {
    match saved_view_group::Entity::find_by_id(id).one(db).await {
        Ok(Some(g)) if g.user_id == user_id => Ok(g),
        // Existence-leak guard: other-user rows look identical to missing.
        Ok(_) => Err(error(StatusCode::NOT_FOUND, "not_found", "group not found")),
        Err(e) => {
            tracing::error!(error = %e, "view_groups: fetch failed");
            Err(internal())
        }
    }
}

/// Member rows for the given groups, keyed by group id and sorted by
/// position.
async fn members_by_group<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    group_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>, sea_orm::DbErr> {
    if group_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = saved_view_group_item::Entity::find()
        .filter(saved_view_group_item::Column::UserId.eq(user_id))
        .filter(saved_view_group_item::Column::GroupId.is_in(group_ids.iter().copied()))
        .order_by_asc(saved_view_group_item::Column::Position)
        .all(db)
        .await?;
    let mut out: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for r in rows {
        if let Some(gid) = r.group_id {
            out.entry(gid).or_default().push(r.view_id);
        }
    }
    Ok(out)
}

/// Pinned order of the ungrouped section for `(user, scope)`.
async fn ungrouped_order<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    scope: &str,
) -> Result<Vec<Uuid>, sea_orm::DbErr> {
    let rows = saved_view_group_item::Entity::find()
        .filter(saved_view_group_item::Column::UserId.eq(user_id))
        .filter(saved_view_group_item::Column::Scope.eq(scope))
        .filter(saved_view_group_item::Column::GroupId.is_null())
        .order_by_asc(saved_view_group_item::Column::Position)
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.view_id).collect())
}

fn to_view(m: &saved_view_group::Model, view_ids: Vec<Uuid>) -> ViewGroupView {
    ViewGroupView {
        id: m.id.to_string(),
        scope: m.scope.clone(),
        name: m.name.clone(),
        position: m.position,
        collapsed: m.collapsed,
        view_ids: view_ids.iter().map(Uuid::to_string).collect(),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// The set of `view_ids` that the caller may organize: their own saved
/// views plus the shared system views. Returns the subset of `candidates`
/// that resolve, so the caller can 422 on anything missing.
async fn visible_view_ids<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    candidates: &[Uuid],
) -> Result<HashSet<Uuid>, sea_orm::DbErr> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    let rows = saved_view::Entity::find()
        .filter(saved_view::Column::Id.is_in(candidates.iter().copied()))
        .filter(
            sea_orm::Condition::any()
                .add(saved_view::Column::UserId.is_null())
                .add(saved_view::Column::UserId.eq(user_id)),
        )
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Reject duplicate ids and ids the caller can't see. Shared by create
/// and the layout replace (where `ids` spans every group plus the
/// ungrouped list, so a view can't be placed twice).
async fn validate_member_ids<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    ids: &[Uuid],
) -> Result<(), axum::response::Response> {
    let unique: HashSet<Uuid> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "a view may appear in only one place",
        ));
    }
    let visible = visible_view_ids(db, user_id, ids).await.map_err(|e| {
        tracing::error!(error = %e, "view_groups: view lookup failed");
        internal()
    })?;
    if visible.len() != unique.len() {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "one or more view ids are unknown",
        ));
    }
    Ok(())
}

/// Insert placement rows for `group_id` (or the ungrouped section of
/// `scope` when `None`), displacing any existing row for the same
/// `(user, view)` — a view lives in one place.
async fn write_members<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    scope: &str,
    group_id: Option<Uuid>,
    view_ids: &[Uuid],
) -> Result<(), sea_orm::DbErr> {
    if view_ids.is_empty() {
        return Ok(());
    }
    saved_view_group_item::Entity::delete_many()
        .filter(saved_view_group_item::Column::UserId.eq(user_id))
        .filter(saved_view_group_item::Column::ViewId.is_in(view_ids.iter().copied()))
        .exec(db)
        .await?;
    let rows: Vec<saved_view_group_item::ActiveModel> = view_ids
        .iter()
        .enumerate()
        .map(|(i, v)| saved_view_group_item::ActiveModel {
            user_id: Set(user_id),
            view_id: Set(*v),
            scope: Set(scope.to_owned()),
            group_id: Set(group_id),
            position: Set(i as i32),
        })
        .collect();
    saved_view_group_item::Entity::insert_many(rows)
        .exec(db)
        .await?;
    Ok(())
}

// ───── handlers ─────

#[utoipa::path(
    operation_id = "view_groups_list",    get,
    path = "/me/view-groups",
    params(("scope" = String, Query,)),
    responses((status = 200, body = ViewGroupListView))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if !SCOPES.contains(&q.scope.as_str()) {
        return invalid_scope_response();
    }
    let rows = match saved_view_group::Entity::find()
        .filter(saved_view_group::Column::UserId.eq(user.id))
        .filter(saved_view_group::Column::Scope.eq(q.scope.as_str()))
        .order_by_asc(saved_view_group::Column::Position)
        .order_by_asc(saved_view_group::Column::CreatedAt)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "view_groups: list failed");
            return internal();
        }
    };
    let ids: Vec<Uuid> = rows.iter().map(|g| g.id).collect();
    let mut members = match members_by_group(&app.db, user.id, &ids).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "view_groups: members failed");
            return internal();
        }
    };
    let groups = rows
        .iter()
        .map(|g| to_view(g, members.remove(&g.id).unwrap_or_default()))
        .collect();
    let ungrouped_ids = match ungrouped_order(&app.db, user.id, &q.scope).await {
        Ok(ids) => ids.iter().map(Uuid::to_string).collect(),
        Err(e) => {
            tracing::error!(error = %e, "view_groups: ungrouped order failed");
            return internal();
        }
    };
    Json(ViewGroupListView {
        scope: q.scope,
        groups,
        ungrouped_ids,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "view_groups_create",    post,
    path = "/me/view-groups",
    request_body = CreateViewGroupReq,
    responses((status = 201, body = ViewGroupView))
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Validated(req): Validated<CreateViewGroupReq>,
) -> impl IntoResponse {
    let name = req.name.trim();
    let count = saved_view_group::Entity::find()
        .filter(saved_view_group::Column::UserId.eq(user.id))
        .filter(saved_view_group::Column::Scope.eq(req.scope.as_str()))
        .count(&app.db)
        .await
        .unwrap_or(0);
    if count >= MAX_GROUPS_PER_SCOPE {
        return error(
            StatusCode::CONFLICT,
            "group_cap_reached",
            "delete a group to add another",
        );
    }
    if let Err(resp) = validate_member_ids(&app.db, user.id, &req.view_ids).await {
        return resp;
    }
    let max_pos = saved_view_group::Entity::find()
        .filter(saved_view_group::Column::UserId.eq(user.id))
        .filter(saved_view_group::Column::Scope.eq(req.scope.as_str()))
        .order_by_desc(saved_view_group::Column::Position)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|g| g.position + 1)
        .unwrap_or(0);
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return internal(),
    };
    let row = match (saved_view_group::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user.id),
        scope: Set(req.scope.clone()),
        name: Set(name.to_owned()),
        position: Set(max_pos),
        collapsed: Set(false),
        created_at: NotSet,
        updated_at: NotSet,
    })
    .insert(&txn)
    .await
    {
        Ok(m) => m,
        Err(e) => {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "view_groups: insert failed");
            return internal();
        }
    };
    if let Err(e) = write_members(&txn, user.id, &req.scope, Some(row.id), &req.view_ids).await {
        let _ = txn.rollback().await;
        tracing::error!(error = %e, "view_groups: member insert failed");
        return internal();
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "view_groups: create commit failed");
        return internal();
    }
    (StatusCode::CREATED, Json(to_view(&row, req.view_ids))).into_response()
}

#[utoipa::path(
    operation_id = "view_groups_update",    patch,
    path = "/me/view-groups/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateViewGroupReq,
    responses((status = 200, body = ViewGroupView))
)]
#[handler]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Validated(req): Validated<UpdateViewGroupReq>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let mut am: saved_view_group::ActiveModel = row.clone().into();
    let mut changed = false;
    if let Some(name) = req.name.as_deref().map(str::trim)
        && name != row.name
    {
        am.name = Set(name.to_owned());
        changed = true;
    }
    if let Some(collapsed) = req.collapsed
        && collapsed != row.collapsed
    {
        am.collapsed = Set(collapsed);
        changed = true;
    }
    let updated = if changed {
        am.updated_at = Set(Utc::now().fixed_offset());
        match am.update(&app.db).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "view_groups: update failed");
                return internal();
            }
        }
    } else {
        row
    };
    let members = match members_by_group(&app.db, user.id, &[updated.id]).await {
        Ok(mut m) => m.remove(&updated.id).unwrap_or_default(),
        Err(e) => {
            tracing::error!(error = %e, "view_groups: members failed");
            return internal();
        }
    };
    Json(to_view(&updated, members)).into_response()
}

#[utoipa::path(
    operation_id = "view_groups_delete_one",    delete,
    path = "/me/view-groups/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
#[handler]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Membership rows cascade with the group → its views are ungrouped.
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "view_groups: delete failed");
        return internal();
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "view_groups_replace_layout",    put,
    path = "/me/view-groups/layout",
    request_body = ReplaceLayoutReq,
    responses(
        (status = 204),
        (status = 422, description = "unknown scope, group set mismatch, or a view id that is duplicate, unknown, or placed twice"),
    )
)]
#[handler]
pub async fn replace_layout(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ReplaceLayoutReq>,
) -> impl IntoResponse {
    if !SCOPES.contains(&req.scope.as_str()) {
        return invalid_scope_response();
    }
    let owned = match saved_view_group::Entity::find()
        .filter(saved_view_group::Column::UserId.eq(user.id))
        .filter(saved_view_group::Column::Scope.eq(req.scope.as_str()))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "view_groups: layout lookup failed");
            return internal();
        }
    };
    // Semantic-validation 422s: the request must name every group in the
    // scope exactly once (so a stale client can't silently drop one) and
    // every view at most once across all groups.
    let owned_ids: HashSet<Uuid> = owned.iter().map(|g| g.id).collect();
    let req_ids: HashSet<Uuid> = req.groups.iter().map(|g| g.id).collect();
    if req_ids.len() != req.groups.len() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "duplicate group id",
        );
    }
    if req_ids != owned_ids {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "groups must list every group in this scope exactly once",
        );
    }
    let all_views: Vec<Uuid> = req
        .groups
        .iter()
        .flat_map(|g| g.view_ids.iter().copied())
        .chain(req.ungrouped.iter().copied())
        .collect();
    if let Err(resp) = validate_member_ids(&app.db, user.id, &all_views).await {
        return resp;
    }

    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return internal(),
    };
    // Placement: whole replace for this scope — group members and the
    // pinned ungrouped order alike. Anything the payload doesn't list is
    // ungrouped with no pinned position.
    if let Err(e) = saved_view_group_item::Entity::delete_many()
        .filter(saved_view_group_item::Column::UserId.eq(user.id))
        .filter(saved_view_group_item::Column::Scope.eq(req.scope.as_str()))
        .exec(&txn)
        .await
    {
        let _ = txn.rollback().await;
        tracing::error!(error = %e, "view_groups: layout clear failed");
        return internal();
    }
    let now = Utc::now().fixed_offset();
    for (i, g) in req.groups.iter().enumerate() {
        let am = saved_view_group::ActiveModel {
            id: Unchanged(g.id),
            position: Set(i as i32),
            updated_at: Set(now),
            ..Default::default()
        };
        if let Err(e) = am.update(&txn).await {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "view_groups: layout reorder failed");
            return internal();
        }
        if let Err(e) = write_members(&txn, user.id, &req.scope, Some(g.id), &g.view_ids).await {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "view_groups: layout members failed");
            return internal();
        }
    }
    if let Err(e) = write_members(&txn, user.id, &req.scope, None, &req.ungrouped).await {
        let _ = txn.rollback().await;
        tracing::error!(error = %e, "view_groups: layout ungrouped failed");
        return internal();
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "view_groups: layout commit failed");
        return internal();
    }
    StatusCode::NO_CONTENT.into_response()
}
