//! Cover art for reading-list and collection cards (cover-stacks PR B).
//!
//! `GET /me/view-covers` returns up to [`COVERS_PER_VIEW`] cover thumbnail
//! URLs for every reading list (`kind = 'cbl'`, own + shared) and
//! collection (own) the caller can see, in list order. The Collections
//! index and Settings → Views draw them as a fanned cover stack.
//!
//! - Reading lists: the first matched entries, in `position` order.
//! - Collections: the first entries in `position` order; an issue entry is
//!   its own cover, a series entry uses the series cover
//!   ([`crate::api::series::series_cover_issue_ids`]) so the stack matches
//!   the series card.
//!
//! Every entry passes the caller's library grants + age cap, so a stack
//! never shows a cover the caller could not open. Two queries for the whole
//! page (plus one series-cover lookup), not one per card.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use sea_orm::{FromQueryResult, Statement, Value};
use serde::Serialize;
use std::collections::BTreeMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

/// Covers per card: the front cover plus up to three behind it.
pub const COVERS_PER_VIEW: i64 = 4;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ViewCoverItem {
    pub view_id: String,
    /// Front cover first. Empty when nothing in the view is readable by
    /// the caller (the card then shows a placeholder stack).
    pub cover_urls: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ViewCoversView {
    pub items: Vec<ViewCoverItem>,
}

#[derive(FromQueryResult)]
struct CblRow {
    view_id: String,
    issue_id: String,
}

#[derive(FromQueryResult)]
struct CollectionRow {
    view_id: String,
    issue_id: Option<String>,
    series_id: Option<Uuid>,
}

fn thumb(issue_id: &str) -> String {
    format!("/issues/{issue_id}/pages/0/thumb")
}

#[utoipa::path(
    operation_id = "view_covers_list", get,
    path = "/me/view-covers",
    responses((status = 200, body = ViewCoversView))
)]
#[handler]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    match build(&app, &user).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "view_covers: list failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

async fn build(app: &AppState, user: &CurrentUser) -> Result<ViewCoversView, sea_orm::DbErr> {
    let acl = access::for_user(app, user).await;
    let backend = app.db.get_database_backend();
    let mut covers: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Reading lists: own + shared, first matched + readable entries.
    let mut params: Vec<Value> = vec![user.id.into(), COVERS_PER_VIEW.into()];
    let vis = access::visible_issues_sql(&acl, "i", &mut params);
    let cbl = CblRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        format!(
            "WITH ranked AS ( \
               SELECT v.id::text AS view_id, i.id AS issue_id, \
                      row_number() OVER (PARTITION BY v.id ORDER BY e.position, e.id) AS rn \
               FROM saved_views v \
               JOIN cbl_entries e ON e.cbl_list_id = v.cbl_list_id \
               JOIN issues i ON i.id = e.matched_issue_id \
               JOIN cbl_lists l ON l.id = v.cbl_list_id \
               WHERE v.kind = 'cbl' AND (v.user_id = $1 OR v.user_id IS NULL) \
                 AND {usable} \
                 AND i.state = 'active'{vis} \
             ) \
             SELECT view_id, issue_id FROM ranked WHERE rn <= $2 ORDER BY view_id, rn",
            usable = crate::api::saved_views::cbl_list_usable_sql("l", "$1"),
        ),
        params,
    ))
    .all(&app.db)
    .await?;
    for r in cbl {
        covers
            .entry(r.view_id)
            .or_default()
            .push(thumb(&r.issue_id));
    }

    // Collections: own, first readable entries (issues or series).
    let mut params: Vec<Value> = vec![user.id.into(), COVERS_PER_VIEW.into()];
    let vis_i = access::visible_issues_sql(&acl, "i", &mut params);
    let vis_s = access::visible_series_sql(&acl, "s", &mut params);
    let coll = CollectionRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        format!(
            "WITH ranked AS ( \
               SELECT v.id::text AS view_id, ce.issue_id, ce.series_id, \
                      row_number() OVER (PARTITION BY v.id ORDER BY ce.position, ce.id) AS rn \
               FROM saved_views v \
               JOIN collection_entries ce ON ce.saved_view_id = v.id \
               LEFT JOIN issues i ON i.id = ce.issue_id \
               LEFT JOIN series s ON s.id = ce.series_id \
               WHERE v.kind = 'collection' AND v.user_id = $1 \
                 AND ((ce.issue_id IS NOT NULL AND i.state = 'active'{vis_i}) \
                      OR (ce.series_id IS NOT NULL{vis_s})) \
             ) \
             SELECT view_id, issue_id, series_id FROM ranked WHERE rn <= $2 ORDER BY view_id, rn"
        ),
        params,
    ))
    .all(&app.db)
    .await?;
    let series_ids: Vec<Uuid> = coll.iter().filter_map(|r| r.series_id).collect();
    let series_covers =
        crate::api::series::series_cover_issue_ids(&app.db, &series_ids, &acl).await;
    for r in coll {
        let url = match (&r.issue_id, r.series_id) {
            (Some(issue_id), _) => Some(thumb(issue_id)),
            (None, Some(series_id)) => series_covers.get(&series_id).map(|id| thumb(id)),
            _ => None,
        };
        if let Some(url) = url {
            covers.entry(r.view_id).or_default().push(url);
        }
    }

    Ok(ViewCoversView {
        items: covers
            .into_iter()
            .map(|(view_id, cover_urls)| ViewCoverItem {
                view_id,
                cover_urls,
            })
            .collect(),
    })
}
