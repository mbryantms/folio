//! Markers + Collections M5 — `/me/markers` + per-issue lookup.
//!
//! Five endpoints back the marker surface:
//!
//!   - `GET /me/markers` — paginated feed for the `/bookmarks` index
//!     page. Supports `kind`, `issue_id`, `q` (full-text against the
//!     note body and OCR `selection.text`), plus opaque cursor pagination
//!     keyed on `updated_at | id`.
//!   - `GET /me/issues/{id}/markers` — fast one-shot lookup the
//!     `<MarkerOverlay>` calls on reader mount; returns every marker
//!     across every page without pagination because issues have a
//!     bounded page count.
//!   - `POST /me/markers` — create. Enforces per-kind shape (`body`
//!     required for `note`, `region` required for `highlight`),
//!     clamps rect dims to [0, 100], and validates `page_index` against
//!     `issues.page_count`.
//!   - `PATCH /me/markers/{id}` — partial update (body / color /
//!     region / selection). Same validation gates.
//!   - `DELETE /me/markers/{id}`.
//!
//! All endpoints scope by `library_user_access` against the issue's
//! library so admins see everything and regular users only see markers
//! on issues they can access via the standard ACL. No audit log —
//! markers are user-personal data.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch},
};
use base64::Engine;
use chrono::Utc;
use entity::{issue, marker};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, ModelTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

const MAX_BODY_BYTES: usize = 10 * 1024;
const MAX_LABEL_BYTES: usize = 280;
const MAX_LIMIT: u64 = 200;
const DEFAULT_LIMIT: u64 = 50;
const KIND_BOOKMARK: &str = "bookmark";
const KIND_NOTE: &str = "note";
const KIND_HIGHLIGHT: &str = "highlight";
const ALL_KINDS: &[&str] = &[KIND_BOOKMARK, KIND_NOTE, KIND_HIGHLIGHT];

const MAX_TAGS_PER_MARKER: usize = 32;
const MAX_TAG_LEN: usize = 80;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/markers", get(list).post(create))
        .route("/me/markers/count", get(count))
        .route("/me/markers/tags", get(tags_index))
        .route("/me/markers/{id}", patch(update).delete(delete_one))
        .route("/me/issues/{issue_id}/markers", get(list_for_issue))
}

// ────────────── DTOs ──────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerView {
    pub id: String,
    pub user_id: String,
    pub series_id: String,
    pub issue_id: String,
    pub page_index: i32,
    /// `'bookmark' | 'note' | 'highlight'`. Favorite is no longer a
    /// kind — see `is_favorite`.
    pub kind: String,
    /// Star flag. Any marker can be favorited; the /bookmarks
    /// "Favorites" chip filters on this rather than on kind.
    pub is_favorite: bool,
    /// Per-user freeform tags. Empty when unset.
    pub tags: Vec<String>,
    /// `{ x, y, w, h, shape }` — see the migration doc for the precise
    /// shape contract. `None` when the marker is page-level.
    pub region: Option<serde_json::Value>,
    /// `{ text?, image_hash?, ocr_confidence? }` — populated by the
    /// reader's text / image-aware highlight modes.
    pub selection: Option<serde_json::Value>,
    /// Markdown body. Required when `kind = 'note'`, optional elsewhere.
    pub body: Option<String>,
    pub color: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Hydrated for the global `/me/markers` feed so the index page can
    /// render series + issue context without a second round-trip. Omitted
    /// from `POST`/`PATCH` responses and the per-issue lookup (those
    /// callers already have the surrounding context).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerListView {
    pub items: Vec<MarkerView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueMarkersView {
    /// Every marker the calling user has on this issue, across all
    /// pages. The reader overlay slices client-side by `page_index`.
    pub items: Vec<MarkerView>,
}

