//! Saved smart views API (saved-views M3).
//!
//! Two surfaces:
//!
//!   - User-scoped routes under `/me/saved-views/*` — the calling user
//!     CRUDs their own views, pins them to the home rail, and runs
//!     queries via `/results`. System views (admin-curated, `user_id IS
//!     NULL`) are read-only here.
//!   - Admin routes under `/admin/saved-views/*` — admins CRUD system
//!     views. Every mutation lands in the audit log.
//!
//! Filter views compile their `conditions` JSONB through
//! `crate::views::compile` into a single sea_query select that joins
//! the per-user reading-state view (M2) on demand. CBL views (kind
//! `'cbl'`) are scaffolded here but their `/results` is empty until M4
//! materializes the underlying entries.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post},
};
use chrono::Utc;
use entity::{saved_view, user_view_pin};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult,
    ModelTrait, PaginatorTrait, QueryFilter, QueryOrder, Statement, TransactionTrait,
    sea_query::PostgresQueryBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::series::SeriesListView;
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::state::AppState;
use crate::views::{
    compile::{self, CompileError, CompileInput, Cursor},
    dsl::{FilterDsl, MatchMode, SortField, SortOrder},
};

pub const KIND_FILTER_SERIES: &str = "filter_series";
pub const KIND_SYSTEM: &str = "system";
pub const KIND_CBL: &str = "cbl";
/// User-owned manual list of mixed series + issue refs (markers +
/// collections M1). Backed by the `collection_entries` join table.
pub const KIND_COLLECTION: &str = "collection";
/// `system_key` for the per-user Want to Read collection (auto-seeded
/// on first GET in M2).
pub const SYSTEM_KEY_WANT_TO_READ: &str = "want_to_read";

const MAX_RESULT_LIMIT: u64 = 200;
const MIN_RESULT_LIMIT: u64 = 1;
const MAX_PIN_COUNT: i64 = 12;

