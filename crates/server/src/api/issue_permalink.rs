//! `GET /issues/{id}` — id-based permalink that redirects to the canonical
//! `/series/{series_slug}/issues/{issue_slug}` page (audit UX-3).
//!
//! Admin surfaces (scan runs, activity timeline, health rows) often hold
//! only an issue id — the content-hash PK — without the two slugs the HTML
//! route needs. This tiny bare-group handler turns that id into a 303 so
//! those surfaces can link `/issues/{id}` directly. ACL mirrors the
//! page-byte path: non-visible issues 404 (never 403) so ids don't leak
//! existence.

use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use entity::{issue, library_user_access, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(redirect_to_canonical))
}

#[utoipa::path(
    operation_id = "issue_permalink",    get,
    path = "/issues/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 303, description = "redirect to the canonical issue URL"),
        (status = 404, description = "issue not found or not visible"),
    )
)]
#[handler]
pub async fn redirect_to_canonical(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<String>,
) -> Response {
    let Ok(Some(row)) = issue::Entity::find_by_id(id).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    if !visible(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let Ok(Some(parent)) = series::Entity::find_by_id(row.series_id).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    // Slugs are URL-safe by construction (`entity::slug::allocate_slug`).
    Redirect::to(&format!("/series/{}/issues/{}", parent.slug, row.slug)).into_response()
}

async fn visible(app: &AppState, user: &CurrentUser, lib_id: uuid::Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}
