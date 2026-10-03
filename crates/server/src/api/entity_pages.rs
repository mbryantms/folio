//! Shared core for the entity landing pages (WP-5.5, audit R21 / UX-7):
//! `/characters/{slug}`, `/teams/{slug}`, `/arcs/{slug}`,
//! `/publishers/{slug}`.
//!
//! The four thin handler modules ([`super::characters`],
//! [`super::teams`], [`super::arcs`], [`super::publishers`]) carry the
//! `#[utoipa::path]` annotations and delegate here; the OPDS navigation
//! feeds in [`super::opds`] reuse the same queries, so the web page and
//! the OPDS feed can never disagree about membership or visibility.
//!
//! ## Membership
//!
//! Membership is the id link first. The scanner series rollup fills
//! `issue_characters.character_id`, `issue_teams.team_id`,
//! `series.publisher_id` and reconciles `issue_arcs` from the story-arc
//! CSV (`metadata_rollup::link_series_entity_ids`); `m20270305` backfilled
//! existing rows. A name fallback (`btrim(lower(name)) =
//! entity.normalized_name`) remains only where the id can still be NULL —
//! a junction row scanned since the last rollup:
//!
//! - character / team — `issue_characters` / `issue_teams` (issue hits)
//!   plus `series_characters` / `series_teams` (series-level cast), by FK
//!   or (FK NULL) name.
//! - story arc — `issue_arcs` only (the link *is* the row), plus
//!   `series_arcs` for series-level membership.
//! - publisher — `series.publisher_id`, or the name while it is NULL.
//!
//! ## Visibility
//!
//! Every query is library-ACL + age-rating-cap filtered exactly like
//! `/creators`: issue hits check the issue's effective rating
//! (`COALESCE(i.age_rating, s.age_rating)`), series rows check the series
//! rating. An entity with no visible appearance for the caller is a 404
//! on the detail route and absent from the browse index, so a restricted
//! user can't learn names that only occur in hidden libraries.

use axum::{Json, http::StatusCode, response::IntoResponse, response::Response};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QuerySelect, Statement, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::error;
use super::issue_card::IssueCardRow;
use super::series::{
    IssueListView, IssueSummaryView, SeriesListView, SeriesView, StartsWithBucket, hydrate_series,
    parse_starts_with,
};
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::state::AppState;
use entity::{issue, series};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};

pub(crate) const LIST_DEFAULT_LIMIT: u64 = 60;
pub(crate) const LIST_MAX_LIMIT: u64 = 100;

/// Which entity table a landing page is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntityKind {
    Character,
    Team,
    Arc,
    Publisher,
}

impl EntityKind {
    pub(crate) fn table(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Team => "team",
            Self::Arc => "story_arc",
            Self::Publisher => "publisher",
        }
    }

    /// Wire value of [`EntityDetailView::kind`] and the URL segment.
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Character => "characters",
            Self::Team => "teams",
            Self::Arc => "arcs",
            Self::Publisher => "publishers",
        }
    }

    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Team => "team",
            Self::Arc => "story arc",
            Self::Publisher => "publisher",
        }
    }

    pub(crate) fn plural_title(self) -> &'static str {
        match self {
            Self::Character => "Characters",
            Self::Team => "Teams",
            Self::Arc => "Story arcs",
            Self::Publisher => "Publishers",
        }
    }
}

// ───────── wire types ─────────

/// One row of an entity browse index (`GET /characters` etc.).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EntityListItem {
    pub id: String,
    /// `/<kind>/<slug>` target.
    pub slug: String,
    pub name: String,
    /// Distinct visible series the entity appears in.
    pub series_count: i64,
    /// Distinct visible issues the entity appears in (0 for a
    /// series-level-only appearance).
    pub issue_count: i64,
}

/// Header payload for an entity landing page. The series / issues grids
/// are fetched separately through the cursor-paginated sub-routes.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EntityDetailView {
    /// `"characters"` | `"teams"` | `"arcs"` | `"publishers"`.
    pub kind: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub aliases: Vec<String>,
    /// Characters only (provider data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_name: Option<String>,
    /// Publishers only (provider data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founded_year: Option<i32>,
    pub series_count: i64,
    pub issue_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct EntityListQuery {
    pub cursor: Option<String>,
    /// Clamped to `[1, 100]`; default 60.
    pub limit: Option<u64>,
    /// A–Z jump-rail bucket (a single letter or `#`). Invalid → 422.
    pub starts_with: Option<String>,
    /// Case-insensitive substring filter on the entity name.
    pub q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EntityPageQuery {
    pub cursor: Option<String>,
    /// Clamped to `[1, 100]`; default 60.
    pub limit: Option<u64>,
}

// ───────── entity row ─────────

#[derive(Debug, Clone, FromQueryResult)]
pub(crate) struct EntityRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub normalized_name: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub aliases: serde_json::Value,
    pub real_name: Option<String>,
    pub founded_year: Option<i32>,
}

