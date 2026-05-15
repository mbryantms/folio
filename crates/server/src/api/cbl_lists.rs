//! CBL reading-list API (saved-views M4).
//!
//! User-scoped CRUD over `cbl_lists` plus the import / refresh /
//! resolve workflows. Three import paths share an internal helper:
//!
//!   - `POST /me/cbl-lists/upload` — `multipart/form-data` with a
//!     `file` field carrying the raw `.cbl` bytes.
//!   - `POST /me/cbl-lists` (JSON, `kind = 'url'`) — fetches the file
//!     from the supplied HTTPS URL.
//!   - `POST /me/cbl-lists` (JSON, `kind = 'catalog'`) — resolves the
//!     `(catalog_source_id, catalog_path)` reference through
//!     `crate::cbl::catalog`.
//!
//! All three feed [`crate::cbl::import::apply_parsed`] which writes the
//! `cbl_lists` row, `cbl_entries`, and `cbl_refresh_log` in one logical
//! pass. Match resolution runs as part of that pass; manual overrides
//! survive subsequent refreshes.

use axum::{
    Extension, Json, Router,
    extract::{Multipart, Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use entity::{catalog_source, cbl_entry, cbl_list, cbl_refresh_log};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, FromQueryResult, ModelTrait,
    QueryFilter, QueryOrder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{CurrentUser, RequireAdmin};
use crate::cbl::{
    catalog,
    import::{self, ImportSummary, RefreshTrigger},
    refresh,
};
use crate::middleware::RequestContext;
use crate::state::AppState;

const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOOKS_PER_FILE: usize = 5_000;

pub fn routes() -> Router<AppState> {
    Router::new()
        // user-scoped CBL lists
        .route("/me/cbl-lists", get(list).post(create_from_json))
        .route("/me/cbl-lists/upload", post(upload))
        .route(
            "/me/cbl-lists/{id}",
            get(detail).patch(update).delete(delete_one),
        )
        .route("/me/cbl-lists/{id}/refresh", post(refresh_one))
        .route("/me/cbl-lists/{id}/refresh-log", get(refresh_log))
        .route("/me/cbl-lists/{id}/entries", get(entries))
        .route("/me/cbl-lists/{id}/issues", get(issues))
        .route("/me/cbl-lists/{id}/window", get(reading_window))
        .route("/me/cbl-lists/{id}/export", get(export))
        .route(
            "/me/cbl-lists/{id}/entries/{entry_id}/match",
            post(manual_match),
        )
        .route(
            "/me/cbl-lists/{id}/entries/{entry_id}/clear-match",
            post(clear_match),
        )
        // catalog browser
        .route("/catalog/sources", get(list_catalog_sources))
        .route(
            "/catalog/sources/{id}/lists",
            get(list_catalog_entries),
        )
        .route(
            "/catalog/sources/{id}/refresh-index",
            post(refresh_catalog_index),
        )
        // admin catalog mgmt
        .route(
            "/admin/catalog-sources",
            post(admin_create_catalog_source),
        )
        .route(
            "/admin/catalog-sources/{id}",
            axum::routing::patch(admin_update_catalog_source).delete(admin_delete_catalog_source),
        )
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblListView {
    pub id: String,
    pub owner_user_id: Option<String>,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub catalog_source_id: Option<String>,
    pub catalog_path: Option<String>,
    pub github_blob_sha: Option<String>,
    pub parsed_name: String,
    pub parsed_matchers_present: bool,
    pub num_issues_declared: Option<i32>,
    pub description: Option<String>,
    pub imported_at: String,
    pub last_refreshed_at: Option<String>,
    pub last_match_run_at: Option<String>,
    pub refresh_schedule: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Counts derived from `cbl_entries` for this list. Populated on
    /// list + detail responses.
    pub stats: CblStatsView,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct CblStatsView {
    pub total: i64,
    pub matched: i64,
    pub ambiguous: i64,
    pub missing: i64,
    pub manual: i64,
    /// Count of matched entries whose issue the **calling user** has
    /// finished. Drives the per-user reading-progress pill on the home
    /// rail header. `0` for fresh accounts or when the caller has no
    /// progress records yet.
    pub read_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblListListView {
    pub items: Vec<CblListView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblEntryView {
    pub id: String,
    pub position: i32,
    pub series_name: String,
    pub issue_number: String,
    pub volume: Option<String>,
    pub year: Option<String>,
    pub cv_series_id: Option<i32>,
    pub cv_issue_id: Option<i32>,
    pub matched_issue_id: Option<String>,
    pub match_status: String,
    pub match_method: Option<String>,
    pub match_confidence: Option<f32>,
    pub ambiguous_candidates: Option<serde_json::Value>,
    pub matched_at: Option<String>,
}

/// Detail response — list metadata + aggregate counts only. Entries
/// moved to the paginated `/me/cbl-lists/{id}/entries` endpoint as of
/// 2026-05-14; see [docs/dev plans](../../../../.claude/plans/list-pagination-completeness-1.0.md)
/// for rationale (large lists silently truncated at 500). The fields
/// on `CblListView.stats` carry the per-status counts that used to be
/// computed client-side from `entries`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblDetailView {
    #[serde(flatten)]
    pub list: CblListView,
}

/// One entry + its hydrated `IssueSummaryView` (when `matched_issue_id`
/// resolves to an issue the caller can see). Returned by the paginated
/// entries endpoint so the consumption grid + Reading Order tab don't
/// need a second `/issues` round-trip.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblEntryHydratedView {
    #[serde(flatten)]
    pub entry: CblEntryView,
    /// Hydrated matched issue. `None` for unmatched entries, and also
    /// `None` when the matched issue lives in a library the caller
    /// can't access (we still surface the entry — the position label
    /// is meaningful even without a cover).
    pub issue: Option<crate::api::series::IssueSummaryView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CblEntryListView {
    pub items: Vec<CblEntryHydratedView>,
    /// Opaque cursor for the next page. `None` when this page is the
    /// last one for the given filter set.
    pub next_cursor: Option<String>,
    /// Count of all entries matching the filter. Returned ONLY on the
    /// first page (cursor absent) — keeps subsequent fetches cheap.
    pub total: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshLogEntryView {
    pub id: String,
    pub ran_at: String,
    pub trigger: String,
    pub upstream_changed: bool,
    pub prev_blob_sha: Option<String>,
    pub new_blob_sha: Option<String>,
    pub added_count: i32,
    pub removed_count: i32,
    pub reordered_count: i32,
    pub rematched_count: i32,
    pub diff_summary: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshLogListView {
    pub items: Vec<RefreshLogEntryView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateCblListReq {
    /// Direct URL to a `.cbl` file. Fetched server-side.
    Url {
        url: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        refresh_schedule: Option<String>,
    },
    /// Pick from a configured `catalog_sources` row.
    Catalog {
        catalog_source_id: Uuid,
        catalog_path: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        refresh_schedule: Option<String>,
    },
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateCblListReq {
    /// `None` = field absent → leave unchanged.
    /// `Some(None)` = JSON `null` → clear the column.
    /// `Some(Some(_))` = explicit value → overwrite.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub refresh_schedule: Option<Option<String>>,
}

/// Tri-state deserialize helper: turns a present-but-`null` JSON field
/// into `Some(None)` instead of serde's default `None`. Without this,
/// `{"refresh_schedule": null}` would be indistinguishable from the
/// field being omitted, and the handler's "clear column" branch would
/// silently no-op.
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ManualMatchReq {
    pub issue_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DetailQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

/// Query params for `GET /me/cbl-lists/{id}/entries`. `status` is a
/// comma-separated subset of {matched, ambiguous, missing, manual};
/// omit for "all". `cursor` is opaque (base64 of `position:id`).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EntriesQuery {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RefreshLogQuery {
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogSourceView {
    pub id: String,
    pub display_name: String,
    pub github_owner: String,
    pub github_repo: String,
    pub github_branch: String,
    pub enabled: bool,
    pub last_indexed_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogSourceListView {
    pub items: Vec<CatalogSourceView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogEntryView {
    pub path: String,
    pub name: String,
    pub publisher: String,
    pub sha: String,
    pub size: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogEntriesView {
    pub source_id: String,
    pub items: Vec<CatalogEntryView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCatalogSourceReq {
    pub display_name: String,
    pub github_owner: String,
    pub github_repo: String,
    #[serde(default)]
    pub github_branch: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateCatalogSourceReq {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub github_branch: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

// ───── helpers ─────

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn list_to_view(m: &cbl_list::Model, stats: CblStatsView) -> CblListView {
    CblListView {
        id: m.id.to_string(),
        owner_user_id: m.owner_user_id.map(|u| u.to_string()),
        source_kind: m.source_kind.clone(),
        source_url: m.source_url.clone(),
        catalog_source_id: m.catalog_source_id.map(|u| u.to_string()),
        catalog_path: m.catalog_path.clone(),
        github_blob_sha: m.github_blob_sha.clone(),
        parsed_name: m.parsed_name.clone(),
        parsed_matchers_present: m.parsed_matchers_present,
        num_issues_declared: m.num_issues_declared,
        description: m.description.clone(),
        imported_at: m.imported_at.to_rfc3339(),
        last_refreshed_at: m.last_refreshed_at.map(|d| d.to_rfc3339()),
        last_match_run_at: m.last_match_run_at.map(|d| d.to_rfc3339()),
        refresh_schedule: m.refresh_schedule.clone(),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
        stats,
    }
}

fn entry_to_view(e: &cbl_entry::Model) -> CblEntryView {
    CblEntryView {
        id: e.id.to_string(),
        position: e.position,
        series_name: e.series_name.clone(),
        issue_number: e.issue_number.clone(),
        volume: e.volume.clone(),
        year: e.year.clone(),
        cv_series_id: e.cv_series_id,
        cv_issue_id: e.cv_issue_id,
        matched_issue_id: e.matched_issue_id.clone(),
        match_status: e.match_status.clone(),
        match_method: e.match_method.clone(),
        match_confidence: e.match_confidence,
        ambiguous_candidates: e.ambiguous_candidates.clone(),
        matched_at: e.matched_at.map(|d| d.to_rfc3339()),
    }
}

async fn stats_for(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    user_id: Uuid,
) -> Result<CblStatsView, sea_orm::DbErr> {
    use sea_orm::PaginatorTrait;
    let base = cbl_entry::Entity::find().filter(cbl_entry::Column::CblListId.eq(list_id));
    let total = base.clone().count(db).await? as i64;
    let mut stats = CblStatsView {
        total,
        ..Default::default()
    };
    for status in ["matched", "ambiguous", "missing", "manual"] {
        let n = base
            .clone()
            .filter(cbl_entry::Column::MatchStatus.eq(status))
            .count(db)
            .await? as i64;
        match status {
            "matched" => stats.matched = n,
            "ambiguous" => stats.ambiguous = n,
            "missing" => stats.missing = n,
            "manual" => stats.manual = n,
            _ => {}
        }
    }

    // Per-user read count: how many of this CBL's matched entries does
    // the caller already have a `finished = true` progress record for?
    // Drives the new reading-progress pill on the home rail header
    // (paired with the existing match/total pill which is per-list, not
    // per-user). One scalar query — fine to issue alongside the other
    // stat lookups even though it touches a different table.
    use sea_orm::{DbBackend, Statement, sea_query::Value};
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct Scalar {
        n: i64,
    }
    let read_count = Scalar::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT COUNT(*) AS n
            FROM cbl_entries e
            JOIN progress_records p
              ON p.issue_id = e.matched_issue_id
             AND p.user_id  = $2
             AND p.finished = TRUE
            WHERE e.cbl_list_id = $1
              AND e.matched_issue_id IS NOT NULL
        "#,
        [Value::from(list_id), Value::from(user_id)],
    ))
    .one(db)
    .await?
    .map(|r| r.n)
    .unwrap_or(0);
    stats.read_count = read_count;

    Ok(stats)
}

async fn ensure_owner(
    list: &cbl_list::Model,
    user: &CurrentUser,
) -> Result<(), axum::response::Response> {
    if let Some(owner) = list.owner_user_id
        && owner != user.id
    {
        return Err(error(StatusCode::FORBIDDEN, "forbidden", "not your list"));
    }
    Ok(())
}

async fn fetch_list(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Result<cbl_list::Model, axum::response::Response> {
    cbl_list::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "list not found"))
}

// ───── handlers: lists ─────

#[utoipa::path(get, path = "/me/cbl-lists", responses((status = 200, body = CblListListView)))]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    let rows = match cbl_list::Entity::find()
        .filter(
            sea_orm::Condition::any()
                .add(cbl_list::Column::OwnerUserId.is_null())
                .add(cbl_list::Column::OwnerUserId.eq(user.id)),
        )
        .order_by_desc(cbl_list::Column::CreatedAt)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let stats = stats_for(&app.db, row.id, user.id)
            .await
            .unwrap_or_default();
        items.push(list_to_view(&row, stats));
    }
    Json(CblListListView { items }).into_response()
}

/// Detail endpoint — returns list metadata + aggregate stats only.
/// Entries used to be embedded here (capped at 500), which silently
/// truncated long lists; they now live exclusively on the paginated
/// `/entries` endpoint. The per-status counts the UI used to derive
/// from `entries` are on `list.stats`.
#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}",
    params(("id" = String, Path,)),
    responses((status = 200, body = CblDetailView))
)]
pub async fn detail(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }
    let stats = stats_for(&app.db, id, user.id).await.unwrap_or_default();
    Json(CblDetailView {
        list: list_to_view(&list, stats),
    })
    .into_response()
}

fn parse_status_filter(s: &str) -> Result<Vec<&str>, ()> {
    const ALLOWED: &[&str] = &["matched", "ambiguous", "missing", "manual"];
    let parts: Vec<&str> = s
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(());
    }
    for p in &parts {
        if !ALLOWED.contains(p) {
            return Err(());
        }
    }
    Ok(parts)
}

fn encode_entry_cursor(position: i32, id: &str) -> String {
    use base64::Engine;
    let s = format!("{position}:{id}");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

fn parse_entry_cursor(s: &str) -> Result<(i32, Uuid), ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let txt = std::str::from_utf8(&bytes).map_err(|_| ())?;
    let (pos, id) = txt.split_once(':').ok_or(())?;
    let parsed_pos: i32 = pos.parse().map_err(|_| ())?;
    let parsed_id = Uuid::parse_str(id).map_err(|_| ())?;
    Ok((parsed_pos, parsed_id))
}

/// `GET /me/cbl-lists/{id}/entries` — paginated walk over CBL entries
/// in position order, hydrated with `IssueSummaryView` for matched
/// rows. Status filter is server-side, so the Resolution tab can
/// stream just `ambiguous,missing` without touching the matched majority.
#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}/entries",
    params(
        ("id" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("status" = Option<String>, Query, description = "Comma-separated subset of matched,ambiguous,missing,manual"),
    ),
    responses(
        (status = 200, body = CblEntryListView),
        (status = 400, description = "invalid cursor or status"),
        (status = 403, description = "not your list"),
        (status = 404, description = "list not found"),
    )
)]
pub async fn entries(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<EntriesQuery>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }

    // Page-size: default 100 (per plan), max 200 — comfortable for
    // virtualized table rows and cover-grid alike. Anything bigger
    // and the hydration round-trip starts to dominate.
    let limit = q.limit.unwrap_or(100).clamp(1, 200);

    let mut sel = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(id))
        .order_by_asc(cbl_entry::Column::Position)
        .order_by_asc(cbl_entry::Column::Id);

    if let Some(status_str) = q.status.as_deref() {
        let parts = match parse_status_filter(status_str) {
            Ok(p) => p,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "invalid status filter (expected comma-separated subset of matched,ambiguous,missing,manual)",
                );
            }
        };
        sel = sel.filter(
            cbl_entry::Column::MatchStatus
                .is_in(parts.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        );
    }

    if let Some(cursor) = q.cursor.as_deref() {
        let (after_pos, after_id) = match parse_entry_cursor(cursor) {
            Ok(c) => c,
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        };
        // Tuple-compare semantics: rows whose (position, id) sorts
        // strictly after the cursor's (position, id). Expressed as
        // (position > P) OR (position = P AND id > I).
        sel = sel.filter(
            sea_orm::Condition::any()
                .add(cbl_entry::Column::Position.gt(after_pos))
                .add(
                    sea_orm::Condition::all()
                        .add(cbl_entry::Column::Position.eq(after_pos))
                        .add(cbl_entry::Column::Id.gt(after_id)),
                ),
        );
    }

    use sea_orm::QuerySelect;
    // Fetch limit+1 to detect "more pages" without a separate count.
    let rows = match sel.limit(limit + 1).all(&app.db).await {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let has_more = rows.len() as u64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page_rows
            .last()
            .map(|e| encode_entry_cursor(e.position, &e.id.to_string()))
    } else {
        None
    };

    // First-page-only total: a COUNT over the filter set. Skipping
    // this on subsequent pages keeps "scroll to the end" cheap.
    let total = if q.cursor.is_none() {
        let mut count_sel = cbl_entry::Entity::find().filter(cbl_entry::Column::CblListId.eq(id));
        if let Some(status_str) = q.status.as_deref() {
            // Already validated above; re-parsing is fine.
            if let Ok(parts) = parse_status_filter(status_str) {
                count_sel = count_sel.filter(
                    cbl_entry::Column::MatchStatus
                        .is_in(parts.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                );
            }
        }
        use sea_orm::PaginatorTrait;
        match count_sel.count(&app.db).await {
            Ok(n) => Some(n as i64),
            Err(_) => None,
        }
    } else {
        None
    };

    // Batch-hydrate matched issues. One query for issues, one for
    // their parent series, then we build the view for entries the
    // caller can see. Unmatched entries are returned with
    // `issue: None` so the UI still renders the row.
    let issue_ids: Vec<String> = page_rows
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let issue_by_id: std::collections::HashMap<String, entity::issue::Model> =
        if issue_ids.is_empty() {
            std::collections::HashMap::new()
        } else {
            match entity::issue::Entity::find()
                .filter(entity::issue::Column::Id.is_in(issue_ids))
                .all(&app.db)
                .await
            {
                Ok(rows) => rows.into_iter().map(|i| (i.id.clone(), i)).collect(),
                Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
            }
        };
    let series_ids: std::collections::HashSet<Uuid> =
        issue_by_id.values().map(|i| i.series_id).collect();
    let series_by_id: std::collections::HashMap<Uuid, entity::series::Model> = if series_ids
        .is_empty()
    {
        std::collections::HashMap::new()
    } else {
        match entity::series::Entity::find()
            .filter(
                entity::series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()),
            )
            .all(&app.db)
            .await
        {
            Ok(rows) => rows.into_iter().map(|s| (s.id, s)).collect(),
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
        }
    };
    use crate::api::series::IssueSummaryView;
    use crate::library::access;
    let visible = access::for_user(&app, &user).await;

    let items: Vec<CblEntryHydratedView> = page_rows
        .iter()
        .map(|entry| {
            let issue_view = entry
                .matched_issue_id
                .as_ref()
                .and_then(|iid| issue_by_id.get(iid))
                .and_then(|issue| {
                    let series = series_by_id.get(&issue.series_id)?;
                    if !visible.contains(series.library_id) {
                        return None;
                    }
                    Some(
                        IssueSummaryView::from_model(issue.clone(), &series.slug)
                            .with_series_name(series.name.clone()),
                    )
                });
            CblEntryHydratedView {
                entry: entry_to_view(entry),
                issue: issue_view,
            }
        })
        .collect();

    Json(CblEntryListView {
        items,
        next_cursor,
        total,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/me/cbl-lists/upload",
    responses((status = 201, body = CblListView))
)]
pub async fn upload(
    State(app): State<AppState>,
    user: CurrentUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut bytes: Option<Vec<u8>> = None;
    let mut name_override: Option<String> = None;
    let mut description: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or_default().to_owned();
        match field_name.as_str() {
            "file" => match field.bytes().await {
                Ok(b) => {
                    if b.len() > MAX_UPLOAD_BYTES {
                        return error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "too_large",
                            "file exceeds 4 MiB",
                        );
                    }
                    bytes = Some(b.to_vec());
                }
                Err(e) => {
                    return error(StatusCode::BAD_REQUEST, "validation", &e.to_string());
                }
            },
            "name" => name_override = field.text().await.ok().filter(|s| !s.trim().is_empty()),
            "description" => description = field.text().await.ok().filter(|s| !s.trim().is_empty()),
            _ => {}
        }
    }
    let Some(bytes) = bytes else {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "missing `file` field",
        );
    };
    let xml = String::from_utf8_lossy(&bytes).into_owned();
    let parsed = match parsers::cbl::parse(xml.as_bytes()) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::BAD_REQUEST, "parse_failed", &e.to_string()),
    };
    if parsed.books.len() > MAX_BOOKS_PER_FILE {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_entries",
            "file exceeds 5000 entries",
        );
    }
    create_list_from_parsed(
        &app,
        Some(user.id),
        &parsed,
        &xml,
        SourceMeta::Upload {
            name_override,
            description,
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/me/cbl-lists",
    request_body = CreateCblListReq,
    responses((status = 201, body = CblListView))
)]
pub async fn create_from_json(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateCblListReq>,
) -> impl IntoResponse {
    match req {
        CreateCblListReq::Url {
            url,
            name,
            description,
            refresh_schedule,
        } => create_from_url(&app, user.id, url, name, description, refresh_schedule).await,
        CreateCblListReq::Catalog {
            catalog_source_id,
            catalog_path,
            name,
            description,
            refresh_schedule,
        } => {
            create_from_catalog(
                &app,
                user.id,
                catalog_source_id,
                catalog_path,
                name,
                description,
                refresh_schedule,
            )
            .await
        }
    }
}

