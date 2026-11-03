//! Shared reading lists: an admin turns one of their own reading lists into
//! a **shared** list (`saved_views.user_id` and `cbl_lists.owner_user_id`
//! both NULL), the "system" shape the rest of the app already treats as
//! visible to every user. Shared lists are read-only for non-admins: see
//! `cbl_lists::ensure_can_manage`.
//!
//!   - `POST /admin/saved-views/{id}/share` — admin's own CBL view → shared

use axum::{
    Extension,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use entity::{cbl_list, saved_view};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait, sea_query::Expr};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::saved_views::SavedViewKind;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(share))
}

#[utoipa::path(
    operation_id = "shared_lists_share", post,
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
            tracing::error!(error = %e, "shared_lists: share lookup failed");
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
        tracing::error!(error = %e, "shared_lists: share failed");
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
