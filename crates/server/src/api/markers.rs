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
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine;
use chrono::Utc;
use entity::{issue, marker};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, ModelTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

/// Slim error newtype for validator helpers — `Result<T, MarkerError>`
/// avoids the `clippy::result_large_err` lint that `axum::response::Response`
/// triggers (Response is ~200 bytes; MarkerError is one pointer + one
/// status). Materialises the canonical envelope at the HTTP boundary
/// via `IntoResponse`. M3 of code-quality-cleanup-1.0.
struct MarkerError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl MarkerError {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
}

impl axum::response::IntoResponse for MarkerError {
    fn into_response(self) -> axum::response::Response {
        error(self.status, self.code, self.message)
    }
}

const MAX_BODY_BYTES: usize = 10 * 1024;
const MAX_LABEL_BYTES: usize = 280;
const MAX_LIMIT: u64 = 200;
const DEFAULT_LIMIT: u64 = 50;
const KIND_BOOKMARK: &str = "bookmark";
const KIND_NOTE: &str = "note";
const KIND_FAVORITE: &str = "favorite";
const KIND_HIGHLIGHT: &str = "highlight";
const ALL_KINDS: &[&str] = &[KIND_BOOKMARK, KIND_NOTE, KIND_FAVORITE, KIND_HIGHLIGHT];

const MAX_TAGS_PER_MARKER: usize = 32;
const MAX_TAG_LEN: usize = 80;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(create))
        .routes(routes!(count))
        .routes(routes!(search))
        .routes(routes!(tags_index))
        .routes(routes!(update))
        .routes(routes!(delete_one))
        .routes(routes!(list_for_issue))
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

/// Global-search M2 — markers as a 4th category. The hit shape is
/// trimmed of fields the modal / `/search` page don't need
/// (color, tags, full timestamps) and carries the `<mark>`-wrapped
/// snippet inline. ACL is per-caller (`/me/...`), so a hit lists
/// only markers the user owns.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerSearchView {
    pub items: Vec<MarkerSearchHit>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkerSearchHit {
    pub id: String,
    /// `'bookmark' | 'note' | 'favorite' | 'highlight'`.
    pub kind: String,
    pub issue_id: String,
    pub series_id: String,
    pub page_index: i32,
    /// `{ x, y, w, h, shape }` when the marker is region-scoped;
    /// `None` for page-level markers. Used by the modal to render a
    /// crop thumbnail.
    pub region: Option<serde_json::Value>,
    /// `ts_headline()` excerpt over `body` and `selection->>'text'`
    /// with `<mark>…</mark>` around matched terms. Same sanitiser as
    /// the series + issue snippets on the client.
    pub snippet: Option<String>,
    /// Hydrated series + issue context so a marker hit knows enough
    /// to render a "Jump to page" link + a series label without a
    /// second round-trip.
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

/// `kind` filter values for `GET /me/markers` — typed enum so a
/// bad value rejects at deserialize time (audit-remediation M9.4).
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MarkerKindFilter {
    Bookmark,
    Note,
    Favorite,
    Highlight,
}

impl MarkerKindFilter {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Bookmark => KIND_BOOKMARK,
            Self::Note => KIND_NOTE,
            Self::Favorite => KIND_FAVORITE,
            Self::Highlight => KIND_HIGHLIGHT,
        }
    }
}

/// `tag_match` mode for the marker list — AND vs OR over selected tags.
#[derive(Debug, Default, Clone, Copy, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TagMatchMode {
    /// Markers must have every selected tag (default).
    #[default]
    All,
    /// Markers need at least one of the selected tags.
    Any,
}

