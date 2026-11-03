//! Start Here — admin-featured reading lists for new readers
//! (Start Here + default pins M2).
//!
//! An admin turns one of their own reading lists into a **shared** list
//! (`saved_views.user_id` and `cbl_lists.owner_user_id` both NULL — the
//! same "system" shape the rest of the app already treats as visible to
//! every user), then **features** shared lists in an ordered set
//! (`saved_views.featured_position`). The `start_here` system rail renders
//! that set as reading-list cards; a brand-new account gets the rail pinned
//! at the top of its home page (see `saved_views::ensure_pins_seeded`).
//!
//! Surface:
//!
//!   - `GET  /me/start-here`                  — featured lists for the caller,
//!     with per-user progress and up to three cover issue ids each
//!   - `POST /admin/saved-views/{id}/share`   — admin's own CBL view → shared
//!   - `PUT  /admin/start-here`               — wholesale replace of the
//!     featured set + order (feature, unfeature and reorder in one call)

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use entity::{cbl_list, saved_view};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, Statement, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::saved_views::{KIND_CBL, SavedViewKind};
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

/// `system_key` of the Start Here rail (seeded by `m20271102_000002`).
pub const SYSTEM_KEY_START_HERE: &str = "start_here";

/// Upper bound on the featured set. The rail is a short curated shelf, not
/// a catalog; keeps `PUT /admin/start-here` and the per-list stats loop
/// small.
pub const MAX_FEATURED: usize = 24;

/// Cover thumbnails returned per card (the client fans them).
const COVERS_PER_LIST: i64 = 3;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(share))
        .routes(routes!(set_featured))
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StartHereItemView {
    /// The shared CBL saved view (`/views/{view_id}`).
    pub view_id: String,
    pub cbl_list_id: String,
    pub name: String,
    pub description: Option<String>,
    /// Entries in the list.
    pub total: i64,
    /// Entries matched to an issue the caller can open.
    pub available: i64,
    /// Matched entries the caller has finished.
    pub read_count: i64,
    /// First matched issues the caller can open, in list order — cover art
    /// for the card (`/issues/{id}/pages/0/thumb`).
    pub cover_issue_ids: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StartHereView {
    /// Lists in featured order. Lists with nothing the caller can open are
    /// left out for non-admins (an admin sees every featured list so the
    /// curation surface stays complete).
    pub items: Vec<StartHereItemView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetFeaturedReq {
    /// Shared CBL view ids, in the order they should appear. Views not
    /// listed are unfeatured. At most [`MAX_FEATURED`].
    pub view_ids: Vec<String>,
}

// ───── handlers ─────

#[utoipa::path(
    operation_id = "start_here_list", get,
    path = "/me/start-here",
    responses((status = 200, body = StartHereView))
)]
#[handler]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    match build(&app, &user).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "start_here: list failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[derive(FromQueryResult)]
struct CountRow {
    available: i64,
    read_count: i64,
}

#[derive(FromQueryResult)]
struct CoverRow {
    issue_id: String,
}

async fn build(app: &AppState, user: &CurrentUser) -> Result<StartHereView, sea_orm::DbErr> {
    let featured = featured_views(&app.db).await?;
    if featured.is_empty() {
        return Ok(StartHereView { items: vec![] });
    }
    let acl = access::for_user(app, user).await;
    let is_admin = user.role == entity::user::UserRole::Admin;
    let backend = app.db.get_database_backend();
    let mut items = Vec::with_capacity(featured.len());
    for (view, list) in featured {
        // Counts over entries matched to an issue the caller can open.
        let mut params: Vec<sea_orm::Value> = vec![list.id.into(), user.id.into()];
        let vis = access::visible_issues_sql(&acl, "i", &mut params);
        let counts = CountRow::find_by_statement(Statement::from_sql_and_values(
            backend,
            format!(
                "SELECT COUNT(*)::bigint AS available, \
                        COUNT(*) FILTER (WHERE p.finished)::bigint AS read_count \
                 FROM cbl_entries e \
                 JOIN issues i ON i.id = e.matched_issue_id \
                 LEFT JOIN progress_records p ON p.issue_id = i.id AND p.user_id = $2 \
                 WHERE e.cbl_list_id = $1{vis}"
            ),
            params,
        ))
        .one(&app.db)
        .await?
        .unwrap_or(CountRow {
            available: 0,
            read_count: 0,
        });
        if counts.available == 0 && !is_admin {
            continue;
        }
        let mut params: Vec<sea_orm::Value> = vec![list.id.into(), COVERS_PER_LIST.into()];
        let vis = access::visible_issues_sql(&acl, "i", &mut params);
        let covers = CoverRow::find_by_statement(Statement::from_sql_and_values(
            backend,
            format!(
                "SELECT i.id AS issue_id FROM cbl_entries e \
                 JOIN issues i ON i.id = e.matched_issue_id \
                 WHERE e.cbl_list_id = $1 AND i.state = 'active'{vis} \
                 ORDER BY e.position ASC LIMIT $2"
            ),
            params,
        ))
        .all(&app.db)
        .await?;
        let total = entity::cbl_entry::Entity::find()
            .filter(entity::cbl_entry::Column::CblListId.eq(list.id))
            .count(&app.db)
            .await? as i64;
        items.push(StartHereItemView {
            view_id: view.id.to_string(),
            cbl_list_id: list.id.to_string(),
            name: view.name.clone(),
            description: view.description.clone().or(list.description.clone()),
            total,
            available: counts.available,
            read_count: counts.read_count,
            cover_issue_ids: covers.into_iter().map(|c| c.issue_id).collect(),
        });
    }
    Ok(StartHereView { items })
}

