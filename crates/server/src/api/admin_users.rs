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
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::{library, library_user_access, user};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::{error, respond};
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::auth::password;
use crate::config::AuthMode;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::state::AppState;
use rand::Rng;
use server_macros::handler;
use shared::error::ApiErrorCode;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(create))
        .routes(routes!(get_one))
        .routes(routes!(update))
        .routes(routes!(disable))
        .routes(routes!(enable))
        .routes(routes!(set_library_access))
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
    /// Total rows matching the active filters. Populated only on the first
    /// page (no `cursor`) so the UI can show a count without paying for it
    /// on every page; `None` on subsequent pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
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

/// Audit-remediation M9.4 typed enums for the `?role=` / `?state=` query
/// params + the matching fields on `UpdateUserReq`. Serde rejects bad
/// values at deserialize time so handlers never see strings they can't
/// trust.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

impl UserRole {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserState {
    PendingVerification,
    Active,
    Disabled,
}

impl UserState {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::PendingVerification => "pending_verification",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListUsersQuery {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub role: Option<UserRole>,
    pub state: Option<UserState>,
    pub q: Option<String>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct UpdateUserReq {
    #[serde(default)]
    #[garde(inner(custom(non_empty_after_trim)))]
    pub display_name: Option<String>,
    #[serde(default)]
    #[garde(skip)]
    pub role: Option<UserRole>,
    #[serde(default)]
    #[garde(skip)]
    pub state: Option<UserState>,
}

fn non_empty_after_trim(value: &str, _: &()) -> garde::Result {
    if value.trim().is_empty() {
        return Err(garde::Error::new("display_name cannot be empty"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct LibraryAccessReq {
    /// Final set of library ids the user should be granted access to. The
    /// server replaces the user's `library_user_access` rows with this list.
    #[garde(skip)]
    pub library_ids: Vec<String>,
}

/// Body for admin create-user (3.8 / audit D9). The server generates the
/// password — the admin only chooses identity + role — so a temporary
/// credential exists without the admin inventing (and likely reusing) one.
#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateUserReq {
    #[garde(custom(valid_email))]
    pub email: String,
    /// Optional display name. Defaults to the email's local part.
    #[serde(default)]
    #[garde(inner(custom(non_empty_after_trim)))]
    pub display_name: Option<String>,
    /// Defaults to `user` when omitted.
    #[serde(default)]
    #[garde(skip)]
    pub role: Option<UserRole>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateUserResp {
    #[serde(flatten)]
    pub user: AdminUserView,
    /// One-time temporary password, generated server-side and returned
    /// ONLY here so the admin can hand it to the new user. It is hashed at
    /// rest like any other password and is never re-retrievable — the user
    /// changes it from their own account settings after first sign-in.
    pub temp_password: String,
}

fn valid_email(value: &str, _: &()) -> garde::Result {
    let v = value.trim();
    if v.len() < 3 || v.len() > 254 || !v.contains('@') {
        return Err(garde::Error::new("invalid email"));
    }
    Ok(())
}

/// Generate a 20-char temporary password from an unambiguous alphabet
/// (no `0/O/1/l/I`). Well over the 12-char minimum the local-auth path
/// enforces; the admin copies it rather than types it, so length over
/// memorability is the right trade.
fn gen_temp_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

// ───────── handlers ─────────

#[utoipa::path(
    operation_id = "admin_users_create",    post,
    path = "/admin/users",
    request_body = CreateUserReq,
    responses(
        (status = 201, body = CreateUserResp),
        (status = 403, description = "admin only"),
        (status = 409, description = "email already in use / local auth disabled"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<CreateUserReq>,
) -> impl IntoResponse {
    // A temp-password user is a *local* account; refuse when local auth is
    // off (OIDC-only) so we never mint an account that can't sign in.
    // Note: this deliberately does NOT consult `local_registration_open` —
    // admin provisioning is the whole point of the endpoint, independent of
    // whether public self-registration is open (audit D9).
    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return respond(
            StatusCode::CONFLICT,
            ApiErrorCode::Conflict,
            "local authentication is disabled; can't create a password user",
        );
    }

    let email = req.email.trim().to_lowercase();
    if let Ok(Some(_)) = user::Entity::find()
        .filter(user::Column::Email.eq(email.clone()))
        .one(&app.db)
        .await
    {
        return respond(
            StatusCode::CONFLICT,
            ApiErrorCode::Conflict,
            "email already in use",
        );
    }

    let temp_password = gen_temp_password();
    let hash = match password::hash(&temp_password, app.secrets.pepper.as_ref()) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "admin create-user: hash failed");
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "internal",
            );
        }
    };

    let role = req.role.unwrap_or(UserRole::User);
    let display = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_owned());
    let user_id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();