async fn create_from_url(
    app: &AppState,
    user_id: Uuid,
    url: String,
    _name_override: Option<String>,
    description: Option<String>,
    refresh_schedule: Option<String>,
) -> axum::response::Response {
    let client = match reqwest::Client::builder()
        .user_agent(concat!("Folio/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                &e.to_string(),
            );
        }
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.to_string()),
    };
    if !resp.status().is_success() {
        return error(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            &format!("status {}", resp.status()),
        );
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let last_modified = resp
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.to_string()),
    };
    if bytes.len() > MAX_UPLOAD_BYTES {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "exceeds 4 MiB");
    }
    let xml = String::from_utf8_lossy(&bytes).into_owned();
    let parsed = match parsers::cbl::parse(xml.as_bytes()) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::BAD_REQUEST, "parse_failed", &e.to_string()),
    };
    if parsed.books.len() > MAX_BOOKS_PER_FILE {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_entries",
            "exceeds 5000 entries",
        );
    }
    create_list_from_parsed(
        app,
        Some(user_id),
        &parsed,
        &xml,
        SourceMeta::Url {
            url,
            etag,
            last_modified,
            description,
            refresh_schedule,
        },
    )
    .await
}

async fn create_from_catalog(
    app: &AppState,
    user_id: Uuid,
    catalog_source_id: Uuid,
    catalog_path: String,
    _name_override: Option<String>,
    description: Option<String>,
    refresh_schedule: Option<String>,
) -> axum::response::Response {
    let source = match catalog_source::Entity::find_by_id(catalog_source_id)
        .one(&app.db)
        .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let blob = match catalog::fetch_blob(&app.db, &source, &catalog_path, false).await {
        Ok(b) => b,
        Err(catalog::CatalogError::NotFound(m)) => {
            return error(StatusCode::NOT_FOUND, "not_found", &m);
        }
        Err(catalog::CatalogError::RateLimited) => {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "GitHub rate-limited the catalog index fetch",
            );
        }
        Err(catalog::CatalogError::TooLarge { actual, limit }) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                &format!("blob {actual} bytes > limit {limit}"),
            );
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.to_string()),
    };
    let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
    let parsed = match parsers::cbl::parse(xml.as_bytes()) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::BAD_REQUEST, "parse_failed", &e.to_string()),
    };
    if parsed.books.len() > MAX_BOOKS_PER_FILE {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_entries",
            "exceeds 5000 entries",
        );
    }
    create_list_from_parsed(
        app,
        Some(user_id),
        &parsed,
        &xml,
        SourceMeta::Catalog {
            catalog_source_id,
            catalog_path,
            blob_sha: blob.blob_sha,
            description,
            refresh_schedule,
        },
    )
    .await
}