pub fn routes() -> Router<AppState> {
    Router::new()
        // user-scoped
        .route("/me/saved-views", get(list).post(create))
        .route(
            "/me/saved-views/{id}",
            patch(update).delete(delete_one),
        )
        .route("/me/saved-views/{id}/pin", post(pin))
        .route("/me/saved-views/{id}/unpin", post(unpin))
        .route("/me/saved-views/{id}/sidebar", post(set_sidebar))
        .route("/me/saved-views/{id}/icon", post(set_icon))
        .route("/me/saved-views/reorder", post(reorder))
        .route("/me/saved-views/{id}/results", get(results))
        .route("/me/saved-views/preview", post(preview))
        // admin
        .route("/admin/saved-views", post(admin_create))
        .route(
            "/admin/saved-views/{id}",
            patch(admin_update).delete(admin_delete),
        )
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SavedViewView {
    pub id: String,
    /// `None` for system views (admin-curated, visible to every user).
    pub user_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub custom_year_start: Option<i32>,
    pub custom_year_end: Option<i32>,
    pub custom_tags: Vec<String>,
    pub match_mode: Option<String>,
    pub conditions: Option<serde_json::Value>,
    pub sort_field: Option<String>,
    pub sort_order: Option<String>,
    pub result_limit: Option<i32>,
    pub cbl_list_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Whether the calling user has this view pinned to their home rail.
    pub pinned: bool,
    /// `None` when not pinned.
    pub pinned_position: Option<i32>,
    /// Whether the calling user wants this view to appear in the
    /// left-sidebar's "Saved views" section.
    pub show_in_sidebar: bool,
    /// True for system views (`user_id IS NULL`).
    pub is_system: bool,
    /// Identifies the built-in rail when `kind = 'system'`
    /// (`'continue_reading'`, `'on_deck'`). `None` for filter/CBL views.
    pub system_key: Option<String>,
    /// Per-user icon override key. `None` falls back to a kind-based
    /// default resolved client-side. Free-form text — the client maps
    /// it against its rail-icon registry and silently falls back if the
    /// key is unknown.
    pub icon: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SavedViewListView {
    pub items: Vec<SavedViewView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateSavedViewReq {
    /// `'filter_series'` or `'cbl'`. Validated server-side; mismatched
    /// kind/body shape returns 422.
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub custom_year_start: Option<i32>,
    #[serde(default)]
    pub custom_year_end: Option<i32>,
    #[serde(default)]
    pub custom_tags: Option<Vec<String>>,
    // ───── filter_series fields ─────
    #[serde(default)]
    pub filter: Option<FilterDsl>,
    #[serde(default)]
    pub sort_field: Option<SortField>,
    #[serde(default)]
    pub sort_order: Option<SortOrder>,
    #[serde(default)]
    pub result_limit: Option<i32>,
    // ───── cbl fields ─────
    #[serde(default)]
    pub cbl_list_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSavedViewReq {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub custom_year_start: Option<Option<i32>>,
    #[serde(default)]
    pub custom_year_end: Option<Option<i32>>,
    #[serde(default)]
    pub custom_tags: Option<Vec<String>>,
    #[serde(default)]
    pub filter: Option<FilterDsl>,
    #[serde(default)]
    pub sort_field: Option<SortField>,
    #[serde(default)]
    pub sort_order: Option<SortOrder>,
    #[serde(default)]
    pub result_limit: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReorderReq {
    /// View IDs in desired pin order. Views not currently pinned are
    /// rejected; views pinned but absent from the list keep their
    /// existing position.
    pub view_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PinView {
    pub view_id: String,
    pub pinned: bool,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreviewReq {
    pub filter: FilterDsl,
    pub sort_field: SortField,
    pub sort_order: SortOrder,
    pub result_limit: i32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListQuery {
    /// When `Some(true)`, list only the views the user has pinned. When
    /// `Some(false)`, only unpinned. When `None`, all visible views.
    #[serde(default)]
    pub pinned: Option<bool>,
    /// Same shape as `pinned` but for sidebar visibility — drives the
    /// `Saved views` section of the main left nav.
    #[serde(default)]
    pub show_in_sidebar: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SidebarQuery {
    /// `true` (default) adds the view to the sidebar; `false` removes
    /// it. Idempotent.
    #[serde(default)]
    pub show: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResultsQuery {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

// ───── shared helpers ─────

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn validate_create(req: &CreateSavedViewReq) -> Result<(), (StatusCode, &'static str, String)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "validation",
            "name required".into(),
        ));
    }
    match req.kind.as_str() {
        KIND_FILTER_SERIES => {
            if req.cbl_list_id.is_some() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "filter_series view must not set cbl_list_id".into(),
                ));
            }
            if req.filter.is_none() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "filter_series view requires `filter`".into(),
                ));
            }
            let limit = req.result_limit.unwrap_or(12);
            if !(MIN_RESULT_LIMIT..=MAX_RESULT_LIMIT).contains(&(limit as u64)) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "validation",
                    format!("result_limit must be {MIN_RESULT_LIMIT}..={MAX_RESULT_LIMIT}"),
                ));
            }
        }
        KIND_CBL => {
            if req.cbl_list_id.is_none() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "cbl view requires cbl_list_id".into(),
                ));
            }
            if req.filter.is_some() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "cbl view must not set filter".into(),
                ));
            }
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "validation",
                "kind must be filter_series or cbl".into(),
            ));
        }
    }
    Ok(())
}

async fn fetch_view(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<saved_view::Model>, sea_orm::DbErr> {
    saved_view::Entity::find_by_id(id).one(db).await
}

async fn user_pin(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    view_id: Uuid,
) -> Result<Option<user_view_pin::Model>, sea_orm::DbErr> {
    user_view_pin::Entity::find_by_id((user_id, view_id))
        .one(db)
        .await
}

async fn user_pins(
    db: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<user_view_pin::Model>, sea_orm::DbErr> {
    user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user_id))
        .order_by_asc(user_view_pin::Column::Position)
        .all(db)
        .await
}

/// Lazy idempotent pin seed for `auto_pin = true` system views. Runs on
/// every `GET /me/saved-views` call but only inserts rows for system
/// views the user **doesn't already have a pin row for** — which means a
/// system rail added in a later release (e.g. Continue reading, On deck)
/// gets picked up by *existing* users on their next home page visit, not
/// only fresh registrations.
///
/// New rows are appended after the user's current max `position` so the
/// new rail lands at the bottom of their pin order instead of shuffling
/// their carefully curated layout. The user can drag-reorder afterward.
///
/// Called from `list` so the home page renders the seeded rails on the
/// next page load after a deploy that ships a new auto-pinned rail.
async fn ensure_pins_seeded(
    db: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    // Existing pins → look up the user's current max position so any new
    // rail we add gets appended (no position collisions, no reshuffling).
    let existing: Vec<user_view_pin::Model> = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    let existing_view_ids: std::collections::HashSet<Uuid> =
        existing.iter().map(|p| p.view_id).collect();
    let mut next_position: i32 = existing
        .iter()
        .map(|p| p.position)
        .max()
        .map(|p| p + 1)
        .unwrap_or(0);

    // Only `auto_pin = true` system views surface here. Other system
    // templates (Just Finished / Want to Read / Stale) live in
    // `/settings/views` for the user to opt into.
    let system: Vec<saved_view::Model> = saved_view::Entity::find()
        .filter(saved_view::Column::UserId.is_null())
        .filter(saved_view::Column::AutoPin.eq(true))
        .order_by_asc(saved_view::Column::CreatedAt)
        .order_by_asc(saved_view::Column::Id)
        .all(db)
        .await?;
    for v in system.iter() {
        if existing_view_ids.contains(&v.id) {
            continue;
        }
        user_view_pin::ActiveModel {
            user_id: Set(user_id),
            view_id: Set(v.id),
            position: Set(next_position),
            pinned: Set(true),
            show_in_sidebar: Set(false),
            icon: Set(None),
        }
        .insert(db)
        .await?;
        next_position += 1;
    }
    Ok(())
}

