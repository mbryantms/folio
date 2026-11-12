//! Views-index groups — per-user named buckets ("folders") for organizing
//! saved views on Settings → Views and the reading lists page.
//!
//! A heavy CBL user ends up with dozens of reading lists in one flat
//! grid. Groups let them drag those lists into categories ("Marvel
//! events", "Finished", …) that are renamed, reordered, and collapsed
//! independently of the lists themselves. Groups are scoped to one index
//! tab (`scope` ∈ `filter` / `cbl` / `collection`) and are purely an
//! organizational overlay: membership never changes what a view *is*,
//! where it's pinned, or whether it shows in the sidebar.
//!
//! Groups nest: `parent_id` names the enclosing group (same user, same
//! scope — the composite foreign key refuses anything else), at most
//! [`MAX_GROUP_DEPTH`] levels deep, never a cycle. `position` is the
//! ordinal among siblings.
//!
//! Surface:
//!
//!   - `GET    /me/view-groups?scope=`      — groups (with `parent_id`) +
//!     ordered member ids, plus the hand-arranged order of the ungrouped
//!     remainder
//!   - `POST   /me/view-groups`             — create (appends to the end
//!     of its siblings; optional `parent_id`)
//!   - `PATCH  /me/view-groups/{id}`        — rename / collapse
//!   - `DELETE /me/view-groups/{id}`        — delete; its contents move up
//!     one level (lists to the parent group or the ungrouped section,
//!     child groups to the parent in the deleted group's place)
//!   - `PUT    /me/view-groups/layout`      — wholesale tree + order +
//!     membership replace for one scope. This is what drag-and-drop and
//!     every "move" commits to: one request carries each group's parent,
//!     the sibling order, each group's member list, and the ungrouped
//!     order, so a move never leaves a half-applied state.
//!
//! **Tree invariants are race-safe.** Every write that changes the tree
//! (create, delete, layout) first takes a transaction-scoped advisory
//! lock keyed on `(user, scope)` ([`lock_scope`]) and only then reads the
//! groups it validates against, so two concurrent requests can never
//! combine into a cycle or a fifth level. An advisory lock rather than
//! `SELECT … FOR UPDATE`: a create has no existing row to lock when the
//! scope is empty, and row locks taken in payload order by two layout
//! writes deadlock each other.
//!
//! Views not listed by any group are "ungrouped". Their order inside that
//! section is persisted too — as membership rows with a NULL `group_id` —
//! so a user who arranges the remainder by hand finds it where they left
//! it; views without any row trail the arranged ones. Membership rows for
//! a view that stops being visible to the caller (deleted view) cascade
//! away at the DB level. The layout write ignores view ids it cannot
//! find or the caller cannot see (a stale client may still name a view
//! deleted elsewhere); they are dropped from the write, and an unknown id
//! and another user's id get the same answer.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{saved_view, saved_view_group, saved_view_group_item};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::{NotSet, Set, Unchanged},
    ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder,
    Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use shared::error::{ApiErrorCode, FieldError};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::{error, respond_with_field_errors};
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

/// Soft cap on groups per `(user, scope)`, nested ones included. Enforced
/// in the API; not a DB constraint.
const MAX_GROUPS_PER_SCOPE: u64 = 50;
const MAX_NAME_LEN: usize = 80;
/// Deepest nesting: a top-level group is level 1, so a group can hold
/// groups three levels down.
pub const MAX_GROUP_DEPTH: usize = 4;

pub const SCOPE_FILTER: &str = "filter";
pub const SCOPE_CBL: &str = "cbl";
pub const SCOPE_COLLECTION: &str = "collection";
const SCOPES: &[&str] = &[SCOPE_FILTER, SCOPE_CBL, SCOPE_COLLECTION];

const TOO_DEEP: &str = "groups nest at most 4 levels deep";
const PARENT_NOT_FOUND: &str = "parent group not found";
const PARENT_CYCLE: &str = "a group cannot be inside itself or its own subgroups";

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
    /// The enclosing group, `null` for a top-level group.
    pub parent_id: Option<String>,
    /// Ordinal among the group's siblings (same scope and parent; dense
    /// 0..N after any layout write).
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
    /// Optional enclosing group (same scope, at most
    /// `MAX_GROUP_DEPTH` levels in all). Absent or null = top level.
    #[serde(default)]
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
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

