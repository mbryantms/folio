//! `/admin/users/*` — admin user management (M3).
//!
//! Endpoints:
//!   - `GET    /admin/users`                       paginated list (filter: role, state, q)
//!   - `GET    /admin/users/{id}`                  single user (with library access)
//!   - `PATCH  /admin/users/{id}`                  update display_name / role / state
//!   - `POST   /admin/users/{id}/disable`          state -> disabled, bump token_version
//!   - `POST   /admin/users/{id}/enable`           state -> active
//!   - `POST   /admin/users/{id}/library-access`   replace library_user_access rows
//!
//! All endpoints require `role == "admin"`. Mutating endpoints emit an
//! audit_log row keyed on the actor and the affected user.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use entity::{library, library_user_access, user};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/users", get(list))
        .route("/admin/users/{id}", get(get_one).patch(update))
        .route("/admin/users/{id}/disable", post(disable))
        .route("/admin/users/{id}/enable", post(enable))
        .route("/admin/users/{id}/library-access", post(set_library_access))
}

// ───────── views ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminUserView {
    pub id: String,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub state: String,
    pub email_verified: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
    /// Count of `library_user_access` rows for this user. For admins this is
    /// always reported as 0 since admins implicitly see every library.
    pub library_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminUserListView {
    pub items: Vec<AdminUserView>,
    /// Opaque cursor for the next page. `None` when the result is exhausted.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryAccessGrantView {
    pub library_id: String,
    pub library_name: String,
    pub role: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminUserDetailView {
    #[serde(flatten)]
    pub user: AdminUserView,
    pub library_access: Vec<LibraryAccessGrantView>,
}

// ───────── request bodies ─────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListUsersQuery {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub role: Option<String>,
    pub state: Option<String>,
    pub q: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateUserReq {
    #[serde(default)]
    pub display_name: Option<String>,
    /// `admin` | `user`
    #[serde(default)]
    pub role: Option<String>,
    /// `pending_verification` | `active` | `disabled`
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LibraryAccessReq {
    /// Final set of library ids the user should be granted access to. The
    /// server replaces the user's `library_user_access` rows with this list.
    pub library_ids: Vec<String>,
}

// ───────── handlers ─────────

#[utoipa::path(
    get,
    path = "/admin/users",
    params(ListUsersQuery),
    responses(
        (status = 200, body = AdminUserListView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListUsersQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let mut query = user::Entity::find().order_by_asc(user::Column::Id);

    if let Some(cursor) = q.cursor.as_deref() {
        let Ok(after) = parse_cursor(cursor) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
        };
        query = query.filter(user::Column::Id.gt(after));
    }
    if let Some(role) = q.role.as_deref() {
        if role != "admin" && role != "user" {
            return error(
                StatusCode::BAD_REQUEST,
                "validation.role",
                "role must be admin or user",
            );
        }
        query = query.filter(user::Column::Role.eq(role));
    }
    if let Some(state) = q.state.as_deref() {
        if !matches!(state, "pending_verification" | "active" | "disabled") {
            return error(StatusCode::BAD_REQUEST, "validation.state", "invalid state");
        }
        query = query.filter(user::Column::State.eq(state));
    }
    if let Some(needle) = q.q.as_deref()
        && !needle.trim().is_empty()
    {
        let pattern = format!("%{}%", needle.trim().to_lowercase());
        query = query.filter(
            user::Column::Email
                .like(pattern.clone())
                .or(user::Column::DisplayName.like(pattern)),
        );
    }

    // Fetch limit+1 so we know whether another page exists.
    let rows: Vec<user::Model> = match query.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list users failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get(limit as usize - 1).map(|r| encode_cursor(r.id))
    } else {
        None
    };
    let page: Vec<user::Model> = rows.into_iter().take(limit as usize).collect();

    let counts = match library_counts_for(&app, page.iter().map(|u| u.id).collect()).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "library counts failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let items: Vec<AdminUserView> = page
        .into_iter()
        .map(|m| AdminUserView::from_model(m, &counts))
        .collect();

    Json(AdminUserListView { items, next_cursor }).into_response()
}

#[utoipa::path(
    get,
    path = "/admin/users/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserDetailView),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn get_one(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return error(StatusCode::BAD_REQUEST, "validation", "invalid id");
    };
    let Ok(Some(target)) = user::Entity::find_by_id(uuid).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "user not found");
    };

    let counts = match library_counts_for(&app, vec![uuid]).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "library count failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let access = match library_access_grants(&app, uuid).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!(error = %e, "library access fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    Json(AdminUserDetailView {
        user: AdminUserView::from_model(target, &counts),
        library_access: access,
    })
    .into_response()
}