    // Admin-provisioned accounts land `active` + email-verified: the admin
    // is vouching for the address, so the user can sign in immediately with
    // the temp password even when SMTP isn't configured. Mirrors the field
    // set in `auth::local::register`.
    let am = user::ActiveModel {
        id: Set(user_id),
        external_id: Set(format!("local:{}", user_id)),
        display_name: Set(display),
        email: Set(Some(email.clone())),
        email_verified: Set(true),
        password_hash: Set(Some(hash)),
        totp_secret: Set(None),
        state: Set("active".into()),
        role: Set(role.as_db_str().to_owned()),
        token_version: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        last_login_at: Set(None),
        default_reading_direction: Set(None),
        default_fit_mode: Set(None),
        default_view_mode: Set(None),
        default_page_strip: Set(false),
        default_page_animation: Set(None),
        default_cover_solo: Set(true),
        theme: Set(None),
        accent_color: Set(None),
        density: Set(None),
        keybinds: Set(serde_json::json!({})),
        activity_tracking_enabled: Set(true),
        timezone: Set("UTC".into()),
        reading_min_active_ms: Set(30_000),
        reading_min_pages: Set(3),
        reading_idle_ms: Set(180_000),
        language: Set("en".into()),
        exclude_from_aggregates: Set(false),
        show_marker_count: Set(false),
        opds_wtr_reorder: Set(true),
        opds_progress_glyphs: Set(true),
        max_rails_per_page: Set(12),
    };

    let inserted = match am.insert(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            // The unique index on lower(email) is the real conflict defense
            // (covers the TOCTOU between the check above and this insert).
            tracing::warn!(error = %e, "admin create-user: insert failed");
            return respond(
                StatusCode::CONFLICT,
                ApiErrorCode::Conflict,
                "email already in use",
            );
        }
    };

    // Audit the creation — never log the password (or its hash).
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.user.create",
        target = ("user", user_id.to_string()),
        payload = serde_json::json!({
            "email": email,
            "role": role.as_db_str(),
        }),
    );

    let counts = HashMap::new();
    (
        StatusCode::CREATED,
        Json(CreateUserResp {
            user: AdminUserView::from_model(inserted, &counts),
            temp_password,
        }),
    )
        .into_response()
}

