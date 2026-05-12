//! Filter-builder option lookups (saved-views M5).
//!
//! Tiny GETs that return distinct values from the metadata junction
//! tables for each filter field that backs a multi-select editor. Used
//! by the client's `MultiSelectEditor` to populate combobox suggestions
//! without round-tripping through the full saved-view compiler.
//!
//! Scoped per library when `?library=<id>` is supplied; falls back to
//! every library the caller can see otherwise. Optional `?q=<prefix>`
//! filters case-insensitively. Capped at 200 rows so the client can
//! cache cheaply.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

const MAX_OPTIONS: i64 = 200;

const CREDIT_ROLES: &[&str] = &[
    "writer",
    "penciller",
    "inker",
    "colorist",
    "letterer",
    "cover_artist",
    "editor",
    "translator",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/filter-options/genres", get(genres))
        .route("/filter-options/tags", get(tags))
        .route("/filter-options/credits/{role}", get(credits))
        .route("/filter-options/publishers", get(publishers))
        .route("/filter-options/languages", get(languages))
        .route("/filter-options/age_ratings", get(age_ratings))
        .route("/filter-options/characters", get(characters))
        .route("/filter-options/teams", get(teams))
        .route("/filter-options/locations", get(locations))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OptionsQuery {
    /// Restrict to a single library. When omitted, every library the
    /// caller can see contributes.
    #[serde(default)]
    pub library: Option<Uuid>,
    /// Case-insensitive prefix filter.
    #[serde(default)]
    pub q: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OptionsView {
    pub values: Vec<String>,
}

#[derive(Debug, FromQueryResult)]
struct ValueRow {
    value: String,
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
    path = "/filter-options/genres",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn genres(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct(&app, &user, "series_genres", "genre", None, q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/tags",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn tags(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct(&app, &user, "series_tags", "tag", None, q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/publishers",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn publishers(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "publisher", q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/languages",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn languages(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "language_code", q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/age_ratings",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn age_ratings(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "age_rating", q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/characters",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn characters(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "characters", q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/teams",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn teams(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "teams", q).await
}

#[utoipa::path(
    get,
    path = "/filter-options/locations",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = OptionsView))
)]
pub async fn locations(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "locations", q).await
}

/// Distinct values pulled out of a CSV column on the `issues` table
/// (`characters`, `teams`, `locations`). Splits on `[,;]` and trims
/// each piece — same shape as `fn aggregate_csv` in the series API,
/// so the picker surfaces the same strings the user sees as chips.
/// Output is the original casing of the first occurrence (case-
/// insensitive dedup matches the aggregator).
async fn fetch_distinct_issue_csv(
    app: &AppState,
    user: &CurrentUser,
    column: &'static str,
    q: OptionsQuery,
) -> axum::response::Response {
    if !matches!(column, "characters" | "teams" | "locations") {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "unsupported column",
        );
    }
    let visible = access::for_user(app, user).await;
    // We pick `min(trim(piece))` as the display value so the result
    // is deterministic across runs — alphabetically-first casing wins
    // when an input mixes "Spider-Man" and "spider-man".
    let mut sql = format!(
        "SELECT min(trim(piece)) AS value
           FROM issues i
           JOIN series s ON s.id = i.series_id
           CROSS JOIN LATERAL unnest( \
             regexp_split_to_array(coalesce(i.{column}, ''), '[,;]') \
           ) AS piece
          WHERE i.removed_at IS NULL
            AND i.state = 'active'
            AND s.removed_at IS NULL
            AND trim(piece) <> ''"
    );
    let mut params: Vec<Value> = Vec::new();

    if let Some(lib) = q.library {
        if !visible.contains(lib) {
            return error(StatusCode::FORBIDDEN, "forbidden", "library not visible");
        }
        params.push(Value::from(lib));
        sql.push_str(&format!(" AND s.library_id = ${}", params.len()));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(OptionsView { values: vec![] }).into_response();
        }
        let placeholders: Vec<String> = visible
            .allowed
            .iter()
            .map(|id| {
                params.push(Value::from(*id));
                format!("${}", params.len())
            })
            .collect();
        sql.push_str(&format!(
            " AND s.library_id IN ({})",
            placeholders.join(",")
        ));
    }

    if let Some(prefix) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        params.push(Value::from(format!("{}%", prefix.to_lowercase())));
        sql.push_str(&format!(" AND lower(trim(piece)) LIKE ${}", params.len()));
    }

    sql.push_str(" GROUP BY lower(trim(piece))");
    sql.push_str(&format!(" ORDER BY value ASC LIMIT {MAX_OPTIONS}"));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(OptionsView {
            values: rows.into_iter().map(|r| r.value).collect(),
        })
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
    }
}