pub(crate) async fn find_by_slug(
    app: &AppState,
    kind: EntityKind,
    slug: &str,
) -> Result<Option<EntityRow>, sea_orm::DbErr> {
    let real_name = if kind == EntityKind::Character {
        "real_name"
    } else {
        "NULL::text"
    };
    let founded = if kind == EntityKind::Publisher {
        "founded_year"
    } else {
        "NULL::int4"
    };
    let sql = format!(
        "SELECT id, slug, name, normalized_name, description, image_url, aliases, \
                {real_name} AS real_name, {founded} AS founded_year \
           FROM {table} WHERE slug = $1",
        table = kind.table(),
    );
    EntityRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        [slug.into()],
    ))
    .one(&app.db)
    .await
}

// ───────── SQL fragments ─────────

/// ACL fragments over the aliases `s` (series) and `i` (issues). Bound
/// values are pushed onto the caller's params; the fragments may be
/// reused any number of times in one statement.
struct AclSql {
    lib: String,
    issue_cap: String,
    series_cap: String,
}

/// `None` when the caller is restricted to an empty library set (no
/// query can return anything).
fn acl_sql(visible: &VisibleLibraries, params: &mut Vec<Value>) -> Option<AclSql> {
    let lib = if visible.unrestricted {
        String::new()
    } else {
        if visible.allowed.is_empty() {
            return None;
        }
        let mut ids: Vec<Uuid> = visible.allowed.iter().copied().collect();
        ids.sort();
        let placeholders: Vec<String> = ids
            .into_iter()
            .map(|id| {
                params.push(Value::from(id));
                format!("${}", params.len())
            })
            .collect();
        format!(" AND s.library_id IN ({})", placeholders.join(","))
    };
    let issue_cap = visible.raw_cap_clause(
        "s.library_id",
        "COALESCE(i.age_rating, s.age_rating)",
        params,
    );
    let series_cap = visible.raw_cap_clause("s.library_id", "s.age_rating", params);
    Some(AclSql {
        lib,
        issue_cap,
        series_cap,
    })
}

/// Visible active-issue predicate (aliases `i` + `s`).
fn issue_visible(acl: &AclSql) -> String {
    format!(
        "i.state = 'active' AND i.removed_at IS NULL AND s.removed_at IS NULL{}{}",
        acl.lib, acl.issue_cap
    )
}

/// Visible series predicate (alias `s`).
fn series_visible(acl: &AclSql) -> String {
    format!("s.removed_at IS NULL{}{}", acl.lib, acl.series_cap)
}

/// The visible-series predicate over alias `s` for `visible` (bound values
/// pushed onto `params`), or `None` when the caller is restricted to an
/// empty library set. Lets other modules page series-keyed rows with the
/// same ACL as the entity pages (WP-7.5 arc tie-ins, WP-7.7 same
/// universe). `include_removed` (admins, matching `series::list`) drops
/// the `removed_at IS NULL` term but keeps the grant + age-rating cap.
pub(crate) fn series_visible_sql_for(
    visible: &VisibleLibraries,
    params: &mut Vec<Value>,
    include_removed: bool,
) -> Option<String> {
    let acl = acl_sql(visible, params)?;
    Some(if include_removed {
        format!("TRUE{}{}", acl.lib, acl.series_cap)
    } else {
        series_visible(&acl)
    })
}

/// Of `arc_ids`, the story arcs with at least one appearance the caller
/// can see — a visible issue in `issue_arcs` or a visible series in
/// `series_arcs`. The same rule that 404s `/arcs/{slug}` (WP-7.5: arc
/// targets of series relationships are listed only when visible).
pub(crate) async fn visible_arc_ids(
    app: &AppState,
    visible: &VisibleLibraries,
    arc_ids: &[Uuid],
) -> Result<std::collections::HashSet<Uuid>, sea_orm::DbErr> {
    if arc_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let ids: Vec<String> = arc_ids.iter().map(Uuid::to_string).collect();
    let mut params: Vec<Value> = vec![Value::from(ids)];
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok(std::collections::HashSet::new());
    };
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let sql = format!(
        "SELECT ia.arc_id AS id \
           FROM issue_arcs ia \
           JOIN issues i ON i.id = ia.issue_id \
           JOIN series s ON s.id = i.series_id \
          WHERE ia.arc_id = ANY($1::uuid[]) AND {ivis} \
         UNION \
         SELECT sa.arc_id AS id \
           FROM series_arcs sa \
           JOIN series s ON s.id = sa.series_id \
          WHERE sa.arc_id = ANY($1::uuid[]) AND {svis}",
        ivis = issue_visible(&acl),
        svis = series_visible(&acl),
    );
    let rows = IdRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .all(&app.db)
    .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// `SELECT issue_id, series_id` of every visible issue one entity