#[utoipa::path(
    operation_id = "admin_users_list",    get,
    path = "/admin/users",
    params(ListUsersQuery),
    responses(
        (status = 200, body = AdminUserListView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListUsersQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    // Validate the cursor up front so a bad value 400s rather than silently
    // restarting the list.
    let after = match q.cursor.as_deref() {
        Some(c) => match parse_cursor(c) {
            Ok(id) => Some(id),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
        None => None,
    };

    // Base query carrying just the filters (no cursor / order / limit), so
    // the same predicate can drive the first-page total count.
    // Enum-typed query params (audit-remediation M9.4) — serde rejects bad
    // values at deserialize time.
    let mut filtered = user::Entity::find();
    if let Some(role) = q.role {
        filtered = filtered.filter(user::Column::Role.eq(role.as_db_str()));
    }
    if let Some(state) = q.state {
        filtered = filtered.filter(user::Column::State.eq(state.as_db_str()));
    }
    if let Some(needle) = q.q.as_deref()
        && !needle.trim().is_empty()
    {
        // Case-insensitive, multi-term: each word must match the email OR the
        // display name. The previous code lowercased the *pattern* but ran a
        // case-sensitive `LIKE` against the raw column, so a mixed-case
        // display name ("Jane Doe") never matched "jane".
        use crate::util::search::{col_ilike, ilike_pattern};
        for token in needle.split_whitespace() {
            let pat = ilike_pattern(token);
            filtered = filtered.filter(
                sea_orm::Condition::any()
                    .add(col_ilike(user::Column::Email, &pat))
                    .add(col_ilike(user::Column::DisplayName, &pat)),
            );
        }
    }

    // Total matching the active filters — first page only (cursor absent),
    // so the UI shows "N users" without re-counting on every page. Soft-fail:
    // a count error drops the total rather than failing the whole list.
    let total = if after.is_none() {
        match filtered.clone().count(&app.db).await {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::error!(error = %e, "count users failed; omitting total");
                None
            }
        }
    } else {
        None
    };

    let mut query = filtered.order_by_asc(user::Column::Id);
    if let Some(after) = after {
        query = query.filter(user::Column::Id.gt(after));
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

    Json(AdminUserListView {
        items,
        next_cursor,
        total,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "admin_users_get_one",    get,
    path = "/admin/users/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserDetailView),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
#[handler]
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
    operation_id = "admin_users_update",    patch,
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
#[handler]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Validated(req): Validated<UpdateUserReq>,
) -> impl IntoResponse {
    // `id` is a path param — malformed UUID stays as 400 because the
    // problem is parse-shape, not semantic content.
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return respond(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::Validation,
            "invalid id",
        );
    };
    let Ok(Some(target)) = user::Entity::find_by_id(uuid).one(&app.db).await else {
        return respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::UserNotFound,
            "user not found",
        );
    };

    // Garde enforces: display_name non-empty-after-trim, role ∈
    // {admin, user}, state ∈ {pending_verification, active, disabled}.

    // Self-demotion guard: an admin cannot strip their own admin role or
    // disable their own account through this endpoint. Locks them out of
    // /admin entirely with no recovery path through the UI.
    if target.id == actor.id {
        if matches!(req.role, Some(UserRole::User)) {
            return respond(
                StatusCode::FORBIDDEN,
                ApiErrorCode::SelfDemote,
                "cannot demote yourself",
            );
        }
        if matches!(req.state, Some(UserState::Disabled)) {
            return respond(
                StatusCode::FORBIDDEN,
                ApiErrorCode::SelfDisable,
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
    if let Some(role) = req.role {
        let role_str = role.as_db_str();
        if role_str != target.role {
            changed.insert("role".into(), serde_json::json!(role_str));
            am.role = Set(role_str.to_owned());
        }
    }
    if let Some(state) = req.state {
        let state_str = state.as_db_str();
        if state_str != target.state {
            changed.insert("state".into(), serde_json::json!(state_str));
            if matches!(state, UserState::Disabled) {
                bump_token_version = true;
            }
            am.state = Set(state_str.to_owned());
        }
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
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "internal",
            );
        }
    };

    // AUTH-1: when an admin update bumps token_version (password/role/state
    // change), revoke the target's sessions so a stolen refresh token can't
    // survive the change.
    if bump_token_version {
        crate::auth::local::revoke_sessions_for_user(&app.db, uuid, None).await;
    }

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.user.update",
        target = ("user", uuid.to_string()),
        payload = serde_json::Value::Object(changed),
    );

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
    operation_id = "admin_users_disable",    post,
    path = "/admin/users/{id}/disable",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserView),
        (status = 403, description = "admin only / cannot disable self"),
        (status = 404, description = "user not found"),
    )
)]
#[handler]
pub async fn disable(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    set_state(app, actor, ctx, id, "disabled").await
}

#[utoipa::path(
    operation_id = "admin_users_enable",    post,
    path = "/admin/users/{id}/enable",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = AdminUserView),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
#[handler]
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

    // AUTH-1: disabling a user revokes their sessions so an already-issued
    // refresh token can't keep rotating (the token_version bump only stops
    // access tokens). Enabling is a no-op here.
    if new_state == "disabled" {
        crate::auth::local::revoke_sessions_for_user(&app.db, uuid, None).await;
    }

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
    operation_id = "admin_users_set_library_access",    post,
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
#[handler]
pub async fn set_library_access(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Validated(req): Validated<LibraryAccessReq>,
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
                StatusCode::UNPROCESSABLE_ENTITY,
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
