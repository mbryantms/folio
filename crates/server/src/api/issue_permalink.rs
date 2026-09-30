//! `GET /issues/{id}` — id-based permalink that redirects to the canonical
//! `/series/{series_slug}/issues/{issue_slug}` page (audit UX-3).
//!
//! Admin surfaces (scan runs, activity timeline, health rows) often hold
//! only an issue id — the content-hash PK — without the two slugs the HTML
//! route needs. This tiny bare-group handler turns that id into a 303 so
//! those surfaces can link `/issues/{id}` directly. ACL mirrors the
//! page-byte path: non-visible issues 404 (never 403) so ids don't leak
//! existence.
//!
//! `GET /markers/{id}` is the marker counterpart (roadmap WP-5.1): it
//! resolves one of the caller's markers to the reader at the marker's
//! page in peek mode (`/read/{series}/{issue}?page=<n>&peek=1`), so a
//! link copied from a bookmark card or printed in the notes export keeps
//! working after a series or issue slug changes. Markers are private: a
//! marker owned by someone else, or one on an issue the caller can no
//! longer see, 404s the same as a missing id. A request with no session
//! goes to `/sign-in?next=/markers/{id}` so a link opened from a notes
//! app lands back on the page after signing in.

use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use entity::{issue, marker, series};
use sea_orm::EntityTrait;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::auth::extractor::AuthRejection;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(redirect_to_canonical))
        .routes(routes!(marker_permalink))
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
    if !crate::library::access::issue_visible(&app, &user, &row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let Ok(Some(parent)) = series::Entity::find_by_id(row.series_id).one(&app.db).await else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };
    // Slugs are URL-safe by construction (`entity::slug::allocate_slug`).
    Redirect::to(&format!("/series/{}/issues/{}", parent.slug, row.slug)).into_response()
}

#[utoipa::path(
    operation_id = "marker_permalink",    get,
    path = "/markers/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 303, description = "redirect to the reader at the marker's page in peek mode, or to sign-in when there is no session"),
        (status = 404, description = "marker not found, not the caller's, or its issue is not visible"),
    )
)]
#[handler]
pub async fn marker_permalink(
    State(app): State<AppState>,
    user: Result<CurrentUser, AuthRejection>,
    AxPath(id): AxPath<String>,
) -> Response {
    let user = match user {
        Ok(u) => u,
        // No (or an expired) session: bounce through sign-in and come
        // back. The id is echoed only after a UUID parse so the `next`
        // value is always a plain path.
        Err(
            AuthRejection::Missing | AuthRejection::Invalid | AuthRejection::TokenVersionMismatch,
        ) => {
            return match Uuid::parse_str(&id) {
                Ok(id) => Redirect::to(&format!("/sign-in?next=/markers/{id}")).into_response(),
                Err(_) => error(StatusCode::NOT_FOUND, "not_found", "marker not found"),
            };
        }
        Err(other) => return other.into_response(),
    };
    let not_found = || error(StatusCode::NOT_FOUND, "not_found", "marker not found");
    let Ok(id) = Uuid::parse_str(&id) else {
        return not_found();
    };
    let Ok(Some(m)) = marker::Entity::find_by_id(id).one(&app.db).await else {
        return not_found();
    };
    if m.user_id != user.id {
        return not_found();
    }
    let Ok(Some(row)) = issue::Entity::find_by_id(m.issue_id.clone())
        .one(&app.db)
        .await
    else {
        return not_found();
    };
    if !crate::library::access::issue_visible(&app, &user, &row).await {
        return not_found();
    }
    let Ok(Some(parent)) = series::Entity::find_by_id(row.series_id).one(&app.db).await else {
        return not_found();
    };
    Redirect::to(&format!(
        "/read/{}/{}?page={}&peek=1",
        parent.slug, row.slug, m.page_index
    ))
    .into_response()
}