/// Cheap `SELECT COUNT(*)` over the caller's markers. Drives the
/// sidebar badge — TanStack caches it for 60s so the navigation
/// doesn't fan out a full /me/markers fetch.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerCountView {
    pub total: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TagEntryView {
    pub tag: String,
    pub count: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerTagsView {
    /// Distinct tag set across the caller's markers, with a usage
    /// count each. Sorted by count desc, then alpha.
    pub items: Vec<TagEntryView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateMarkerReq {
    pub issue_id: String,
    pub page_index: i32,
    /// `'bookmark' | 'note' | 'highlight'`.
    pub kind: String,
    /// `{ x, y, w, h, shape }` — rect dims as 0–100 percent floats
    /// normalized to the page's natural pixel dims. Omit for
    /// whole-page markers.
    #[serde(default)]
    pub region: Option<serde_json::Value>,
    /// `{ text?, image_hash?, ocr_confidence? }`.
    #[serde(default)]
    pub selection: Option<serde_json::Value>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// Star flag. Omit / false for a regular marker.
    #[serde(default)]
    pub is_favorite: Option<bool>,
    /// Freeform tag list. Trimmed + de-duped + lowercased server-side.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateMarkerReq {
    /// Sending `null` clears the field; omitting leaves it unchanged.
    #[serde(default)]
    pub body: Option<Option<String>>,
    #[serde(default)]
    pub color: Option<Option<String>>,
    /// Toggle star flag. Omit to leave unchanged.
    #[serde(default)]
    pub is_favorite: Option<bool>,
    /// Replace tag list. Send `[]` to clear, omit to leave unchanged.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub region: Option<Option<serde_json::Value>>,
    #[serde(default)]
    pub selection: Option<Option<serde_json::Value>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListQuery {
    /// One of `bookmark | note | highlight`. Omit for all.
    #[serde(default)]
    pub kind: Option<String>,
    /// Filter to a single issue.
    #[serde(default)]
    pub issue_id: Option<String>,
    /// ILIKE search against `body` and `selection->>'text'`.
    #[serde(default)]
    pub q: Option<String>,
    /// When `true`, only star-flagged markers are returned. Drives the
    /// /bookmarks Favorites chip.
    #[serde(default)]
    pub is_favorite: Option<bool>,
    /// Comma-separated tag list. Combined with `tag_match` to pick
    /// AND vs. OR semantics across selected tags.
    #[serde(default)]
    pub tags: Option<String>,
    /// `"all"` (default) — markers must have every selected tag.
    /// `"any"` — markers need at least one.
    #[serde(default)]
    pub tag_match: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

// ────────────── helpers ──────────────

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn to_view(m: marker::Model) -> MarkerView {
    MarkerView {
        id: m.id.to_string(),
        user_id: m.user_id.to_string(),
        series_id: m.series_id.to_string(),
        issue_id: m.issue_id,
        page_index: m.page_index,
        kind: m.kind,
        is_favorite: m.is_favorite,
        tags: m.tags,
        region: m.region,
        selection: m.selection,
        body: m.body,
        color: m.color,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
        series_name: None,
        series_slug: None,
        issue_slug: None,
        issue_title: None,
        issue_number: None,
    }
}

/// Normalize a freeform tag list to a stable storage shape: trim
/// whitespace, lowercase, drop empties, dedupe while preserving order
/// of first appearance. Returns an error response if any tag is too
/// long or the list exceeds the per-marker cap.
#[allow(clippy::result_large_err)]
fn normalize_tags(raw: Vec<String>) -> Result<Vec<String>, axum::response::Response> {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in raw {
        let trimmed = s.trim().to_lowercase();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_TAG_LEN {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "tag too long",
            ));
        }
        if seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    if out.len() > MAX_TAGS_PER_MARKER {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "too many tags",
        ));
    }
    Ok(out)
}

fn parse_tag_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Attach `series_name` / `series_slug` / `issue_slug` / `issue_title` /
/// `issue_number` to a page of markers in a single batched series +
/// issue fetch. Used by `/me/markers` so the index page can render
/// thumbnails and "Jump to page" links without a second round-trip.
async fn hydrate_views(
    db: &sea_orm::DatabaseConnection,
    rows: Vec<marker::Model>,
) -> Result<Vec<MarkerView>, axum::response::Response> {
    use entity::{issue, series};
    use std::collections::{HashMap, HashSet};

    let mut series_ids: HashSet<Uuid> = HashSet::new();
    let mut issue_ids: HashSet<String> = HashSet::new();
    for m in &rows {
        series_ids.insert(m.series_id);
        issue_ids.insert(m.issue_id.clone());
    }

    let series_rows = if series_ids.is_empty() {
        Vec::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
            .all(db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "markers: hydrate series failed");
                error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
            })?
    };
    let issue_rows = if issue_ids.is_empty() {
        Vec::new()
    } else {
        issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids.iter().cloned().collect::<Vec<_>>()))
            .all(db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "markers: hydrate issues failed");
                error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
            })?
    };

    let series_map: HashMap<Uuid, (String, String)> = series_rows
        .into_iter()
        .map(|s| (s.id, (s.name, s.slug)))
        .collect();
    let issue_map: HashMap<String, (String, Option<String>, Option<String>)> = issue_rows
        .into_iter()
        .map(|i| (i.id.clone(), (i.slug, i.title, i.number_raw)))
        .collect();

    Ok(rows
        .into_iter()
        .map(|m| {
            let mut view = to_view(m);
            if let Some((name, slug)) = view
                .series_id
                .parse::<Uuid>()
                .ok()
                .and_then(|id| series_map.get(&id))
                .cloned()
            {
                view.series_name = Some(name);
                view.series_slug = Some(slug);
            }
            if let Some((slug, title, number)) = issue_map.get(&view.issue_id).cloned() {
                view.issue_slug = Some(slug);
                view.issue_title = title;
                view.issue_number = number;
            }
            view
        })
        .collect())
}