fn to_view(model: &saved_view::Model, pref: Option<&user_view_pin::Model>) -> SavedViewView {
    let custom_tags = model.custom_tags.clone();
    let pinned = pref.map(|p| p.pinned).unwrap_or(false);
    let show_in_sidebar = pref.map(|p| p.show_in_sidebar).unwrap_or(false);
    SavedViewView {
        id: model.id.to_string(),
        user_id: model.user_id.map(|u| u.to_string()),
        kind: model.kind.clone(),
        name: model.name.clone(),
        description: model.description.clone(),
        custom_year_start: model.custom_year_start,
        custom_year_end: model.custom_year_end,
        custom_tags,
        match_mode: model.match_mode.clone(),
        conditions: model.conditions.clone(),
        sort_field: model.sort_field.clone(),
        sort_order: model.sort_order.clone(),
        result_limit: model.result_limit,
        cbl_list_id: model.cbl_list_id.map(|u| u.to_string()),
        created_at: model.created_at.to_rfc3339(),
        updated_at: model.updated_at.to_rfc3339(),
        pinned,
        pinned_position: if pinned {
            pref.map(|p| p.position)
        } else {
            None
        },
        show_in_sidebar,
        is_system: model.user_id.is_none(),
        system_key: model.system_key.clone(),
        icon: pref.and_then(|p| p.icon.clone()),
    }
}

// ───── handlers ─────

#[utoipa::path(
    get,
    path = "/me/saved-views",
    params(("pinned" = Option<bool>, Query,)),
    responses((status = 200, body = SavedViewListView))
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if let Err(e) = ensure_pins_seeded(&app.db, user.id).await {
        tracing::warn!(user_id = %user.id, error = %e, "saved_views: pin seed failed");
    }
    // Markers + Collections M3: ensure Want to Read exists before the
    // sidebar reads from this endpoint. The dedicated /me/collections
    // path also seeds it; double-call is a no-op via the partial unique.
    if let Err(e) = crate::api::collections::ensure_want_to_read_seeded(&app.db, user.id).await {
        tracing::warn!(user_id = %user.id, error = %e, "saved_views: want_to_read seed failed");
    }

    let pins = match user_pins(&app.db, user.id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "saved_views: list pins failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let pin_by_view: std::collections::HashMap<Uuid, user_view_pin::Model> =
        pins.into_iter().map(|p| (p.view_id, p)).collect();

    let mut select = saved_view::Entity::find();
    // System views (`user_id IS NULL`) and the caller's own.
    select = select.filter(
        sea_orm::Condition::any()
            .add(saved_view::Column::UserId.is_null())
            .add(saved_view::Column::UserId.eq(user.id)),
    );

    let rows = match select.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "saved_views: list views failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut items: Vec<SavedViewView> = rows
        .into_iter()
        .filter_map(|m| {
            let pin = pin_by_view.get(&m.id);
            let is_pinned = pin.map(|p| p.pinned).unwrap_or(false);
            let in_sidebar = pin.map(|p| p.show_in_sidebar).unwrap_or(false);
            match q.pinned {
                Some(true) if !is_pinned => return None,
                Some(false) if is_pinned => return None,
                _ => {}
            }
            match q.show_in_sidebar {
                Some(true) if !in_sidebar => return None,
                Some(false) if in_sidebar => return None,
                _ => {}
            }
            Some(to_view(&m, pin))
        })
        .collect();

    // Pinned first (by position), then everything else alpha by name.
    items.sort_by(|a, b| match (a.pinned_position, b.pinned_position) {
        (Some(ap), Some(bp)) => ap.cmp(&bp),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Json(SavedViewListView { items }).into_response()
}

#[utoipa::path(
    post,
    path = "/me/saved-views",
    request_body = CreateSavedViewReq,
    responses((status = 201, body = SavedViewView))
)]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateSavedViewReq>,
) -> impl IntoResponse {
    create_inner(&app, user.id.into(), &req, false).await
}