/// Featured shared CBL views with their lists, in featured order.
async fn featured_views(
    db: &impl ConnectionTrait,
) -> Result<Vec<(saved_view::Model, cbl_list::Model)>, sea_orm::DbErr> {
    let views = saved_view::Entity::find()
        .filter(saved_view::Column::FeaturedPosition.is_not_null())
        .filter(saved_view::Column::UserId.is_null())
        .filter(saved_view::Column::Kind.eq(KIND_CBL))
        .order_by_asc(saved_view::Column::FeaturedPosition)
        .order_by_asc(saved_view::Column::Id)
        .all(db)
        .await?;
    let list_ids: Vec<Uuid> = views.iter().filter_map(|v| v.cbl_list_id).collect();
    let lists: HashMap<Uuid, cbl_list::Model> = cbl_list::Entity::find()
        .filter(cbl_list::Column::Id.is_in(list_ids))
        .filter(cbl_list::Column::OwnerUserId.is_null())
        .all(db)
        .await?
        .into_iter()
        .map(|l| (l.id, l))
        .collect();
    Ok(views
        .into_iter()
        .filter_map(|v| {
            let list = lists.get(&v.cbl_list_id?)?.clone();
            Some((v, list))
        })
        .collect())
}

#[utoipa::path(
    operation_id = "start_here_share", post,
    path = "/admin/saved-views/{id}/share",
    params(("id" = String, Path,)),
    responses(
        (status = 204, description = "the reading list is now shared with every user"),
        (status = 403, description = "admin only; the view must be the caller's own reading list"),
        (status = 404, description = "view not found"),
    )
)]
#[handler]
pub async fn share(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let view = match saved_view::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "view not found"),
        Err(e) => {
            tracing::error!(error = %e, "start_here: share lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if view.kind != SavedViewKind::Cbl || view.user_id != Some(user.id) {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only your own reading lists can be shared",
        );
    }
    let Some(list_id) = view.cbl_list_id else {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "view has no reading list",
        );
    };
    let result = app
        .db
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                let now = Utc::now().fixed_offset();
                // The list first: a shared view over a still-private list
                // would be invisible to everyone else.
                cbl_list::Entity::update_many()
                    .col_expr(
                        cbl_list::Column::OwnerUserId,
                        Expr::value(Option::<Uuid>::None),
                    )
                    .col_expr(cbl_list::Column::UpdatedAt, Expr::value(now))
                    .filter(cbl_list::Column::Id.eq(list_id))
                    .exec(txn)
                    .await?;
                saved_view::Entity::update_many()
                    .col_expr(
                        saved_view::Column::UserId,
                        Expr::value(Option::<Uuid>::None),
                    )
                    .col_expr(saved_view::Column::UpdatedAt, Expr::value(now))
                    .filter(saved_view::Column::Id.eq(id))
                    .exec(txn)
                    .await?;
                Ok(())
            })
        })
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, "start_here: share failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.saved_view.share",
            target_type: Some("saved_view"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({ "cbl_list_id": list_id.to_string() }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "start_here_set_featured", put,
    path = "/admin/start-here",
    request_body = SetFeaturedReq,
    responses(
        (status = 200, body = StartHereView),
        (status = 403, description = "admin only"),
        (status = 422, description = "unknown id, duplicate, not a shared reading list, or too many"),
    )
)]
#[handler]
pub async fn set_featured(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<SetFeaturedReq>,
) -> impl IntoResponse {
    if req.view_ids.len() > MAX_FEATURED {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.view_ids",
            &format!("at most {MAX_FEATURED} featured lists"),
        );
    }
    let mut ids: Vec<Uuid> = Vec::with_capacity(req.view_ids.len());
    let mut seen = HashSet::new();
    for raw in &req.view_ids {
        let Ok(id) = Uuid::parse_str(raw) else {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.view_ids",
                "invalid view id",
            );
        };
        if !seen.insert(id) {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.view_ids",
                "duplicate view id",
            );
        }
        ids.push(id);
    }
    // Every id must be a shared CBL view (the DB CHECK enforces the same,
    // but a 422 beats a 500).
    let shared: HashSet<Uuid> = match saved_view::Entity::find()
        .filter(saved_view::Column::Id.is_in(ids.clone()))
        .filter(saved_view::Column::UserId.is_null())
        .filter(saved_view::Column::Kind.eq(KIND_CBL))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|v| v.id).collect(),
        Err(e) => {
            tracing::error!(error = %e, "start_here: featured lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if ids.iter().any(|id| !shared.contains(id)) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.view_ids",
            "only shared reading lists can be featured",
        );
    }
    let ordered = ids.clone();
    let result = app
        .db
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                saved_view::Entity::update_many()
                    .col_expr(
                        saved_view::Column::FeaturedPosition,
                        Expr::value(Option::<i32>::None),
                    )
                    .filter(saved_view::Column::FeaturedPosition.is_not_null())
                    .exec(txn)
                    .await?;
                for (pos, id) in ordered.iter().enumerate() {
                    let am = saved_view::ActiveModel {
                        id: sea_orm::ActiveValue::Unchanged(*id),
                        featured_position: Set(Some(pos as i32)),
                        ..Default::default()
                    };
                    am.update(txn).await?;
                }
                Ok(())
            })
        })
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e, "start_here: set featured failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.start_here.set",
            target_type: None,
            target_id: None,
            payload: serde_json::json!({
                "view_ids": ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    match build(&app, &user).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "start_here: rebuild failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}
