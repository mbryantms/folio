//! The `/bookmarks` page feed: every marker **and** every issue-level
//! closing note the caller owns, in one cursor-paged stream.
//!
//! Markers (`markers`) and closing notes (`user_issue_notes`) are
//! different tables, so the page cannot list both through
//! `GET /me/markers`. This module runs one `UNION ALL` over a common
//! projection — `(entry_kind, id, ts, series_key, series_id, issue_sort,
//! page_index)` — sorts and pages that, then hydrates each side by id.
//! Three endpoints share the filter vocabulary:
//!
//! - `GET /me/bookmarks` — the feed. `kind` adds `closing_note` to the
//!   marker kinds; `sort` is `newest` (default) / `oldest` / `reading`
//!   (series name → issue number → page, closing notes before page 1).
//!   The cursor is the full sort key of the last row, so a page boundary
//!   never drops or repeats an entry even when two rows share a
//!   timestamp.
//! - `GET /me/bookmarks/counts` — per-kind counts under every filter
//!   except `kind` / `is_favorite` (the chips show "what would I get"),
//!   plus `all`, the caller's unfiltered total.
//! - `GET /me/bookmarks/series` — the series the caller has entries in,
//!   for the Series filter combobox.
//!
//! Visibility is the marker rule (DI-20): the entry's issue must be live
//! and inside the caller's library grants / age cap.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine;
use entity::marker::MarkerKind;
use entity::{issue, marker, series, user_issue_note, user_rating};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter,
    Statement, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::markers::{MarkerView, TagMatchMode, hydrate_views, parse_tag_list, visible_marker_sql};
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::state::AppState;
use crate::util::search::ilike_pattern;
use server_macros::handler;

const DEFAULT_LIMIT: u64 = 60;
const MAX_LIMIT: u64 = 200;
/// `COALESCE(sort_number, …)` so unnumbered issues sort last in reading
/// order instead of dropping out of a row comparison as NULL.
const UNNUMBERED_SORT: &str = "1e12";

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(feed))
        .routes(routes!(counts))
        .routes(routes!(series_list))
}

// ───────── query shapes ─────────

/// Kind filter for the feed: the four marker kinds plus `closing_note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    Bookmark,
    Note,
    Favorite,
    Highlight,
    ClosingNote,
}

