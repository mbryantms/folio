//! `/locations` entity landing pages (settings — places issues take place in). Thin `#[utoipa::path]`
//! wrappers over the shared core in [`super::entity_pages`], which owns
//! the membership SQL, library-ACL + age-rating-cap filtering and the
//! keyset cursors.

use axum::{
    extract::{Path as AxPath, Query, State},
    response::Response,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::entity_pages::{
    self as ep, EntityDetailView, EntityKind, EntityListItem, EntityListQuery, EntityPageQuery,
};
use super::series::{IssueListView, SeriesListView};
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;
use shared::pagination::CursorPage;

const KIND: EntityKind = EntityKind::Location;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(get_one))
        .routes(routes!(series))
        .routes(routes!(issues))
}

/// `GET /locations` — alphabetical, cursor-paginated browse of every
/// location with at least one appearance visible to the caller.
#[utoipa::path(
    operation_id = "locations_list",    get,
    path = "/locations",
    params(
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("starts_with" = Option<String>, Query,),
        ("q" = Option<String>, Query,),
    ),
    responses((status = 200, body = CursorPage<EntityListItem>))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<EntityListQuery>,
) -> Response {
    ep::list_handler(&app, &user, KIND, q).await
}

/// `GET /locations/{slug}` — landing-page header. 404 when the slug is
/// unknown or the location has no appearance the caller can see.
#[utoipa::path(
    operation_id = "locations_get_one",    get,
    path = "/locations/{slug}",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = EntityDetailView),
        (status = 404,),
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> Response {
    ep::detail_handler(&app, &user, KIND, &slug).await
}

/// `GET /locations/{slug}/series` — the location's visible series,
/// alphabetical, cursor-paginated; `total` on the first page only.
#[utoipa::path(
    operation_id = "locations_series",    get,
    path = "/locations/{slug}/series",
    params(
        ("slug" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses(
        (status = 200, body = SeriesListView),
        (status = 404,),
    )
)]
#[handler]
pub async fn series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<EntityPageQuery>,
) -> Response {
    ep::series_handler(&app, &user, KIND, &slug, q).await
}

/// `GET /locations/{slug}/issues` — the location's visible issues,
/// cursor-paginated by publication year, series, number. `total` on the first page only.
#[utoipa::path(
    operation_id = "locations_issues",    get,
    path = "/locations/{slug}/issues",
    params(
        ("slug" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses(
        (status = 200, body = IssueListView),
        (status = 404,),
    )
)]
#[handler]
pub async fn issues(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<EntityPageQuery>,
) -> Response {
    ep::issues_handler(&app, &user, KIND, &slug, q).await
}
