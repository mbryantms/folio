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
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::{ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, Statement, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::series::{SeriesView, StartsWithBucket, hydrate_series, parse_starts_with};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use entity::{person, series};
use server_macros::handler;
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};

const LIST_DEFAULT_LIMIT: u64 = 60;
const LIST_MAX_LIMIT: u64 = 100;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(get_one))
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

// ───────── Browse index (A11) ─────────

/// One creator row in the browse index — same shape the people-search
/// hit uses (name + slug + role rollup + credit count) so the web can
/// render them with one card.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatorListItem {
    pub person: String,
    /// `/creators/<slug>` target. `None` until the `person` backfill
    /// catches a freshly-scanned credit; the client falls back to the
    /// legacy `?library=all&credits=<name>` URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub roles: Vec<String>,
    pub credit_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Opaque keyset cursor (a prior page's `next_cursor`).
    pub cursor: Option<String>,
    /// Page size. Clamped to `[1, 100]`; default 60.
    pub limit: Option<u64>,
    /// A–Z jump-rail bucket: a single letter `a`–`z` (case-insensitive)
    /// matching creators whose name starts with it, or `#` for names that
    /// sort under a non-letter. Invalid values 422.
    pub starts_with: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ListRow {
    person: String,
    slug: Option<String>,
    roles: Vec<String>,
    credit_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    n: i64,
}

/// `GET /creators` — alphabetical, cursor-paginated browse of every
/// creator visible to the caller (distinct names aggregated across
/// `series_credits` + `issue_credits`, with role rollup + credit count).
/// Library-ACL gated like `/people` + the detail page. Keyset on the
/// creator name so the list never silently truncates (audit A11 +
/// list-pagination-completeness).
#[utoipa::path(
    operation_id = "creators_list",    get,
    path = "/creators",
    params(
        ("cursor" = Option<String>, Query,),
        ("limit"  = Option<u64>,    Query,),
        ("starts_with" = Option<String>, Query,),
    ),
    responses((status = 200, body = CursorPage<CreatorListItem>))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q
        .limit
        .unwrap_or(LIST_DEFAULT_LIMIT)
        .clamp(1, LIST_MAX_LIMIT);

    // A–Z jump-rail bucket (validated → reused for both the page and count
    // queries). Bad values 422 before any DB work.
    let starts_with = match q.starts_with.as_deref() {
        Some(raw) => match parse_starts_with(raw) {
            Some(b) => Some(b),
            None => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "starts_with must be a single letter or #",
                );
            }
        },
        None => None,
    };

    let visible = access::for_user(&app, &user).await;

    // Library-allowlist clause woven into BOTH UNION branches of the
    // credit CTE; the same UUID placeholders are reused across branches.
    let mut lib_params: Vec<Value> = Vec::new();
    let library_filter = if visible.unrestricted {
        String::new()
    } else {
        if visible.allowed.is_empty() {
            return Json(CursorPage {
                items: Vec::<CreatorListItem>::new(),
                next_cursor: None,
                total: Some(0),
            })
            .into_response();
        }
        let placeholders: Vec<String> = visible
            .allowed
            .iter()
            .map(|id| {
                lib_params.push(Value::from(*id));
                format!("${}", lib_params.len())
            })
            .collect();
        // Qualify the column: the issue-credits UNION branch joins both
        // `issues` and `series`, each of which exposes `library_id`, so a
        // bare reference is ambiguous. `series s` is present in both
        // branches, so `s.library_id` resolves cleanly everywhere.
        format!(" AND s.library_id IN ({})", placeholders.join(","))
    };

    // Keyset cursor = the last creator name of the prior page.
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<String>(c) {
            Ok(name) => Some(name),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
        None => None,
    };

    let credits_cte = format!(
        "WITH all_credits AS ( \
           SELECT sc.person AS person, sc.role AS role, \
                  'series:' || sc.series_id::text AS ref_id, s.library_id AS library_id \
             FROM series_credits sc JOIN series s ON s.id = sc.series_id \
            WHERE s.removed_at IS NULL{library_filter} \
           UNION ALL \
           SELECT ic.person AS person, ic.role AS role, \
                  'issue:' || ic.issue_id AS ref_id, s.library_id AS library_id \
             FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             JOIN series s ON s.id = i.series_id \
            WHERE s.removed_at IS NULL AND i.removed_at IS NULL \
              AND i.state = 'active'{library_filter} \
         ), \
         agg AS ( \
           SELECT person, ARRAY_AGG(DISTINCT role ORDER BY role) AS roles, \
                  COUNT(DISTINCT ref_id)::bigint AS credit_count \
             FROM all_credits GROUP BY person \
         )"
    );

    // Page query: keyset filter (+ optional jump-rail bucket) + over-fetch
    // one to detect a next page. Both are WHERE conditions on `agg a`.
    let mut params = lib_params.clone();
    let mut conds: Vec<String> = Vec::new();
    if let Some(name) = &after {
        params.push(Value::from(name.clone()));
        conds.push(format!("a.person > ${}", params.len()));
    }
    match &starts_with {
        Some(StartsWithBucket::Letter(c)) => {
            params.push(Value::from(format!("{c}%")));
            conds.push(format!("lower(a.person) LIKE ${}", params.len()));
        }
        // "#" catches everything that doesn't sort under a letter.
        Some(StartsWithBucket::Digit) => conds.push("lower(a.person) !~ '^[a-z]'".to_string()),
        None => {}
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    let sql = format!(
        "{credits_cte} \
         SELECT a.person, a.roles, a.credit_count, p.slug \
           FROM agg a \
           LEFT JOIN person p ON p.normalized_name = btrim(lower(a.person)){where_clause} \
          ORDER BY a.person ASC \
          LIMIT {fetch}",
        fetch = limit + 1,
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    let mut rows = match ListRow::find_by_statement(stmt).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "creators: list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.truncate(limit as usize);
        match rows.last().map(|r| encode_cursor(&r.person)) {
            Some(Ok(c)) => Some(c),
            _ => None,
        }
    } else {
        None
    };

    // Total only on the first page (keyset pages stay cheap). The count
    // honors the same jump-rail bucket as the page query (but never the
    // cursor — total is the whole filtered set).
    let total = if after.is_none() {
        let mut count_params = lib_params;
        let count_where = match &starts_with {
            Some(StartsWithBucket::Letter(c)) => {
                count_params.push(Value::from(format!("{c}%")));
                format!(" WHERE lower(a.person) LIKE ${}", count_params.len())
            }
            Some(StartsWithBucket::Digit) => " WHERE lower(a.person) !~ '^[a-z]'".to_string(),
            None => String::new(),
        };
        let count_sql =
            format!("{credits_cte} SELECT COUNT(*)::bigint AS n FROM agg a{count_where}");
        let count_stmt = Statement::from_sql_and_values(backend, count_sql, count_params);
        CountRow::find_by_statement(count_stmt)
            .one(&app.db)
            .await
            .ok()
            .flatten()
            .map(|r| r.n as u64)
    } else {
        None
    };

    Json(CursorPage {
        items: rows
            .into_iter()
            .map(|r| CreatorListItem {
                person: r.person,
                slug: r.slug,
                roles: r.roles,
                credit_count: r.credit_count,
            })
            .collect(),
        next_cursor,
        total,
    })
    .into_response()
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