/// Most view ids one list in a layout write may carry (a group's members
/// or the ungrouped list). Views per user are not capped, so this is a
/// safety bound far above any real board.
pub const MAX_LAYOUT_VIEW_IDS: usize = 10_000;
/// Most groups a layout write may list: the per-scope group cap.
const MAX_LAYOUT_GROUPS: usize = MAX_GROUPS_PER_SCOPE as usize;

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct LayoutGroupReq {
    #[garde(skip)]
    pub id: Uuid,
    /// The enclosing group, which must itself be listed in the same
    /// payload. Absent or null = top level. Sibling order is the order
    /// in which the groups of one parent appear in `groups`.
    #[serde(default)]
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
    /// Member view ids in display order. A view listed under no group is
    /// ungrouped after the write. Ids the caller cannot see (deleted, or
    /// not theirs) are ignored.
    #[garde(length(max = MAX_LAYOUT_VIEW_IDS))]
    pub view_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct ReplaceLayoutReq {
    // Checked in the handler (its own 422 message).
    #[garde(skip)]
    pub scope: String,
    /// Every group in the scope, exactly once. Each group's siblings
    /// (same `parent_id`) are ordered as they appear here; the tree must
    /// be acyclic and at most `MAX_GROUP_DEPTH` levels deep.
    // A scope holds at most `MAX_GROUPS_PER_SCOPE` groups, so a longer
    // list can never be valid.
    #[garde(length(max = MAX_LAYOUT_GROUPS), dive)]
    pub groups: Vec<LayoutGroupReq>,
    /// Ungrouped views in display order. Optional and may be partial:
    /// ungrouped views not listed simply have no pinned position.
    #[serde(default)]
    #[garde(length(max = MAX_LAYOUT_VIEW_IDS))]
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

// ───── tree rules (pure) ─────

/// Why a parent map is not a valid group tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeError {
    /// `.0` names a parent that is not in the map.
    MissingParent(Uuid),
    /// `.0` is its own ancestor.
    Cycle(Uuid),
    /// `.0` sits deeper than [`MAX_GROUP_DEPTH`].
    TooDeep(Uuid),
}

/// Depth of every group (1 = top level) in a `group → parent` map, or
/// the first violation found. Groups are checked in `order` so the error
/// names a deterministic group.
pub(crate) fn tree_depths(
    parents: &HashMap<Uuid, Option<Uuid>>,
    order: &[Uuid],
) -> Result<HashMap<Uuid, usize>, TreeError> {
    let mut depths: HashMap<Uuid, usize> = HashMap::with_capacity(parents.len());
    for &start in order {
        let mut depth = 1;
        let mut cur = start;
        loop {
            let Some(&parent) = parents.get(&cur) else {
                return Err(TreeError::MissingParent(cur));
            };
            let Some(p) = parent else { break };
            if !parents.contains_key(&p) {
                return Err(TreeError::MissingParent(cur));
            }
            if p == start || depth > parents.len() {
                return Err(TreeError::Cycle(start));
            }
            depth += 1;
            cur = p;
        }
        if depth > MAX_GROUP_DEPTH {
            return Err(TreeError::TooDeep(start));
        }
        depths.insert(start, depth);
    }
    Ok(depths)
}

// ───── helpers ─────

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn db_fail(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> Response {
    move |e| {
        tracing::error!(error = %e, "view_groups: {what} failed");
        internal()
    }
}

fn invalid_scope_response() -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation",
        "scope must be filter, cbl, or collection",
    )
}

/// A 422 bound to one request field, so a form can show it in place.
fn field_error(field: String, message: &str) -> Response {
    respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        message,
        vec![FieldError {
            field,
            message: message.to_owned(),
        }],
    )
}