/// (`$e` = id, `$n` = normalized name) appears in.
fn issue_hits_one(kind: EntityKind, e: usize, n: usize, acl: &AclSql) -> String {
    let vis = issue_visible(acl);
    match kind {
        EntityKind::Character | EntityKind::Team => {
            let (junction, fk, name) = junction_cols(kind);
            format!(
                "SELECT i.id AS issue_id, i.series_id AS series_id \
                   FROM {junction} j \
                   JOIN issues i ON i.id = j.issue_id \
                   JOIN series s ON s.id = i.series_id \
                  WHERE (j.{fk} = ${e} OR (j.{fk} IS NULL AND btrim(lower(j.{name})) = ${n})) \
                    AND {vis}"
            )
        }
        // Arc membership is the `issue_arcs` link only: the series rollup
        // reconciles scanner-sourced CSV arcs into it (WP-5.5), so no
        // per-row CSV split is needed on the read side.
        EntityKind::Arc => format!(
            "SELECT i.id AS issue_id, i.series_id AS series_id \
               FROM issue_arcs ia \
               JOIN issues i ON i.id = ia.issue_id \
               JOIN series s ON s.id = i.series_id \
              WHERE ia.arc_id = ${e} AND {vis}"
        ),
        EntityKind::Publisher => format!(
            "SELECT i.id AS issue_id, i.series_id AS series_id \
               FROM series s \
               JOIN issues i ON i.series_id = s.id \
              WHERE {pub_match} AND {vis}",
            pub_match = publisher_match(e, n),
        ),
    }
}

fn publisher_match(e: usize, n: usize) -> String {
    format!(
        "(s.publisher_id = ${e} OR (s.publisher_id IS NULL \
          AND btrim(lower(s.publisher)) = ${n}))"
    )
}

fn junction_cols(kind: EntityKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        EntityKind::Character => ("issue_characters", "character_id", "character"),
        EntityKind::Team => ("issue_teams", "team_id", "team"),
        _ => unreachable!("junction_cols: character/team only"),
    }
}

/// `SELECT series_id` of the series one entity is a member of — the
/// issue hits plus any series-level membership rows. Series-level
/// visibility is applied by the caller on the final `series s` select.
fn series_members_one(kind: EntityKind, e: usize, n: usize, acl: &AclSql) -> String {
    let hits = issue_hits_one(kind, e, n, acl);
    let extra = match kind {
        EntityKind::Character => Some(format!(
            "SELECT sj.series_id FROM series_characters sj \
              WHERE sj.character_id = ${e} \
                 OR (sj.character_id IS NULL AND btrim(lower(sj.character)) = ${n})"
        )),
        EntityKind::Team => Some(format!(
            "SELECT sj.series_id FROM series_teams sj \
              WHERE sj.team_id = ${e} \
                 OR (sj.team_id IS NULL AND btrim(lower(sj.team)) = ${n})"
        )),
        EntityKind::Arc => Some(format!(
            "SELECT sa.series_id FROM series_arcs sa WHERE sa.arc_id = ${e}"
        )),
        EntityKind::Publisher => Some(format!(
            "SELECT s.id AS series_id FROM series s WHERE {}",
            publisher_match(e, n)
        )),
    };
    match extra {
        Some(x) => format!("SELECT h.series_id FROM ({hits}) h UNION {x}"),
        None => format!("SELECT h.series_id FROM ({hits}) h"),
    }
}

