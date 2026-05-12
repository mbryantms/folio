//! Publisher search (global-search M3).
//!
//! Distinct publishers across the caller's visible libraries with a
//! per-publisher series count, ordered by count desc. Powers the
//! global-search modal + `/search` page's Publishers section.

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
    Router::new().route("/publishers", get(list))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PublisherHit {
    pub publisher: String,
    pub series_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PublisherListView {
    pub items: Vec<PublisherHit>,
}

#[derive(Debug, FromQueryResult)]
struct Row {
    publisher: String,
    series_count: i64,
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
    path = "/publishers",
    params(
        ("q" = Option<String>, Query,),
        ("limit" = Option<i64>, Query,),
    ),
    responses((status = 200, body = PublisherListView))
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let text = q.q.as_deref().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return Json(PublisherListView { items: Vec::new() }).into_response();
    }
    if text.len() > MAX_QUERY_LEN {
        return error(StatusCode::BAD_REQUEST, "validation", "q too long");
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;
    let mut sql = String::from(
        "SELECT publisher, COUNT(*)::bigint AS series_count
           FROM series
          WHERE removed_at IS NULL
            AND publisher IS NOT NULL
            AND publisher <> ''
            AND lower(publisher) LIKE $1",
    );
    let mut params: Vec<Value> = vec![Value::from(format!("%{}%", text.to_lowercase()))];

    if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(PublisherListView { items: vec![] }).into_response();
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

    sql.push_str(" GROUP BY publisher ORDER BY series_count DESC, publisher ASC");
    sql.push_str(&format!(" LIMIT {limit}"));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match Row::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(PublisherListView {
            items: rows
                .into_iter()
                .map(|r| PublisherHit {
                    publisher: r.publisher,
                    series_count: r.series_count,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
    }
}