/// Serialise every tree write of one `(user, scope)`: a transaction-
/// scoped advisory lock, released at commit / rollback. Must be the
/// first statement of the transaction that then reads and writes the
/// groups. Keys of different scopes or users may collide in the hash;
/// that only serialises two unrelated writes, never admits a bad tree.
async fn lock_scope(txn: &DatabaseTransaction, user_id: Uuid, scope: &str) -> Result<(), Response> {
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [format!("saved_view_groups:{user_id}:{scope}").into()],
    ))
    .await
    .map(|_| ())
    .map_err(db_fail("scope lock"))
}

/// Every group of `(user, scope)`, in sibling order.
async fn groups_in_scope<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    scope: &str,
) -> Result<Vec<saved_view_group::Model>, sea_orm::DbErr> {
    saved_view_group::Entity::find()
        .filter(saved_view_group::Column::UserId.eq(user_id))
        .filter(saved_view_group::Column::Scope.eq(scope))
        .order_by_asc(saved_view_group::Column::Position)
        .order_by_asc(saved_view_group::Column::CreatedAt)
        .all(db)
        .await
}

async fn fetch_owned<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    id: Uuid,
) -> Result<saved_view_group::Model, Response> {
    match saved_view_group::Entity::find_by_id(id).one(db).await {
        Ok(Some(g)) if g.user_id == user_id => Ok(g),
        // Existence-leak guard: other-user rows look identical to missing.
        Ok(_) => Err(error(StatusCode::NOT_FOUND, "not_found", "group not found")),
        Err(e) => Err(db_fail("fetch")(e)),
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
        parent_id: m.parent_id.map(|p| p.to_string()),
        position: m.position,
        collapsed: m.collapsed,
        view_ids: view_ids.iter().map(Uuid::to_string).collect(),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// The set of `view_ids` that the caller may organize: their own saved
/// views plus the shared system views. Returns the subset of `candidates`
/// that resolve.
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

fn duplicate_views() -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation",
        "a view may appear in only one place",
    )
}

/// Reject duplicate ids and ids the caller can't see (create: the caller
/// names the members of a new group, so an unknown one is a mistake).
async fn validate_member_ids<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    ids: &[Uuid],
) -> Result<(), Response> {
    let unique: HashSet<Uuid> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        return Err(duplicate_views());
    }
    let visible = visible_view_ids(db, user_id, ids)
        .await
        .map_err(db_fail("view lookup"))?;
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
    let rows = match groups_in_scope(&app.db, user.id, &q.scope).await {
        Ok(r) => r,
        Err(e) => return db_fail("list")(e),
    };
    let ids: Vec<Uuid> = rows.iter().map(|g| g.id).collect();
    let mut members = match members_by_group(&app.db, user.id, &ids).await {
        Ok(m) => m,
        Err(e) => return db_fail("members")(e),
    };
    let groups = rows
        .iter()
        .map(|g| to_view(g, members.remove(&g.id).unwrap_or_default()))
        .collect();
    let ungrouped_ids = match ungrouped_order(&app.db, user.id, &q.scope).await {
        Ok(ids) => ids.iter().map(Uuid::to_string).collect(),
        Err(e) => return db_fail("ungrouped order")(e),
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
    responses(
        (status = 201, body = ViewGroupView),
        (status = 422, description = "invalid name or scope, an unknown or duplicate view id, or a parent that is missing, in another scope, or already at the deepest level"),
    )
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Validated(req): Validated<CreateViewGroupReq>,
) -> impl IntoResponse {
    match create_inner(&app, user.id, req).await {
        Ok(r) | Err(r) => r,
    }
}