#[utoipa::path(
    post,
    path = "/admin/saved-views",
    request_body = CreateSavedViewReq,
    responses((status = 201, body = SavedViewView))
)]
pub async fn admin_create(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateSavedViewReq>,
) -> impl IntoResponse {
    let resp = create_inner(&app, None, &req, true).await;
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.saved_view.create",
            target_type: Some("saved_view"),
            target_id: None,
            payload: serde_json::json!({ "kind": req.kind, "name": req.name }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    resp
}

/// `owner` is `Some(user_id)` for user-scoped creates; `None` for system
/// views. The caller wraps with the appropriate auth + audit boilerplate.
async fn create_inner(
    app: &AppState,
    owner: Option<Uuid>,
    req: &CreateSavedViewReq,
    is_admin_caller: bool,
) -> axum::response::Response {
    if let Err((status, code, msg)) = validate_create(req) {
        return error(status, code, &msg);
    }
    // Compile-validate the DSL on filter views before persisting.
    if req.kind == KIND_FILTER_SERIES
        && let Some(filter) = &req.filter
    {
        let dummy = CompileInput {
            dsl: filter,
            sort_field: req.sort_field.unwrap_or(SortField::CreatedAt),
            sort_order: req.sort_order.unwrap_or(SortOrder::Desc),
            limit: 12,
            cursor: None,
            user_id: owner.unwrap_or_else(Uuid::nil),
            visible_libraries: access::VisibleLibraries::unrestricted(),
        };
        if let Err(e) = compile::compile(&dummy) {
            return compile_error_response(e);
        }
    }

    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let am = saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(owner),
        kind: Set(req.kind.clone()),
        name: Set(req.name.trim().to_owned()),
        description: Set(req
            .description
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())),
        custom_year_start: Set(req.custom_year_start),
        custom_year_end: Set(req.custom_year_end),
        custom_tags: Set(req.custom_tags.clone().unwrap_or_default()),
        match_mode: Set(if req.kind == KIND_FILTER_SERIES {
            Some(match_mode_str(
                req.filter
                    .as_ref()
                    .map(|f| f.match_mode)
                    .unwrap_or(MatchMode::All),
            ))
        } else {
            None
        }),
        conditions: Set(if req.kind == KIND_FILTER_SERIES {
            let conds = req
                .filter
                .as_ref()
                .map(|f| f.conditions.clone())
                .unwrap_or_default();
            Some(serde_json::to_value(conds).unwrap_or(serde_json::json!([])))
        } else {
            None
        }),
        sort_field: Set(if req.kind == KIND_FILTER_SERIES {
            Some(
                req.sort_field
                    .unwrap_or(SortField::CreatedAt)
                    .as_str()
                    .to_owned(),
            )
        } else {
            None
        }),
        sort_order: Set(if req.kind == KIND_FILTER_SERIES {
            Some(
                req.sort_order
                    .unwrap_or(SortOrder::Desc)
                    .as_str()
                    .to_owned(),
            )
        } else {
            None
        }),
        result_limit: Set(if req.kind == KIND_FILTER_SERIES {
            Some(req.result_limit.unwrap_or(12))
        } else {
            None
        }),
        cbl_list_id: Set(req.cbl_list_id),
        // Only the migration seeds `kind='system'` rows; user/admin creation
        // never sets `system_key`.
        system_key: Set(None),
        // Admin- and user-created views never auto-pin. The flag is
        // reserved for the M3-seeded built-ins (and future migrations).
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let saved = match am.insert(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "saved_views: insert failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // Admin-created system views aren't pinned automatically (per Q8 / C8).
    let _ = is_admin_caller;
    (StatusCode::CREATED, Json(to_view(&saved, None))).into_response()
}

fn match_mode_str(m: MatchMode) -> String {
    match m {
        MatchMode::All => "all".to_owned(),
        MatchMode::Any => "any".to_owned(),
    }
}

fn compile_error_response(e: CompileError) -> axum::response::Response {
    let msg = e.to_string();
    error(StatusCode::UNPROCESSABLE_ENTITY, "filter_invalid", &msg)
}

#[utoipa::path(
    patch,
    path = "/me/saved-views/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateSavedViewReq,
    responses((status = 200, body = SavedViewView))
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateSavedViewReq>,
) -> impl IntoResponse {
    let row = match fetch_view(&app.db, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(e) => {
            tracing::error!(error = %e, "saved_views: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // System views are read-only on this user-scoped path.
    if row.user_id != Some(user.id) {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }
    apply_update(&app, &row, &req).await
}

#[utoipa::path(
    patch,
    path = "/admin/saved-views/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateSavedViewReq,
    responses((status = 200, body = SavedViewView))
)]
pub async fn admin_update(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateSavedViewReq>,
) -> impl IntoResponse {
    let row = match fetch_view(&app.db, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(e) => {
            tracing::error!(error = %e, "saved_views: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.user_id.is_some() {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin can only edit system views",
        );
    }
    // Built-in system rails (Continue reading / On deck) are immutable —
    // they carry no editable filter/sort, and the home renderer keys off
    // their `system_key`.
    if row.kind == KIND_SYSTEM {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "system rails cannot be edited",
        );
    }
    let resp = apply_update(&app, &row, &req).await;
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.saved_view.update",
            target_type: Some("saved_view"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({ "id": id.to_string() }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    resp
}

async fn apply_update(
    app: &AppState,
    row: &saved_view::Model,
    req: &UpdateSavedViewReq,
) -> axum::response::Response {
    // For filter views, validate the new DSL via compile if `filter` was sent.
    if row.kind == KIND_FILTER_SERIES
        && let Some(filter) = &req.filter
    {
        let dummy = CompileInput {
            dsl: filter,
            sort_field: req.sort_field.unwrap_or(SortField::CreatedAt),
            sort_order: req.sort_order.unwrap_or(SortOrder::Desc),
            limit: 12,
            cursor: None,
            user_id: Uuid::nil(),
            visible_libraries: access::VisibleLibraries::unrestricted(),
        };
        if let Err(e) = compile::compile(&dummy) {
            return compile_error_response(e);
        }
    }

    let mut am: saved_view::ActiveModel = row.clone().into();
    if let Some(name) = req.name.as_ref() {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return error(StatusCode::BAD_REQUEST, "validation", "name required");
        }
        am.name = Set(trimmed.to_owned());
    }
    if let Some(desc) = req.description.as_ref() {
        am.description = Set(desc
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()));
    }
    if let Some(ys) = req.custom_year_start.as_ref() {
        am.custom_year_start = Set(*ys);
    }
    if let Some(ye) = req.custom_year_end.as_ref() {
        am.custom_year_end = Set(*ye);
    }
    if let Some(tags) = req.custom_tags.as_ref() {
        am.custom_tags = Set(tags.clone());
    }
    if row.kind == KIND_FILTER_SERIES {
        if let Some(filter) = req.filter.as_ref() {
            am.match_mode = Set(Some(match_mode_str(filter.match_mode)));
            am.conditions = Set(Some(
                serde_json::to_value(&filter.conditions).unwrap_or(serde_json::json!([])),
            ));
        }
        if let Some(sf) = req.sort_field {
            am.sort_field = Set(Some(sf.as_str().to_owned()));
        }
        if let Some(so) = req.sort_order {
            am.sort_order = Set(Some(so.as_str().to_owned()));
        }
        if let Some(lim) = req.result_limit {
            if !(MIN_RESULT_LIMIT..=MAX_RESULT_LIMIT).contains(&(lim as u64)) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "result_limit out of range",
                );
            }
            am.result_limit = Set(Some(lim));
        }
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "saved_views: update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let pin = user_pin(&app.db, row.user_id.unwrap_or(Uuid::nil()), updated.id)
        .await
        .unwrap_or_default();
    Json(to_view(&updated, pin.as_ref())).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/saved-views/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_view(&app.db, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(e) => {
            tracing::error!(error = %e, "saved_views: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.user_id != Some(user.id) {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "saved_views: delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    delete,
    path = "/admin/saved-views/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn admin_delete(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_view(&app.db, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(e) => {
            tracing::error!(error = %e, "saved_views: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.user_id.is_some() {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "admin can only delete system views",
        );
    }
    // Built-in system rails cannot be deleted; the home renderer keys off
    // their `system_key`, and the lazy pin seed would re-create the row.
    if row.kind == KIND_SYSTEM {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "system rails cannot be deleted",
        );
    }
    // Count affected pins for the audit log before the cascade fires.
    let affected_pins = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::ViewId.eq(id))
        .count(&app.db)
        .await
        .unwrap_or(0);
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "saved_views: admin delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.saved_view.delete",
            target_type: Some("saved_view"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({ "affected_user_pins": affected_pins }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/saved-views/{id}/pin",
    params(("id" = String, Path,)),
    responses((status = 200, body = PinView))
)]
pub async fn pin(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let view = match fetch_view(&app.db, id).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Visibility: own + system.
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }

    let existing = match user_pin(&app.db, user.id, id).await {
        Ok(p) => p,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if let Some(row) = existing.as_ref()
        && row.pinned
    {
        return Json(PinView {
            view_id: id.to_string(),
            pinned: true,
            position: Some(row.position),
        })
        .into_response();
    }

    // Cap enforcement (Q4: hard cap of 12 with "Show all views →" disclosure).
    let active_count = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .filter(user_view_pin::Column::Pinned.eq(true))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;
    if active_count >= MAX_PIN_COUNT {
        return error(
            StatusCode::CONFLICT,
            "pin_cap_reached",
            "unpin one to add another",
        );
    }

    let pos = active_count as i32;
    let result = if let Some(row) = existing {
        let mut am: user_view_pin::ActiveModel = row.into();
        am.pinned = Set(true);
        am.position = Set(pos);
        am.update(&app.db).await
    } else {
        user_view_pin::ActiveModel {
            user_id: Set(user.id),
            view_id: Set(id),
            position: Set(pos),
            pinned: Set(true),
            show_in_sidebar: Set(false),
            icon: Set(None),
        }
        .insert(&app.db)
        .await
    };
    if let Err(e) = result {
        tracing::error!(error = %e, "saved_views: pin upsert failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    Json(PinView {
        view_id: id.to_string(),
        pinned: true,
        position: Some(pos),
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/me/saved-views/{id}/unpin",
    params(("id" = String, Path,)),
    responses((status = 200, body = PinView))
)]
pub async fn unpin(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    if let Some(row) = match user_pin(&app.db, user.id, id).await {
        Ok(p) => p,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    } {
        let prune = !row.show_in_sidebar;
        if prune {
            // Drop the row entirely if no other prefs are set on it; the
            // PK row is just a position holder otherwise.
            let _ = user_view_pin::Entity::delete_by_id((user.id, id))
                .exec(&app.db)
                .await;
        } else {
            let mut am: user_view_pin::ActiveModel = row.into();
            am.pinned = Set(false);
            let _ = am.update(&app.db).await;
        }
    }
    if let Err(e) = compact_pin_positions(&app.db, user.id).await {
        tracing::warn!(error = %e, "saved_views: position compaction failed");
    }
    Json(PinView {
        view_id: id.to_string(),
        pinned: false,
        position: None,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/me/saved-views/{id}/sidebar",
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
    Query(q): Query<SidebarQuery>,
) -> impl IntoResponse {
    let view = match fetch_view(&app.db, id).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }
    let want = q.show.unwrap_or(true);
    let existing = match user_pin(&app.db, user.id, id).await {
        Ok(p) => p,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let result = match existing {
        Some(row) => {
            // Drop the row entirely if turning sidebar off and nothing else
            // is keeping it alive.
            if !want && !row.pinned {
                let _ = user_view_pin::Entity::delete_by_id((user.id, id))
                    .exec(&app.db)
                    .await;
                return StatusCode::NO_CONTENT.into_response();
            }
            let mut am: user_view_pin::ActiveModel = row.into();
            am.show_in_sidebar = Set(want);
            am.update(&app.db).await.map(|_| ())
        }
        None if want => user_view_pin::ActiveModel {
            user_id: Set(user.id),
            view_id: Set(id),
            // Position is irrelevant for a sidebar-only entry; park at 0.
            position: Set(0),
            pinned: Set(false),
            show_in_sidebar: Set(true),
            icon: Set(None),
        }
        .insert(&app.db)
        .await
        .map(|_| ()),
        None => Ok(()),
    };
    if let Err(e) = result {
        tracing::error!(error = %e, "saved_views: sidebar toggle failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Body for `POST /me/saved-views/{id}/icon` — pick (or clear) the icon
/// that represents this rail in the user's home + sidebar. `None` /
/// missing means "reset to the kind-based default."
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetIconReq {
    /// Free-form key matched against the client's icon registry. Reset
    /// to default by sending `null` or omitting the field.
    #[serde(default)]
    pub icon: Option<String>,
}

#[utoipa::path(
    post,
    path = "/me/saved-views/{id}/icon",
    params(("id" = String, Path,)),
    request_body = SetIconReq,
    responses((status = 204))
)]
pub async fn set_icon(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<SetIconReq>,
) -> impl IntoResponse {
    let view = match fetch_view(&app.db, id).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Owner check mirrors the sidebar/pin handlers — system views (NULL
    // user_id) are accessible to every user; non-system views only by
    // their owner.
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }
    // Light validation: trim, bound the length (the registry keys are
    // short), and treat empty as None so the client can "reset" without
    // sending a null literal. No allow-listing here — the client
    // silently falls back to the default for unknown keys.
    let trimmed = req
        .icon
        .as_ref()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Some(s) = &trimmed
        && s.len() > 64
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "icon key must be 64 chars or fewer",
        );
    }

    let existing = match user_pin(&app.db, user.id, id).await {
        Ok(p) => p,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let result = match (existing, &trimmed) {
        (Some(row), _) => {
            let mut am: user_view_pin::ActiveModel = row.into();
            am.icon = Set(trimmed.clone());
            am.update(&app.db).await.map(|_| ())
        }
        (None, Some(_)) => user_view_pin::ActiveModel {
            user_id: Set(user.id),
            view_id: Set(id),
            // No pin / sidebar bound by setting an icon alone — same
            // policy the sidebar toggle uses when starting from scratch.
            position: Set(0),
            pinned: Set(false),
            show_in_sidebar: Set(false),
            icon: Set(trimmed.clone()),
        }
        .insert(&app.db)
        .await
        .map(|_| ()),
        (None, None) => Ok(()),
    };
    if let Err(e) = result {
        tracing::error!(error = %e, "saved_views: icon update failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn compact_pin_positions(
    db: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    let mut pins = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user_id))
        .filter(user_view_pin::Column::Pinned.eq(true))
        .order_by_asc(user_view_pin::Column::Position)
        .all(db)
        .await?;
    for (i, p) in pins.iter_mut().enumerate() {
        if p.position != i as i32 {
            let mut am: user_view_pin::ActiveModel = p.clone().into();
            am.position = Set(i as i32);
            am.update(db).await?;
        }
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/me/saved-views/reorder",
    request_body = ReorderReq,
    responses((status = 204))
)]
pub async fn reorder(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ReorderReq>,
) -> impl IntoResponse {
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Validate every id is currently pinned by the user.
    for view_id in &req.view_ids {
        let pin = match user_pin(&txn, user.id, *view_id).await {
            Ok(p) => p,
            Err(_) => {
                let _ = txn.rollback().await;
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        if pin.is_none() {
            let _ = txn.rollback().await;
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "all view_ids must be pinned",
            );
        }
    }
    // Apply new positions in two passes to avoid composite-PK uniqueness
    // collisions during the rewrite (Postgres checks unique inside the
    // statement). Pass 1: bump each affected pin to a high temporary
    // value. Pass 2: assign the final position. Position values only
    // matter for ordering, so the temporary range never leaks.
    use sea_orm::{ActiveValue::NotSet, ActiveValue::Unchanged};
    for (i, view_id) in req.view_ids.iter().enumerate() {
        let am = user_view_pin::ActiveModel {
            user_id: Unchanged(user.id),
            view_id: Unchanged(*view_id),
            position: Set(10_000 + i as i32),
            pinned: NotSet,
            show_in_sidebar: NotSet,
            icon: NotSet,
        };
        am.update(&txn).await.ok();
    }
    for (i, view_id) in req.view_ids.iter().enumerate() {
        let am = user_view_pin::ActiveModel {
            user_id: Unchanged(user.id),
            view_id: Unchanged(*view_id),
            position: Set(i as i32),
            pinned: NotSet,
            show_in_sidebar: NotSet,
            icon: NotSet,
        };
        am.update(&txn).await.ok();
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "saved_views: reorder commit failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/saved-views/preview",
    request_body = PreviewReq,
    responses((status = 200, body = SeriesListView))
)]
pub async fn preview(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<PreviewReq>,
) -> impl IntoResponse {
    let limit = (req.result_limit as u64).clamp(MIN_RESULT_LIMIT, MAX_RESULT_LIMIT);
    let visible = access::for_user(&app, &user).await;
    let input = CompileInput {
        dsl: &req.filter,
        sort_field: req.sort_field,
        sort_order: req.sort_order,
        limit,
        cursor: None,
        user_id: user.id,
        visible_libraries: visible,
    };
    run_filter_query(&app, input).await
}

#[utoipa::path(
    get,
    path = "/me/saved-views/{id}/results",
    params(
        ("id" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = SeriesListView))
)]
pub async fn results(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<ResultsQuery>,
) -> impl IntoResponse {
    let view = match fetch_view(&app.db, id).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your view");
    }

    if view.kind == KIND_CBL {
        // M4 stub: empty result set until cbl_lists/entries materialize.
        return Json(SeriesListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response();
    }
    if view.kind == KIND_COLLECTION {
        // Collections carry *mixed* series + issue entries, which don't
        // round-trip cleanly through `SeriesListView`. Pinned-collection
        // rails and the detail page fetch via
        // `GET /me/collections/{id}/entries` instead. Return an empty
        // stub here so callers that hit the generic path get a clean
        // response.
        return Json(SeriesListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response();
    }

    let filter = match dsl_from_view(&view) {
        Ok(f) => f,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let sort_field = view
        .sort_field
        .as_deref()
        .and_then(SortField::parse)
        .unwrap_or(SortField::CreatedAt);
    let sort_order = match view.sort_order.as_deref() {
        Some("asc") => SortOrder::Asc,
        _ => SortOrder::Desc,
    };
    let view_limit = view.result_limit.unwrap_or(12) as u64;
    let limit = q
        .limit
        .unwrap_or(view_limit)
        .clamp(MIN_RESULT_LIMIT, MAX_RESULT_LIMIT);
    let cursor = q.cursor.and_then(|s| parse_cursor(&s).ok());

    let visible = access::for_user(&app, &user).await;
    let input = CompileInput {
        dsl: &filter,
        sort_field,
        sort_order,
        limit,
        cursor,
        user_id: user.id,
        visible_libraries: visible,
    };
    run_filter_query(&app, input).await
}

fn dsl_from_view(view: &saved_view::Model) -> Result<FilterDsl, serde_json::Error> {
    let mode = match view.match_mode.as_deref() {
        Some("any") => MatchMode::Any,
        _ => MatchMode::All,
    };
    let conditions = match view.conditions.as_ref() {
        Some(j) => serde_json::from_value(j.clone())?,
        None => Vec::new(),
    };
    Ok(FilterDsl {
        match_mode: mode,
        conditions,
    })
}

async fn run_filter_query(app: &AppState, input: CompileInput<'_>) -> axum::response::Response {
    let stmt = match compile::compile(&input) {
        Ok(s) => s,
        Err(e) => return compile_error_response(e),
    };
    let backend = app.db.get_database_backend();
    let (sql, values) = stmt.build(PostgresQueryBuilder);
    let raw = Statement::from_sql_and_values(backend, sql, values);
    let rows = match entity::series::Model::find_by_statement(raw)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "saved_views: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let limit = input.limit;
    let mut rows = rows;
    let next_cursor = if rows.len() as u64 > limit {
        let extra = rows.pop();
        extra.map(|r| {
            let value = sort_value_for(&r, input.sort_field);
            encode_cursor(&value, &r.id.to_string())
        })
    } else {
        None
    };

    // hydrate_series attaches `issue_count` + `cover_url`. Skipping it
    // — as we did originally — was the M7 home page's empty-cover bug.
    let items = crate::api::series::hydrate_series(app, rows).await;
    // Saved-view results don't surface a total today — saved views are
    // capped at `result_limit` (12 by default) and the caller knows
    // that ceiling. Leaving `None` so we don't pretend we counted.
    Json(SeriesListView {
        items,
        next_cursor,
        total: None,
    })
    .into_response()
}

fn sort_value_for(row: &entity::series::Model, field: SortField) -> String {
    match field {
        SortField::Name => row.name.clone(),
        SortField::Year => row.year.map(|y| y.to_string()).unwrap_or_default(),
        SortField::CreatedAt => row.created_at.to_rfc3339(),
        SortField::UpdatedAt => row.updated_at.to_rfc3339(),
        // Reading-state sorts: cursor uses id-only fallback (sort_value
        // empty); the compiler's id tiebreaker keeps page boundaries
        // stable.
        SortField::LastRead | SortField::ReadProgress => String::new(),
    }
}

fn encode_cursor(sort_value: &str, id: &str) -> String {
    use base64::Engine;
    let s = format!("{sort_value}|{id}");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

fn parse_cursor(s: &str) -> Result<Cursor, ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let decoded = String::from_utf8(bytes).map_err(|_| ())?;
    let (sort_value, id) = decoded.rsplit_once('|').ok_or(())?;
    let id = Uuid::parse_str(id).map_err(|_| ())?;
    Ok(Cursor {
        sort_value: sort_value.to_owned(),
        id,
    })
}