/// Shared SQL builder for distinct values pulled from a single column
/// on the `series` table (`publisher`, `language_code`, `age_rating`).
/// Excludes NULL and empty rows, applies the library-ACL gate, and
/// honours the optional case-insensitive prefix filter.
async fn fetch_distinct_series_column(
    app: &AppState,
    user: &CurrentUser,
    column: &'static str,
    q: OptionsQuery,
) -> axum::response::Response {
    // The column name is a hard-coded `'static` chosen by callers, so
    // formatting it into the SQL is safe — no user input ever reaches
    // this string. Defensive belt: only allow the known list.
    if !matches!(column, "publisher" | "language_code" | "age_rating") {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "unsupported column",
        );
    }
    let visible = access::for_user(app, user).await;
    let mut sql = format!(
        "SELECT DISTINCT {column} AS value
           FROM series
          WHERE removed_at IS NULL
            AND {column} IS NOT NULL
            AND {column} <> ''"
    );
    let mut params: Vec<Value> = Vec::new();

    if let Some(lib) = q.library {
        if !visible.contains(lib) {
            return error(StatusCode::FORBIDDEN, "forbidden", "library not visible");
        }
        params.push(Value::from(lib));
        sql.push_str(&format!(" AND library_id = ${}", params.len()));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(OptionsView { values: vec![] }).into_response();
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

    if let Some(prefix) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        params.push(Value::from(format!("{}%", prefix.to_lowercase())));
        sql.push_str(&format!(" AND lower({column}) LIKE ${}", params.len()));
    }

    sql.push_str(&format!(" ORDER BY value ASC LIMIT {MAX_OPTIONS}"));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(OptionsView {
            values: rows.into_iter().map(|r| r.value).collect(),
        })
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/filter-options/credits/{role}",
    params(("role" = String, Path,), ("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses(
        (status = 200, body = OptionsView),
        (status = 400, description = "unknown credit role"),
    )
)]
pub async fn credits(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(role): AxPath<String>,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    if !CREDIT_ROLES.contains(&role.as_str()) {
        return error(StatusCode::BAD_REQUEST, "validation", "unknown credit role");
    }
    fetch_distinct(&app, &user, "series_credits", "person", Some(role), q).await
}

async fn fetch_distinct(
    app: &AppState,
    user: &CurrentUser,
    junction_table: &'static str,
    value_col: &'static str,
    role_filter: Option<String>,
    q: OptionsQuery,
) -> axum::response::Response {
    let visible = access::for_user(app, user).await;

    let mut sql = format!(
        "SELECT DISTINCT j.{value_col} AS value
           FROM {junction_table} j
           JOIN series s ON s.id = j.series_id
          WHERE s.removed_at IS NULL"
    );
    let mut params: Vec<Value> = Vec::new();

    if let Some(role) = role_filter.as_deref() {
        params.push(Value::from(role));
        sql.push_str(&format!(" AND j.role = ${}", params.len()));
    }

    if let Some(lib) = q.library {
        if !visible.contains(lib) {
            return error(StatusCode::FORBIDDEN, "forbidden", "library not visible");
        }
        params.push(Value::from(lib));
        sql.push_str(&format!(" AND s.library_id = ${}", params.len()));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(OptionsView { values: vec![] }).into_response();
        }
        let placeholders: Vec<String> = visible
            .allowed
            .iter()
            .map(|id| {
                params.push(Value::from(*id));
                format!("${}", params.len())
            })
            .collect();
        sql.push_str(&format!(
            " AND s.library_id IN ({})",
            placeholders.join(",")
        ));
    }

    if let Some(prefix) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        params.push(Value::from(format!("{}%", prefix.to_lowercase())));
        sql.push_str(&format!(" AND lower(j.{value_col}) LIKE ${}", params.len()));
    }

    sql.push_str(&format!(" ORDER BY value ASC LIMIT {MAX_OPTIONS}"));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(OptionsView {
            values: rows.into_iter().map(|r| r.value).collect(),
        })
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "db", &e.to_string()),
    }
}