enum SourceMeta {
    Upload {
        // Reserved for the future "rename on import" path.
        #[allow(dead_code)]
        name_override: Option<String>,
        description: Option<String>,
    },
    Url {
        url: String,
        etag: Option<String>,
        last_modified: Option<String>,
        description: Option<String>,
        refresh_schedule: Option<String>,
    },
    Catalog {
        catalog_source_id: Uuid,
        catalog_path: String,
        blob_sha: String,
        description: Option<String>,
        refresh_schedule: Option<String>,
    },
}

/// Insert the `cbl_lists` row, then hand off to
/// [`crate::cbl::import::apply_parsed`] for entry persistence + match.
async fn create_list_from_parsed(
    app: &AppState,
    owner: Option<Uuid>,
    parsed: &parsers::cbl::ParsedCbl,
    xml: &str,
    source: SourceMeta,
) -> axum::response::Response {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    let raw_sha = import::sha256_of(xml.as_bytes());
    let mut am = cbl_list::ActiveModel {
        id: Set(id),
        owner_user_id: Set(owner),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(raw_sha),
        raw_xml: Set(xml.to_owned()),
        parsed_name: Set(parsed.name.clone()),
        parsed_matchers_present: Set(parsed.matchers_present),
        num_issues_declared: Set(parsed.num_issues_declared),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(Some(now)),
        last_match_run_at: Set(Some(now)),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    match source {
        SourceMeta::Upload {
            name_override: _,
            description,
        } => {
            am.source_kind = Set("upload".into());
            if let Some(d) = description {
                am.description = Set(Some(d));
            }
        }
        SourceMeta::Url {
            url,
            etag,
            last_modified,
            description,
            refresh_schedule,
        } => {
            am.source_kind = Set("url".into());
            am.source_url = Set(Some(url));
            am.source_etag = Set(etag);
            am.source_last_modified = Set(last_modified);
            am.description = Set(description);
            am.refresh_schedule = Set(refresh_schedule);
        }
        SourceMeta::Catalog {
            catalog_source_id,
            catalog_path,
            blob_sha,
            description,
            refresh_schedule,
        } => {
            am.source_kind = Set("catalog".into());
            am.catalog_source_id = Set(Some(catalog_source_id));
            am.catalog_path = Set(Some(catalog_path));
            am.github_blob_sha = Set(Some(blob_sha));
            am.description = Set(description);
            am.refresh_schedule = Set(refresh_schedule.or(Some("0 0 * * 0".to_owned())));
        }
    }
    if let Err(e) = am.insert(&app.db).await {
        tracing::error!(error = %e, "cbl_lists: insert failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    let summary =
        match import::apply_parsed(&app.db, id, parsed, xml, None, RefreshTrigger::Manual).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cbl_lists: apply_parsed failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
    // Re-read the freshly-written row + stats so the response shape
    // mirrors the regular GET.
    let row = match cbl_list::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let stats = CblStatsView {
        total: i64::from(summary.matched + summary.ambiguous + summary.missing + summary.manual),
        matched: i64::from(summary.matched),
        ambiguous: i64::from(summary.ambiguous),
        missing: i64::from(summary.missing),
        manual: i64::from(summary.manual),
        // Freshly-uploaded list — the user can't have progress on
        // anything yet. Subsequent reads call `stats_for` which
        // populates this against `progress_records`.
        read_count: 0,
    };
    (StatusCode::CREATED, Json(list_to_view(&row, stats))).into_response()
}

#[utoipa::path(
    patch,
    path = "/me/cbl-lists/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateCblListReq,
    responses((status = 200, body = CblListView))
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateCblListReq>,
) -> impl IntoResponse {
    let row = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&row, &user).await {
        return resp;
    }
    let mut am: cbl_list::ActiveModel = row.into();
    if let Some(d) = req.description {
        am.description = Set(d.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()));
    }
    if let Some(s) = req.refresh_schedule {
        am.refresh_schedule = Set(s.map(|v| v.trim().to_owned()).filter(|s| !s.is_empty()));
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let stats = stats_for(&app.db, id, user.id).await.unwrap_or_default();
    Json(list_to_view(&updated, stats)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/cbl-lists/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&row, &user).await {
        return resp;
    }
    if row.delete(&app.db).await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/cbl-lists/{id}/refresh",
    params(("id" = String, Path,)),
    responses((status = 200))
)]
pub async fn refresh_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&row, &user).await {
        return resp;
    }
    match refresh::refresh(&app.db, id, RefreshTrigger::Manual, false).await {
        Ok(summary) => Json(summary_to_view(&app, &summary)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "cbl_lists: refresh failed");
            error(StatusCode::BAD_GATEWAY, "refresh_failed", &e.to_string())
        }
    }
}