impl TagMatchMode {
    pub fn sql_op(self) -> &'static str {
        match self {
            Self::All => "@>",
            Self::Any => "&&",
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListQuery {
    /// Filter by marker kind. Omit for all.
    #[serde(default)]
    pub kind: Option<MarkerKindFilter>,
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
    /// AND/OR semantics over the `tags` filter. Defaults to `all`.
    #[serde(default)]
    pub tag_match: Option<TagMatchMode>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

// ────────────── helpers ──────────────

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
fn normalize_tags(raw: Vec<String>) -> Result<Vec<String>, MarkerError> {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in raw {
        let trimmed = s.trim().to_lowercase();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_TAG_LEN {
            return Err(MarkerError::new(
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
        return Err(MarkerError::new(
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
/// or a [`MarkerError`] on miss / forbidden / DB failure. Caller
/// converts to `Response` via `IntoResponse` at the HTTP boundary.
async fn fetch_visible_issue(
    app: &AppState,
    user: &CurrentUser,
    issue_id: &str,
) -> Result<issue::Model, MarkerError> {
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "markers: issue fetch failed");
            MarkerError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        })?;
    let Some(row) = row else {
        return Err(MarkerError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "issue not found",
        ));
    };
    let visible = access::for_user(app, user).await;
    if !visible.contains(row.library_id) {
        return Err(MarkerError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "issue not visible",
        ));
    }
    Ok(row)
}

/// Per-kind shape + region/selection validation. Returns Ok on success
/// or a 400/422 response with a stable error code.
fn validate_shape(
    kind: &str,
    region: Option<&serde_json::Value>,
    body: Option<&str>,
) -> Result<(), MarkerError> {
    if !ALL_KINDS.contains(&kind) {
        return Err(MarkerError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "kind must be bookmark | note | favorite | highlight",
        ));
    }
    if kind == KIND_NOTE {
        let body = body.unwrap_or("");
        if body.trim().is_empty() {
            return Err(MarkerError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "note requires body",
            ));
        }
    }
    if kind == KIND_HIGHLIGHT && region.is_none() {
        return Err(MarkerError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "highlight requires region",
        ));
    }
    if let Some(b) = body
        && b.len() > MAX_BODY_BYTES
    {
        return Err(MarkerError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "body too large (max 10 KB)",
        ));
    }
    Ok(())
}

/// Clamp the `{ x, y, w, h }` numeric values to [0, 100] and ensure
/// `shape` is one of the allowed tokens. Tolerates extra fields (so
/// future client revisions don't trip an over-strict check).
fn normalize_region(
    raw: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, MarkerError> {
    let Some(serde_json::Value::Object(mut obj)) = raw else {
        return Ok(None);
    };
    for key in ["x", "y", "w", "h"] {
        let Some(v) = obj.get(key) else {
            return Err(MarkerError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region requires x, y, w, h",
            ));
        };
        let Some(n) = v.as_f64() else {
            return Err(MarkerError::new(
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
            return Err(MarkerError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "region.shape must be a string",
            ));
        };
        if !matches!(s, "rect" | "text" | "image") {
            return Err(MarkerError::new(
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

fn normalize_selection(
    raw: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, MarkerError> {
    let Some(value) = raw else { return Ok(None) };
    let serde_json::Value::Object(obj) = &value else {
        return Err(MarkerError::new(
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
        return Err(MarkerError::new(
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

fn decode_cursor(raw: &str) -> Result<(chrono::DateTime<chrono::FixedOffset>, Uuid), MarkerError> {
    let bad = || MarkerError::new(StatusCode::BAD_REQUEST, "validation", "bad cursor");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| bad())?;
    let decoded = String::from_utf8(bytes).map_err(|_| bad())?;
    let (ts, id) = decoded.rsplit_once('|').ok_or_else(bad)?;
    let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts).map_err(|_| bad())?;
    let parsed_id = Uuid::parse_str(id).map_err(|_| bad())?;
    Ok((parsed_ts, parsed_id))
}

// ────────────── handlers ──────────────

#[utoipa::path(
    operation_id = "markers_list",    get,
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
#[handler]
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

    if let Some(kind) = q.kind {
        // Enum-typed query param (audit-remediation M9.4) — serde
        // rejects bad values at deserialize time (400 from axum).
        select = select.filter(marker::Column::Kind.eq(kind.as_db_str()));
    }
    if let Some(issue_id) = q.issue_id.as_ref() {
        select = select.filter(marker::Column::IssueId.eq(issue_id));
    }
    if let Some(true) = q.is_favorite {
        // v0.3.44: union over the two favorite shapes — the legacy
        // `is_favorite=true` flag (any kind) AND the new standalone
        // `kind='favorite'` rows the page-level star button creates.
        // Without the OR, switching the chrome's star button to
        // `kind='favorite'` would hide pre-2026-05-20 favorited
        // highlights/notes from the favorites list. See
        // [m20260520_000001_marker_kind_favorite].
        use sea_orm::Condition;
        select = select.filter(
            Condition::any()
                .add(marker::Column::IsFavorite.eq(true))
                .add(marker::Column::Kind.eq(KIND_FAVORITE)),
        );
    }
    if let Some(raw_tags) = q.tags.as_deref().filter(|s| !s.trim().is_empty()) {
        let parsed = parse_tag_list(raw_tags);
        if !parsed.is_empty() {
            // Postgres array operators take a typed array literal.
            // `@>` (contains) implements AND semantics; `&&` (overlap)
            // implements OR. Default is AND when the client omits
            // `tag_match` (audit-remediation M9.4).
            let op = q.tag_match.unwrap_or_default().sql_op();
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
            Err(resp) => return resp.into_response(),
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
    operation_id = "markers_count",    get,
    path = "/me/markers/count",
    responses((status = 200, body = MarkerCountView))
)]
#[handler]
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
    operation_id = "markers_search",    get,
    path = "/me/markers/search",
    params(
        ("q" = String, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = MarkerSearchView))
)]
#[handler]
pub async fn search(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQueryParams>,
) -> impl IntoResponse {
    // Same minimum-length contract as the modal/page surfaces.
    let text = q.q.trim();
    if text.len() < 2 {
        return Json(MarkerSearchView { items: Vec::new() }).into_response();
    }
    if text.len() > MARKER_SEARCH_MAX_QUERY_LEN {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", "q too long");
    }
    let limit = q
        .limit
        .unwrap_or(MARKER_SEARCH_DEFAULT_LIMIT)
        .clamp(1, MARKER_SEARCH_MAX_LIMIT);

    // ILIKE pattern shared with the `/me/markers?q=` path. We escape
    // the user's `%` / `_` so a query like "10_things" doesn't get
    // misread as an LIKE wildcard. The escape is then forwarded to
    // ts_headline as a websearch query for highlighting.
    let pattern = format!("%{}%", text.replace('%', "\\%").replace('_', "\\_"));
    let pattern_for_sel = pattern.clone();

    let rows = match marker::Entity::find()
        .filter(marker::Column::UserId.eq(user.id))
        .filter(
            sea_orm::Condition::any()
                .add(marker::Column::Body.like(pattern.as_str()))
                .add(sea_orm::sea_query::Expr::cust_with_values(
                    "(selection->>'text') ILIKE $1",
                    [pattern_for_sel],
                )),
        )
        .order_by_desc(marker::Column::UpdatedAt)
        .order_by_desc(marker::Column::Id)
        .limit(limit)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "markers: search failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if rows.is_empty() {
        return Json(MarkerSearchView { items: Vec::new() }).into_response();
    }

    // Second pass: ts_headline excerpts. Cheap PK lookup over already-
    // fetched ids. Failures degrade silently — hit still renders with
    // body/selection text in its place.
    let snippets = fetch_marker_snippets(&app, &rows, text)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "markers: snippet fetch failed");
            std::collections::HashMap::new()
        });

    // Hydrate series + issue context.
    let hydrated = match hydrate_views(&app.db, rows).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let items: Vec<MarkerSearchHit> = hydrated
        .into_iter()
        .map(|m| {
            let snippet = snippets.get(&m.id).cloned();
            MarkerSearchHit {
                snippet,
                id: m.id,
                kind: m.kind,
                issue_id: m.issue_id,
                series_id: m.series_id,
                page_index: m.page_index,
                region: m.region,
                series_name: m.series_name,
                series_slug: m.series_slug,
                issue_slug: m.issue_slug,
                issue_title: m.issue_title,
                issue_number: m.issue_number,
            }
        })
        .collect();
    Json(MarkerSearchView { items }).into_response()
}

const MARKER_SEARCH_DEFAULT_LIMIT: u64 = 20;
const MARKER_SEARCH_MAX_LIMIT: u64 = 50;
const MARKER_SEARCH_MAX_QUERY_LEN: usize = 200;

#[derive(Debug, serde::Deserialize)]
pub struct SearchQueryParams {
    pub q: String,
    #[serde(default)]
    pub limit: Option<u64>,
}

/// `ts_headline` excerpt per marker over the concatenation of `body`
/// and `selection->>'text'`. Same shape as the series + issues
/// snippet helpers. Returns a `(marker_id → snippet)` map; markers
/// whose searchable text doesn't yield a highlight are omitted.
async fn fetch_marker_snippets(
    app: &AppState,
    rows: &[marker::Model],
    q_text: &str,
) -> Result<std::collections::HashMap<String, String>, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
    if rows.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    #[derive(Debug, FromQueryResult)]
    struct SnippetRow {
        // `markers.id` is a Postgres `uuid`, so deserialise it as
        // `Uuid` and convert to `String` for the HashMap key — that
        // matches `to_view`'s `m.id.to_string()` stringification used
        // throughout the marker DTOs.
        id: Uuid,
        snippet: Option<String>,
    }

    let mut params: Vec<Value> = Vec::with_capacity(rows.len() + 1);
    params.push(Value::from(q_text.to_string()));
    let id_placeholders: Vec<String> = rows
        .iter()
        .map(|r| {
            params.push(Value::from(r.id));
            format!("${}", params.len())
        })
        .collect();
    let sql = format!(
        r#"SELECT id,
                  ts_headline(
                    'simple',
                    COALESCE(body, '') || ' ' || COALESCE(selection->>'text', ''),
                    websearch_to_tsquery('simple', $1),
                    'MaxFragments=1, MaxWords=18, MinWords=5, ShortWord=2, StartSel=<mark>, StopSel=</mark>, HighlightAll=false'
                  ) AS snippet
             FROM markers
             WHERE id IN ({})"#,
        id_placeholders.join(",")
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    let rows: Vec<SnippetRow> = SnippetRow::find_by_statement(stmt).all(&app.db).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let s = r.snippet?;
            if s.contains("<mark>") {
                Some((r.id.to_string(), s))
            } else {
                None
            }
        })
        .collect())
}

#[utoipa::path(
    operation_id = "markers_tags_index",    get,
    path = "/me/markers/tags",
    responses((status = 200, body = MarkerTagsView))
)]
#[handler]
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
    operation_id = "markers_list_for_issue",    get,
    path = "/me/issues/{issue_id}/markers",
    params(("issue_id" = String, Path,)),
    responses((status = 200, body = IssueMarkersView))
)]
#[handler]
pub async fn list_for_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
) -> impl IntoResponse {
    // ACL: fetch_visible_issue returns 404/403 if the caller can't
    // reach this issue, before we leak whether they have markers on
    // it.
    if let Err(resp) = fetch_visible_issue(&app, &user, &issue_id).await {
        return resp.into_response();
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
    operation_id = "markers_create",    post,
    path = "/me/markers",
    request_body = CreateMarkerReq,
    responses((status = 201, body = MarkerView))
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateMarkerReq>,
) -> impl IntoResponse {
    let issue_row = match fetch_visible_issue(&app, &user, &req.issue_id).await {
        Ok(r) => r,
        Err(resp) => return resp.into_response(),
    };

    let body = req
        .body
        .as_ref()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Err(resp) = validate_shape(&req.kind, req.region.as_ref(), body.as_deref()) {
        return resp.into_response();
    }
    let region = match normalize_region(req.region) {
        Ok(r) => r,
        Err(resp) => return resp.into_response(),
    };
    let selection = match normalize_selection(req.selection) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
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
            Err(resp) => return resp.into_response(),
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
        hidden_from_log: Set(false),
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
                    StatusCode::UNPROCESSABLE_ENTITY,
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
    operation_id = "markers_update",    patch,
    path = "/me/markers/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateMarkerReq,
    responses((status = 200, body = MarkerView))
)]
#[handler]
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
            Err(resp) => return resp.into_response(),
        };
    }
    let mut next_selection = row.selection.clone();
    if let Some(selection_opt) = req.selection {
        next_selection = match normalize_selection(selection_opt) {
            Ok(s) => s,
            Err(resp) => return resp.into_response(),
        };
    }
    if let Err(resp) = validate_shape(&row.kind, next_region.as_ref(), next_body.as_deref()) {
        return resp.into_response();
    }

    let next_tags = match req.tags {
        Some(t) => match normalize_tags(t) {
            Ok(v) => Some(v),
            Err(resp) => return resp.into_response(),
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
    operation_id = "markers_delete_one",    delete,
    path = "/me/markers/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
#[handler]
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
