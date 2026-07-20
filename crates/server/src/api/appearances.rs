//! `…/appearances` — reverse lookup of the reading lists, collections, and
//! story arcs that a given issue or series shows up in.
//!
//! Two read-only endpoints back the "Appears in" tab on the issue and series
//! detail pages: as a user browses an issue/series organically, the tab
//! surfaces the other reading lists, collections, and arcs it belongs to so
//! they can jump straight to them.
//!
//! Membership is **explicit** (no filter-DSL resolution): CBL entries
//! (`cbl_entries.matched_issue_id`), collection entries
//! (`collection_entries`), and the arc junctions (`issue_arcs` /
//! `series_arcs`). Reading lists and collections are scoped to the
//! requesting user's own lists; story arcs are shared metadata. Series-level
//! arc membership is rolled up from `issue_arcs` (the `series_arcs` rollup is
//! not always populated).

use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::{library_user_access, series};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement, Value,
};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(issue_appearances))
        .routes(routes!(series_appearances))
}

/// One container (reading list / collection / story arc) that an issue or
/// series belongs to.
#[derive(Serialize, ToSchema)]
pub struct AppearanceView {
    /// `"cbl"` | `"collection"` | `"arc"`.
    pub kind: String,
    /// Link-target id. For `cbl`/`collection` this is the **saved-view** id
    /// (open at `/views/{id}`); for `arc` it's the arc slug (no detail route
    /// yet — the web app renders arcs as informational chips).
    pub id: String,
    pub name: String,
    /// The issue's reading-order position within a `cbl`/`arc`, when the
    /// container carries one. Only populated by the issue endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// How many of the series' issues appear in this container. Only
    /// populated by the series endpoint (0 when a series was added to a
    /// collection wholesale rather than issue-by-issue).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_count: Option<i64>,
}

/// Grouped membership for one issue or series.
#[derive(Serialize, ToSchema)]
pub struct AppearancesView {
    pub reading_lists: Vec<AppearanceView>,
    pub collections: Vec<AppearanceView>,
    pub arcs: Vec<AppearanceView>,
}

/// Same admin-OR-ACL gate the issue/series read endpoints use. Duplicated
/// per-module to match the existing convention (issues.rs / series.rs /
/// ratings.rs each carry their own private copy).
async fn visible_in_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
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

/// Run a statement that selects exactly `id` (text), `name` (text),
/// `position` (int, nullable) and `issue_count` (bigint, nullable) and map
/// each row to an [`AppearanceView`] tagged with `kind`.
async fn collect(
    app: &AppState,
    kind: &str,
    stmt: Statement,
) -> Result<Vec<AppearanceView>, sea_orm::DbErr> {
    let rows = app.db.query_all_raw(stmt).await?;
    rows.into_iter()
        .map(|r| {
            Ok(AppearanceView {
                kind: kind.to_owned(),
                id: r.try_get::<String>("", "id")?,
                name: r.try_get::<String>("", "name")?,
                position: r.try_get::<Option<i32>>("", "position")?,
                issue_count: r.try_get::<Option<i64>>("", "issue_count")?,
            })
        })
        .collect()
}

fn pg(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

// ───────────────────────── issue ─────────────────────────

const ISSUE_CBLS: &str = "\
    SELECT sv.id::text AS id, cl.parsed_name AS name, \
           ce.position AS position, NULL::bigint AS issue_count \
    FROM cbl_entries ce \
    JOIN cbl_lists cl ON cl.id = ce.cbl_list_id \
    JOIN saved_views sv ON sv.cbl_list_id = cl.id AND sv.kind = 'cbl' \
    WHERE ce.matched_issue_id = $1 AND sv.user_id = $2 \
    ORDER BY cl.parsed_name";

const ISSUE_COLLECTIONS: &str = "\
    SELECT sv.id::text AS id, sv.name AS name, \
           NULL::int AS position, NULL::bigint AS issue_count \
    FROM collection_entries coe \
    JOIN saved_views sv ON sv.id = coe.saved_view_id \
    WHERE sv.kind = 'collection' AND sv.user_id = $1 \
      AND (coe.issue_id = $2 OR (coe.entry_kind = 'series' AND coe.series_id = $3)) \
    GROUP BY sv.id, sv.name \
    ORDER BY sv.name";

const ISSUE_ARCS: &str = "\
    SELECT sa.slug AS id, sa.name AS name, \
           ia.position_in_arc AS position, NULL::bigint AS issue_count \
    FROM issue_arcs ia \
    JOIN story_arc sa ON sa.id = ia.arc_id \
    WHERE ia.issue_id = $1 \
    ORDER BY sa.name";

#[utoipa::path(
    operation_id = "issue_appearances",
    get,
    path = "/series/{series_slug}/issues/{issue_slug}/appearances",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = AppearancesView),
        (status = 404),
    )
)]
#[handler]
pub async fn issue_appearances(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let iid: Value = row.id.clone().into();
    let uid: Value = user.id.into();
    let sid: Value = row.series_id.into();

    let reading_lists = collect(&app, "cbl", pg(ISSUE_CBLS, vec![iid.clone(), uid.clone()])).await;
    let collections = collect(
        &app,
        "collection",
        pg(ISSUE_COLLECTIONS, vec![uid, iid.clone(), sid]),
    )
    .await;
    let arcs = collect(&app, "arc", pg(ISSUE_ARCS, vec![iid])).await;

    respond(reading_lists, collections, arcs)
}