/// `SELECT eid, issue_id, series_id` over **every** entity of a kind —
/// the browse-index aggregation source. `issue_id` is NULL for
/// series-level membership rows.
fn hits_all(kind: EntityKind, acl: &AclSql) -> String {
    let ivis = issue_visible(acl);
    let svis = series_visible(acl);
    let table = kind.table();
    match kind {
        EntityKind::Character | EntityKind::Team => {
            let (junction, fk, name) = junction_cols(kind);
            let series_junction = if kind == EntityKind::Character {
                "series_characters"
            } else {
                "series_teams"
            };
            format!(
                "SELECT COALESCE(j.{fk}, e.id) AS eid, i.id AS issue_id, i.series_id AS series_id \
                   FROM {junction} j \
                   JOIN issues i ON i.id = j.issue_id \
                   JOIN series s ON s.id = i.series_id \
                   LEFT JOIN {table} e ON j.{fk} IS NULL \
                        AND e.normalized_name = btrim(lower(j.{name})) \
                  WHERE {ivis} \
                 UNION ALL \
                 SELECT COALESCE(sj.{fk}, e.id), NULL::text, s.id \
                   FROM {series_junction} sj \
                   JOIN series s ON s.id = sj.series_id \
                   LEFT JOIN {table} e ON sj.{fk} IS NULL \
                        AND e.normalized_name = btrim(lower(sj.{name})) \
                  WHERE {svis}"
            )
        }
        EntityKind::Arc => format!(
            "SELECT ia.arc_id AS eid, i.id AS issue_id, i.series_id AS series_id \
               FROM issue_arcs ia \
               JOIN issues i ON i.id = ia.issue_id \
               JOIN series s ON s.id = i.series_id \
              WHERE {ivis} \
             UNION ALL \
             SELECT sa.arc_id, NULL::text, s.id \
               FROM series_arcs sa \
               JOIN series s ON s.id = sa.series_id \
              WHERE {svis}"
        ),
        EntityKind::Publisher => format!(
            "SELECT COALESCE(s.publisher_id, e.id) AS eid, i.id AS issue_id, s.id AS series_id \
               FROM series s \
               LEFT JOIN issues i ON i.series_id = s.id \
                    AND i.state = 'active' AND i.removed_at IS NULL{icap} \
               LEFT JOIN publisher e ON s.publisher_id IS NULL \
                    AND e.normalized_name = btrim(lower(s.publisher)) \
              WHERE (s.publisher_id IS NOT NULL OR s.publisher IS NOT NULL) AND {svis}",
            icap = acl.issue_cap,
        ),
    }
}

// ───────── queries ─────────

#[derive(Debug, FromQueryResult)]
struct CountsRow {
    series_count: i64,
    issue_count: i64,
}

/// `(series_count, issue_count)` visible to the caller for one entity.
pub(crate) async fn counts(
    app: &AppState,
    kind: EntityKind,
    row: &EntityRow,
    visible: &VisibleLibraries,
) -> Result<(i64, i64), sea_orm::DbErr> {
    let mut params: Vec<Value> = vec![row.id.into(), row.normalized_name.clone().into()];
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok((0, 0));
    };
    let sql = format!(
        "WITH hits AS ({hits}) \
         SELECT (SELECT COUNT(*) FROM series s \
                  WHERE s.id IN ({members}) AND {svis})::bigint AS series_count, \
                (SELECT COUNT(DISTINCT issue_id) FROM hits)::bigint AS issue_count",
        hits = issue_hits_one(kind, 1, 2, &acl),
        members = series_members_one(kind, 1, 2, &acl),
        svis = series_visible(&acl),
    );
    let r = CountsRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .one(&app.db)
    .await?;
    Ok(r.map(|r| (r.series_count, r.issue_count)).unwrap_or((0, 0)))
}

/// Page of member-series ids in `(lower(name), id)` order.
pub(crate) struct Paging<K> {
    /// Keyset: resume strictly after this key.
    pub after: Option<K>,
    /// Offset (OPDS numbered pages). Ignored when `after` is set.
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, FromQueryResult)]
struct SeriesKeyRow {
    id: Uuid,
    k: String,
}

pub(crate) type SeriesKey = (String, Uuid);

/// Member series ids (visible to the caller) plus the key of the last
/// row returned. Over-fetches one row to report `has_more`.
pub(crate) async fn series_page(
    app: &AppState,
    kind: EntityKind,
    row: &EntityRow,
    visible: &VisibleLibraries,
    paging: &Paging<SeriesKey>,
) -> Result<(Vec<(Uuid, SeriesKey)>, bool), sea_orm::DbErr> {
    let mut params: Vec<Value> = vec![row.id.into(), row.normalized_name.clone().into()];
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok((Vec::new(), false));
    };
    let mut keyset = String::new();
    if let Some((name, id)) = &paging.after {
        params.push(name.clone().into());
        params.push((*id).into());
        keyset = format!(
            " AND (lower(s.name), s.id) > (${}, ${})",
            params.len() - 1,
            params.len()
        );
    }
    let offset = if paging.after.is_some() {
        0
    } else {
        paging.offset
    };
    // L-4 (WP-6.3): LIMIT / OFFSET are bound parameters, not interpolated.
    params.push(Value::from((paging.limit + 1) as i64));
    let fetch_param = params.len();
    params.push(Value::from(offset as i64));
    let sql = format!(
        "SELECT s.id, lower(s.name) AS k FROM series s \
          WHERE s.id IN ({members}) AND {svis}{keyset} \
          ORDER BY lower(s.name), s.id \
          LIMIT ${fetch} OFFSET ${offset}",
        members = series_members_one(kind, 1, 2, &acl),
        svis = series_visible(&acl),
        fetch = fetch_param,
        offset = fetch_param + 1,
    );
    let mut rows = SeriesKeyRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .all(&app.db)
    .await?;
    let more = rows.len() as u64 > paging.limit;
    rows.truncate(paging.limit as usize);
    Ok((
        rows.into_iter().map(|r| (r.id, (r.k, r.id))).collect(),
        more,
    ))
}