fn summary_to_view(_app: &AppState, summary: &ImportSummary) -> ImportSummary {
    summary.clone()
}

#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}/refresh-log",
    params(("id" = String, Path,), ("limit" = Option<u64>, Query,)),
    responses((status = 200, body = RefreshLogListView))
)]
pub async fn refresh_log(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<RefreshLogQuery>,
) -> impl IntoResponse {
    let row = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&row, &user).await {
        return resp;
    }
    use sea_orm::QuerySelect;
    let limit = q.limit.unwrap_or(20).min(100);
    let rows = match cbl_refresh_log::Entity::find()
        .filter(cbl_refresh_log::Column::CblListId.eq(id))
        .order_by_desc(cbl_refresh_log::Column::RanAt)
        .limit(limit)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let items = rows
        .into_iter()
        .map(|m| RefreshLogEntryView {
            id: m.id.to_string(),
            ran_at: m.ran_at.to_rfc3339(),
            trigger: m.trigger,
            upstream_changed: m.upstream_changed,
            prev_blob_sha: m.prev_blob_sha,
            new_blob_sha: m.new_blob_sha,
            added_count: m.added_count,
            removed_count: m.removed_count,
            reordered_count: m.reordered_count,
            rematched_count: m.rematched_count,
            diff_summary: m.diff_summary,
        })
        .collect();
    Json(RefreshLogListView { items }).into_response()
}