// ───────────────────────── series ─────────────────────────

const SERIES_CBLS: &str = "\
    SELECT sv.id::text AS id, cl.parsed_name AS name, \
           NULL::int AS position, count(DISTINCT ce.matched_issue_id)::bigint AS issue_count \
    FROM cbl_entries ce \
    JOIN cbl_lists cl ON cl.id = ce.cbl_list_id \
    JOIN saved_views sv ON sv.cbl_list_id = cl.id AND sv.kind = 'cbl' \
    JOIN issues i ON i.id = ce.matched_issue_id \
    WHERE i.series_id = $1 AND sv.user_id = $2 \
    GROUP BY sv.id, cl.parsed_name \
    ORDER BY cl.parsed_name";

const SERIES_COLLECTIONS: &str = "\
    SELECT sv.id::text AS id, sv.name AS name, NULL::int AS position, \
           count(DISTINCT i2.id)::bigint AS issue_count \
    FROM saved_views sv \
    JOIN collection_entries coe ON coe.saved_view_id = sv.id \
    LEFT JOIN issues i2 ON i2.id = coe.issue_id AND i2.series_id = $2 AND coe.entry_kind = 'issue' \
    WHERE sv.kind = 'collection' AND sv.user_id = $1 \
      AND ((coe.entry_kind = 'series' AND coe.series_id = $2) \
           OR (coe.entry_kind = 'issue' AND i2.id IS NOT NULL)) \
    GROUP BY sv.id, sv.name \
    ORDER BY sv.name";

const SERIES_ARCS: &str = "\
    SELECT sa.slug AS id, sa.name AS name, NULL::int AS position, \
           count(DISTINCT ia.issue_id)::bigint AS issue_count \
    FROM issue_arcs ia \
    JOIN issues i ON i.id = ia.issue_id \
    JOIN story_arc sa ON sa.id = ia.arc_id \
    WHERE i.series_id = $1 \
    GROUP BY sa.slug, sa.name \
    ORDER BY sa.name";

#[utoipa::path(
    operation_id = "series_appearances",
    get,
    path = "/series/{series_slug}/appearances",
    params(
        ("series_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = AppearancesView),
        (status = 404),
    )
)]
#[handler]
pub async fn series_appearances(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_slug): AxPath<String>,
) -> impl IntoResponse {
    let s = match series::Entity::find()
        .filter(series::Column::Slug.eq(&series_slug))
        .one(&app.db)
        .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "series not found"),
        Err(e) => {
            tracing::error!(error = %e, series_slug, "series slug lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !visible_in_library(&app, &user, s.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }

    let uid: Value = user.id.into();
    let sid: Value = s.id.into();

    let reading_lists = collect(&app, "cbl", pg(SERIES_CBLS, vec![sid.clone(), uid.clone()])).await;
    let collections = collect(
        &app,
        "collection",
        pg(SERIES_COLLECTIONS, vec![uid, sid.clone()]),
    )
    .await;
    let arcs = collect(&app, "arc", pg(SERIES_ARCS, vec![sid])).await;

    respond(reading_lists, collections, arcs)
}

/// Fold the three sub-query results into the response, surfacing the first DB
/// error as a 500. Keeping the error handling in one place means a transient
/// failure on any one section fails the whole request rather than silently
/// returning a partial "appears in" list (which would read as "not in that
/// list").
fn respond(
    reading_lists: Result<Vec<AppearanceView>, sea_orm::DbErr>,
    collections: Result<Vec<AppearanceView>, sea_orm::DbErr>,
    arcs: Result<Vec<AppearanceView>, sea_orm::DbErr>,
) -> axum::response::Response {
    match (reading_lists, collections, arcs) {
        (Ok(reading_lists), Ok(collections), Ok(arcs)) => axum::Json(AppearancesView {
            reading_lists,
            collections,
            arcs,
        })
        .into_response(),
        (r, c, a) => {
            let err = r.err().or(c.err()).or(a.err());
            tracing::error!(error = ?err, "appearances lookup failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}
