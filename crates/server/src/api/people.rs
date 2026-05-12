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
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

const MAX_QUERY_LEN: usize = 200;
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

pub fn routes() -> Router<AppState> {
    Router::new().route("/people", get(list))
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
    roles: Vec<String>,
    credit_count: i64,
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/people",
    params(
        ("q" = Option<String>, Query,),
        ("limit" = Option<i64>, Query,),
    ),
    responses((status = 200, body = PeopleListView))
)]
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
        return error(StatusCode::BAD_REQUEST, "validation", "q too long");
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;

    // We UNION the two credit tables, joining each to series so the
    // library-visibility filter applies uniformly. Issue credits go
    // through `issues -> series` to reach a library_id; series credits
    // join directly.
    //
    // Param ordering: $1 is the (lowercased) query string used for
    // similarity ranking + ILIKE substring. Subsequent params are the
    // library-allowlist UUIDs when the caller is restricted.
    let mut sql = String::from(
        "WITH all_credits AS ( \
           SELECT sc.person AS person, sc.role AS role, \
                  'series:' || sc.series_id::text AS ref_id, \
                  s.library_id AS library_id \
             FROM series_credits sc \
             JOIN series s ON s.id = sc.series_id \
            WHERE s.removed_at IS NULL \
           UNION ALL \
           SELECT ic.person AS person, ic.role AS role, \
                  'issue:' || ic.issue_id AS ref_id, \
                  s.library_id AS library_id \
             FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             JOIN series s ON s.id = i.series_id \
            WHERE s.removed_at IS NULL \
              AND i.removed_at IS NULL \
              AND i.state = 'active' \
         ) \
         SELECT person, \
                ARRAY_AGG(DISTINCT role ORDER BY role) AS roles, \
                COUNT(DISTINCT ref_id)::bigint AS credit_count \
           FROM all_credits \
          WHERE (person % $1 OR lower(person) LIKE '%' || $1 || '%')",
    );
    let mut params: Vec<Value> = vec![Value::from(text.to_lowercase())];

    if !visible.unrestricted {
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
        sql.push_str(&format!(" AND library_id IN ({})", placeholders.join(",")));
    }

    sql.push_str(
        " GROUP BY person \
          ORDER BY similarity(person, $1) DESC, credit_count DESC, person ASC",
    );
    sql.push_str(&format!(" LIMIT {limit}"));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match Row::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(PeopleListView {
            items: rows
                .into_iter()
                .map(|r| PersonHit {
                    person: r.person,
                    roles: r.roles,
                    credit_count: r.credit_count,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
    }
}