/// Keyset for the issue grid: `(arc position, year, series name, issue
/// sort number, issue id)`. `k0` is always 0 outside story arcs.
pub(crate) type IssueKey = (f64, i32, String, f64, String);

#[derive(Debug, FromQueryResult)]
struct IssueKeyRow {
    id: String,
    k0: f64,
    k1: i32,
    k2: String,
    k3: f64,
}

/// Member issue ids (visible) in reading order — arc position first for
/// story arcs, then publication year, series name, issue number.
pub(crate) async fn issue_page(
    app: &AppState,
    kind: EntityKind,
    row: &EntityRow,
    visible: &VisibleLibraries,
    paging: &Paging<IssueKey>,
) -> Result<(Vec<(String, IssueKey)>, bool), sea_orm::DbErr> {
    let mut params: Vec<Value> = vec![row.id.into(), row.normalized_name.clone().into()];
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok((Vec::new(), false));
    };
    let k0 = if kind == EntityKind::Arc {
        "COALESCE( \
            (SELECT min(ia.position_in_arc) FROM issue_arcs ia \
              WHERE ia.issue_id = i.id AND ia.arc_id = $1)::float8, \
            CASE WHEN i.story_arc_number ~ '^\\s*[0-9]+(\\.[0-9]+)?\\s*$' \
                 THEN btrim(i.story_arc_number)::float8 END, \
            1e9::float8)"
    } else {
        "0::float8"
    };
    let mut keyset = String::new();
    if let Some((a, b, c, d, id)) = &paging.after {
        let base = params.len();
        params.push((*a).into());
        params.push((*b).into());
        params.push(c.clone().into());
        params.push((*d).into());
        params.push(id.clone().into());
        keyset = format!(
            " WHERE (k0, k1, k2, k3, id) > (${}, ${}, ${}, ${}, ${})",
            base + 1,
            base + 2,
            base + 3,
            base + 4,
            base + 5
        );
    }
    let offset = if paging.after.is_some() {
        0
    } else {
        paging.offset
    };
    // L-4 (WP-6.3): LIMIT / OFFSET are bound parameters, not interpolated.
    params.push(Value::from((paging.limit + 1) as i64));
    let fetch_param = params.len();
    params.push(Value::from(offset as i64));
    let sql = format!(
        "SELECT id, k0, k1, k2, k3 FROM ( \
            SELECT i.id AS id, {k0} AS k0, \
                   COALESCE(i.year, 9999)::int4 AS k1, \
                   lower(s.name) AS k2, \
                   COALESCE(i.sort_number, 1e9)::float8 AS k3 \
              FROM issues i JOIN series s ON s.id = i.series_id \
             WHERE i.id IN (SELECT issue_id FROM ({hits}) h) \
         ) t{keyset} \
         ORDER BY k0, k1, k2, k3, id \
         LIMIT ${fetch} OFFSET ${offset}",
        hits = issue_hits_one(kind, 1, 2, &acl),
        fetch = fetch_param,
        offset = fetch_param + 1,
    );
    let mut rows = IssueKeyRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .all(&app.db)
    .await?;
    let more = rows.len() as u64 > paging.limit;
    rows.truncate(paging.limit as usize);
    Ok((
        rows.into_iter()
            .map(|r| {
                let key = (r.k0, r.k1, r.k2, r.k3, r.id.clone());
                (r.id, key)
            })
            .collect(),
        more,
    ))
}

pub(crate) type ListKey = (String, String);

