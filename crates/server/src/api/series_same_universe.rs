//! `GET /series/{slug}/same-universe` — the series page's derived "Same
//! universe" section (WP-7.7).
//!
//! "Same universe" is not a curated or suggested pairwise edge (M7b
//! decision; WP-7.6 retires the pairwise `same_universe` suggestion
//! source). It is derived on read from shared membership:
//!
//! - a `universe` row both series roll up to (`series_universes`), or
//! - a non-empty `series.series_group` value both carry (ComicInfo
//!   `SeriesGroup`; split on `,` / `;`, compared trimmed and
//!   case-insensitively).
//!
//! Each item names what is shared. ACL: the source series must be visible
//! (404 otherwise, the `GET /series/{slug}` gate); listed series pass the
//! library grant + age-rating cap, and removed series are listed for
//! admins only (as `series::list`). Cursor-paginated over `(name, id)`;
//! `total` on the first page only. Manual `same_universe` edges are
//! ordinary relationships and stay in `GET /series/{slug}/relationships`.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::series;
use sea_orm::{ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, Statement, Value};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::respond;
use super::series::{SeriesView, find_by_slug, hydrate_series};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(same_universe))
}

const DEFAULT_LIMIT: u64 = 24;
const MAX_LIMIT: u64 = 60;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SameUniverseQuery {
    /// Opaque continuation token from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size, 1–60 (default 24).
    pub limit: Option<u64>,
}

/// What two series share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedUniverseVia {
    /// A `universe` both series roll up to.
    Universe,
    /// A ComicInfo `SeriesGroup` value both carry.
    SeriesGroup,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SharedUniverse {
    pub via: SharedUniverseVia,
    /// Universe name, or the series-group value as this series spells it.
    pub name: String,
}

/// One series sharing a universe or series group with the requested one.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SameUniverseItem {
    pub series: SeriesView,
    /// What is shared (universes first, then series groups; by name).
    pub shared: Vec<SharedUniverse>,
}