async fn create_inner(
    app: &AppState,
    user_id: Uuid,
    req: CreateViewGroupReq,
) -> Result<Response, Response> {
    let name = req.name.trim();
    validate_member_ids(&app.db, user_id, &req.view_ids).await?;
    let txn = app.db.begin().await.map_err(db_fail("begin"))?;
    lock_scope(&txn, user_id, &req.scope).await?;
    let existing = groups_in_scope(&txn, user_id, &req.scope)
        .await
        .map_err(db_fail("scope groups"))?;
    if existing.len() as u64 >= MAX_GROUPS_PER_SCOPE {
        return Err(error(
            StatusCode::CONFLICT,
            "group_cap_reached",
            "delete a group to add another",
        ));
    }
    if let Some(parent) = req.parent_id {
        // Another user's group and another scope's group look exactly
        // like a missing one.
        if !existing.iter().any(|g| g.id == parent) {
            return Err(field_error("parent_id".into(), PARENT_NOT_FOUND));
        }
        let parents: HashMap<Uuid, Option<Uuid>> =
            existing.iter().map(|g| (g.id, g.parent_id)).collect();
        let depth = tree_depths(&parents, &[parent])
            .map_err(|_| internal())?
            .get(&parent)
            .copied()
            .unwrap_or(MAX_GROUP_DEPTH);
        if depth + 1 > MAX_GROUP_DEPTH {
            return Err(field_error("parent_id".into(), TOO_DEEP));
        }
    }
    let position = existing
        .iter()
        .filter(|g| g.parent_id == req.parent_id)
        .map(|g| g.position + 1)
        .max()
        .unwrap_or(0);
    let row = saved_view_group::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user_id),
        scope: Set(req.scope.clone()),
        name: Set(name.to_owned()),
        parent_id: Set(req.parent_id),
        position: Set(position),
        collapsed: Set(false),
        created_at: NotSet,
        updated_at: NotSet,
    }
    .insert(&txn)
    .await
    .map_err(db_fail("insert"))?;
    write_members(&txn, user_id, &req.scope, Some(row.id), &req.view_ids)
        .await
        .map_err(db_fail("member insert"))?;
    txn.commit().await.map_err(db_fail("create commit"))?;
    Ok((StatusCode::CREATED, Json(to_view(&row, req.view_ids))).into_response())
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
            Err(e) => return db_fail("update")(e),
        }
    } else {
        row
    };
    let members = match members_by_group(&app.db, user.id, &[updated.id]).await {
        Ok(mut m) => m.remove(&updated.id).unwrap_or_default(),
        Err(e) => return db_fail("members")(e),
    };
    Json(to_view(&updated, members)).into_response()
}

#[utoipa::path(
    operation_id = "view_groups_delete_one",    delete,
    path = "/me/view-groups/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 204, description = "deleted; its lists moved to its parent group (or the ungrouped section when it was top-level) after the parent's own, and its child groups took its place among its siblings"),
    )
)]
#[handler]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    match delete_inner(&app, user.id, id).await {
        Ok(r) | Err(r) => r,
    }
}

