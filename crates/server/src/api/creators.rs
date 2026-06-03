//! Creator detail page — M8 of the search-improvements plan.
//!
//! `GET /creators/{slug}` returns the named creator plus per-role
//! series rails so a single page can show every series they touched,
//! regardless of which role they held on each. Replaces the previous
//! search-modal click-through that landed on `/?library=all&credits=…`
//! with a real entity page.
//!
//! ACL: library-scoped exactly like `/people` search — the SQL
//! filters credits by visible library so a hidden library's series
//! never leaks.

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, Statement, Value,
};
use serde::Serialize;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::series::{SeriesView, hydrate_series};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use entity::{person, series};
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_one))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatorRoleRail {
    /// Canonical role name as stored on the credit rows (e.g. `"writer"`,
    /// `"penciller"`, `"cover_artist"`).
    pub role: String,
    /// Series the creator touched in this role, hydrated with the same
    /// fields the library grid renders so the page can show full cards.
    pub series: Vec<SeriesView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatorDetailView {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// Distinct roles the creator holds across visible credits, ordered
    /// canonically (writer → penciller → inker → colorist → letterer →
    /// cover_artist → editor → translator → anything else alphabetical).
    pub roles: Vec<String>,
    /// Total visible credit count across both junction tables (each
    /// distinct (series_id) or (issue_id) counted once).
    pub credit_count: i64,
    /// Series rails — one per role the creator held. Order matches
    /// `roles`; each rail is sorted by series name. Empty when the
    /// creator only has credits in libraries the caller can't see.
    pub rails: Vec<CreatorRoleRail>,
}

/// Canonical role order. Roles outside this list fall through to
/// alphabetical at the bottom.
const ROLE_ORDER: &[&str] = &[
    "writer",
    "penciller",
    "inker",
    "colorist",
    "letterer",
    "cover_artist",
    "editor",
    "translator",
];

#[derive(Debug, FromQueryResult)]
struct CreditRow {
    series_id: Uuid,
    role: String,
}

#[utoipa::path(
    operation_id = "creators_get_one",    get,
    path = "/creators/{slug}",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = CreatorDetailView),
        (status = 404,),
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match person::Entity::find()
        .filter(person::Column::Slug.eq(&slug))
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return error(StatusCode::NOT_FOUND, "not_found", "creator not found");
        }
        Err(e) => {
            tracing::error!(error = %e, "creators: lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let visible = access::for_user(&app, &user).await;
    let mut params: Vec<Value> = vec![Value::from(row.normalized_name.clone())];
    let library_filter = if visible.unrestricted {
        String::new()
    } else {
        if visible.allowed.is_empty() {
            return Json(CreatorDetailView {
                id: row.id.to_string(),
                slug: row.slug.clone(),
                name: row.name.clone(),
                roles: Vec::new(),
                credit_count: 0,
                rails: Vec::new(),
            })
            .into_response();
        }
        let placeholders: Vec<String> = visible
            .allowed
            .iter()
            .map(|id| {
                params.push(Value::from(*id));
                format!("${}", params.len())
            })
            .collect();
        format!(" AND s.library_id IN ({})", placeholders.join(","))
    };

    // Distinct (series_id, role) pairs across BOTH credit tables.
    // Series-level credits already key on series_id; issue-level
    // credits join through `issues` to reach the series. We dedupe
    // by (series_id, role) so a creator who's "writer" on every issue
    // of a series appears once as writer for that series.
    let sql = format!(
        "WITH credits AS ( \
           SELECT s.id AS series_id, sc.role AS role \
             FROM series_credits sc \
             JOIN series s ON s.id = sc.series_id \
            WHERE s.removed_at IS NULL \
              AND btrim(lower(sc.person)) = $1{library_filter} \
           UNION \
           SELECT s.id AS series_id, ic.role AS role \
             FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             JOIN series s ON s.id = i.series_id \
            WHERE s.removed_at IS NULL \
              AND i.removed_at IS NULL \
              AND i.state = 'active' \
              AND btrim(lower(ic.person)) = $1{library_filter} \
         ) \
         SELECT series_id, role FROM credits",
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    let credit_rows: Vec<CreditRow> = match CreditRow::find_by_statement(stmt).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "creators: credits fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Bucket series IDs by role, preserving insertion-order dedupe.
    let mut by_role: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut all_series_ids: Vec<Uuid> = Vec::new();
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for cr in credit_rows {
        by_role.entry(cr.role).or_default().push(cr.series_id);
        if seen.insert(cr.series_id) {
            all_series_ids.push(cr.series_id);
        }
    }

    let series_rows: Vec<series::Model> = if all_series_ids.is_empty() {
        Vec::new()
    } else {
        match series::Entity::find()
            .filter(series::Column::Id.is_in(all_series_ids.clone()))
            .all(&app.db)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "creators: series hydrate failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };
    let hydrated = hydrate_series(&app, series_rows).await;
    let series_by_id: HashMap<String, SeriesView> =
        hydrated.into_iter().map(|s| (s.id.clone(), s)).collect();

    // Build per-role rails in canonical order.
    let mut role_keys: Vec<String> = by_role.keys().cloned().collect();
    role_keys.sort_by(|a, b| {
        let ai = ROLE_ORDER.iter().position(|r| *r == a);
        let bi = ROLE_ORDER.iter().position(|r| *r == b);
        match (ai, bi) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });

    let rails: Vec<CreatorRoleRail> = role_keys
        .iter()
        .map(|role| {
            let mut ids = by_role.get(role).cloned().unwrap_or_default();
            ids.sort();
            ids.dedup();
            let mut series: Vec<SeriesView> = ids
                .into_iter()
                .filter_map(|id| series_by_id.get(&id.to_string()).cloned())
                .collect();
            series.sort_by_key(|a| a.name.to_lowercase());
            CreatorRoleRail {
                role: role.clone(),
                series,
            }
        })
        .filter(|r| !r.series.is_empty())
        .collect();

    let roles: Vec<String> = rails.iter().map(|r| r.role.clone()).collect();
    let credit_count: i64 = all_series_ids.len() as i64;

    Json(CreatorDetailView {
        id: row.id.to_string(),
        slug: row.slug,
        name: row.name,
        roles,
        credit_count,
        rails,
    })
    .into_response()
}