pub(crate) struct ListFilter {
    pub starts_with: Option<StartsWithBucket>,
    pub q: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ListRow {
    id: Uuid,
    slug: String,
    name: String,
    series_count: i64,
    issue_count: i64,
}

/// Build the shared `WITH hits …, agg …` prefix and the WHERE conditions
/// on the `e` (entity) alias for the browse index.
fn list_prefix(
    kind: EntityKind,
    acl: &AclSql,
    filter: &ListFilter,
    params: &mut Vec<Value>,
) -> (String, Vec<String>) {
    let cte = format!(
        "WITH hits AS ({hits}), \
         agg AS ( \
           SELECT eid, COUNT(DISTINCT issue_id)::bigint AS issue_count, \
                  COUNT(DISTINCT series_id)::bigint AS series_count \
             FROM hits WHERE eid IS NOT NULL GROUP BY eid)",
        hits = hits_all(kind, acl),
    );
    let mut conds = Vec::new();
    match &filter.starts_with {
        Some(StartsWithBucket::Letter(c)) => {
            params.push(format!("{c}%").into());
            conds.push(format!("lower(e.name) LIKE ${}", params.len()));
        }
        Some(StartsWithBucket::Digit) => conds.push("lower(e.name) !~ '^[a-z]'".to_owned()),
        None => {}
    }
    if let Some(q) = filter.q.as_deref() {
        params.push(q.to_lowercase().into());
        conds.push(format!("strpos(lower(e.name), ${}) > 0", params.len()));
    }
    (cte, conds)
}

pub(crate) async fn list_page(
    app: &AppState,
    kind: EntityKind,
    visible: &VisibleLibraries,
    filter: &ListFilter,
    paging: &Paging<ListKey>,
) -> Result<(Vec<(EntityListItem, ListKey)>, bool), sea_orm::DbErr> {
    let mut params: Vec<Value> = Vec::new();
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok((Vec::new(), false));
    };
    let (cte, mut conds) = list_prefix(kind, &acl, filter, &mut params);
    if let Some((name, slug)) = &paging.after {
        params.push(name.clone().into());
        params.push(slug.clone().into());
        conds.push(format!(
            "(lower(e.name), e.slug) > (${}, ${})",
            params.len() - 1,
            params.len()
        ));
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    let offset = if paging.after.is_some() {
        0
    } else {
        paging.offset
    };
    // L-4 (WP-6.3): LIMIT / OFFSET are bound parameters, not interpolated.
    params.push(Value::from((paging.limit + 1) as i64));
    let fetch_param = params.len();
    params.push(Value::from(offset as i64));
    let sql = format!(
        "{cte} SELECT e.id, e.slug, e.name, a.series_count, a.issue_count \
           FROM agg a JOIN {table} e ON e.id = a.eid{where_clause} \
          ORDER BY lower(e.name), e.slug \
          LIMIT ${fetch} OFFSET ${offset}",
        table = kind.table(),
        fetch = fetch_param,
        offset = fetch_param + 1,
    );
    let mut rows = ListRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .all(&app.db)
    .await?;
    let more = rows.len() as u64 > paging.limit;
    rows.truncate(paging.limit as usize);
    Ok((
        rows.into_iter()
            .map(|r| {
                let key = (r.name.to_lowercase(), r.slug.clone());
                (
                    EntityListItem {
                        id: r.id.to_string(),
                        slug: r.slug,
                        name: r.name,
                        series_count: r.series_count,
                        issue_count: r.issue_count,
                    },
                    key,
                )
            })
            .collect(),
        more,
    ))
}

#[derive(Debug, FromQueryResult)]
struct N {
    n: i64,
}

pub(crate) async fn list_count(
    app: &AppState,
    kind: EntityKind,
    visible: &VisibleLibraries,
    filter: &ListFilter,
) -> Result<u64, sea_orm::DbErr> {
    let mut params: Vec<Value> = Vec::new();
    let Some(acl) = acl_sql(visible, &mut params) else {
        return Ok(0);
    };
    let (cte, conds) = list_prefix(kind, &acl, filter, &mut params);
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };
    let sql = format!(
        "{cte} SELECT COUNT(*)::bigint AS n FROM agg a JOIN {table} e ON e.id = a.eid{where_clause}",
        table = kind.table(),
    );
    Ok(N::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        params,
    ))
    .one(&app.db)
    .await?
    .map(|r| r.n as u64)
    .unwrap_or(0))
}

/// Load `issue::Model`-free card rows for `ids`, preserving order, with
/// series slug + name attached. Used by the JSON issues route.
pub(crate) async fn issue_cards(
    app: &AppState,
    ids: &[String],
) -> Result<Vec<IssueSummaryView>, sea_orm::DbErr> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<IssueCardRow> = issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.to_vec()))
        .into_partial_model::<IssueCardRow>()
        .all(&app.db)
        .await?;
    let series_ids: Vec<Uuid> = rows.iter().map(|r| r.series_id).collect();
    let series_rows: Vec<(Uuid, String, String)> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .column(series::Column::Slug)
        .column(series::Column::Name)
        .filter(series::Column::Id.is_in(series_ids))
        .into_tuple()
        .all(&app.db)
        .await?;
    let series_by_id: HashMap<Uuid, (String, String)> = series_rows
        .into_iter()
        .map(|(id, slug, name)| (id, (slug, name)))
        .collect();
    let mut by_id: HashMap<String, IssueCardRow> =
        rows.into_iter().map(|r| (r.id.clone(), r)).collect();
    Ok(ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .map(|r| {
            let (slug, name) = series_by_id.get(&r.series_id).cloned().unwrap_or_default();
            r.into_summary_view(&slug).with_series_name(name)
        })
        .collect())
}