#[utoipa::path(
    patch,
    path = "/admin/users/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateUserReq,
    responses(
        (status = 200, body = AdminUserDetailView),
        (status = 400, description = "validation"),
        (status = 403, description = "admin only / cannot demote self"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Json(req): Json<UpdateUserReq>,
) -> impl IntoResponse {
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return error(StatusCode::BAD_REQUEST, "validation", "invalid id");
    };
    let Ok(Some(target)) = user::Entity::find_by_id(uuid).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "user not found");
    };

    if let Some(role) = req.role.as_deref()
        && role != "admin"
        && role != "user"
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.role",
            "role must be admin or user",
        );
    }
    if let Some(state) = req.state.as_deref()
        && !matches!(state, "pending_verification" | "active" | "disabled")
    {
        return error(StatusCode::BAD_REQUEST, "validation.state", "invalid state");
    }
    if let Some(name) = req.display_name.as_deref()
        && name.trim().is_empty()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.display_name",
            "display_name cannot be empty",
        );
    }

    // Self-demotion guard: an admin cannot strip their own admin role or
    // disable their own account through this endpoint. Locks them out of
    // /admin entirely with no recovery path through the UI.
    if target.id == actor.id {
        if matches!(req.role.as_deref(), Some("user")) {
            return error(
                StatusCode::FORBIDDEN,
                "self_demote",
                "cannot demote yourself",
            );
        }
        if matches!(req.state.as_deref(), Some("disabled")) {
            return error(
                StatusCode::FORBIDDEN,
                "self_disable",
                "cannot disable yourself",
            );
        }
    }

    let mut changed = serde_json::Map::new();
    let mut bump_token_version = false;
    let mut am: user::ActiveModel = target.clone().into();
    if let Some(name) = req.display_name {
        let trimmed = name.trim().to_owned();
        if trimmed != target.display_name {
            changed.insert("display_name".into(), serde_json::json!(trimmed));
            am.display_name = Set(trimmed);
        }
    }
    if let Some(role) = req.role
        && role != target.role
    {
        changed.insert("role".into(), serde_json::json!(role));
        am.role = Set(role);
    }
    if let Some(state) = req.state
        && state != target.state
    {
        changed.insert("state".into(), serde_json::json!(state.clone()));
        if state == "disabled" {
            bump_token_version = true;
        }
        am.state = Set(state);
    }

    if changed.is_empty() {
        // Idempotent no-op.
        let counts = library_counts_for(&app, vec![uuid])
            .await
            .unwrap_or_default();
        let access = library_access_grants(&app, uuid).await.unwrap_or_default();
        return Json(AdminUserDetailView {
            user: AdminUserView::from_model(target, &counts),
            library_access: access,
        })
        .into_response();
    }

    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if bump_token_version {
        am.token_version = Set(target.token_version + 1);
    }

    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, user_id = %uuid, "update user failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.user.update",
            target_type: Some("user"),
            target_id: Some(uuid.to_string()),
            payload: serde_json::Value::Object(changed),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let counts = library_counts_for(&app, vec![uuid])
        .await
        .unwrap_or_default();
    let access = library_access_grants(&app, uuid).await.unwrap_or_default();
    Json(AdminUserDetailView {
        user: AdminUserView::from_model(updated, &counts),
        library_access: access,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/admin/users/{id}/disable",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserView),
        (status = 403, description = "admin only / cannot disable self"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn disable(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    set_state(app, actor, ctx, id, "disabled").await
}

#[utoipa::path(
    post,
    path = "/admin/users/{id}/enable",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserView),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn enable(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    set_state(app, actor, ctx, id, "active").await
}

async fn set_state(
    app: AppState,
    actor: crate::auth::CurrentUser,
    ctx: RequestContext,
    id: String,
    new_state: &'static str,
) -> axum::response::Response {
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return error(StatusCode::BAD_REQUEST, "validation", "invalid id");
    };
    let Ok(Some(target)) = user::Entity::find_by_id(uuid).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "user not found");
    };
    if target.id == actor.id && new_state == "disabled" {
        return error(
            StatusCode::FORBIDDEN,
            "self_disable",
            "cannot disable yourself",
        );
    }

    if target.state == new_state {
        let counts = library_counts_for(&app, vec![uuid])
            .await
            .unwrap_or_default();
        return Json(AdminUserView::from_model(target, &counts)).into_response();
    }

    let mut am: user::ActiveModel = target.clone().into();
    am.state = Set(new_state.into());
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if new_state == "disabled" {
        am.token_version = Set(target.token_version + 1);
    }
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, user_id = %uuid, new_state, "set state failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: if new_state == "disabled" {
                "admin.user.disable"
            } else {
                "admin.user.enable"
            },
            target_type: Some("user"),
            target_id: Some(uuid.to_string()),
            payload: serde_json::json!({ "state": new_state }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let counts = library_counts_for(&app, vec![uuid])
        .await
        .unwrap_or_default();
    Json(AdminUserView::from_model(updated, &counts)).into_response()
}

