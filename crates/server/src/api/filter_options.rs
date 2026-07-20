//! Filter-builder option lookups (saved-views M5).
//!
//! Tiny GETs that return distinct values from the metadata junction
//! tables for each filter field that backs a multi-select editor. Used
//! by the client's `MultiSelectEditor` to populate combobox suggestions
//! without round-tripping through the full saved-view compiler.
//!
//! Scoped per library when `?library=<id>` is supplied; falls back to
//! every library the caller can see otherwise. Optional `?q=<prefix>`
//! filters case-insensitively.
//!
//! Cursor pagination per audit-remediation M4-residual: each endpoint
//! returns `CursorPage<String>` (uniform envelope across the API).
//! Default `limit` is 100; max is 200. Walk via `?cursor=<token>` from
//! the previous response's `next_cursor`. The cursor is an opaque
//! base64-JSON `{after: "<last value seen>"}` — clients should treat
//! it as opaque and round-trip unchanged. Most callers won't paginate;
//! they'll refine via `?q=<prefix>` instead.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::{FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 200;

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

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(genres))
        .routes(routes!(tags))
        .routes(routes!(credits))
        .routes(routes!(publishers))
        .routes(routes!(languages))
        .routes(routes!(age_ratings))
        .routes(routes!(characters))
        .routes(routes!(teams))
        .routes(routes!(locations))
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
    /// Opaque continuation token returned as `next_cursor` on the
    /// previous page. Clients pass it back verbatim.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size. Capped at [`MAX_LIMIT`]; default [`DEFAULT_LIMIT`].
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Cursor body: just the last value we returned. Values are unique per
/// query (DISTINCT) so a single-key cursor is sufficient.
#[derive(Debug, Serialize, serde::Deserialize)]
struct OptionsCursor {
    after: String,
}

#[derive(Debug, FromQueryResult)]
struct ValueRow {
    value: String,
}

/// Decode the `?cursor=` param. `Err(())` means malformed — the caller
/// turns this into a 400; `Ok(None)` means absent (first page).
/// (Returns `()` rather than the full `Response` to keep the `Err`
/// variant small for the `result_large_err` lint.)
fn parse_cursor(raw: Option<&str>) -> Result<Option<String>, ()> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    decode_cursor::<OptionsCursor>(raw)
        .map(|c| Some(c.after))
        .map_err(|_| ())
}

/// Normalize `?limit=` per the documented bounds.
fn parse_limit(raw: Option<i64>) -> i64 {
    raw.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Build a [`CursorPage<String>`] from raw rows + a (limit+1) fetch.
/// If the fetch returned more than `limit` rows, the extra row signals
/// "more available" and is sliced off; `next_cursor` carries the last
/// in-page value.
fn page_from_rows(mut rows: Vec<String>, limit: i64) -> CursorPage<String> {
    let limit = limit as usize;
    let next_cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last()
            .and_then(|v| encode_cursor(&OptionsCursor { after: v.clone() }).ok())
    } else {
        None
    };
    CursorPage::paginated(rows, next_cursor, None)
}

#[utoipa::path(
    operation_id = "filter_options_genres",    get,
    path = "/filter-options/genres",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn genres(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct(&app, &user, "series_genres", "genre", None, q).await
}

#[utoipa::path(
    operation_id = "filter_options_tags",    get,
    path = "/filter-options/tags",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn tags(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct(&app, &user, "series_tags", "tag", None, q).await
}

#[utoipa::path(
    operation_id = "filter_options_publishers",    get,
    path = "/filter-options/publishers",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn publishers(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "publisher", q).await
}

#[utoipa::path(
    operation_id = "filter_options_languages",    get,
    path = "/filter-options/languages",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn languages(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "language_code", q).await
}

#[utoipa::path(
    operation_id = "filter_options_age_ratings",    get,
    path = "/filter-options/age_ratings",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn age_ratings(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_series_column(&app, &user, "age_rating", q).await
}

#[utoipa::path(
    operation_id = "filter_options_characters",    get,
    path = "/filter-options/characters",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn characters(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "characters", q).await
}

#[utoipa::path(
    operation_id = "filter_options_teams",    get,
    path = "/filter-options/teams",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn teams(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "teams", q).await
}