// ───── manual-match override ─────

#[utoipa::path(
    post,
    path = "/me/cbl-lists/{id}/entries/{entry_id}/match",
    request_body = ManualMatchReq,
    responses((status = 200, body = CblEntryView))
)]
pub async fn manual_match(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((list_id, entry_id)): AxPath<(Uuid, Uuid)>,
    Json(req): Json<ManualMatchReq>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, list_id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }
    let entry = match cbl_entry::Entity::find_by_id(entry_id).one(&app.db).await {
        Ok(Some(e)) if e.cbl_list_id == list_id => e,
        Ok(_) => return error(StatusCode::NOT_FOUND, "not_found", "entry"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Validate the issue exists.
    let issue_exists = entity::issue::Entity::find_by_id(req.issue_id.clone())
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if !issue_exists {
        return error(StatusCode::BAD_REQUEST, "validation", "issue not found");
    }
    let now = Utc::now().fixed_offset();
    let mut am: cbl_entry::ActiveModel = entry.into();
    am.matched_issue_id = Set(Some(req.issue_id));
    am.match_status = Set("manual".into());
    am.match_method = Set(Some("manual".into()));
    am.match_confidence = Set(Some(1.0));
    am.ambiguous_candidates = Set(None);
    am.matched_at = Set(Some(now));
    am.user_resolved_at = Set(Some(now));
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    Json(entry_to_view(&updated)).into_response()
}

#[utoipa::path(
    post,
    path = "/me/cbl-lists/{id}/entries/{entry_id}/clear-match",
    responses((status = 200, body = CblEntryView))
)]
pub async fn clear_match(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((list_id, entry_id)): AxPath<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, list_id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }
    let entry = match cbl_entry::Entity::find_by_id(entry_id).one(&app.db).await {
        Ok(Some(e)) if e.cbl_list_id == list_id => e,
        _ => return error(StatusCode::NOT_FOUND, "not_found", "entry"),
    };
    let mut am: cbl_entry::ActiveModel = entry.into();
    am.match_status = Set("missing".into());
    am.match_method = Set(None);
    am.match_confidence = Set(None);
    am.matched_issue_id = Set(None);
    am.ambiguous_candidates = Set(None);
    am.user_resolved_at = Set(None);
    am.matched_at = Set(None);
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    Json(entry_to_view(&updated)).into_response()
}

