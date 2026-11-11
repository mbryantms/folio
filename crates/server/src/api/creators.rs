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
    /// `roles`; each rail is sorted by series name. Never empty: a
    /// creator with no credit visible to the caller is a 404.
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

/// The series-level arm of the creator credit union (`person`, `role`,
/// `ref_id = 'series:<id>'`, `library_id`) shared by `/creators` and
/// `/people`. `library_filter`, `series_cap` and `issue_cap` are the
/// caller's fragments over the aliases `s` / `i`.
///
/// Uncapped callers read the `series_credits` rollup. For a caller whose
/// cap hides anything (`capped`) a rollup row counts only while an issue
/// of the series that caller can see carries the same credit, because the
/// rollup also carries the credits of issues above the cap.
pub(crate) fn series_credit_arm(
    library_filter: &str,
    series_cap: &str,
    issue_cap: &str,
    capped: bool,
) -> String {
    let arm = format!(
        "SELECT sc.person AS person, sc.role AS role, \
                'series:' || sc.series_id::text AS ref_id, s.library_id AS library_id \
           FROM series_credits sc JOIN series s ON s.id = sc.series_id \
          WHERE s.removed_at IS NULL{library_filter}{series_cap}"
    );
    if !capped {
        return arm;
    }
    // Keep a rollup row only while an issue of that series the caller
    // can see carries the same credit. `issue_cap` is over `i` + `s`.
    format!(
        "{arm} AND EXISTS (SELECT 1 FROM issue_credits ic \
           JOIN issues i ON i.id = ic.issue_id \
          WHERE i.series_id = sc.series_id AND ic.role = sc.role \
            AND ic.person = sc.person AND i.removed_at IS NULL \
            AND i.state = 'active'{issue_cap})"
    )
}

/// The issue-level arm of the creator credit union (`person`, `role`,
/// `ref_id = 'issue:<id>'`, `library_id`) shared by `/creators` and
/// `/people`: credits on active, non-removed issues of non-removed series,
/// filtered by the caller's `library_filter` and entity `issue_cap`
/// fragments over the aliases `s` / `i`.
pub(crate) fn issue_credit_arm(library_filter: &str, issue_cap: &str) -> String {
    format!(
        "SELECT ic.person AS person, ic.role AS role, \
                'issue:' || ic.issue_id AS ref_id, s.library_id AS library_id \
           FROM issue_credits ic \
           JOIN issues i ON i.id = ic.issue_id \
           JOIN series s ON s.id = i.series_id \
          WHERE s.removed_at IS NULL AND i.removed_at IS NULL \
            AND i.state = 'active'{library_filter}{issue_cap}"
    )
}

/// `WITH RECURSIVE names(person) AS (…)`: every distinct credit name of
/// `issue_credits` ∪ `series_credits`, in `person` order, starting after
/// the `$bound` placeholder (or from the first name), as a skip scan over
/// `issue_credits_person_idx` + `series_credits_person_idx` — each step
/// is the next name after the previous one in either table, two index
/// probes per distinct name. Read lazily: a consumer that stops early
/// (`LIMIT`) stops the walk. The last row is a NULL terminator, which
/// callers filter out.
pub(crate) fn credit_names_walk(bound: Option<usize>) -> String {
    let first = |table: &str| match bound {
        Some(p) => format!("(SELECT min(c.person) FROM {table} c WHERE c.person > ${p})"),
        None => format!("(SELECT min(c.person) FROM {table} c)"),
    };
    format!(
        "WITH RECURSIVE names(person) AS ( \
           SELECT LEAST({ic0}, {sc0}) \
           UNION ALL \
           SELECT LEAST( \
                    (SELECT min(c.person) FROM issue_credits c WHERE c.person > n.person), \
                    (SELECT min(c.person) FROM series_credits c WHERE c.person > n.person)) \
             FROM names n WHERE n.person IS NOT NULL)",
        ic0 = first("issue_credits"),
        sc0 = first("series_credits"),
    )
}

/// Per-name probe + aggregate over the two credit arms, for the creator
/// index and people search (P-01). Both arms are filtered to the outer
/// name `{name}` (a column reference), so they read that name's credit
/// rows through `issue_credits_person_idx` / `series_credits_person_idx`
/// instead of aggregating every visible credit first.
pub(crate) struct CreditArms {
    pub series_arm: String,
    pub issue_arm: String,
}

impl CreditArms {
    /// Join that keeps only names with a credit the caller can see — the
    /// same rule as "the name has a row in the union". A `LATERAL` with
    /// `LIMIT 1` stops at the first visible credit and, unlike `EXISTS`,
    /// can never be planned as a hashed sub-plan over every credit row.
    pub(crate) fn visible_join(&self, name: &str) -> String {
        format!(
            " CROSS JOIN LATERAL (SELECT 1 FROM ({series} UNION ALL {issue}) x \
               WHERE x.person = {name} LIMIT 1) vis",
            series = self.series_arm,
            issue = self.issue_arm,
        )
    }