#[utoipa::path(
    operation_id = "filter_options_locations",    get,
    path = "/filter-options/locations",
    params(("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses((status = 200, body = shared::pagination::CursorPage<String>))
)]
#[handler]
pub async fn locations(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    fetch_distinct_issue_csv(&app, &user, "locations", q).await
}

/// Distinct values pulled out of a CSV column on the `issues` table
/// (`characters`, `teams`, `locations`). Mirrors `aggregate_csv` /
/// `split_csv`'s per-value rule: when the column contains `;` we use
/// `;` as the sole separator (so names like `"Capes, Inc."` survive);
/// otherwise we split on `,`.
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
    let after = match parse_cursor(q.cursor.as_deref()) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "bad_cursor", "invalid cursor"),
    };
    let limit = parse_limit(q.limit);
    let visible = access::for_user(app, user).await;
    // We pick `min(trim(piece))` as the display value so the result
    // is deterministic across runs — alphabetically-first casing wins
    // when an input mixes "Spider-Man" and "spider-man".
    let mut sql = format!(
        "SELECT min(trim(piece)) AS value
           FROM issues i
           JOIN series s ON s.id = i.series_id
           CROSS JOIN LATERAL unnest( \
             regexp_split_to_array( \
               coalesce(i.{column}, ''), \
               CASE WHEN coalesce(i.{column}, '') LIKE '%;%' THEN ';' ELSE ',' END \
             ) \
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
            return Json(CursorPage::<String>::bounded(vec![])).into_response();
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

    // Cursor filter applies to the aggregated `min(trim(piece))` display
    // value, so it goes in HAVING (not WHERE — `piece` and the grouped
    // value can differ in casing).
    if let Some(after) = after {
        params.push(Value::from(after));
        sql.push_str(&format!(" HAVING min(trim(piece)) > ${}", params.len()));
    }

    sql.push_str(&format!(" ORDER BY value ASC LIMIT {}", limit + 1));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => {
            let values: Vec<String> = rows.into_iter().map(|r| r.value).collect();
            Json(page_from_rows(values, limit)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "filter_options query failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "db", "internal")
        }
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
    let after = match parse_cursor(q.cursor.as_deref()) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "bad_cursor", "invalid cursor"),
    };
    let limit = parse_limit(q.limit);
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
            return Json(CursorPage::<String>::bounded(vec![])).into_response();
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

    if let Some(after) = after {
        params.push(Value::from(after));
        sql.push_str(&format!(" AND {column} > ${}", params.len()));
    }

    sql.push_str(&format!(" ORDER BY value ASC LIMIT {}", limit + 1));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => {
            let values: Vec<String> = rows.into_iter().map(|r| r.value).collect();
            Json(page_from_rows(values, limit)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "filter_options query failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "db", "internal")
        }
    }
}

#[utoipa::path(
    operation_id = "filter_options_credits",    get,
    path = "/filter-options/credits/{role}",
    params(("role" = String, Path,), ("library" = Option<String>, Query,), ("q" = Option<String>, Query,)),
    responses(
        (status = 200, body = shared::pagination::CursorPage<String>),
        (status = 400, description = "unknown credit role"),
    )
)]
#[handler]
pub async fn credits(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(role): AxPath<String>,
    Query(q): Query<OptionsQuery>,
) -> impl IntoResponse {
    if !CREDIT_ROLES.contains(&role.as_str()) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "unknown credit role",
        );
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
    let after = match parse_cursor(q.cursor.as_deref()) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "bad_cursor", "invalid cursor"),
    };
    let limit = parse_limit(q.limit);
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
            return Json(CursorPage::<String>::bounded(vec![])).into_response();
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

    if let Some(after) = after {
        params.push(Value::from(after));
        sql.push_str(&format!(" AND j.{value_col} > ${}", params.len()));
    }

    // Fetch limit+1 to detect "more available."
    sql.push_str(&format!(" ORDER BY value ASC LIMIT {}", limit + 1));

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    match ValueRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => {
            let values: Vec<String> = rows.into_iter().map(|r| r.value).collect();
            Json(page_from_rows(values, limit)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "filter_options query failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "db", "internal")
        }
    }
}