// ───── matched-issues hydration (used by saved_views/{id}/results when kind='cbl') ─────

#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}/issues",
    params(
        ("id" = String, Path,),
        ("limit" = Option<u64>, Query,),
        ("offset" = Option<u64>, Query,),
    ),
    responses((status = 200, body = crate::api::series::IssueListView))
)]
pub async fn issues(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<DetailQuery>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }
    use crate::api::series::IssueListView;
    use crate::library::access;
    use sea_orm::QuerySelect;

    let visible = access::for_user(&app, &user).await;
    // Default 200 for thumbnail rails / spot lookups; cap at 2000 so a
    // full-page consumption view of a CBL can hydrate every matched
    // entry in one fetch (CBL spec C7 caps lists at 5000 entries; in
    // practice anything over 2000 should paginate).
    let limit = q.limit.unwrap_or(200).min(2000);
    let offset = q.offset.unwrap_or(0);

    let entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .order_by_asc(cbl_entry::Column::Position)
        .limit(limit)
        .offset(offset)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let issue_ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    if issue_ids.is_empty() {
        return Json(IssueListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response();
    }
    let issues = match entity::issue::Entity::find()
        .filter(entity::issue::Column::Id.is_in(issue_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // Fetch parent series for slug + library access filtering.
    let series_ids: std::collections::HashSet<Uuid> = issues.iter().map(|i| i.series_id).collect();
    let series_rows = match entity::series::Entity::find()
        .filter(entity::series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let series_by_id: std::collections::HashMap<Uuid, entity::series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();

    let issue_by_id: std::collections::HashMap<String, entity::issue::Model> =
        issues.into_iter().map(|i| (i.id.clone(), i)).collect();

    use crate::api::series::IssueSummaryView;
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(issue_id) = entry.matched_issue_id else {
            continue;
        };
        let Some(issue) = issue_by_id.get(&issue_id) else {
            continue;
        };
        let Some(series) = series_by_id.get(&issue.series_id) else {
            continue;
        };
        // Library visibility — silently drop issues the user can't see.
        if !visible.contains(series.library_id) {
            continue;
        }
        let series_slug = series.slug.clone();
        items.push(
            IssueSummaryView::from_model(issue.clone(), &series_slug)
                .with_series_name(series.name.clone()),
        );
    }
    let total = Some(items.len() as i64);
    Json(IssueListView {
        items,
        next_cursor: None,
        total,
    })
    .into_response()
}

/// `GET /me/cbl-lists/{id}/export` — round-trip the original `<Books>`
/// XML back to the user as a `.cbl` download. The bytes are exactly
/// what we imported (or last refreshed) — every CBL list keeps its
/// `raw_xml` for re-match purposes, so this is just a Content-Disposition
/// dressing on top of the stored string.
#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}/export",
    params(("id" = String, Path,)),
    responses(
        (status = 200, description = "CBL bytes", content_type = "application/xml"),
        (status = 404, description = "list not found"),
    )
)]
pub async fn export(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let list = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }
    // Slug-ify the parsed name for a friendlier filename.
    let safe: String = list
        .parsed_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = safe.trim_matches('_');
    let filename = if trimmed.is_empty() {
        format!("{id}.cbl")
    } else {
        format!("{trimmed}.cbl")
    };

    let bytes = list.raw_xml.into_bytes();
    use axum::http::header;
    let disposition = format!("attachment; filename=\"{filename}\"");
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/xml; charset=utf-8".to_string(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response()
}

// ───── catalog endpoints ─────