impl FeedKind {
    /// The marker kind this filter selects; `None` for `closing_note`.
    fn marker_kind(self) -> Option<MarkerKind> {
        match self {
            Self::Bookmark => Some(MarkerKind::Bookmark),
            Self::Note => Some(MarkerKind::Note),
            Self::Favorite => Some(MarkerKind::Favorite),
            Self::Highlight => Some(MarkerKind::Highlight),
            Self::ClosingNote => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedSort {
    /// Most recently updated first (the previous `/me/markers` order).
    #[default]
    Newest,
    Oldest,
    /// Series name, issue number, page — the order you would re-read in.
    /// A closing note sorts before an issue's first page.
    Reading,
}

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    pub kind: Option<FeedKind>,
    pub issue_id: Option<String>,
    pub series_id: Option<Uuid>,
    /// Case-insensitive, every word must match: marker body / captured
    /// text, or the closing note body.
    pub q: Option<String>,
    /// Markers only: `is_favorite` flag OR `kind = favorite`.
    pub is_favorite: Option<bool>,
    /// Markers only. Comma-separated; `tag_match` picks AND / OR.
    pub tags: Option<String>,
    pub tag_match: Option<TagMatchMode>,
    pub sort: Option<FeedSort>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

// ───────── wire types ─────────

/// One closing note as the feed renders it: enough identity to draw the
/// row (series, issue, cover, rating) without a second request.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ClosingNoteEntryView {
    pub issue_id: String,
    pub series_id: String,
    pub series_name: Option<String>,
    pub series_slug: Option<String>,
    pub issue_slug: Option<String>,
    pub issue_title: Option<String>,
    pub issue_number: Option<String>,
    /// `/issues/{id}/pages/0/thumb` while the issue is active.
    pub cover_url: Option<String>,
    /// The caller's 0..=5 rating of the issue, if any.
    pub rating: Option<f64>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A feed entry — a marker, or an issue-level closing note.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BookmarkEntryView {
    Marker { marker: MarkerView },
    ClosingNote { note: ClosingNoteEntryView },
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BookmarksFeedView {
    pub items: Vec<BookmarkEntryView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct BookmarkKindCounts {
    pub bookmark: u64,
    pub note: u64,
    /// `is_favorite` OR `kind = favorite` — what the Favorites chip lists.
    pub favorite: u64,
    pub highlight: u64,
    pub closing_note: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BookmarkCountsView {
    /// Entries matching every filter except `kind` / `is_favorite`.
    pub total: u64,
    pub by_kind: BookmarkKindCounts,
    /// The caller's unfiltered total (visibility only).
    pub all: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BookmarkSeriesView {
    pub id: String,
    pub name: String,
    pub slug: String,
    /// Markers + closing notes the caller has in this series.
    pub count: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BookmarkSeriesListView {
    pub items: Vec<BookmarkSeriesView>,
}

// ───────── SQL assembly ─────────

/// Which filters apply to which arm. Closing notes carry no tags and no
/// favorite flag, so those filters (and the marker kinds) exclude the
/// note arm entirely.
struct Arms {
    markers: bool,
    notes: bool,
}

fn arms_for(q: &FeedQuery) -> Arms {
    let notes_excluded =
        q.is_favorite == Some(true) || q.tags.as_deref().is_some_and(|t| !t.trim().is_empty());
    match q.kind {
        Some(FeedKind::ClosingNote) => Arms {
            markers: false,
            notes: !notes_excluded,
        },
        Some(_) => Arms {
            markers: true,
            notes: false,
        },
        None => Arms {
            markers: true,
            notes: !notes_excluded,
        },
    }
}

/// `WHERE` tail shared by both arms: visibility, then the scope filters.
/// `body_expr` is the column the `q` search runs against for the arm.
fn common_where(
    acl: &VisibleLibraries,
    q: &FeedQuery,
    params: &mut Vec<Value>,
    body_exprs: &[&str],
) -> String {
    let mut sql = visible_marker_sql(acl, params);
    if let Some(issue_id) = q.issue_id.as_deref() {
        params.push(Value::from(issue_id.to_owned()));
        sql.push_str(&format!(" AND i.id = ${}", params.len()));
    }
    if let Some(series_id) = q.series_id {
        params.push(Value::from(series_id));
        sql.push_str(&format!(" AND i.series_id = ${}", params.len()));
    }
    if let Some(needle) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        for token in needle.split_whitespace() {
            params.push(Value::from(ilike_pattern(token)));
            let n = params.len();
            let ors: Vec<String> = body_exprs
                .iter()
                .map(|e| format!("{e} ILIKE ${n}"))
                .collect();
            sql.push_str(&format!(" AND ({})", ors.join(" OR ")));
        }
    }
    sql
}

/// Marker-only filters: kind, favorite, tags.
fn marker_only_where(q: &FeedQuery, params: &mut Vec<Value>) -> String {
    let mut sql = String::new();
    if let Some(kind) = q.kind.and_then(FeedKind::marker_kind) {
        sql.push_str(&format!(" AND m.kind = '{}'", kind.as_str()));
    }
    if q.is_favorite == Some(true) {
        sql.push_str(&format!(
            " AND (m.is_favorite OR m.kind = '{}')",
            MarkerKind::Favorite.as_str()
        ));
    }
    if let Some(raw) = q.tags.as_deref().filter(|s| !s.trim().is_empty()) {
        let parsed = parse_tag_list(raw);
        if !parsed.is_empty() {
            params.push(Value::from(parsed));
            let op = q.tag_match.unwrap_or_default().sql_op();
            sql.push_str(&format!(" AND m.tags {op} ${}::text[]", params.len()));
        }
    }
    sql
}

const MARKER_BODY_EXPRS: &[&str] = &["m.body", "(m.selection->>'text')"];
const NOTE_BODY_EXPRS: &[&str] = &["n.body"];

/// The two arms with the feed projection. `$1` is the user id.
fn feed_arms(acl: &VisibleLibraries, q: &FeedQuery, params: &mut Vec<Value>) -> Vec<String> {
    let arms = arms_for(q);
    let mut out = Vec::new();
    if arms.markers {
        let mut sql = format!(
            "SELECT 'marker' AS entry_kind, m.id::text AS id, m.updated_at AS ts, \
             lower(s.name) AS series_key, i.series_id::text AS series_id, \
             COALESCE(i.sort_number, {UNNUMBERED_SORT})::float8 AS issue_sort, \
             m.page_index AS page_index \
             FROM markers m JOIN issues i ON i.id = m.issue_id \
             JOIN series s ON s.id = i.series_id WHERE m.user_id = $1"
        );
        sql.push_str(&common_where(acl, q, params, MARKER_BODY_EXPRS));
        sql.push_str(&marker_only_where(q, params));
        out.push(sql);
    }
    if arms.notes {
        let mut sql = format!(
            "SELECT 'closing_note' AS entry_kind, n.issue_id AS id, n.updated_at AS ts, \
             lower(s.name) AS series_key, i.series_id::text AS series_id, \
             COALESCE(i.sort_number, {UNNUMBERED_SORT})::float8 AS issue_sort, \
             -1 AS page_index \
             FROM user_issue_notes n JOIN issues i ON i.id = n.issue_id \
             JOIN series s ON s.id = i.series_id WHERE n.user_id = $1"
        );
        sql.push_str(&common_where(acl, q, params, NOTE_BODY_EXPRS));
        out.push(sql);
    }
    out
}

#[derive(Debug, FromQueryResult)]
struct FeedRow {
    entry_kind: String,
    id: String,
    ts: chrono::DateTime<chrono::FixedOffset>,
    series_key: String,
    series_id: String,
    issue_sort: f64,
    page_index: i32,
}

/// Opaque cursor: the complete sort key of the last row on the page.
#[derive(Debug, Serialize, Deserialize)]
struct CursorKey {
    ts: String,
    series_key: String,
    series_id: String,
    issue_sort: f64,
    page_index: i32,
    id: String,
}

fn encode_cursor(row: &FeedRow) -> String {
    let key = CursorKey {
        ts: row.ts.to_rfc3339(),
        series_key: row.series_key.clone(),
        series_id: row.series_id.clone(),
        issue_sort: row.issue_sort,
        page_index: row.page_index,
        id: row.id.clone(),
    };
    let json = serde_json::to_vec(&key).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn decode_cursor(raw: &str) -> Option<(CursorKey, chrono::DateTime<chrono::FixedOffset>)> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .ok()?;
    let key: CursorKey = serde_json::from_slice(&bytes).ok()?;
    let ts = chrono::DateTime::parse_from_rfc3339(&key.ts).ok()?;
    Some((key, ts))
}

/// `ORDER BY` for a sort, and the row-comparison `WHERE` that continues
/// it after `cursor`. Both name the same columns in the same order, which
/// is what makes the cursor exact.
fn order_and_cursor(
    sort: FeedSort,
    cursor: Option<&(CursorKey, chrono::DateTime<chrono::FixedOffset>)>,
    params: &mut Vec<Value>,
) -> (String, String) {
    match sort {
        FeedSort::Newest | FeedSort::Oldest => {
            let (dir, cmp) = if sort == FeedSort::Newest {
                ("DESC", "<")
            } else {
                ("ASC", ">")
            };
            let order = format!("ORDER BY f.ts {dir}, f.id {dir}");
            let where_ = cursor.map_or(String::new(), |(key, ts)| {
                params.push(Value::from(*ts));
                let a = params.len();
                params.push(Value::from(key.id.clone()));
                let b = params.len();
                format!("WHERE (f.ts, f.id) {cmp} (${a}::timestamptz, ${b}::text)")
            });
            (order, where_)
        }
        FeedSort::Reading => {
            let order = "ORDER BY f.series_key ASC, f.series_id ASC, f.issue_sort ASC, \
                         f.page_index ASC, f.id ASC"
                .to_owned();
            let where_ = cursor.map_or(String::new(), |(key, _)| {
                params.push(Value::from(key.series_key.clone()));
                let a = params.len();
                params.push(Value::from(key.series_id.clone()));
                let b = params.len();
                params.push(Value::from(key.issue_sort));
                let c = params.len();
                params.push(Value::from(key.page_index));
                let d = params.len();
                params.push(Value::from(key.id.clone()));
                let e = params.len();
                format!(
                    "WHERE (f.series_key, f.series_id, f.issue_sort, f.page_index, f.id) > \
                     (${a}::text, ${b}::text, ${c}::float8, ${d}::int4, ${e}::text)"
                )
            });
            (order, where_)
        }
    }
}

// ───────── handlers ─────────

#[utoipa::path(
    operation_id = "bookmarks_feed",
    get,
    path = "/me/bookmarks",
    params(
        ("kind" = Option<String>, Query, description = "bookmark | note | favorite | highlight | closing_note"),
        ("issue_id" = Option<String>, Query,),
        ("series_id" = Option<String>, Query, description = "series UUID"),
        ("q" = Option<String>, Query,),
        ("is_favorite" = Option<bool>, Query,),
        ("tags" = Option<String>, Query,),
        ("tag_match" = Option<String>, Query, description = "all | any"),
        ("sort" = Option<String>, Query, description = "newest (default) | oldest | reading"),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = BookmarksFeedView), (status = 400, description = "bad cursor"))
)]
#[handler]
pub async fn feed(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<FeedQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let sort = q.sort.unwrap_or_default();
    let cursor = match q.cursor.as_deref() {
        None => None,
        Some(raw) => match decode_cursor(raw) {
            Some(c) => Some(c),
            None => return error(StatusCode::BAD_REQUEST, "validation", "bad cursor"),
        },
    };

    let acl = access::for_user(&app, &user).await;
    let mut params: Vec<Value> = vec![Value::from(user.id)];
    let arms = feed_arms(&acl, &q, &mut params);
    if arms.is_empty() {
        return Json(BookmarksFeedView {
            items: Vec::new(),
            next_cursor: None,
        })
        .into_response();
    }
    let (order, where_) = order_and_cursor(sort, cursor.as_ref(), &mut params);
    params.push(Value::from(limit + 1));
    let sql = format!(
        "SELECT * FROM ({}) f {where_} {order} LIMIT ${}",
        arms.join(" UNION ALL "),
        params.len()
    );
    let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, params);
    let mut rows = match FeedRow::find_by_statement(stmt).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "bookmarks: feed query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let next_cursor = if rows.len() as u64 > limit {
        rows.pop();
        rows.last().map(encode_cursor)
    } else {
        None
    };

    let items = match hydrate(&app.db, user.id, &rows).await {
        Ok(items) => items,
        Err(resp) => return resp,
    };
    Json(BookmarksFeedView { items, next_cursor }).into_response()
}

/// Turn the sorted key rows back into full entries, in the same order.
async fn hydrate(
    db: &DatabaseConnection,
    user_id: Uuid,
    rows: &[FeedRow],
) -> Result<Vec<BookmarkEntryView>, axum::response::Response> {
    let internal = |what: &str, e: sea_orm::DbErr| {
        tracing::error!(error = %e, "bookmarks: hydrate {what} failed");
        error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
    };

    let marker_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.entry_kind == "marker")
        .filter_map(|r| r.id.parse().ok())
        .collect();
    let note_issue_ids: Vec<String> = rows
        .iter()
        .filter(|r| r.entry_kind == "closing_note")
        .map(|r| r.id.clone())
        .collect();

    let markers: HashMap<String, MarkerView> = if marker_ids.is_empty() {
        HashMap::new()
    } else {
        let models = marker::Entity::find()
            .filter(marker::Column::Id.is_in(marker_ids))
            .all(db)
            .await
            .map_err(|e| internal("markers", e))?;
        hydrate_views(db, models)
            .await?
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect()
    };

    let notes: HashMap<String, ClosingNoteEntryView> = if note_issue_ids.is_empty() {
        HashMap::new()
    } else {
        let note_rows = user_issue_note::Entity::find()
            .filter(user_issue_note::Column::UserId.eq(user_id))
            .filter(user_issue_note::Column::IssueId.is_in(note_issue_ids.clone()))
            .all(db)
            .await
            .map_err(|e| internal("notes", e))?;
        let issue_rows = issue::Entity::find()
            .filter(issue::Column::Id.is_in(note_issue_ids.clone()))
            .into_partial_model::<crate::api::issue_card::IssueCardRow>()
            .all(db)
            .await
            .map_err(|e| internal("issues", e))?;
        let series_ids: HashSet<Uuid> = issue_rows.iter().map(|i| i.series_id).collect();
        let series_rows = if series_ids.is_empty() {
            Vec::new()
        } else {
            series::Entity::find()
                .filter(series::Column::Id.is_in(series_ids.into_iter().collect::<Vec<_>>()))
                .all(db)
                .await
                .map_err(|e| internal("series", e))?
        };
        let ratings: HashMap<String, f64> = user_rating::Entity::find()
            .filter(user_rating::Column::UserId.eq(user_id))
            .filter(user_rating::Column::TargetType.eq("issue"))
            .filter(user_rating::Column::TargetId.is_in(note_issue_ids.clone()))
            .all(db)
            .await
            .map_err(|e| internal("ratings", e))?
            .into_iter()
            .map(|r| (r.target_id, r.rating))
            .collect();
        let series_map: HashMap<Uuid, (String, String)> = series_rows
            .into_iter()
            .map(|s| (s.id, (s.name, s.slug)))
            .collect();
        let issue_map: HashMap<String, crate::api::issue_card::IssueCardRow> =
            issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();
        note_rows
            .into_iter()
            .map(|n| {
                let issue_row = issue_map.get(&n.issue_id);
                let series = issue_row.and_then(|i| series_map.get(&i.series_id));
                let view = ClosingNoteEntryView {
                    series_id: issue_row
                        .map(|i| i.series_id.to_string())
                        .unwrap_or_default(),
                    series_name: series.map(|s| s.0.clone()),
                    series_slug: series.map(|s| s.1.clone()),
                    issue_slug: issue_row.map(|i| i.slug.clone()),
                    issue_title: issue_row.and_then(|i| i.title.clone()),
                    issue_number: issue_row.and_then(|i| i.number_raw.clone()),
                    cover_url: issue_row.and_then(|i| {
                        (i.state == entity::issue::IssueState::Active)
                            .then(|| format!("/issues/{}/pages/0/thumb", i.id))
                    }),
                    rating: ratings.get(&n.issue_id).copied(),
                    body: n.body,
                    created_at: n.created_at.to_rfc3339(),
                    updated_at: n.updated_at.to_rfc3339(),
                    issue_id: n.issue_id,
                };
                (view.issue_id.clone(), view)
            })
            .collect()
    };

    Ok(rows
        .iter()
        .filter_map(|r| {
            if r.entry_kind == "marker" {
                markers
                    .get(&r.id)
                    .cloned()
                    .map(|marker| BookmarkEntryView::Marker { marker })
            } else {
                notes
                    .get(&r.id)
                    .cloned()
                    .map(|note| BookmarkEntryView::ClosingNote { note })
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct CountsQuery {
    pub issue_id: Option<String>,
    pub series_id: Option<Uuid>,
    pub q: Option<String>,
    pub tags: Option<String>,
    pub tag_match: Option<TagMatchMode>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    /// `None` for the closing-note arm.
    kind: Option<MarkerKind>,
    is_favorite: bool,
    n: i64,
}

/// Per-kind counts under `(issue_id, series_id, q, tags)`. The marker arm
/// groups by `(kind, is_favorite)` so the Favorites figure can union the
/// legacy flag with the standalone kind the way the chip filters.
async fn count_rows(
    db: &DatabaseConnection,
    user_id: Uuid,
    acl: &VisibleLibraries,
    q: &FeedQuery,
) -> Result<Vec<CountRow>, sea_orm::DbErr> {
    let mut params: Vec<Value> = vec![Value::from(user_id)];
    let mut marker_sql = String::from(
        "SELECT 'marker' AS entry_kind, m.kind AS kind, m.is_favorite AS is_favorite \
         FROM markers m JOIN issues i ON i.id = m.issue_id WHERE m.user_id = $1",
    );
    marker_sql.push_str(&common_where(acl, q, &mut params, MARKER_BODY_EXPRS));
    marker_sql.push_str(&marker_only_where(q, &mut params));
    let mut sql = marker_sql;
    if arms_for(q).notes {
        let mut note_sql = String::from(
            " UNION ALL SELECT 'closing_note', NULL::text, false \
             FROM user_issue_notes n JOIN issues i ON i.id = n.issue_id WHERE n.user_id = $1",
        );
        note_sql.push_str(&common_where(acl, q, &mut params, NOTE_BODY_EXPRS));
        sql.push_str(&note_sql);
    }
    let sql = format!(
        "SELECT entry_kind, kind, is_favorite, count(*)::int8 AS n FROM ({sql}) f \
         GROUP BY entry_kind, kind, is_favorite"
    );
    let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, params);
    CountRow::find_by_statement(stmt).all(db).await
}

fn fold_counts(rows: &[CountRow]) -> (u64, BookmarkKindCounts) {
    let mut by = BookmarkKindCounts::default();
    let mut total = 0u64;
    for r in rows {
        let n = r.n.max(0) as u64;
        total += n;
        let Some(kind) = r.kind else {
            by.closing_note += n;
            continue;
        };
        match kind {
            MarkerKind::Bookmark => by.bookmark += n,
            MarkerKind::Note => by.note += n,
            MarkerKind::Highlight => by.highlight += n,
            MarkerKind::Favorite => by.favorite += n,
        }
        // Legacy favorited bookmarks / notes / highlights count toward
        // the Favorites chip too (same union the feed applies).
        if r.is_favorite && kind != MarkerKind::Favorite {
            by.favorite += n;
        }
    }
    (total, by)
}

#[utoipa::path(
    operation_id = "bookmarks_counts",
    get,
    path = "/me/bookmarks/counts",
    params(
        ("issue_id" = Option<String>, Query,),
        ("series_id" = Option<String>, Query, description = "series UUID"),
        ("q" = Option<String>, Query,),
        ("tags" = Option<String>, Query,),
        ("tag_match" = Option<String>, Query, description = "all | any"),
    ),
    responses((status = 200, body = BookmarkCountsView))
)]
#[handler]
pub async fn counts(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<CountsQuery>,
) -> impl IntoResponse {
    let acl = access::for_user(&app, &user).await;
    let filtered = FeedQuery {
        kind: None,
        issue_id: q.issue_id,
        series_id: q.series_id,
        q: q.q,
        is_favorite: None,
        tags: q.tags,
        tag_match: q.tag_match,
        sort: None,
        cursor: None,
        limit: None,
    };
    let unfiltered = FeedQuery {
        kind: None,
        issue_id: None,
        series_id: None,
        q: None,
        is_favorite: None,
        tags: None,
        tag_match: None,
        sort: None,
        cursor: None,
        limit: None,
    };
    let (filtered_rows, all_rows) = tokio::join!(
        count_rows(&app.db, user.id, &acl, &filtered),
        count_rows(&app.db, user.id, &acl, &unfiltered),
    );
    let (filtered_rows, all_rows) = match (filtered_rows, all_rows) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!(error = %e, "bookmarks: counts failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let (total, by_kind) = fold_counts(&filtered_rows);
    let (all, _) = fold_counts(&all_rows);
    Json(BookmarkCountsView {
        total,
        by_kind,
        all,
    })
    .into_response()
}

#[derive(Debug, FromQueryResult)]
struct SeriesRow {
    id: String,
    name: String,
    slug: String,
    n: i64,
}

#[utoipa::path(
    operation_id = "bookmarks_series",
    get,
    path = "/me/bookmarks/series",
    responses((status = 200, body = BookmarkSeriesListView))
)]
#[handler]
pub async fn series_list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    let acl = access::for_user(&app, &user).await;
    let empty = FeedQuery {
        kind: None,
        issue_id: None,
        series_id: None,
        q: None,
        is_favorite: None,
        tags: None,
        tag_match: None,
        sort: None,
        cursor: None,
        limit: None,
    };
    let mut params: Vec<Value> = vec![Value::from(user.id)];
    let mut sql = String::from(
        "SELECT s.id::text AS id, s.name AS name, s.slug AS slug, count(*)::int8 AS n FROM (\
         SELECT i.series_id FROM markers m JOIN issues i ON i.id = m.issue_id WHERE m.user_id = $1",
    );
    sql.push_str(&common_where(&acl, &empty, &mut params, MARKER_BODY_EXPRS));
    sql.push_str(
        " UNION ALL SELECT i.series_id FROM user_issue_notes n \
         JOIN issues i ON i.id = n.issue_id WHERE n.user_id = $1",
    );
    sql.push_str(&common_where(&acl, &empty, &mut params, NOTE_BODY_EXPRS));
    sql.push_str(
        ") f JOIN series s ON s.id = f.series_id \
         GROUP BY s.id, s.name, s.slug ORDER BY lower(s.name) ASC, s.id ASC",
    );
    let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, params);
    match SeriesRow::find_by_statement(stmt).all(&app.db).await {
        Ok(rows) => Json(BookmarkSeriesListView {
            items: rows
                .into_iter()
                .map(|r| BookmarkSeriesView {
                    id: r.id,
                    name: r.name,
                    slug: r.slug,
                    count: r.n.max(0) as u64,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "bookmarks: series list failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}
