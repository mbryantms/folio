//! `/arcs` entity landing pages (WP-5.5). Thin `#[utoipa::path]`
//! wrappers over the shared core in [`super::entity_pages`], which owns
//! the membership SQL, library-ACL + age-rating-cap filtering and the
//! keyset cursors.

use axum::{
    Extension,
    extract::{Path as AxPath, Query, State},
    response::Response,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::entity_pages::{
    self as ep, EntityDetailView, EntityInsightsView, EntityKind, EntityListItem, EntityListQuery,
    EntityPageQuery,
};
use super::series::{IssueListView, SeriesListView};
use super::series_relationships::ArcTieInView;
use crate::auth::{CurrentUser, RequireAdmin};
use crate::middleware::RequestContext;
use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;
use shared::pagination::CursorPage;

const KIND: EntityKind = EntityKind::Arc;

pub fn routes() -> OpenApiRouter<AppState> {
    // The `q`-filtered index is a global-search source: per-user `SEARCH`
    // bucket (E3, audit N-A5) in its own sub-router.
    let search = OpenApiRouter::new()
        .routes(routes!(list))
        .route_layer(rate_limit::SEARCH.build_per_user());
    OpenApiRouter::new()
        .merge(search)
        .routes(routes!(get_one))
        .routes(routes!(series))
        .routes(routes!(issues))
        .routes(routes!(refresh_profile))
        .routes(routes!(insights))
        .routes(routes!(tie_ins))
}

/// `GET /arcs` — alphabetical, cursor-paginated browse of every
/// story arc with at least one appearance visible to the caller.
#[utoipa::path(
    operation_id = "arcs_list",    get,
    path = "/arcs",
    params(
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("starts_with" = Option<String>, Query,),
        ("q" = Option<String>, Query,),
    ),
    responses((status = 200, body = CursorPage<EntityListItem>), (status = 429, description = "search rate limit"))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<EntityListQuery>,
) -> Response {
    ep::list_handler(&app, &user, KIND, q).await
}

/// `GET /arcs/{slug}` — landing-page header. 404 when the slug is
/// unknown or the story arc has no appearance the caller can see.
#[utoipa::path(
    operation_id = "arcs_get_one",    get,
    path = "/arcs/{slug}",
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

/// `GET /arcs/{slug}/series` — the story arc's visible series,
/// alphabetical, cursor-paginated; `total` on the first page only.
#[utoipa::path(
    operation_id = "arcs_series",    get,
    path = "/arcs/{slug}/series",
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

/// `GET /arcs/{slug}/issues` — the story arc's visible issues,
/// cursor-paginated in arc reading order. `total` on the first page only.
#[utoipa::path(
    operation_id = "arcs_issues",    get,
    path = "/arcs/{slug}/issues",
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

/// `GET /arcs/{slug}/tie-ins` — series linked to this story arc by a
/// `tie_in_to` relationship (WP-7.5), oldest first, cursor-paginated;
/// `total` on the first page only. 404 when the arc isn't visible to the
/// caller; tying-in series the caller can't see are left out.
#[utoipa::path(
    operation_id = "arcs_tie_ins",    get,
    path = "/arcs/{slug}/tie-ins",
    params(
        ("slug" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses(
        (status = 200, body = CursorPage<ArcTieInView>),
        (status = 400, description = "invalid cursor"),
        (status = 404,),
    )
)]
#[handler]
pub async fn tie_ins(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<EntityPageQuery>,
) -> Response {
    super::series_relationships::arc_tie_ins_handler(
        &app,
        &user,
        &slug,
        q.cursor.as_deref(),
        q.limit,
    )
    .await
}

/// `POST /arcs/{slug}/profile/refresh` — queue a fresh provider fetch
/// of the story arc's profile. Admin only (it spends provider budget).
#[utoipa::path(
    operation_id = "arcs_refresh_profile",    post,
    path = "/arcs/{slug}/profile/refresh",
    params(("slug" = String, Path,)),
    responses(
        (status = 202, description = "queued"),
        (status = 403, description = "admin only"),
        (status = 404,),
        (status = 503, description = "no provider configured"),
    )
)]
#[handler]
pub async fn refresh_profile(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> Response {
    ep::refresh_profile_handler(&app, &actor, &ctx, KIND, &slug).await
}

/// `GET /arcs/{slug}/insights` — the story arc's "In your library"
/// statistics (counts, decades, top series, co-appearances, creators,
/// earliest owned issues), ACL-filtered like the grids.
#[utoipa::path(
    operation_id = "arcs_insights",    get,
    path = "/arcs/{slug}/insights",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = EntityInsightsView),
        (status = 404,),
    )
)]
#[handler]
pub async fn insights(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> Response {
    ep::insights_handler(&app, &user, KIND, &slug).await
}