/// Verify the calling user can see `issue_id`. Returns the issue row
/// (we need its `series_id` and `page_count` to populate the marker)
/// or an HTTP response on miss / forbidden.
async fn fetch_visible_issue(
    app: &AppState,
    user: &CurrentUser,
    issue_id: &str,
) -> Result<issue::Model, axum::response::Response> {
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "markers: issue fetch failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        })?;
    let Some(row) = row else {
        return Err(error(StatusCode::NOT_FOUND, "not_found", "issue not found"));
    };
    let visible = access::for_user(app, user).await;
    if !visible.contains(row.library_id) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "issue not visible",
        ));
    }
    Ok(row)
}

/// Per-kind shape + region/selection validation. Returns Ok on success
/// or a 400/422 response with a stable error code.
#[allow(clippy::result_large_err)]
fn validate_shape(
    kind: &str,
    region: Option<&serde_json::Value>,
    body: Option<&str>,
) -> Result<(), axum::response::Response> {
    if !ALL_KINDS.contains(&kind) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "validation",
            "kind must be bookmark | note | highlight",
        ));
    }
    if kind == KIND_NOTE {
        let body = body.unwrap_or("");
        if body.trim().is_empty() {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "note requires body",
            ));
        }
    }
    if kind == KIND_HIGHLIGHT && region.is_none() {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "highlight requires region",
        ));
    }
    if let Some(b) = body
        && b.len() > MAX_BODY_BYTES
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "validation",
            "body too large (max 10 KB)",
        ));
    }
    Ok(())
}

/// Clamp the `{ x, y, w, h }` numeric values to [0, 100] and ensure
/// `shape` is one of the allowed tokens. Tolerates extra fields (so
/// future client revisions don't trip an over-strict check).
#[allow(clippy::result_large_err)]
fn normalize_region(
    raw: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, axum::response::Response> {
    let Some(serde_json::Value::Object(mut obj)) = raw else {
        return Ok(None);
    };
    for key in ["x", "y", "w", "h"] {
        let Some(v) = obj.get(key) else {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region requires x, y, w, h",
            ));
        };
        let Some(n) = v.as_f64() else {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region x/y/w/h must be numbers",
            ));
        };
        let clamped = n.clamp(0.0, 100.0);
        obj.insert(
            key.to_owned(),
            serde_json::Number::from_f64(clamped)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Number(serde_json::Number::from(0))),
        );
    }
    if let Some(shape) = obj.get("shape") {
        let Some(s) = shape.as_str() else {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region.shape must be a string",
            ));
        };
        if !matches!(s, "rect" | "text" | "image") {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region.shape must be rect | text | image",
            ));
        }
    } else {
        // Default to plain rect when the client omits the shape token.
        obj.insert(
            "shape".to_owned(),
            serde_json::Value::String("rect".to_owned()),
        );
    }
    Ok(Some(serde_json::Value::Object(obj)))
}