async fn delete_inner(app: &AppState, user_id: Uuid, id: Uuid) -> Result<Response, Response> {
    // Scope first (outside the lock) to know which lock to take; the row
    // is read again under it.
    let scope = fetch_owned(&app.db, user_id, id).await?.scope;
    let txn = app.db.begin().await.map_err(db_fail("begin"))?;
    lock_scope(&txn, user_id, &scope).await?;
    let row = fetch_owned(&txn, user_id, id).await?;
    let groups = groups_in_scope(&txn, user_id, &scope)
        .await
        .map_err(db_fail("scope groups"))?;
    let parent = row.parent_id;
    let now = Utc::now().fixed_offset();

    // Child groups take the deleted group's place among its siblings, in
    // their own order.
    let children: Vec<Uuid> = groups
        .iter()
        .filter(|g| g.parent_id == Some(row.id))
        .map(|g| g.id)
        .collect();
    let siblings: Vec<Uuid> = groups
        .iter()
        .filter(|g| g.parent_id == parent)
        .flat_map(|g| {
            if g.id == row.id {
                children.clone()
            } else {
                vec![g.id]
            }
        })
        .collect();
    for (i, gid) in siblings.iter().enumerate() {
        saved_view_group::ActiveModel {
            id: Unchanged(*gid),
            parent_id: Set(parent),
            position: Set(i as i32),
            updated_at: Set(now),
            ..Default::default()
        }
        .update(&txn)
        .await
        .map_err(db_fail("reparent"))?;
    }

    // Lists go to the parent group (or the ungrouped section), after the
    // ones already there, in their own order.
    txn.execute_raw(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "UPDATE saved_view_group_items i \
         SET group_id = $3, \
             position = base.next + moved.rn - 1 \
         FROM ( \
             SELECT view_id, row_number() OVER (ORDER BY position, view_id) AS rn \
             FROM saved_view_group_items WHERE user_id = $1 AND group_id = $2 \
         ) moved, ( \
             SELECT COALESCE(MAX(position) + 1, 0) AS next \
             FROM saved_view_group_items \
             WHERE user_id = $1 AND scope = $4 AND group_id IS NOT DISTINCT FROM $3 \
         ) base \
         WHERE i.user_id = $1 AND i.view_id = moved.view_id",
        [
            user_id.into(),
            row.id.into(),
            parent.into(),
            scope.clone().into(),
        ],
    ))
    .await
    .map_err(db_fail("move members"))?;

    // The foreign key is NO ACTION: had a child group been left under
    // this row, the delete would fail here rather than cascade.
    saved_view_group::Entity::delete_by_id(row.id)
        .exec(&txn)
        .await
        .map_err(db_fail("delete"))?;
    txn.commit().await.map_err(db_fail("delete commit"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    operation_id = "view_groups_replace_layout",    put,
    path = "/me/view-groups/layout",
    request_body = ReplaceLayoutReq,
    responses(
        (status = 204),
        (status = 422, description = "unknown scope, group set mismatch, a parent that is not listed, a cycle, a tree deeper than four levels, or a view id placed twice"),
    )
)]
#[handler]
pub async fn replace_layout(
    State(app): State<AppState>,
    user: CurrentUser,
    Validated(req): Validated<ReplaceLayoutReq>,
) -> impl IntoResponse {
    if !SCOPES.contains(&req.scope.as_str()) {
        return invalid_scope_response();
    }
    match replace_layout_inner(&app, user.id, req).await {
        Ok(r) | Err(r) => r,
    }
}

/// Check the tree a layout payload describes: every parent is listed in
/// the payload, no cycles, at most [`MAX_GROUP_DEPTH`] levels.
fn validate_layout_tree(groups: &[LayoutGroupReq]) -> Result<(), Response> {
    let parents: HashMap<Uuid, Option<Uuid>> = groups.iter().map(|g| (g.id, g.parent_id)).collect();
    let order: Vec<Uuid> = groups.iter().map(|g| g.id).collect();
    let index_of = |id: Uuid| order.iter().position(|g| *g == id).unwrap_or(0);
    let field = |id: Uuid| format!("groups[{}].parent_id", index_of(id));
    match tree_depths(&parents, &order) {
        Ok(_) => Ok(()),
        Err(TreeError::MissingParent(id)) => Err(field_error(field(id), PARENT_NOT_FOUND)),
        Err(TreeError::Cycle(id)) => Err(field_error(field(id), PARENT_CYCLE)),
        Err(TreeError::TooDeep(id)) => Err(field_error(field(id), TOO_DEEP)),
    }
}