// ───────── handler bodies (shared by the four thin modules) ─────────

fn clamp_limit(limit: Option<u64>) -> u64 {
    limit.unwrap_or(LIST_DEFAULT_LIMIT).clamp(1, LIST_MAX_LIMIT)
}

fn internal(e: sea_orm::DbErr, what: &str) -> Response {
    tracing::error!(error = %e, "entity_pages: {what} failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn bad_cursor() -> Response {
    error(StatusCode::BAD_REQUEST, "validation", "invalid cursor")
}

/// `GET /<kind>` — alphabetical cursor-paginated browse index.
pub(crate) async fn list_handler(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    q: EntityListQuery,
) -> Response {
    let limit = clamp_limit(q.limit);
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
    let text = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if text.is_some_and(|t| t.len() > 200) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", "q too long");
    }
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<ListKey>(c) {
            Ok(k) => Some(k),
            Err(_) => return bad_cursor(),
        },
        None => None,
    };
    let first_page = after.is_none();
    let filter = ListFilter {
        starts_with,
        q: text.map(str::to_owned),
    };
    let visible = access::for_user(app, user).await;
    let paging = Paging {
        after,
        offset: 0,
        limit,
    };
    let (rows, more) = match list_page(app, kind, &visible, &filter, &paging).await {
        Ok(r) => r,
        Err(e) => return internal(e, "list"),
    };
    let next_cursor = if more {
        rows.last().and_then(|(_, k)| encode_cursor(k).ok())
    } else {
        None
    };
    let total = if first_page {
        match list_count(app, kind, &visible, &filter).await {
            Ok(n) => Some(n),
            Err(e) => return internal(e, "list count"),
        }
    } else {
        None
    };
    Json(CursorPage {
        items: rows.into_iter().map(|(item, _)| item).collect::<Vec<_>>(),
        next_cursor,
        total,
    })
    .into_response()
}

/// Resolve `slug` and require at least one visible appearance. Returns
/// the row + `(series_count, issue_count)`, or a ready 404/500.
async fn resolve_visible(
    app: &AppState,
    kind: EntityKind,
    slug: &str,
    visible: &VisibleLibraries,
) -> Result<(EntityRow, i64, i64), Response> {
    let not_found = || {
        error(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("{} not found", kind.noun()),
        )
    };
    let row = match find_by_slug(app, kind, slug).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(not_found()),
        Err(e) => return Err(internal(e, "lookup")),
    };
    let (series_count, issue_count) = match counts(app, kind, &row, visible).await {
        Ok(c) => c,
        Err(e) => return Err(internal(e, "counts")),
    };
    if series_count == 0 && issue_count == 0 {
        return Err(not_found());
    }
    Ok((row, series_count, issue_count))
}

pub(crate) async fn resolve_visible_for_user(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    slug: &str,
) -> Result<(EntityRow, VisibleLibraries, i64, i64), Response> {
    let visible = access::for_user(app, user).await;
    let (row, s, i) = resolve_visible(app, kind, slug, &visible).await?;
    Ok((row, visible, s, i))
}

/// `GET /<kind>/{slug}` — header payload.
pub(crate) async fn detail_handler(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    slug: &str,
) -> Response {
    let (row, _, series_count, issue_count) =
        match resolve_visible_for_user(app, user, kind, slug).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let aliases = row
        .aliases
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Json(EntityDetailView {
        kind: kind.path().to_owned(),
        id: row.id.to_string(),
        slug: row.slug,
        name: row.name,
        description: row.description,
        image_url: row.image_url,
        aliases,
        real_name: row.real_name,
        founded_year: row.founded_year,
        series_count,
        issue_count,
    })
    .into_response()
}