#[allow(clippy::result_large_err)]
fn normalize_selection(
    raw: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, axum::response::Response> {
    let Some(value) = raw else { return Ok(None) };
    let serde_json::Value::Object(obj) = &value else {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "selection must be an object",
        ));
    };
    if let Some(text) = obj.get("text")
        && let Some(s) = text.as_str()
        && s.len() > MAX_LABEL_BYTES * 4
    {
        // Generous upper bound — OCR'd text can run long but anything
        // beyond ~1 KB suggests a runaway crop.
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "selection.text too long",
        ));
    }
    Ok(Some(value))
}

fn encode_cursor(updated_at: chrono::DateTime<chrono::FixedOffset>, id: Uuid) -> String {
    let s = format!("{}|{}", updated_at.to_rfc3339(), id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

#[allow(clippy::result_large_err)]
fn decode_cursor(
    raw: &str,
) -> Result<(chrono::DateTime<chrono::FixedOffset>, Uuid), axum::response::Response> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| error(StatusCode::BAD_REQUEST, "validation", "bad cursor"))?;
    let decoded = String::from_utf8(bytes)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "validation", "bad cursor"))?;
    let (ts, id) = decoded
        .rsplit_once('|')
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "validation", "bad cursor"))?;
    let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "validation", "bad cursor"))?;
    let parsed_id = Uuid::parse_str(id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "validation", "bad cursor"))?;
    Ok((parsed_ts, parsed_id))
}

// ────────────── handlers ──────────────

#[utoipa::path(
    get,
    path = "/me/markers",
    params(
        ("kind" = Option<String>, Query,),
        ("issue_id" = Option<String>, Query,),
        ("q" = Option<String>, Query,),
        ("is_favorite" = Option<bool>, Query,),
        ("tags" = Option<String>, Query,),
        ("tag_match" = Option<String>, Query,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = MarkerListView))
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let mut select = marker::Entity::find()
        .filter(marker::Column::UserId.eq(user.id))
        .order_by_desc(marker::Column::UpdatedAt)
        .order_by_desc(marker::Column::Id);

    if let Some(kind) = q.kind.as_deref() {
        if !ALL_KINDS.contains(&kind) {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "kind must be bookmark | note | highlight",
            );
        }
        select = select.filter(marker::Column::Kind.eq(kind));
    }
    if let Some(issue_id) = q.issue_id.as_ref() {
        select = select.filter(marker::Column::IssueId.eq(issue_id));
    }
    if let Some(true) = q.is_favorite {
        select = select.filter(marker::Column::IsFavorite.eq(true));
    }
    if let Some(raw_tags) = q.tags.as_deref().filter(|s| !s.trim().is_empty()) {
        let parsed = parse_tag_list(raw_tags);
        if !parsed.is_empty() {
            // Postgres array operators take a typed array literal.
            // `@>` (contains) implements AND semantics; `&&` (overlap)
            // implements OR. Falls through to AND when the client
            // doesn't pass a `tag_match`.
            let op = match q.tag_match.as_deref() {
                Some("any") => "&&",
                Some("all") | None => "@>",
                Some(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "validation",
                        "tag_match must be 'all' or 'any'",
                    );
                }
            };
            let sql = format!("tags {op} $1::text[]");
            select = select.filter(sea_orm::sea_query::Expr::cust_with_values(&sql, [parsed]));
        }
    }
    if let Some(needle) = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let pattern = format!("%{}%", needle.replace('%', "\\%").replace('_', "\\_"));
        // Match against `body` OR `selection->>'text'` — covers notes
        // (free-form markdown) and text-aware highlights (OCR
        // payload).
        let pattern_for_sel = pattern.clone();
        select = select.filter(
            sea_orm::Condition::any()
                .add(marker::Column::Body.like(pattern.as_str()))
                .add(sea_orm::sea_query::Expr::cust_with_values(
                    "(selection->>'text') ILIKE $1",
                    [pattern_for_sel],
                )),
        );
    }
    if let Some(c) = q.cursor.as_deref() {
        match decode_cursor(c) {
            Ok((ts, id)) => {
                // Strict-less cursor on (updated_at desc, id desc).
                select = select.filter(
                    sea_orm::Condition::any()
                        .add(marker::Column::UpdatedAt.lt(ts))
                        .add(
                            sea_orm::Condition::all()
                                .add(marker::Column::UpdatedAt.eq(ts))
                                .add(marker::Column::Id.lt(id)),
                        ),
                );
            }
            Err(resp) => return resp,
        }
    }

    let rows = match select.limit(limit + 1).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "markers: list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut rows = rows;
    let next_cursor = if rows.len() as u64 > limit {
        let extra = rows.pop();
        extra.map(|r| encode_cursor(r.updated_at, r.id))
    } else {
        None
    };

    let items = match hydrate_views(&app.db, rows).await {
        Ok(items) => items,
        Err(resp) => return resp,
    };
    Json(MarkerListView { items, next_cursor }).into_response()
}

