//! People search (global-search M4).
//!
//! Distinct creator names across `series_credits` and `issue_credits`,
//! with role rollup + credit count, matched by `pg_trgm` similarity
//! (`%`) or a literal substring and ranked by similarity — names like
//! "Jhon" still surface "John …". The candidate names come from the
//! ordered distinct-name walk shared with `/creators` (P-01); see
//! [`list`].
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
use crate::middleware::rate_limit;
use crate::state::AppState;
use crate::util::search::{SQL_LIKE_ESCAPE, contains_pattern};
use server_macros::handler;

const MAX_QUERY_LEN: usize = 200;
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

pub fn routes() -> OpenApiRouter<AppState> {
    // Free-text credit search: per-user `SEARCH` bucket (E3, audit N-A5),
    // in its own sub-router so a route added here later doesn't inherit it.
    let search = OpenApiRouter::new()
        .routes(routes!(list))
        .route_layer(rate_limit::SEARCH.build_per_user());
    OpenApiRouter::new().merge(search)
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
    responses((status = 200, body = PeopleListView), (status = 429, description = "search rate limit"))
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
    // similarity ranking; $2 is the same string as an escaped `%…%`
    // substring pattern (E9: `_` / `%` / `\` match literally). Subsequent
    // params are the library-allowlist UUIDs when the caller is restricted.
    let lowered = text.to_lowercase();
    let substring = contains_pattern(&lowered);
    let mut params: Vec<Value> = vec![Value::from(lowered), Value::from(substring)];

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
        // Qualified: the issue-credits branch joins both `issues` and
        // `series`, each of which exposes `library_id`.
        format!(" AND s.library_id IN ({})", placeholders.join(","))
    };
    // WP-2.7: age-rating cap per UNION branch (mirrors `creators`).
    let series_cap = visible.raw_cap_clause("s.library_id", "s.age_rating", &mut params);
    let issue_cap = visible.issue_cap_sql_joined("i", "s", &mut params);

    // P-01: no aggregate over every visible credit. The candidates are the
    // distinct credit names matching `q` — the walk of
    // [`super::creators::credit_names_walk`] (two index probes per distinct
    // name, so the cost follows the number of names, not of credits)
    // filtered by the same predicate as before — each probed for a credit
    // the caller can see. The ranking needs a name's credit count only to
    // order names of equal similarity, so counts are aggregated only for
    // the names at least as similar as the `limit`-th best visible name
    // (`cut`). Same rows and order as before: each of the top `limit` rows
    // has a similarity at or above that cut, and the final ORDER BY is
    // unchanged. A name without a `person` row (credit scanned since the
    // last backfill) returns `slug = NULL`; the client falls back to the
    // legacy `?library=all&credits=<name>` URL.
    // L-4 (WP-6.3): LIMIT / OFFSET are bound parameters.
    params.push(Value::from(limit));
    let limit_param = params.len();
    params.push(Value::from(limit - 1));
    let cut_offset_param = params.len();
    let arms = super::creators::CreditArms {
        series_arm: super::creators::series_credit_arm(
            &library_filter,
            &series_cap,
            &issue_cap,
            visible.hides_any(),
        ),
        issue_arm: super::creators::issue_credit_arm(&library_filter, &issue_cap),
    };
    let sql = format!(
        "{names}, \
         cand AS MATERIALIZED ( \
           SELECT n.person, similarity(n.person, $1) AS sim FROM names n \
            WHERE n.person IS NOT NULL \
              AND (n.person % $1 OR lower(n.person) LIKE $2{SQL_LIKE_ESCAPE}) \
         ), \
         vis AS MATERIALIZED ( \
           SELECT c.person, c.sim FROM cand c{visible} \
         ), \
         cut AS ( \
           SELECT v.sim FROM vis v ORDER BY v.sim DESC LIMIT 1 OFFSET ${cut_offset_param} \
         ) \
         SELECT v.person, a.roles, a.credit_count, p.slug \
           FROM vis v \
           CROSS JOIN LATERAL ({agg}) a \
           LEFT JOIN person p ON p.normalized_name = btrim(lower(v.person)) \
          WHERE v.sim >= COALESCE((SELECT cut.sim FROM cut), 0) \
          ORDER BY v.sim DESC, a.credit_count DESC, v.person ASC \
          LIMIT ${limit_param}",
        names = super::creators::credit_names_walk(None),
        visible = arms.visible_join("c.person"),
        agg = arms.aggregate("v.person"),
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