/// `GET /<kind>/{slug}/series` — cursor-paginated member series.
pub(crate) async fn series_handler(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    slug: &str,
    q: EntityPageQuery,
) -> Response {
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<SeriesKey>(c) {
            Ok(k) => Some(k),
            Err(_) => return bad_cursor(),
        },
        None => None,
    };
    let first_page = after.is_none();
    let (row, visible, series_count, _) =
        match resolve_visible_for_user(app, user, kind, slug).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let paging = Paging {
        after,
        offset: 0,
        limit: clamp_limit(q.limit),
    };
    let (keys, more) = match series_page(app, kind, &row, &visible, &paging).await {
        Ok(r) => r,
        Err(e) => return internal(e, "series page"),
    };
    let next_cursor = if more {
        keys.last().and_then(|(_, k)| encode_cursor(k).ok())
    } else {
        None
    };
    let ids: Vec<Uuid> = keys.iter().map(|(id, _)| *id).collect();
    let models = if ids.is_empty() {
        Vec::new()
    } else {
        match series::Entity::find()
            .filter(series::Column::Id.is_in(ids.clone()))
            .all(&app.db)
            .await
        {
            Ok(m) => m,
            Err(e) => return internal(e, "series hydrate"),
        }
    };
    let mut by_id: HashMap<String, SeriesView> = hydrate_series(app, models, user.id)
        .await
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect();
    let items: Vec<SeriesView> = ids
        .iter()
        .filter_map(|id| by_id.remove(&id.to_string()))
        .collect();
    Json(SeriesListView {
        items,
        next_cursor,
        total: first_page.then_some(series_count),
    })
    .into_response()
}

/// `GET /<kind>/{slug}/issues` — cursor-paginated member issues.
pub(crate) async fn issues_handler(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    slug: &str,
    q: EntityPageQuery,
) -> Response {
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<IssueKey>(c) {
            Ok(k) => Some(k),
            Err(_) => return bad_cursor(),
        },
        None => None,
    };
    let first_page = after.is_none();
    let (row, visible, _, issue_count) = match resolve_visible_for_user(app, user, kind, slug).await
    {
        Ok(v) => v,
        Err(r) => return r,
    };
    let paging = Paging {
        after,
        offset: 0,
        limit: clamp_limit(q.limit),
    };
    let (keys, more) = match issue_page(app, kind, &row, &visible, &paging).await {
        Ok(r) => r,
        Err(e) => return internal(e, "issue page"),
    };
    let next_cursor = if more {
        keys.last().and_then(|(_, k)| encode_cursor(k).ok())
    } else {
        None
    };
    let ids: Vec<String> = keys.into_iter().map(|(id, _)| id).collect();
    let items = match issue_cards(app, &ids).await {
        Ok(v) => v,
        Err(e) => return internal(e, "issue cards"),
    };
    Json(IssueListView {
        items,
        next_cursor,
        total: first_page.then_some(issue_count),
    })
    .into_response()
}

// ───────── chip slug maps (series + issue detail pages) ─────────

/// Name → slug maps for the cast / arc / publisher chips on the series
/// and issue detail pages, so the web can link each chip to its landing
/// page without a per-chip round-trip. Keys are the display names as
/// they appear on the page; names without an entity row are absent (the
/// chip falls back to the library-grid filter).
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct EntitySlugs {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub characters: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub teams: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub arcs: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub publishers: HashMap<String, String>,
}

#[derive(Debug, FromQueryResult)]
struct SlugRow {
    normalized_name: String,
    slug: String,
}

async fn slug_map(app: &AppState, kind: EntityKind, names: &[String]) -> HashMap<String, String> {
    if names.is_empty() {
        return HashMap::new();
    }
    let norm: Vec<String> = names.iter().map(|n| n.trim().to_lowercase()).collect();
    let sql = format!(
        "SELECT normalized_name, slug FROM {} WHERE normalized_name = ANY($1)",
        kind.table()
    );
    let rows = SlugRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        sql,
        [Value::from(norm)],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default();
    let by_norm: HashMap<String, String> = rows
        .into_iter()
        .map(|r| (r.normalized_name, r.slug))
        .collect();
    names
        .iter()
        .filter_map(|n| {
            by_norm
                .get(&n.trim().to_lowercase())
                .map(|s| (n.clone(), s.clone()))
        })
        .collect()
}

/// Build [`EntitySlugs`] for the names a detail page renders. Best
/// effort: a lookup failure just leaves the chips on their fallback.
pub(crate) async fn entity_slugs(
    app: &AppState,
    characters: &[String],
    teams: &[String],
    arcs: &[String],
    publishers: &[String],
) -> EntitySlugs {
    let (characters, teams, arcs, publishers) = tokio::join!(
        slug_map(app, EntityKind::Character, characters),
        slug_map(app, EntityKind::Team, teams),
        slug_map(app, EntityKind::Arc, arcs),
        slug_map(app, EntityKind::Publisher, publishers),
    );
    EntitySlugs {
        characters,
        teams,
        arcs,
        publishers,
    }
}

/// Split a CSV read-cache column the way the scanner does
/// (`metadata_rollup::split_csv`).
pub(crate) fn split_csv(value: Option<&str>) -> Vec<String> {
    value
        .map(crate::library::scanner::metadata_rollup::split_csv)
        .unwrap_or_default()
}