/// Keyset position: `(name, id)`.
#[derive(Debug, Serialize, Deserialize)]
struct Cursor {
    #[serde(rename = "n")]
    name: String,
    #[serde(rename = "i")]
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct Row {
    id: Uuid,
    name: String,
    shared: serde_json::Value,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    n: i64,
}

/// The shared-membership CTE (`$1` = the source series id): one row per
/// (other series, shared thing).
///
/// The series-group split is `unnest(regexp_split_to_array(…))`, not the
/// equivalent `regexp_split_to_table(…)`: the planner assumes a set-returning
/// function yields 1,000 rows but estimates `unnest` of an array at 10. At
/// 2,500 series the 1,000-row guess put both statements past
/// `jit_above_cost`, and JIT compilation (~13 ms) cost ten times the query
/// itself (WP-8.3, `docs/dev/load-testing.md` "M7 surfaces").
const SHARED_CTE: &str = r"
WITH src_groups AS (
    SELECT DISTINCT lower(btrim(g)) AS key, btrim(g) AS name
      FROM series s0,
           unnest(regexp_split_to_array(COALESCE(s0.series_group, ''), '\s*[,;]\s*')) AS g
     WHERE s0.id = $1 AND btrim(g) <> ''
),
shared AS (
    SELECT su2.series_id AS sid, 'universe'::text AS via, u.name AS name
      FROM series_universes su1
      JOIN series_universes su2
        ON su2.universe_id = su1.universe_id AND su2.series_id <> su1.series_id
      JOIN universe u ON u.id = su1.universe_id
     WHERE su1.series_id = $1
    UNION
    SELECT s2.id AS sid, 'series_group'::text AS via, sg.name AS name
      FROM series s2
      CROSS JOIN LATERAL unnest(regexp_split_to_array(s2.series_group, '\s*[,;]\s*')) AS g
      JOIN src_groups sg ON sg.key = lower(btrim(g))
     WHERE s2.series_group IS NOT NULL AND s2.id <> $1
)";

#[utoipa::path(
    operation_id = "series_same_universe", get,
    path = "/series/{slug}/same-universe",
    params(("slug" = String, Path), SameUniverseQuery),
    responses(
        (status = 200, body = CursorPage<SameUniverseItem>),
        (status = 400, description = "invalid cursor"),
        (status = 404, description = "series not found or not visible"),
    )
)]
#[handler]
pub async fn same_universe(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<SameUniverseQuery>,
) -> Response {
    let s = match find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !access::series_visible(&app, &user, &s).await {
        return respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "series not found",
        );
    }
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<Cursor>(c) {
            Ok(k) => Some(k),
            Err(_) => {
                return respond(
                    StatusCode::BAD_REQUEST,
                    ApiErrorCode::Validation,
                    "invalid cursor",
                );
            }
        },
        None => None,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;
    let mut params: Vec<Value> = vec![Value::from(s.id)];
    let include_removed = user.role == "admin";
    let Some(svis) =
        super::entity_pages::series_visible_sql_for(&visible, &mut params, include_removed)
    else {
        return Json(CursorPage::<SameUniverseItem>::paginated(
            Vec::new(),
            None,
            Some(0),
        ))
        .into_response();
    };
    let backend = app.db.get_database_backend();

    let total = if after.is_none() {
        let sql = format!(
            "{SHARED_CTE} SELECT COUNT(DISTINCT s.id)::bigint AS n \
               FROM shared sh JOIN series s ON s.id = sh.sid WHERE {svis}"
        );
        match CountRow::find_by_statement(Statement::from_sql_and_values(
            backend,
            sql,
            params.clone(),
        ))
        .one(&app.db)
        .await
        {
            Ok(r) => Some(r.map_or(0, |r| u64::try_from(r.n).unwrap_or(0))),
            Err(e) => return internal(&e),
        }
    } else {
        None
    };

    let mut page_params = params;
    let keyset = match after {
        Some(k) => {
            page_params.push(Value::from(k.name));
            page_params.push(Value::from(k.id));
            let n = page_params.len();
            format!(" AND (s.name, s.id) > (${}, ${})", n - 1, n)
        }
        None => String::new(),
    };
    page_params.push(Value::from(i64::try_from(limit + 1).unwrap_or(61)));
    let limit_param = page_params.len();
    let sql = format!(
        "{SHARED_CTE} \
         SELECT s.id, s.name, \
                jsonb_agg(DISTINCT jsonb_build_object('via', sh.via, 'name', sh.name)) AS shared \
           FROM shared sh JOIN series s ON s.id = sh.sid \
          WHERE {svis}{keyset} \
          GROUP BY s.id, s.name \
          ORDER BY s.name, s.id \
          LIMIT ${limit_param}"
    );
    let rows =
        match Row::find_by_statement(Statement::from_sql_and_values(backend, sql, page_params))
            .all(&app.db)
            .await
        {
            Ok(r) => r,
            Err(e) => return internal(&e),
        };
    let mut rows = rows;
    let has_more = rows.len() as u64 > limit;
    rows.truncate(usize::try_from(limit).unwrap_or(MAX_LIMIT as usize));

    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let models = match series::Entity::find()
        .filter(series::Column::Id.is_in(ids))
        .all(&app.db)
        .await
    {
        Ok(m) => m,
        Err(e) => return internal(&e),
    };
    let views: HashMap<String, SeriesView> = hydrate_series(&app, models, user.id)
        .await
        .into_iter()
        .map(|v| (v.id.clone(), v))
        .collect();

    let next_cursor = if has_more {
        rows.last().and_then(|r| {
            encode_cursor(&Cursor {
                name: r.name.clone(),
                id: r.id,
            })
            .ok()
        })
    } else {
        None
    };
    let items: Vec<SameUniverseItem> = rows
        .into_iter()
        .filter_map(|r| {
            let series = views.get(&r.id.to_string())?.clone();
            let mut shared: Vec<SharedUniverse> =
                serde_json::from_value(r.shared).unwrap_or_default();
            shared.sort_by(|a, b| {
                (a.via != SharedUniverseVia::Universe, a.name.to_lowercase())
                    .cmp(&(b.via != SharedUniverseVia::Universe, b.name.to_lowercase()))
            });
            Some(SameUniverseItem { series, shared })
        })
        .collect();
    Json(CursorPage::paginated(items, next_cursor, total)).into_response()
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "same-universe query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}
