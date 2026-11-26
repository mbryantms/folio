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
use entity::{cbl_list, reading_list_link, saved_view};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::Expr,
};
use std::collections::HashSet;
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
        (status = 204, description = "the reading list is now shared with every user, along with the lists linked into it (its events and next parts) that the admin owned"),
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
    // Lists linked into this one (its events, its next part, and theirs)
    // that the admin still owns personally. A link may only point at a
    // list everyone who sees the parent can see, so they are shared with
    // it; left personal, every other reader would see the master with no
    // events.
    let linked = match linked_personal_lists(&app.db, list_id, user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "shared_lists: linked lists lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let linked_ids: Vec<Uuid> = linked.iter().map(|l| l.id).collect();
    let linked_in_txn = linked_ids.clone();
    let owner = user.id;
    let result = app
        .db
        .transaction::<_, (), sea_orm::DbErr>(|txn| {
            Box::pin(async move {
                let now = Utc::now().fixed_offset();
                if !linked_in_txn.is_empty() {
                    cbl_list::Entity::update_many()
                        .col_expr(
                            cbl_list::Column::OwnerUserId,
                            Expr::value(Option::<Uuid>::None),
                        )
                        .col_expr(cbl_list::Column::UpdatedAt, Expr::value(now))
                        .filter(cbl_list::Column::Id.is_in(linked_in_txn.clone()))
                        .filter(cbl_list::Column::OwnerUserId.eq(owner))
                        .exec(txn)
                        .await?;
                    // The admin's own views of them become the shared views.
                    saved_view::Entity::update_many()
                        .col_expr(
                            saved_view::Column::UserId,
                            Expr::value(Option::<Uuid>::None),
                        )
                        .col_expr(saved_view::Column::UpdatedAt, Expr::value(now))
                        .filter(saved_view::Column::Kind.eq(SavedViewKind::Cbl))
                        .filter(saved_view::Column::CblListId.is_in(linked_in_txn))
                        .filter(saved_view::Column::UserId.eq(owner))
                        .exec(txn)
                        .await?;
                }
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
    // A linked list the admin had no view of still needs a shared one, so
    // every reader can open it from its band.
    for mut l in linked {
        l.owner_user_id = None;
        if let Err(e) = super::saved_views::ensure_shared_cbl_view(&app.db, &l).await {
            tracing::warn!(error = %e, list_id = %l.id, "shared_lists: shared view for a linked list failed");
        }
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.saved_view.share",
            target_type: Some("saved_view"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({
                "cbl_list_id": list_id.to_string(),
                "linked_lists_shared": linked_ids
                    .iter()
                    .map(Uuid::to_string)
                    .collect::<Vec<_>>(),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

/// How many links deep [`linked_personal_lists`] follows. Events nest one
/// level and parts chain; this only bounds a malformed graph.
const MAX_LINK_DEPTH: usize = 128;

/// Every list reachable from `list_id` through reading-list links (its
/// events and next part, then theirs) that `owner` owns personally.
async fn linked_personal_lists(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    owner: Uuid,
) -> Result<Vec<cbl_list::Model>, sea_orm::DbErr> {
    let mut seen: HashSet<Uuid> = HashSet::from([list_id]);
    let mut frontier = vec![list_id];
    let mut out = Vec::new();
    for _ in 0..MAX_LINK_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let child_ids: Vec<Uuid> = reading_list_link::Entity::find()
            .select_only()
            .column(reading_list_link::Column::ChildListId)
            .filter(reading_list_link::Column::ParentListId.is_in(frontier))
            .into_tuple::<Uuid>()
            .all(db)
            .await?
            .into_iter()
            .filter(|id| seen.insert(*id))
            .collect();
        if child_ids.is_empty() {
            break;
        }
        let children = cbl_list::Entity::find()
            .filter(cbl_list::Column::Id.is_in(child_ids))
            .filter(cbl_list::Column::OwnerUserId.eq(owner))
            .all(db)
            .await?;
        frontier = children.iter().map(|c| c.id).collect();
        out.extend(children);
    }
    Ok(out)
}