#[utoipa::path(get, path = "/catalog/sources", responses((status = 200, body = CatalogSourceListView)))]
pub async fn list_catalog_sources(
    State(app): State<AppState>,
    _user: CurrentUser,
) -> impl IntoResponse {
    let rows = match catalog_source::Entity::find()
        .filter(catalog_source::Column::Enabled.eq(true))
        .order_by_asc(catalog_source::Column::DisplayName)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let items = rows
        .into_iter()
        .map(|m| CatalogSourceView {
            id: m.id.to_string(),
            display_name: m.display_name,
            github_owner: m.github_owner,
            github_repo: m.github_repo,
            github_branch: m.github_branch,
            enabled: m.enabled,
            last_indexed_at: m.last_indexed_at.map(|d| d.to_rfc3339()),
        })
        .collect();
    Json(CatalogSourceListView { items }).into_response()
}

#[utoipa::path(
    get,
    path = "/catalog/sources/{id}/lists",
    params(("id" = String, Path,)),
    responses((status = 200, body = CatalogEntriesView))
)]
pub async fn list_catalog_entries(
    State(app): State<AppState>,
    _user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let source = match catalog_source::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let index = match catalog::refresh_index(&app.db, &source, false).await {
        Ok(i) => i,
        Err(catalog::CatalogError::RateLimited) => {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "GitHub rate-limited",
            );
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.to_string()),
    };
    let items = index
        .entries
        .into_iter()
        .map(|e| CatalogEntryView {
            path: e.path,
            name: e.name,
            publisher: e.publisher,
            sha: e.sha,
            size: e.size,
        })
        .collect();
    Json(CatalogEntriesView {
        source_id: id.to_string(),
        items,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/catalog/sources/{id}/refresh-index",
    params(("id" = String, Path,)),
    responses((status = 200, body = CatalogEntriesView))
)]
pub async fn refresh_catalog_index(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let source = match catalog_source::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let index = match catalog::refresh_index(&app.db, &source, true).await {
        Ok(i) => i,
        Err(e) => return error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.to_string()),
    };
    let items = index
        .entries
        .into_iter()
        .map(|e| CatalogEntryView {
            path: e.path,
            name: e.name,
            publisher: e.publisher,
            sha: e.sha,
            size: e.size,
        })
        .collect();
    Json(CatalogEntriesView {
        source_id: id.to_string(),
        items,
    })
    .into_response()
}

// ───── admin catalog source CRUD ─────