async fn replace_layout_inner(
    app: &AppState,
    user_id: Uuid,
    req: ReplaceLayoutReq,
) -> Result<Response, Response> {
    // Shape checks that need no database.
    let req_ids: HashSet<Uuid> = req.groups.iter().map(|g| g.id).collect();
    if req_ids.len() != req.groups.len() {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "duplicate group id",
        ));
    }
    let all_views: Vec<Uuid> = req
        .groups
        .iter()
        .flat_map(|g| g.view_ids.iter().copied())
        .chain(req.ungrouped.iter().copied())
        .collect();
    if all_views.iter().collect::<HashSet<_>>().len() != all_views.len() {
        return Err(duplicate_views());
    }
    validate_layout_tree(&req.groups)?;

    let txn = app.db.begin().await.map_err(db_fail("begin"))?;
    lock_scope(&txn, user_id, &req.scope).await?;
    let owned = groups_in_scope(&txn, user_id, &req.scope)
        .await
        .map_err(db_fail("layout lookup"))?;
    // The request must name every group in the scope exactly once (so a
    // stale client can't silently drop one). Read under the lock, so the
    // set can't change before the write.
    let owned_ids: HashSet<Uuid> = owned.iter().map(|g| g.id).collect();
    if req_ids != owned_ids {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "groups must list every group in this scope exactly once",
        ));
    }
    // View ids the caller cannot see — deleted elsewhere, or someone
    // else's — are dropped from the write, never added, and answered
    // exactly like a visible id: nothing tells the two apart.
    let visible = visible_view_ids(&txn, user_id, &all_views)
        .await
        .map_err(db_fail("view lookup"))?;
    let keep = |ids: &[Uuid]| -> Vec<Uuid> {
        ids.iter()
            .copied()
            .filter(|v| visible.contains(v))
            .collect()
    };

    // Placement: whole replace for this scope — group members and the
    // pinned ungrouped order alike. Anything the payload doesn't list is
    // ungrouped with no pinned position.
    saved_view_group_item::Entity::delete_many()
        .filter(saved_view_group_item::Column::UserId.eq(user_id))
        .filter(saved_view_group_item::Column::Scope.eq(req.scope.as_str()))
        .exec(&txn)
        .await
        .map_err(db_fail("layout clear"))?;
    let now = Utc::now().fixed_offset();
    let mut next_position: HashMap<Option<Uuid>, i32> = HashMap::new();
    for g in &req.groups {
        let slot = next_position.entry(g.parent_id).or_insert(0);
        let position = *slot;
        *slot += 1;
        saved_view_group::ActiveModel {
            id: Unchanged(g.id),
            parent_id: Set(g.parent_id),
            position: Set(position),
            updated_at: Set(now),
            ..Default::default()
        }
        .update(&txn)
        .await
        .map_err(db_fail("layout reorder"))?;
        write_members(&txn, user_id, &req.scope, Some(g.id), &keep(&g.view_ids))
            .await
            .map_err(db_fail("layout members"))?;
    }
    write_members(&txn, user_id, &req.scope, None, &keep(&req.ungrouped))
        .await
        .map_err(db_fail("layout ungrouped"))?;
    txn.commit().await.map_err(db_fail("layout commit"))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::now_v7()).collect()
    }

    #[test]
    fn depths_of_a_chain() {
        let g = ids(4);
        let parents: HashMap<_, _> = [
            (g[0], None),
            (g[1], Some(g[0])),
            (g[2], Some(g[1])),
            (g[3], Some(g[2])),
        ]
        .into();
        let d = tree_depths(&parents, &g).unwrap();
        assert_eq!(d[&g[0]], 1);
        assert_eq!(d[&g[3]], MAX_GROUP_DEPTH);
    }

    #[test]
    fn a_fifth_level_is_too_deep() {
        let g = ids(5);
        let mut parents: HashMap<_, _> = [(g[0], None)].into();
        for i in 1..5 {
            parents.insert(g[i], Some(g[i - 1]));
        }
        assert_eq!(tree_depths(&parents, &g), Err(TreeError::TooDeep(g[4])));
    }

    #[test]
    fn cycles_and_self_parents_are_refused() {
        let g = ids(3);
        let parents: HashMap<_, _> =
            [(g[0], Some(g[2])), (g[1], Some(g[0])), (g[2], Some(g[1]))].into();
        assert_eq!(tree_depths(&parents, &g), Err(TreeError::Cycle(g[0])));
        let selfish: HashMap<_, _> = [(g[0], Some(g[0]))].into();
        assert_eq!(tree_depths(&selfish, &g[..1]), Err(TreeError::Cycle(g[0])));
        // A cycle below a valid root is still found.
        let below: HashMap<_, _> = [(g[0], None), (g[1], Some(g[2])), (g[2], Some(g[1]))].into();
        assert!(matches!(tree_depths(&below, &g), Err(TreeError::Cycle(_))));
    }

    #[test]
    fn a_parent_outside_the_map_is_missing() {
        let g = ids(2);
        let parents: HashMap<_, _> = [(g[0], Some(g[1]))].into();
        assert_eq!(
            tree_depths(&parents, &g[..1]),
            Err(TreeError::MissingParent(g[0]))
        );
    }
}