#[utoipa::path(
    get,
    path = "/me/markers/count",
    responses((status = 200, body = MarkerCountView))
)]
pub async fn count(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    match marker::Entity::find()
        .filter(marker::Column::UserId.eq(user.id))
        .count(&app.db)
        .await
    {
        Ok(total) => Json(MarkerCountView { total }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "markers: count failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    get,
    path = "/me/markers/tags",
    responses((status = 200, body = MarkerTagsView))
)]
pub async fn tags_index(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    // Distinct tag set + per-tag count, scoped to the caller. The
    // GIN(user_id, tags) index covers the user filter; the unnest +
    // group-by happens at sql layer so we don't ship every marker
    // payload back through the orm for client-side rollup.
    use sea_orm::FromQueryResult;
    #[derive(FromQueryResult)]
    struct TagRow {
        tag: String,
        count: i64,
    }
    let rows: Vec<TagRow> =
        match TagRow::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT tag, COUNT(*)::bigint AS count \
             FROM (SELECT UNNEST(tags) AS tag FROM markers WHERE user_id = $1) t \
             GROUP BY tag \
             ORDER BY count DESC, tag ASC",
            [user.id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "markers: tag index failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
    let items: Vec<TagEntryView> = rows
        .into_iter()
        .map(|r| TagEntryView {
            tag: r.tag,
            count: r.count.max(0) as u64,
        })
        .collect();
    Json(MarkerTagsView { items }).into_response()
}

#[utoipa::path(
    get,
    path = "/me/issues/{issue_id}/markers",
    params(("issue_id" = String, Path,)),
    responses((status = 200, body = IssueMarkersView))
)]
pub async fn list_for_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
) -> impl IntoResponse {
    // ACL: fetch_visible_issue returns 404/403 if the caller can't
    // reach this issue, before we leak whether they have markers on
    // it.
    if let Err(resp) = fetch_visible_issue(&app, &user, &issue_id).await {
        return resp;
    }
    let rows = match marker::Entity::find()
        .filter(marker::Column::UserId.eq(user.id))
        .filter(marker::Column::IssueId.eq(issue_id))
        .order_by_asc(marker::Column::PageIndex)
        .order_by_asc(marker::Column::CreatedAt)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "markers: per-issue list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let items: Vec<MarkerView> = rows.into_iter().map(to_view).collect();
    Json(IssueMarkersView { items }).into_response()
}