#[utoipa::path(
    post,
    path = "/admin/catalog-sources",
    request_body = CreateCatalogSourceReq,
    responses((status = 201, body = CatalogSourceView))
)]
pub async fn admin_create_catalog_source(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateCatalogSourceReq>,
) -> impl IntoResponse {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let am = catalog_source::ActiveModel {
        id: Set(id),
        display_name: Set(req.display_name.trim().to_owned()),
        github_owner: Set(req.github_owner.trim().to_owned()),
        github_repo: Set(req.github_repo.trim().to_owned()),
        github_branch: Set(req
            .github_branch
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_owned())),
        enabled: Set(true),
        last_indexed_at: Set(None),
        index_etag: Set(None),
        index_json: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let saved = match am.insert(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "catalog_sources: insert failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.catalog_source.create",
            target_type: Some("catalog_source"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({
                "owner": saved.github_owner,
                "repo": saved.github_repo,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    (
        StatusCode::CREATED,
        Json(CatalogSourceView {
            id: saved.id.to_string(),
            display_name: saved.display_name,
            github_owner: saved.github_owner,
            github_repo: saved.github_repo,
            github_branch: saved.github_branch,
            enabled: saved.enabled,
            last_indexed_at: None,
        }),
    )
        .into_response()
}

#[utoipa::path(
    patch,
    path = "/admin/catalog-sources/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateCatalogSourceReq,
    responses((status = 200, body = CatalogSourceView))
)]
pub async fn admin_update_catalog_source(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateCatalogSourceReq>,
) -> impl IntoResponse {
    let source = match catalog_source::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let mut am: catalog_source::ActiveModel = source.into();
    if let Some(name) = req.display_name {
        am.display_name = Set(name.trim().to_owned());
    }
    if let Some(branch) = req.github_branch {
        am.github_branch = Set(branch.trim().to_owned());
    }
    if let Some(en) = req.enabled {
        am.enabled = Set(en);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.catalog_source.update",
            target_type: Some("catalog_source"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({"id": id.to_string()}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    Json(CatalogSourceView {
        id: updated.id.to_string(),
        display_name: updated.display_name,
        github_owner: updated.github_owner,
        github_repo: updated.github_repo,
        github_branch: updated.github_branch,
        enabled: updated.enabled,
        last_indexed_at: updated.last_indexed_at.map(|d| d.to_rfc3339()),
    })
    .into_response()
}

#[utoipa::path(
    delete,
    path = "/admin/catalog-sources/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn admin_delete_catalog_source(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let source = match catalog_source::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if source.delete(&app.db).await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "admin.catalog_source.delete",
            target_type: Some("catalog_source"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

// ───────── Reading window (home rail) ─────────

/// Query for `GET /me/cbl-lists/{id}/window`. Defaults aim for a 12-card
/// rail anchored on the user's next unfinished entry, with three already-
/// finished entries scrolled off to the left for context.
#[derive(Debug, serde::Deserialize)]
pub struct WindowQuery {
    /// Number of *previously read* (finished) matched entries to include
    /// before the current up-next entry. Clamped to [0, 20].
    #[serde(default)]
    pub before: Option<u32>,
    /// Number of *upcoming* matched entries to include after the current.
    /// Clamped to [1, 40].
    #[serde(default)]
    pub after: Option<u32>,
}

/// One entry in the reading-window response. Mirrors `IssueSummaryView`
/// plus the per-user progress overlay so the rail can render finished /
/// in-progress / unread cards without a second round-trip.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct CblWindowEntry {
    pub issue: crate::api::series::IssueSummaryView,
    /// 0-based position within the CBL — matches the `#N` badge other
    /// surfaces use.
    pub position: i32,
    pub finished: bool,
    /// Last page index the user reached on this issue (0 if unread).
    pub last_page: i32,
    /// 0.0–1.0 fraction read.
    pub percent: f64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct CblWindowView {
    pub items: Vec<CblWindowEntry>,
    /// Index within `items` of the user's current (first-unfinished)
    /// matched entry. `None` when every matched entry is finished —
    /// callers should show a "caught up" affordance.
    pub current_index: Option<i32>,
    /// Count of matched entries in the full list (so the rail can show
    /// "Position N / total" without a second fetch).
    pub total_matched: i32,
    /// Count of ALL entries (including unmatched). Surfaced for parity
    /// with `useCblList().stats.total`.
    pub total_entries: i32,
}

/// `GET /me/cbl-lists/{id}/window` — slice of the CBL anchored on the
/// user's next unfinished matched entry. The window is `before` finished
/// entries + the current entry + `after` upcoming entries. Used by the
/// home rail to give a "where am I in this list" view instead of always
/// starting from position 0.
///
/// Library-ACL filtering happens at hydrate time: an entry whose issue
/// lives in a library the user can't see is silently dropped from the
/// window (same policy as `/issues`). That can leave the window short of
/// `before + 1 + after` items in heavily-restricted environments; we
/// don't backfill because filling past the natural cutoff would surface
/// out-of-order entries to the user.
#[utoipa::path(
    get,
    path = "/me/cbl-lists/{id}/window",
    params(
        ("id"     = String, Path,),
        ("before" = Option<u32>, Query,),
        ("after"  = Option<u32>, Query,),
    ),
    responses(
        (status = 200, body = CblWindowView),
        (status = 404, description = "list not found"),
    )
)]
pub async fn reading_window(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<WindowQuery>,
) -> impl IntoResponse {
    use crate::api::series::IssueSummaryView;
    use crate::library::access;
    use entity::progress_record;

    let list = match fetch_list(&app.db, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(resp) = ensure_owner(&list, &user).await {
        return resp;
    }

    let before = q.before.unwrap_or(3).min(20) as i32;
    let after = q.after.unwrap_or(8).min(40) as i32;
    let acl = access::for_user(&app, &user).await;

    // All entries — matched + unmatched — so the "total entries" stat is
    // accurate. Matched-only is filtered out before we hydrate.
    let all_entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(id))
        .order_by_asc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let total_entries = all_entries.len() as i32;
    let matched_entries: Vec<_> = all_entries
        .into_iter()
        .filter(|e| e.matched_issue_id.is_some())
        .collect();
    let total_matched = matched_entries.len() as i32;

    if matched_entries.is_empty() {
        return Json(CblWindowView {
            items: Vec::new(),
            current_index: None,
            total_matched: 0,
            total_entries,
        })
        .into_response();
    }

    // Progress lookup for every matched issue — single batched query.
    let matched_issue_ids: Vec<String> = matched_entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let progress_rows = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user.id))
        .filter(progress_record::Column::IssueId.is_in(matched_issue_ids.clone()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let progress_by_issue: std::collections::HashMap<String, progress_record::Model> =
        progress_rows
            .into_iter()
            .map(|p| (p.issue_id.clone(), p))
            .collect();

    // Find the current entry — first matched entry whose progress shows
    // it as not-finished (or has no progress row).
    let current_pos: usize = match matched_entries.iter().position(|e| {
        let Some(issue_id) = &e.matched_issue_id else {
            return false;
        };
        progress_by_issue
            .get(issue_id)
            .map(|p| !p.finished)
            .unwrap_or(true)
    }) {
        Some(p) => p,
        None => {
            // Everything finished — show the tail so the user can re-read.
            matched_entries.len().saturating_sub(1)
        }
    };
    let all_done = matched_entries.iter().all(|e| {
        e.matched_issue_id
            .as_ref()
            .and_then(|id| progress_by_issue.get(id))
            .map(|p| p.finished)
            .unwrap_or(false)
    });

    let start = current_pos.saturating_sub(before as usize);
    let end = (current_pos + after as usize + 1).min(matched_entries.len());
    let slice = &matched_entries[start..end];

    // Hydrate the slice's issues + parent series for slugs.
    let slice_issue_ids: Vec<String> = slice
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let issues = match entity::issue::Entity::find()
        .filter(entity::issue::Column::Id.is_in(slice_issue_ids))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let series_ids: std::collections::HashSet<Uuid> = issues.iter().map(|i| i.series_id).collect();
    let series_rows = match entity::series::Entity::find()
        .filter(entity::series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let series_by_id: std::collections::HashMap<Uuid, entity::series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    let issue_by_id: std::collections::HashMap<String, entity::issue::Model> =
        issues.into_iter().map(|i| (i.id.clone(), i)).collect();

    let mut items: Vec<CblWindowEntry> = Vec::with_capacity(slice.len());
    // Track where the current entry lands in the *output* list (after
    // ACL filtering may have dropped some preceding rows).
    let mut current_output_index: Option<i32> = None;
    for (offset, entry) in slice.iter().enumerate() {
        let Some(issue_id) = entry.matched_issue_id.clone() else {
            continue;
        };
        let Some(issue) = issue_by_id.get(&issue_id) else {
            continue;
        };
        let Some(series) = series_by_id.get(&issue.series_id) else {
            continue;
        };
        if !acl.contains(series.library_id) {
            continue;
        }
        let progress = progress_by_issue.get(&issue_id);
        let finished = progress.map(|p| p.finished).unwrap_or(false);
        let last_page = progress.map(|p| p.last_page).unwrap_or(0);
        let percent = progress.map(|p| p.percent).unwrap_or(0.0);

        // Compare the slice offset to where the current row lives in
        // `slice` (i.e. `current_pos - start`).
        let slice_current_offset = current_pos.saturating_sub(start);
        if offset == slice_current_offset {
            current_output_index = Some(items.len() as i32);
        }

        items.push(CblWindowEntry {
            issue: IssueSummaryView::from_model(issue.clone(), &series.slug)
                .with_series_name(series.name.clone()),
            position: entry.position,
            finished,
            last_page,
            percent,
        });
    }

    Json(CblWindowView {
        items,
        // When everything's finished we still return the tail of the
        // list, but flag `current_index = None` so the client can
        // render the "caught up" state instead of a "current" marker.
        current_index: if all_done { None } else { current_output_index },
        total_matched,
        total_entries,
    })
    .into_response()
}
