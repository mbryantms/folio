//! `/issues/{id}` — read, edit, and refresh-metadata endpoints.
//!
//! The DB schema column-set on `issues` is shared with the scanner, so a
//! `PATCH /issues/{id}` records its writes in `user_edited` to flag those
//! columns as sticky. The scanner's update path checks the flag set and
//! skips matching columns, preserving user edits across rescans.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use entity::{issue, library_user_access, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, Value,
    sea_query::Expr,
};

use crate::library::access;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::api::libraries::{ScanMode, ScanResp};
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::middleware::RequestContext;
use crate::state::AppState;

use super::series::{IssueDetailView, IssueLink, IssueSummaryView};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/issues", get(list))
        .route("/issues/search", get(search))
        .route(
            "/series/{series_slug}/issues/{issue_slug}",
            get(get_one).patch(update),
        )
        .route(
            "/series/{series_slug}/issues/{issue_slug}/scan",
            post(scan_issue),
        )
        .route(
            "/series/{series_slug}/issues/{issue_slug}/next",
            get(next_in_series),
        )
}

/// Resolve `(series_slug, issue_slug)` to the canonical issue row. Returns
/// the standard 404 envelope on miss for either slug. Visibility-by-library
/// is the caller's responsibility.
pub(crate) async fn find_by_slugs(
    db: &sea_orm::DatabaseConnection,
    series_slug: &str,
    issue_slug: &str,
) -> Result<issue::Model, axum::response::Response> {
    let s = match series::Entity::find()
        .filter(series::Column::Slug.eq(series_slug))
        .one(db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "series not found",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, series_slug, "series slug lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::Slug.eq(issue_slug))
        .one(db)
        .await
    {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(error(StatusCode::NOT_FOUND, "not_found", "issue not found")),
        Err(e) => {
            tracing::error!(error = %e, issue_slug, "issue slug lookup failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

// ───── GET /issues/{id} ─────

#[utoipa::path(
    get,
    path = "/series/{series_slug}/issues/{issue_slug}",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = IssueDetailView),
        (status = 404)
    )
)]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let rating = crate::api::series::lookup_user_rating(&app, user.id, "issue", &row.id).await;
    let mut view = IssueDetailView::from_model(row, &series_slug);
    view.user_rating = rating;
    Json(view).into_response()
}

// ───── PATCH /issues/{id} ─────

/// Body for `PATCH /series/{series_slug}/issues/{issue_slug}`.
///
/// Every field is optional. For nullable columns the body distinguishes:
///   - field absent     → leave column untouched
///   - field present, null → clear column, mark as user-edited
///   - field present, set  → write column, mark as user-edited
///
/// `additional_links` is replace-all: send the full desired array, or `[]`
/// to clear. Empty / whitespace-only `url` entries are rejected.
///
/// Mirrors the editable subset of ComicInfo.xml — fields the scanner reads
/// from the file. The scanner consults `user_edited` on rescan and skips
/// matching columns, so DB edits are sticky and the source file is never
/// rewritten.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct UpdateIssueReq {
    // Identity / publication
    #[serde(default, deserialize_with = "deserialize_some")]
    pub title: Option<Option<String>>,
    /// Maps to the entity's `number_raw` column (e.g. "1", "1.5", "Annual 2").
    #[serde(default, deserialize_with = "deserialize_some")]
    pub number: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub volume: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub year: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub month: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub day: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub summary: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub notes: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub publisher: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub imprint: Option<Option<String>>,

    // Credits
    #[serde(default, deserialize_with = "deserialize_some")]
    pub writer: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub penciller: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub inker: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub colorist: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub letterer: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub cover_artist: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub editor: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub translator: Option<Option<String>>,

    // Cast / setting / story
    #[serde(default, deserialize_with = "deserialize_some")]
    pub characters: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub teams: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub locations: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub alternate_series: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub story_arc: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub story_arc_number: Option<Option<String>>,

    // Classification
    #[serde(default, deserialize_with = "deserialize_some")]
    pub genre: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub tags: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub language_code: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub age_rating: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub format: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub black_and_white: Option<Option<bool>>,
    /// One of `Yes`, `YesAndRightToLeft`, `No`, or null.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub manga: Option<Option<String>>,

    // Ordering / external
    #[serde(default, deserialize_with = "deserialize_some")]
    pub sort_number: Option<Option<f64>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub web_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub gtin: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub comicvine_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub metron_id: Option<Option<i64>>,

    /// Replace-all. Each link must have a non-empty `url`.
    pub additional_links: Option<Vec<IssueLink>>,
}

fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Trim, then collapse empty strings to `None`. Matches scanner behavior so
/// the DB never contains whitespace-only / empty CSV strings.
fn norm_str(v: Option<String>) -> Option<String> {
    v.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() { None } else { Some(t) }
    })
}

#[utoipa::path(
    patch,
    path = "/series/{series_slug}/issues/{issue_slug}",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = UpdateIssueReq,
    responses(
        (status = 200, body = IssueDetailView),
        (status = 400, description = "validation error"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Json(req): Json<UpdateIssueReq>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // ── pre-write validation (everything that can reject up front, before
    // any active model writes) ──
    if let Some(links) = req.additional_links.as_ref() {
        for l in links {
            if l.url.trim().is_empty() {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation.additional_links",
                    "each link needs a non-empty url",
                );
            }
            // No URL parsing — accept anything non-empty so the user can
            // store internal notes like "wiki:foo". Downstream renderers
            // treat the value as plain text if it's not a valid href.
        }
    }
    if let Some(Some(f)) = req.sort_number
        && !f.is_finite()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.sort_number",
            "sort_number must be finite",
        );
    }
    if let Some(Some(y)) = req.year
        && !(1800..=2999).contains(&y)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.year",
            "year out of range",
        );
    }
    if let Some(Some(m)) = req.month
        && !(1..=12).contains(&m)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.month",
            "month must be 1..=12",
        );
    }
    if let Some(Some(d)) = req.day
        && !(1..=31).contains(&d)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.day",
            "day must be 1..=31",
        );
    }
    if let Some(Some(v)) = req.volume
        && !(0..=9999).contains(&v)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.volume",
            "volume out of range",
        );
    }
    if let Some(Some(ref s)) = req.manga {
        let t = s.trim();
        if !matches!(t, "Yes" | "YesAndRightToLeft" | "No") {
            return error(
                StatusCode::BAD_REQUEST,
                "validation.manga",
                "manga must be Yes, YesAndRightToLeft, or No",
            );
        }
    }
    if let Some(Some(ref s)) = req.language_code
        && s.len() > 16
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.language_code",
            "language_code too long",
        );
    }

    // Carry forward existing edited-flag set; new writes append to it.
    let mut edited: BTreeSet<String> = match row.user_edited.as_array() {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        None => BTreeSet::new(),
    };

    // Track what changed so the audit payload reflects the actual diff.
    let mut changes = serde_json::Map::new();

    let mut am: issue::ActiveModel = row.clone().into();
    let mut touched = false;

    // ── nullable string columns ──
    macro_rules! apply_str {
        ($req_field:ident, $col:ident, $name:literal) => {
            if let Some(v) = req.$req_field {
                let normalized = norm_str(v);
                am.$col = Set(normalized.clone());
                edited.insert($name.into());
                changes.insert($name.into(), serde_json::json!(normalized));
                touched = true;
            }
        };
    }
    apply_str!(title, title, "title");
    apply_str!(number, number_raw, "number_raw");
    apply_str!(summary, summary, "summary");
    apply_str!(notes, notes, "notes");
    apply_str!(publisher, publisher, "publisher");
    apply_str!(imprint, imprint, "imprint");
    apply_str!(writer, writer, "writer");
    apply_str!(penciller, penciller, "penciller");
    apply_str!(inker, inker, "inker");
    apply_str!(colorist, colorist, "colorist");
    apply_str!(letterer, letterer, "letterer");
    apply_str!(cover_artist, cover_artist, "cover_artist");
    apply_str!(editor, editor, "editor");
    apply_str!(translator, translator, "translator");
    apply_str!(characters, characters, "characters");
    apply_str!(teams, teams, "teams");
    apply_str!(locations, locations, "locations");
    apply_str!(alternate_series, alternate_series, "alternate_series");
    apply_str!(story_arc, story_arc, "story_arc");
    apply_str!(story_arc_number, story_arc_number, "story_arc_number");
    apply_str!(genre, genre, "genre");
    apply_str!(tags, tags, "tags");
    apply_str!(language_code, language_code, "language_code");
    apply_str!(age_rating, age_rating, "age_rating");
    apply_str!(format, format, "format");
    apply_str!(manga, manga, "manga");
    apply_str!(web_url, web_url, "web_url");
    apply_str!(gtin, gtin, "gtin");

    // ── nullable scalar columns ──
    if let Some(v) = req.volume {
        am.volume = Set(v);
        edited.insert("volume".into());
        changes.insert("volume".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.year {
        am.year = Set(v);
        edited.insert("year".into());
        changes.insert("year".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.month {
        am.month = Set(v);
        edited.insert("month".into());
        changes.insert("month".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.day {
        am.day = Set(v);
        edited.insert("day".into());
        changes.insert("day".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.black_and_white {
        am.black_and_white = Set(v);
        edited.insert("black_and_white".into());
        changes.insert("black_and_white".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.sort_number {
        am.sort_number = Set(v);
        edited.insert("sort_number".into());
        changes.insert("sort_number".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.comicvine_id {
        am.comicvine_id = Set(v);
        edited.insert("comicvine_id".into());
        changes.insert("comicvine_id".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.metron_id {
        am.metron_id = Set(v);
        edited.insert("metron_id".into());
        changes.insert("metron_id".into(), serde_json::json!(v));
        touched = true;
    }

    if let Some(links) = req.additional_links {
        let normalized: Vec<IssueLink> = links
            .into_iter()
            .map(|l| IssueLink {
                label: norm_str(l.label),
                url: l.url.trim().to_owned(),
            })
            .collect();
        let json = serde_json::to_value(&normalized).unwrap_or(serde_json::json!([]));
        am.additional_links = Set(json.clone());
        // additional_links has no scanner counterpart so we don't add it to
        // `user_edited`; the scanner never touches it.
        changes.insert("additional_links".into(), json);
        touched = true;
    }

    if !touched {
        return Json(IssueDetailView::from_model(row, &series_slug)).into_response();
    }

    let edited_arr: Vec<String> = edited.into_iter().collect();
    am.user_edited = Set(serde_json::json!(edited_arr));
    am.updated_at = Set(chrono::Utc::now().fixed_offset());

    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(issue_id = %id, error = %e, "update issue failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.update",
            target_type: Some("issue"),
            target_id: Some(updated.id.clone()),
            payload: serde_json::json!({
                "changes": changes,
                "user_edited": edited_arr,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(IssueDetailView::from_model(updated, &series_slug)).into_response()
}

// ───── POST /issues/{id}/scan ─────

/// Optional query params for the scan-issue endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct ScanIssueQuery {
    /// Defaults to `true` — clicking "Scan issue" is an explicit user
    /// request, so re-parse the file even if its mtime hasn't moved. The
    /// query string can opt back into the cheap fast path with `?force=false`
    /// (mostly useful for the file-watch trigger, not the UI).
    #[serde(default = "default_true")]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

#[utoipa::path(
    post,
    path = "/series/{series_slug}/issues/{issue_slug}/scan",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
        ("force" = Option<bool>, Query, description = "Bypass the size+mtime fast path. Defaults to true."),
    ),
    responses(
        (status = 202, description = "issue scan job enqueued"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
pub async fn scan_issue(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Query(q): Query<ScanIssueQuery>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    let outcome = match app
        .jobs
        .coalesce_scoped_scan(
            row.library_id,
            row.series_id,
            None,
            crate::jobs::scan_series::JobKind::Issue,
            Some(id.clone()),
            q.force,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(issue_id = %id, error = %e, "scan_issue enqueue failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.scan",
            target_type: Some("issue"),
            target_id: Some(id.clone()),
            payload: serde_json::json!({
                "series_id": row.series_id.to_string(),
                "force": q.force,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let mode = if q.force {
        ScanMode::MetadataRefresh
    } else {
        ScanMode::Normal
    };
    (
        StatusCode::ACCEPTED,
        Json(ScanResp {
            scan_id: outcome.scan_id().to_string(),
            state: if outcome.was_coalesced() {
                "coalesced"
            } else {
                "queued"
            },
            coalesced: outcome.was_coalesced(),
            kind: "issue",
            library_id: row.library_id.to_string(),
            mode: mode.as_str(),
            coalesced_into: outcome
                .was_coalesced()
                .then(|| outcome.scan_id().to_string()),
            queued_followup: false,
            reason: mode.reason().to_owned(),
            series_id: Some(row.series_id.to_string()),
            issue_id: Some(id),
        }),
    )
        .into_response()
}

// ───── GET /issues/{id}/next ─────

#[derive(Debug, Default, Deserialize)]
pub struct NextInSeriesQuery {
    /// Number of upcoming issues to return. Clamped to 1..=20, default 5.
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NextInSeriesView {
    pub items: Vec<IssueSummaryView>,
}

/// Returns the next N issues in the same series, ordered by `sort_number`
/// ASC (NULLS LAST) with `id` as a stable tie-breaker. Removed / soft-deleted
/// issues are filtered out so the list mirrors the series page. The current
/// issue is excluded from the result.
#[utoipa::path(
    get,
    path = "/series/{series_slug}/issues/{issue_slug}/next",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
        ("limit" = Option<u64>, Query, description = "Max upcoming issues (1..=20, default 5)"),
    ),
    responses(
        (status = 200, body = NextInSeriesView),
        (status = 404, description = "issue not found"),
    )
)]
pub async fn next_in_series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Query(q): Query<NextInSeriesQuery>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let limit = q.limit.unwrap_or(5).clamp(1, 20);

    // Match the series-page sort: sort_number ASC NULLS LAST, then id.
    // The "next" cursor is the (sort_number, id) tuple of the current row;
    // anything strictly after is a candidate.
    let mut select = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.series_id))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::Id.ne(row.id.clone()));

    // Sort handling — emulate "NULLS LAST" via a synthesized ASC bool.
    let nulls_last = Expr::cust("sort_number IS NULL");
    select = select
        .order_by_asc(nulls_last)
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id);

    // Cursor: only rows that come *after* the current row in the same sort.
    // (sort_number IS NULL) covers the "current row has a number, NULL rows
    // come after"; the > / = clauses cover the strict-greater + tiebreak.
    select = match row.sort_number {
        Some(curr) => select.filter(Expr::cust_with_values(
            "(sort_number IS NULL) OR (sort_number > $1) OR (sort_number = $1 AND id > $2)",
            vec![Value::from(curr), Value::from(row.id.clone())],
        )),
        // Current row has no sort_number — NULLS LAST means the only "after"
        // candidates are other NULL rows with a larger id.
        None => select
            .filter(Expr::cust("sort_number IS NULL"))
            .filter(issue::Column::Id.gt(row.id.clone())),
    };

    let rows: Vec<issue::Model> = match select.limit(limit).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_in_series query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let items = rows
        .into_iter()
        .map(|m| IssueSummaryView::from_model(m, &series_slug))
        .collect();
    Json(NextInSeriesView { items }).into_response()
}

// ───── GET /issues/search ─────

const SEARCH_MAX_QUERY_LEN: usize = 200;
const SEARCH_DEFAULT_LIMIT: u64 = 20;
const SEARCH_MAX_LIMIT: u64 = 50;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    /// Optional series-id constraint; useful when a CBL entry's series
    /// already resolves but the issue number is missing/ambiguous.
    pub series_id: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueSearchView {
    pub items: Vec<IssueSearchHit>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueSearchHit {
    #[serde(flatten)]
    pub issue: IssueSummaryView,
    pub series_name: String,
}

/// Cross-library issue search backed by `issues.search_doc`. Used by
/// the CBL Resolution UI to pick a manual match for ambiguous /
/// missing entries. Visibility-filtered to the caller's libraries.
/// Cross-library issues listing with the same metadata-facet surface
/// the library grid offers for series. Hooks into `series.rs`'s shared
/// cursor helpers so pagination encoding stays consistent.
#[derive(Debug, Deserialize)]
pub struct ListIssuesCrossQuery {
    pub library: Option<Uuid>,
    pub q: Option<String>,
    #[serde(default = "default_cross_limit")]
    pub limit: u64,
    #[serde(default)]
    pub sort: Option<super::series::IssueSort>,
    #[serde(default)]
    pub order: Option<super::series::SortOrder>,
    #[serde(default)]
    pub cursor: Option<String>,
    /// Inclusive bounds on `issue.year`. NULLs are excluded when
    /// either bound is set.
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    /// CSV facets — server splits and applies as IN-set or
    /// includes-any against the issues' own metadata columns.
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub age_rating: Option<String>,
    #[serde(default)]
    pub genres: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub writers: Option<String>,
    #[serde(default)]
    pub pencillers: Option<String>,
    #[serde(default)]
    pub inkers: Option<String>,
    #[serde(default)]
    pub colorists: Option<String>,
    #[serde(default)]
    pub letterers: Option<String>,
    #[serde(default)]
    pub cover_artists: Option<String>,
    #[serde(default)]
    pub editors: Option<String>,
    #[serde(default)]
    pub translators: Option<String>,
    #[serde(default)]
    pub characters: Option<String>,
    #[serde(default)]
    pub teams: Option<String>,
    #[serde(default)]
    pub locations: Option<String>,
    /// Inclusive bounds on the calling user's per-issue rating
    /// (0..=5). Issues the user hasn't rated are excluded when set.
    #[serde(default)]
    pub user_rating_min: Option<f64>,
    #[serde(default)]
    pub user_rating_max: Option<f64>,
}

fn default_cross_limit() -> u64 {
    50
}

const MAX_QUERY_LEN: usize = 200;

#[utoipa::path(
    get,
    path = "/issues",
    responses((status = 200, body = super::series::IssueListView))
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListIssuesCrossQuery>,
) -> impl IntoResponse {
    use super::series::{
        IssueListView, IssueSort, SortOrder, apply_f64_cursor, apply_i32_cursor, apply_ts_cursor,
        clamp_limit, encode_cursor, parse_cursor, split_csv,
    };

    if let Some(s) = q.q.as_ref()
        && s.len() > MAX_QUERY_LEN
    {
        return error(StatusCode::BAD_REQUEST, "validation", "q too long");
    }

    let visible = access::for_user(&app, &user).await;
    let empty = || {
        Json(IssueListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response()
    };

    let mut select = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null());

    if let Some(lib) = q.library {
        if !visible.contains(lib) {
            return empty();
        }
        select = select.filter(issue::Column::LibraryId.eq(lib));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return empty();
        }
        select = select.filter(
            issue::Column::LibraryId.is_in(visible.allowed.iter().copied().collect::<Vec<_>>()),
        );
    }

    // Year range (inclusive). NULLs implicitly excluded by the
    // comparison.
    if let Some(y) = q.year_from {
        select = select.filter(issue::Column::Year.gte(y));
    }
    if let Some(y) = q.year_to {
        select = select.filter(issue::Column::Year.lte(y));
    }
    // IN-set facets on direct issue columns.
    if let Some(raw) = q.publisher.as_deref() {
        let v = split_csv(raw);
        if !v.is_empty() {
            select = select.filter(issue::Column::Publisher.is_in(v));
        }
    }
    if let Some(raw) = q.language.as_deref() {
        let v = split_csv(raw);
        if !v.is_empty() {
            select = select.filter(issue::Column::LanguageCode.is_in(v));
        }
    }
    if let Some(raw) = q.age_rating.as_deref() {
        let v = split_csv(raw);
        if !v.is_empty() {
            select = select.filter(issue::Column::AgeRating.is_in(v));
        }
    }
    // CSV-includes-any against issue's own CSV columns. Splitting on
    // `[,;]` mirrors the series `aggregate_csv` so chip values match
    // here.
    let csv_facets: [(Option<&str>, &'static str); 11] = [
        (q.genres.as_deref(), "genre"),
        (q.tags.as_deref(), "tags"),
        (q.writers.as_deref(), "writer"),
        (q.pencillers.as_deref(), "penciller"),
        (q.inkers.as_deref(), "inker"),
        (q.colorists.as_deref(), "colorist"),
        (q.letterers.as_deref(), "letterer"),
        (q.cover_artists.as_deref(), "cover_artist"),
        (q.editors.as_deref(), "editor"),
        (q.translators.as_deref(), "translator"),
        (q.characters.as_deref(), "characters"),
    ];
    for (raw, column) in csv_facets {
        let Some(raw) = raw else { continue };
        let values = split_csv(raw);
        if values.is_empty() {
            continue;
        }
        let lowered: Vec<String> = values.iter().map(|s| s.to_lowercase()).collect();
        let sql = format!(
            "EXISTS (SELECT 1 FROM unnest( \
               regexp_split_to_array(coalesce(issues.{column}, ''), '[,;]') \
             ) AS piece WHERE lower(trim(piece)) = ANY($1))",
        );
        select = select.filter(Expr::cust_with_values(&sql, [Value::from(lowered)]));
    }
    // Teams and locations weren't expressible in the loop above
    // because the column-list literal is fixed-size. Keep the same
    // shape as the loop body.
    for (raw, column) in [
        (q.teams.as_deref(), "teams"),
        (q.locations.as_deref(), "locations"),
    ] {
        let Some(raw) = raw else { continue };
        let values = split_csv(raw);
        if values.is_empty() {
            continue;
        }
        let lowered: Vec<String> = values.iter().map(|s| s.to_lowercase()).collect();
        let sql = format!(
            "EXISTS (SELECT 1 FROM unnest( \
               regexp_split_to_array(coalesce(issues.{column}, ''), '[,;]') \
             ) AS piece WHERE lower(trim(piece)) = ANY($1))",
        );
        select = select.filter(Expr::cust_with_values(&sql, [Value::from(lowered)]));
    }
    // Per-user rating bounds — issues are keyed by their string id
    // (BLAKE3) in `user_ratings.target_id`.
    if q.user_rating_min.is_some() || q.user_rating_max.is_some() {
        let min = q.user_rating_min.unwrap_or(0.0);
        let max = q.user_rating_max.unwrap_or(5.0);
        if !(0.0..=5.0).contains(&min) || !(0.0..=5.0).contains(&max) || min > max {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "user_rating bounds must be 0..=5 with min <= max",
            );
        }
        select = select.filter(Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM user_ratings ur \
             WHERE ur.user_id = $1 \
               AND ur.target_type = 'issue' \
               AND ur.target_id = issues.id \
               AND ur.rating BETWEEN $2 AND $3)",
            [Value::from(user.id), Value::from(min), Value::from(max)],
        ));
    }

    let limit = clamp_limit(q.limit);
    let q_text = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());

    // Search mode: rank by ts_rank_cd; cursor + sort options are
    // ignored. Search results are always a single ranked page.
    if let Some(text) = q_text {
        let select = select
            .filter(Expr::cust_with_values(
                "search_doc @@ websearch_to_tsquery('simple', $1)",
                [text],
            ))
            .order_by_desc(Expr::cust_with_values(
                "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
                [text],
            ))
            .limit(limit);
        let rows = match select.all(&app.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "list issues cross search failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        // Search mode is one ranked page — items.len() IS the total.
        let total = Some(rows.len() as i64);
        return hydrate_and_respond(&app, rows, None, total).await;
    }

    let sort = q.sort.unwrap_or_default();
    let order = q.order.unwrap_or(match sort {
        IssueSort::Number => SortOrder::Asc,
        IssueSort::CreatedAt
        | IssueSort::UpdatedAt
        | IssueSort::Year
        | IssueSort::PageCount
        | IssueSort::UserRating => SortOrder::Desc,
    });
    let asc = matches!(order, SortOrder::Asc);

    // First-page-only count — see `series::list` for the rationale.
    use sea_orm::PaginatorTrait;
    let total: Option<i64> = if q.cursor.is_none() {
        match select.clone().count(&app.db).await {
            Ok(n) => Some(n as i64),
            Err(e) => {
                tracing::error!(error = %e, "list issues cross count failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    if let Some(cursor) = q.cursor.as_deref() {
        let Ok((c_value, c_id)) = parse_cursor(cursor) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
        };
        select = match sort {
            IssueSort::Number => {
                let parsed = if c_value.is_empty() {
                    None
                } else {
                    match c_value.parse::<f64>() {
                        Ok(v) => Some(v),
                        Err(_) => {
                            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                        }
                    }
                };
                apply_f64_cursor(
                    select,
                    issue::Column::SortNumber,
                    issue::Column::Id,
                    parsed,
                    c_id,
                    asc,
                )
            }
            IssueSort::Year => {
                let parsed = if c_value.is_empty() {
                    None
                } else {
                    match c_value.parse::<i32>() {
                        Ok(n) => Some(n),
                        Err(_) => {
                            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                        }
                    }
                };
                apply_i32_cursor(
                    select,
                    issue::Column::Year,
                    issue::Column::Id,
                    parsed,
                    c_id,
                    asc,
                )
            }
            IssueSort::PageCount => {
                let parsed = if c_value.is_empty() {
                    None
                } else {
                    match c_value.parse::<i32>() {
                        Ok(n) => Some(n),
                        Err(_) => {
                            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                        }
                    }
                };
                apply_i32_cursor(
                    select,
                    issue::Column::PageCount,
                    issue::Column::Id,
                    parsed,
                    c_id,
                    asc,
                )
            }
            IssueSort::CreatedAt => match chrono::DateTime::parse_from_rfc3339(&c_value) {
                Ok(ts) => apply_ts_cursor(
                    select,
                    issue::Column::CreatedAt,
                    issue::Column::Id,
                    ts,
                    c_id,
                    asc,
                ),
                Err(_) => {
                    return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                }
            },
            IssueSort::UpdatedAt => match chrono::DateTime::parse_from_rfc3339(&c_value) {
                Ok(ts) => apply_ts_cursor(
                    select,
                    issue::Column::UpdatedAt,
                    issue::Column::Id,
                    ts,
                    c_id,
                    asc,
                ),
                Err(_) => {
                    return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                }
            },
            IssueSort::UserRating => {
                // Rating cursor: empty value means the boundary row had
                // no rating — paginate within the no-rating bucket on id
                // alone. Otherwise compare the correlated subquery
                // against the parsed f64. We bind `user.id` once per
                // arm of the boundary so the SQL stays stand-alone.
                let parsed = if c_value.is_empty() {
                    None
                } else {
                    match c_value.parse::<f64>() {
                        Ok(v) => Some(v),
                        Err(_) => {
                            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
                        }
                    }
                };
                apply_user_rating_cursor(select, user.id, parsed, c_id, asc)
            }
        };
    }

    select = match sort {
        IssueSort::Number => {
            let nulls_last = Expr::cust("sort_number IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::SortNumber)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::SortNumber)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::Year => {
            let nulls_last = Expr::cust("year IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::Year)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::Year)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::PageCount => {
            let nulls_last = Expr::cust("page_count IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::PageCount)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::PageCount)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::CreatedAt => {
            if asc {
                select
                    .order_by_asc(issue::Column::CreatedAt)
                    .order_by_asc(issue::Column::Id)
            } else {
                select
                    .order_by_desc(issue::Column::CreatedAt)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::UpdatedAt => {
            if asc {
                select
                    .order_by_asc(issue::Column::UpdatedAt)
                    .order_by_asc(issue::Column::Id)
            } else {
                select
                    .order_by_desc(issue::Column::UpdatedAt)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::UserRating => {
            // Correlated subquery for the calling user's rating; NULLs
            // (no rating from this user) sort last on ASC.
            let rating_expr = Expr::cust_with_values(
                "(SELECT ur.rating FROM user_ratings ur \
                  WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                    AND ur.target_id = issues.id)",
                [Value::from(user.id)],
            );
            let nulls_last_expr = Expr::cust_with_values(
                "(SELECT ur.rating FROM user_ratings ur \
                  WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                    AND ur.target_id = issues.id) IS NULL",
                [Value::from(user.id)],
            );
            let s = select.order_by_asc(nulls_last_expr);
            if asc {
                s.order_by_asc(rating_expr).order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(rating_expr)
                    .order_by_desc(issue::Column::Id)
            }
        }
    };

    let rows: Vec<issue::Model> = match select.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list issues cross failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        // We need the rating value for the cursor when sorting by
        // user_rating; pre-compute via a one-off scalar query keyed on
        // the boundary issue's id.
        let boundary = rows.get(limit as usize - 1).cloned();
        if let Some(r) = boundary {
            let value = match sort {
                IssueSort::Number => r.sort_number.map(|n| n.to_string()).unwrap_or_default(),
                IssueSort::Year => r.year.map(|y| y.to_string()).unwrap_or_default(),
                IssueSort::PageCount => r.page_count.map(|p| p.to_string()).unwrap_or_default(),
                IssueSort::CreatedAt => r.created_at.to_rfc3339(),
                IssueSort::UpdatedAt => r.updated_at.to_rfc3339(),
                IssueSort::UserRating => fetch_user_rating(&app, user.id, &r.id)
                    .await
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            };
            Some(encode_cursor(&value, &r.id))
        } else {
            None
        }
    } else {
        None
    };
    let page: Vec<issue::Model> = rows.into_iter().take(limit as usize).collect();
    hydrate_and_respond(&app, page, next_cursor, total).await
}

/// Look up the calling user's rating for one issue by id; used to
/// compute the cursor sort_value for the `user_rating` sort.
async fn fetch_user_rating(app: &AppState, user_id: Uuid, issue_id: &str) -> Option<f64> {
    use entity::user_rating;
    use sea_orm::ColumnTrait;
    user_rating::Entity::find()
        .filter(user_rating::Column::UserId.eq(user_id))
        .filter(user_rating::Column::TargetType.eq("issue"))
        .filter(user_rating::Column::TargetId.eq(issue_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|m| m.rating)
}

/// Rating cursor: filter on `(rating > c) OR (rating = c AND id > id)`
/// using a correlated subquery for the join. NULL rating boundary
/// keeps within the no-rating bucket and paginates by id alone.
fn apply_user_rating_cursor(
    select: sea_orm::Select<issue::Entity>,
    user_id: Uuid,
    c_value: Option<f64>,
    c_id: String,
    asc: bool,
) -> sea_orm::Select<issue::Entity> {
    use sea_orm::ColumnTrait;
    let rating_sq = "(SELECT ur.rating FROM user_ratings ur \
                       WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                         AND ur.target_id = issues.id)";
    match c_value {
        Some(v) => {
            // Two-arm OR: strictly past the boundary value, OR equal value with id past boundary.
            let cmp = if asc { ">" } else { "<" };
            let sql =
                format!("({rating_sq} {cmp} $2 OR ({rating_sq} = $2 AND issues.id {cmp} $3))",);
            select.filter(Expr::cust_with_values(
                &sql,
                [Value::from(user_id), Value::from(v), Value::from(c_id)],
            ))
        }
        None => {
            // No-rating boundary: stay in the NULL bucket, paginate by id.
            let sql = format!("{rating_sq} IS NULL");
            let s = select.filter(Expr::cust_with_values(&sql, [Value::from(user_id)]));
            if asc {
                s.filter(issue::Column::Id.gt(c_id))
            } else {
                s.filter(issue::Column::Id.lt(c_id))
            }
        }
    }
}

/// Hydrate `issue::Model`s into `IssueSummaryView`s with their parent
/// series slug. One batched series fetch keeps it O(1) round-trips.
async fn hydrate_and_respond(
    app: &AppState,
    rows: Vec<issue::Model>,
    next_cursor: Option<String>,
    total: Option<i64>,
) -> axum::response::Response {
    use super::series::IssueListView;
    if rows.is_empty() {
        return Json(IssueListView {
            items: Vec::new(),
            next_cursor,
            total,
        })
        .into_response();
    }
    let series_ids: BTreeSet<Uuid> = rows.iter().map(|r| r.series_id).collect();
    let series_rows: Vec<series::Model> = match series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "issues hydrate (series lookup) failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series_lookup: std::collections::HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    let items: Vec<IssueSummaryView> = rows
        .into_iter()
        .filter_map(|i| {
            let s = series_lookup.get(&i.series_id)?;
            let series_slug = s.slug.clone();
            Some(IssueSummaryView::from_model(i, &series_slug))
        })
        .collect();
    Json(IssueListView {
        items,
        next_cursor,
        total,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/issues/search",
    params(
        ("q" = String, Query,),
        ("series_id" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = IssueSearchView))
)]
pub async fn search(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let text = q.q.trim();
    if text.is_empty() {
        return Json(IssueSearchView { items: Vec::new() }).into_response();
    }
    if text.len() > SEARCH_MAX_QUERY_LEN {
        return error(StatusCode::BAD_REQUEST, "validation", "q too long");
    }
    let limit = q
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;
    let mut sel = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(Expr::cust_with_values(
            "search_doc @@ websearch_to_tsquery('simple', $1)",
            [text],
        ))
        .order_by_desc(Expr::cust_with_values(
            "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
            [text],
        ))
        .limit(limit);
    if let Some(sid) = q.series_id {
        sel = sel.filter(issue::Column::SeriesId.eq(sid));
    }
    if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(IssueSearchView { items: Vec::new() }).into_response();
        }
        let ids: Vec<Uuid> = visible.allowed.iter().copied().collect();
        sel = sel.filter(issue::Column::LibraryId.is_in(ids));
    }
    let rows = match sel.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "issue search failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if rows.is_empty() {
        return Json(IssueSearchView { items: Vec::new() }).into_response();
    }
    let series_ids: BTreeSet<Uuid> = rows.iter().map(|r| r.series_id).collect();
    let series_rows: Vec<series::Model> = match series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "series hydrate failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series_lookup: std::collections::HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    let items = rows
        .into_iter()
        .filter_map(|i| {
            let s = series_lookup.get(&i.series_id)?;
            let series_slug = s.slug.clone();
            let series_name = s.name.clone();
            Some(IssueSearchHit {
                issue: IssueSummaryView::from_model(i, &series_slug),
                series_name,
            })
        })
        .collect();
    Json(IssueSearchView { items }).into_response()
}

// ───── helpers ─────

async fn visible_in_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
