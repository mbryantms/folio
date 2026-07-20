//! People search (global-search M4).
//!
//! Distinct creator names aggregated across `series_credits` and
//! `issue_credits`, with role rollup + credit count. Uses the
//! `pg_trgm` GIN index added in
//! `m20261218_000001_people_search.rs` for fuzzy substring + similarity
//! ranking — names like "Jhon" still surface "John …".
//!
//! Library-ACL gated: we join through series so the same per-user
//! visibility rules apply as everywhere else.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::{FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

const MAX_QUERY_LEN: usize = 200;
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PersonHit {
    pub person: String,
    /// Stable URL slug for `/creators/<slug>`. Populated from the
    /// `person` table (M8 of the search-improvements plan). `None`
    /// when the name hasn't been backfilled yet — caller falls back
    /// to the legacy `?library=all&credits=<name>` URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub roles: Vec<String>,
    pub credit_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PeopleListView {
    pub items: Vec<PersonHit>,
}

#[derive(Debug, FromQueryResult)]
struct Row {
    person: String,
    slug: Option<String>,
    roles: Vec<String>,
    credit_count: i64,
}

#[utoipa::path(
    operation_id = "people_list",    get,
    path = "/people",
    params(
        ("q" = Option<String>, Query,),
        ("limit" = Option<i64>, Query,),
    ),
    responses((status = 200, body = PeopleListView))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let text = q.q.as_deref().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return Json(PeopleListView { items: Vec::new() }).into_response();
    }
    if text.len() > MAX_QUERY_LEN {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", "q too long");
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;

    // Param ordering: $1 is the (lowercased) query string used for
    // similarity ranking + ILIKE substring. Subsequent params are
    // the library-allowlist UUIDs when the caller is restricted.
    let mut params: Vec<Value> = vec![Value::from(text.to_lowercase())];

    // Library-allowlist clause appended into the inner CTE when the
    // caller is restricted. Empty allowlist → empty response (no
    // libraries visible means no credits visible).
    let library_filter = if visible.unrestricted {
        String::new()
    } else {
        if visible.allowed.is_empty() {
            return Json(PeopleListView { items: vec![] }).into_response();
        }
        let placeholders: Vec<String> = visible
            .allowed
            .iter()
            .map(|id| {
                params.push(Value::from(*id));
                format!("${}", params.len())
            })
            .collect();
        format!(" AND library_id IN ({})", placeholders.join(","))
    };

    // Aggregate (UNION over both credit tables → distinct names with
    // role + count rollup) inside the `agg` CTE so the outer SELECT
    // can LEFT JOIN the `person` table for slug resolution. Persons
    // missing from the table (scanner inserted a credit since the
    // last backfill) just return `slug = NULL` and the client falls
    // back to the legacy `?library=all&credits=<name>` URL.
    let sql = format!(
        "WITH all_credits AS ( \
           SELECT sc.person AS person, sc.role AS role, \
                  'series:' || sc.series_id::text AS ref_id, \
                  s.library_id AS library_id \
             FROM series_credits sc \
             JOIN series s ON s.id = sc.series_id \
            WHERE s.removed_at IS NULL{library_filter} \
           UNION ALL \
           SELECT ic.person AS person, ic.role AS role, \
                  'issue:' || ic.issue_id AS ref_id, \
                  s.library_id AS library_id \
             FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             JOIN series s ON s.id = i.series_id \
            WHERE s.removed_at IS NULL \
              AND i.removed_at IS NULL \
              AND i.state = 'active'{library_filter} \
         ), \
         agg AS ( \
           SELECT person, \
                  ARRAY_AGG(DISTINCT role ORDER BY role) AS roles, \
                  COUNT(DISTINCT ref_id)::bigint AS credit_count \
             FROM all_credits \
            WHERE (person % $1 OR lower(person) LIKE '%' || $1 || '%') \
            GROUP BY person \
         ) \
         SELECT a.person, a.roles, a.credit_count, p.slug \
           FROM agg a \
           LEFT JOIN person p ON p.normalized_name = btrim(lower(a.person)) \
          ORDER BY similarity(a.person, $1) DESC, a.credit_count DESC, a.person ASC \
          LIMIT {limit}",
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match Row::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(PeopleListView {
            items: rows
                .into_iter()
                .map(|r| PersonHit {
                    person: r.person,
                    slug: r.slug,
                    roles: r.roles,
                    credit_count: r.credit_count,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "people query failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "db", "internal")
        }
    }
}