#[utoipa::path(
    post,
    path = "/admin/users/{id}/library-access",
    params(("id" = String, Path,)),
    request_body = LibraryAccessReq,
    responses(
        (status = 200, body = AdminUserDetailView),
        (status = 400, description = "validation"),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn set_library_access(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Json(req): Json<LibraryAccessReq>,
) -> impl IntoResponse {
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return error(StatusCode::BAD_REQUEST, "validation", "invalid id");
    };
    let Ok(Some(target)) = user::Entity::find_by_id(uuid).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "user not found");
    };

    let mut wanted: Vec<Uuid> = Vec::with_capacity(req.library_ids.len());
    for raw in &req.library_ids {
        match Uuid::parse_str(raw) {
            Ok(u) => wanted.push(u),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation.library_ids",
                    "invalid library id",
                );
            }
        }
    }
    wanted.sort();
    wanted.dedup();

    // Verify every requested library exists. Reject the whole request on any
    // unknown id so the client can't silently drop typos.
    if !wanted.is_empty() {
        let found: i64 = match library::Entity::find()
            .filter(library::Column::Id.is_in(wanted.clone()))
            .count(&app.db)
            .await
        {
            Ok(n) => n as i64,
            Err(e) => {
                tracing::error!(error = %e, "library count failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        if (found as usize) != wanted.len() {
            return error(
                StatusCode::BAD_REQUEST,
                "validation.library_ids",
                "one or more libraries not found",
            );
        }
    }

    // Replace the access set inside a transaction so the API never leaves the
    // user with a half-applied grant.
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "begin tx failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    if let Err(e) = library_user_access::Entity::delete_many()
        .filter(library_user_access::Column::UserId.eq(uuid))
        .exec(&txn)
        .await
    {
        tracing::error!(error = %e, "library_user_access delete failed");
        let _ = txn.rollback().await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    let now = chrono::Utc::now().fixed_offset();
    for lib_id in &wanted {
        let am = library_user_access::ActiveModel {
            library_id: Set(*lib_id),
            user_id: Set(uuid),
            role: Set("reader".into()),
            age_rating_max: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        if let Err(e) = am.insert(&txn).await {
            tracing::error!(error = %e, "library_user_access insert failed");
            let _ = txn.rollback().await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "tx commit failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.user.library_access.set",
            target_type: Some("user"),
            target_id: Some(uuid.to_string()),
            payload: serde_json::json!({
                "library_ids": wanted.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let counts = library_counts_for(&app, vec![uuid])
        .await
        .unwrap_or_default();
    let access = library_access_grants(&app, uuid).await.unwrap_or_default();
    Json(AdminUserDetailView {
        user: AdminUserView::from_model(target, &counts),
        library_access: access,
    })
    .into_response()
}

// ───────── helpers ─────────

impl AdminUserView {
    fn from_model(m: user::Model, counts: &HashMap<Uuid, i64>) -> Self {
        let library_count = if m.role == "admin" {
            0
        } else {
            counts.get(&m.id).copied().unwrap_or(0)
        };
        Self {
            id: m.id.to_string(),
            email: m.email,
            display_name: m.display_name,
            role: m.role,
            state: m.state,
            email_verified: m.email_verified,
            created_at: m.created_at.to_rfc3339(),
            last_login_at: m.last_login_at.map(|t| t.to_rfc3339()),
            library_count,
        }
    }
}

async fn library_counts_for(
    app: &AppState,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, i64>, sea_orm::DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.is_in(ids))
        .all(&app.db)
        .await?;
    let mut counts: HashMap<Uuid, i64> = HashMap::new();
    for r in rows {
        *counts.entry(r.user_id).or_insert(0) += 1;
    }
    Ok(counts)
}

async fn library_access_grants(
    app: &AppState,
    user_id: Uuid,
) -> Result<Vec<LibraryAccessGrantView>, sea_orm::DbErr> {
    let grants = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user_id))
        .all(&app.db)
        .await?;
    if grants.is_empty() {
        return Ok(Vec::new());
    }
    let lib_ids: Vec<Uuid> = grants.iter().map(|g| g.library_id).collect();
    let libs = library::Entity::find()
        .filter(library::Column::Id.is_in(lib_ids))
        .all(&app.db)
        .await?;
    let by_id: HashMap<Uuid, library::Model> = libs.into_iter().map(|l| (l.id, l)).collect();
    Ok(grants
        .into_iter()
        .filter_map(|g| {
            by_id.get(&g.library_id).map(|lib| LibraryAccessGrantView {
                library_id: g.library_id.to_string(),
                library_name: lib.name.clone(),
                role: g.role,
            })
        })
        .collect())
}

fn encode_cursor(id: Uuid) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id.as_bytes())
}

fn parse_cursor(s: &str) -> Result<Uuid, ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let arr: [u8; 16] = bytes.as_slice().try_into().map_err(|_| ())?;
    Ok(Uuid::from_bytes(arr))
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