    /// `LATERAL` body: `roles` + `credit_count` of one name, exactly the
    /// former `GROUP BY person` aggregate.
    pub(crate) fn aggregate(&self, name: &str) -> String {
        format!(
            "SELECT ARRAY_AGG(DISTINCT x.role ORDER BY x.role) AS roles, \
                    COUNT(DISTINCT x.ref_id)::bigint AS credit_count \
               FROM ({series} UNION ALL {issue}) x \
              WHERE x.person = {name}",
            series = self.series_arm,
            issue = self.issue_arm,
        )
    }
}

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
    // WP-2.7: age-rating cap per UNION branch — series-level credits
    // check the series rating, issue-level credits the issue's own
    // rating with the series as fallback.
    let series_cap = visible.raw_cap_clause("s.library_id", "s.age_rating", &mut lib_params);
    let issue_cap = visible.entity_issue_cap_sql("i", "s", &mut lib_params);

    // Keyset cursor = the last creator name of the prior page.
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<String>(c) {
            Ok(name) => Some(name),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
        None => None,
    };

    let arms = CreditArms {
        series_arm: series_credit_arm(
            &library_filter,
            &series_cap,
            &issue_cap,
            visible.hides_any(),
        ),
        issue_arm: issue_credit_arm(&library_filter, &issue_cap),
    };

    // P-01: walk the distinct credit names in `person` order with a skip
    // scan over `issue_credits_person_idx` + `series_credits_person_idx`
    // (each step is the next name after the previous one in either table),
    // keep the names with a credit the caller can see, and stop once the
    // page is full. The per-name roles + count are aggregated for the
    // page's names only. Same rows, order and counts as the former
    // aggregate-every-visible-credit query: a name is listed iff the
    // union has a row for it; the keyset is still `person > cursor`.

    // A–Z bucket: a plain condition on the name, applied to the walk
    // before the visibility probe.
    let bucket = |params: &mut Vec<Value>| match &starts_with {
        Some(StartsWithBucket::Letter(c)) => {
            params.push(Value::from(format!("{c}%")));
            format!(" AND lower(n.person) LIKE ${}", params.len())
        }
        // "#" catches everything that doesn't sort under a letter.
        Some(StartsWithBucket::Digit) => " AND lower(n.person) !~ '^[a-z]'".to_string(),
        None => String::new(),
    };

    let mut params = lib_params.clone();
    let after_param = after.as_ref().map(|name| {
        params.push(Value::from(name.clone()));
        params.len()
    });
    let bucket_sql = bucket(&mut params);
    // L-4 (WP-6.3): bound, not interpolated.
    params.push(Value::from((limit + 1) as i64));
    let sql = format!(
        "{names}, \
         page AS MATERIALIZED ( \
           SELECT n.person FROM names n{visible} \
            WHERE n.person IS NOT NULL{bucket_sql} \
            LIMIT ${fetch}) \
         SELECT pg.person, a.roles, a.credit_count, p.slug \
           FROM page pg \
           CROSS JOIN LATERAL ({agg}) a \
           LEFT JOIN person p ON p.normalized_name = btrim(lower(pg.person)) \
          ORDER BY pg.person ASC",
        names = credit_names_walk(after_param),
        visible = arms.visible_join("n.person"),
        agg = arms.aggregate("pg.person"),
        fetch = params.len(),
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
    // cursor — total is the whole filtered set): one probe per distinct
    // credit name, never an aggregate over the credits.
    let total = if after.is_none() {
        let mut count_params = lib_params;
        let bucket_sql = bucket(&mut count_params);
        let count_sql = format!(
            "{names} SELECT COUNT(*)::bigint AS n FROM names n{visible} \
              WHERE n.person IS NOT NULL{bucket_sql}",
            names = credit_names_walk(None),
            visible = arms.visible_join("n.person"),
        );
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
            // WP-8.1: nothing visible → 404, like the arc / character /
            // team pages (`entity_pages::resolve_visible`), so a
            // restricted user can't confirm a name that only occurs in
            // hidden libraries.
            return error(StatusCode::NOT_FOUND, "not_found", "creator not found");
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
    // WP-2.7: age-rating cap per UNION branch (see `list`).
    let series_cap = visible.raw_cap_clause("s.library_id", "s.age_rating", &mut params);
    let issue_cap = visible.entity_issue_cap_sql("i", "s", &mut params);

    // Distinct (series_id, role) pairs across BOTH credit tables.
    // Series-level credits already key on series_id; issue-level
    // credits join through `issues` to reach the series. We dedupe
    // by (series_id, role) so a creator who's "writer" on every issue
    // of a series appears once as writer for that series.
    let issue_arm = format!(
        "SELECT s.id AS series_id, ic.role AS role \
           FROM issue_credits ic \
           JOIN issues i ON i.id = ic.issue_id \
           JOIN series s ON s.id = i.series_id \
          WHERE s.removed_at IS NULL \
            AND i.removed_at IS NULL \
            AND i.state = 'active' \
            AND btrim(lower(ic.person)) = $1{library_filter}{issue_cap}"
    );
    let sql = if visible.hides_any() {
        // Capped caller: the `series_credits` rollup includes credits on
        // issues above the cap, so only credits on visible issues count —
        // in series the caller can see (the rails are series cards).
        format!(
            "WITH credits AS ({issue_arm}{series_cap}) \
             SELECT DISTINCT series_id, role FROM credits"
        )
    } else {
        format!(
            "WITH credits AS ( \
               SELECT s.id AS series_id, sc.role AS role \
                 FROM series_credits sc \
                 JOIN series s ON s.id = sc.series_id \
                WHERE s.removed_at IS NULL \
                  AND btrim(lower(sc.person)) = $1{library_filter}{series_cap} \
               UNION \
               {issue_arm} \
             ) \
             SELECT series_id, role FROM credits",
        )
    };

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
    let hydrated = hydrate_series(&app, series_rows, user.id, &visible).await;
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

    // WP-8.1: a creator with no visible credit is a 404, not a 200 with
    // empty rails (same rule as the other entity pages).
    if rails.is_empty() {
        return error(StatusCode::NOT_FOUND, "not_found", "creator not found");
    }

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