#[utoipa::path(
    post,
    path = "/me/markers",
    request_body = CreateMarkerReq,
    responses((status = 201, body = MarkerView))
)]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateMarkerReq>,
) -> impl IntoResponse {
    let issue_row = match fetch_visible_issue(&app, &user, &req.issue_id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let body = req
        .body
        .as_ref()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Err(resp) = validate_shape(&req.kind, req.region.as_ref(), body.as_deref()) {
        return resp;
    }
    let region = match normalize_region(req.region) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let selection = match normalize_selection(req.selection) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let page_count = issue_row.page_count.unwrap_or(i32::MAX);
    if req.page_index < 0 || req.page_index >= page_count {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "page_index out of range",
        );
    }

    let tags = match req.tags {
        Some(t) => match normalize_tags(t) {
            Ok(v) => v,
            Err(resp) => return resp,
        },
        None => Vec::new(),
    };

    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let am = marker::ActiveModel {
        id: Set(id),
        user_id: Set(user.id),
        series_id: Set(issue_row.series_id),
        issue_id: Set(issue_row.id),
        page_index: Set(req.page_index),
        kind: Set(req.kind.clone()),
        is_favorite: Set(req.is_favorite.unwrap_or(false)),
        tags: Set(tags),
        region: Set(region),
        selection: Set(selection),
        body: Set(body),
        color: Set(req
            .color
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let saved = match am.insert(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            // Translate the schema-level CHECK violations into stable
            // 422s so the client can surface a precise message
            // instead of an opaque 500.
            let msg = e.to_string();
            if msg.contains("markers_body_required_for_note_chk") {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "note requires body",
                );
            }
            if msg.contains("markers_region_required_for_highlight_chk") {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "highlight requires region",
                );
            }
            if msg.contains("markers_body_size_chk") {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "body too large (max 10 KB)",
                );
            }
            tracing::error!(error = %e, "markers: create failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    (StatusCode::CREATED, Json(to_view(saved))).into_response()
}

#[utoipa::path(
    patch,
    path = "/me/markers/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateMarkerReq,
    responses((status = 200, body = MarkerView))
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateMarkerReq>,
) -> impl IntoResponse {
    let row = match marker::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "marker not found"),
        Err(e) => {
            tracing::error!(error = %e, "markers: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.user_id != user.id {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your marker");
    }

    // Apply diffs while preserving the per-kind invariants.
    let mut next_body = row.body.clone();
    if let Some(body_opt) = req.body.as_ref() {
        next_body = body_opt
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
    }
    let mut next_region = row.region.clone();
    if let Some(region_opt) = req.region {
        next_region = match normalize_region(region_opt) {
            Ok(r) => r,
            Err(resp) => return resp,
        };
    }
    let mut next_selection = row.selection.clone();
    if let Some(selection_opt) = req.selection {
        next_selection = match normalize_selection(selection_opt) {
            Ok(s) => s,
            Err(resp) => return resp,
        };
    }
    if let Err(resp) = validate_shape(&row.kind, next_region.as_ref(), next_body.as_deref()) {
        return resp;
    }

    let next_tags = match req.tags {
        Some(t) => match normalize_tags(t) {
            Ok(v) => Some(v),
            Err(resp) => return resp,
        },
        None => None,
    };

    let mut am: marker::ActiveModel = row.into();
    am.body = Set(next_body);
    am.region = Set(next_region);
    am.selection = Set(next_selection);
    if let Some(color_opt) = req.color {
        am.color = Set(color_opt
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()));
    }
    if let Some(fav) = req.is_favorite {
        am.is_favorite = Set(fav);
    }
    if let Some(tags) = next_tags {
        am.tags = Set(tags);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "markers: update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    Json(to_view(updated)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/markers/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match marker::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "marker not found"),
        Err(e) => {
            tracing::error!(error = %e, "markers: fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.user_id != user.id {
        return error(StatusCode::FORBIDDEN, "forbidden", "not your marker");
    }
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "markers: delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}
