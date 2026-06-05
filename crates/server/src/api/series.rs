//! `/series` and `/series/{id}` (Phase 1a).

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use entity::{issue, library_user_access, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, Value, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

const MAX_QUERY_LEN: usize = 200;

use super::error;
use crate::api::libraries::{ScanMode, ScanResp};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(get_one))
        .routes(routes!(collection_report))
        .routes(routes!(update_series))
        .routes(routes!(scan_series))
        .routes(routes!(list_issues))
        .routes(routes!(resume))
}

/// Resolve a series slug to its row. Standard 404 envelope on miss.
/// Resolve a series by its public path component. Accepts either the
/// human-readable slug (the canonical form post-migration) or a raw UUID.
///
/// The UUID path exists for two reasons:
///   1. The reader persists per-series overrides under the series **id**
///      in `localStorage` (`reader:viewMode:<uuid>`). The settings page's
///      "Per-series overrides" card resolves those ids via `useSeries(id)`,
///      so the GET endpoint must keep accepting UUIDs or every row falls
///      back to "Unknown series".
///   2. Older clients / bookmarks predating the slug migration.
///
/// Slug values are kebab-cased and never collide with the canonical UUID
/// 8-4-4-4-12 format, so a successful `Uuid::parse_str` unambiguously means
/// the caller passed an id.
pub(crate) async fn find_by_slug(
    db: &sea_orm::DatabaseConnection,
    slug: &str,
) -> Result<series::Model, axum::response::Response> {
    let lookup = if let Ok(id) = Uuid::parse_str(slug) {
        series::Entity::find_by_id(id).one(db).await
    } else {
        series::Entity::find()
            .filter(series::Column::Slug.eq(slug))
            .one(db)
            .await
    };
    match lookup {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "series not found",
        )),
        Err(e) => {
            tracing::error!(error = %e, slug, "series lookup failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

/// Optional query params for the scan-series endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct ScanSeriesQuery {
    /// Defaults to `true` — clicking "Scan series" is an explicit user
    /// request. The query string can opt back into the cheap fast path
    /// with `?force=false`.
    #[serde(default = "scan_force_default")]
    pub force: bool,
}

fn scan_force_default() -> bool {
    true
}

#[utoipa::path(
    operation_id = "series_scan_series",    post,
    path = "/series/{slug}/scan",
    params(
        ("slug" = String, Path,),
        ("force" = Option<bool>, Query, description = "Bypass the size+mtime fast path. Defaults to true."),
    ),
    responses(
        (status = 202, description = "scan_series job enqueued"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn scan_series(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<ScanSeriesQuery>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let outcome = match app
        .jobs
        .coalesce_scoped_scan(
            row.library_id,
            row.id,
            row.folder_path.clone(),
            crate::jobs::scan_series::JobKind::Series,
            None,
            q.force,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(series_id = %row.id, error = %e, "scan_series enqueue failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let mode = if q.force {
        ScanMode::ContentVerify
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
            kind: "series",
            library_id: row.library_id.to_string(),
            mode: mode.as_str(),
            coalesced_into: outcome
                .was_coalesced()
                .then(|| outcome.scan_id().to_string()),
            queued_followup: false,
            reason: mode.reason().to_owned(),
            series_id: Some(row.id.to_string()),
            issue_id: None,
        }),
    )
        .into_response()
}

/// Body for `PATCH /series/{id}`. `match_key` is the §7.4 sticky override
/// the scanner won't touch; `slug` is the admin-rename hook for the URL
/// segment (validated unique across all series). `status` and the external
/// IDs are surfaced in the issue drawer so curators can correct
/// continuing/ended state and database links without leaving the issue page.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSeriesReq {
    /// `null` clears the override; an empty/whitespace string is treated as null.
    #[serde(default)]
    pub match_key: Option<String>,
    /// Admin override for the URL slug. Slugified server-side; rejected on
    /// collision.
    #[serde(default)]
    pub slug: Option<String>,
    /// Publication status — one of `continuing`, `ended`, `cancelled`,
    /// `hiatus`, `limited`. Scanner defaults to `continuing` on create.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some_i64")]
    pub comicvine_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_some_i64")]
    pub metron_id: Option<Option<i64>>,
    /// Series-level summary. `null` clears (the API will fall back to the
    /// first issue's summary on read).
    #[serde(default, deserialize_with = "deserialize_some_string")]
    pub summary: Option<Option<String>>,
    /// Per-series reading-direction override. `"ltr"` / `"rtl"` /
    /// `"ttb"` (future-compat) or `null` for "Auto, inherit". Empty or
    /// whitespace-only strings are rejected — clients should send
    /// `null` explicitly to clear. See `manga-and-bulk-metadata-1.0` M2.
    #[serde(default, deserialize_with = "deserialize_some_string")]
    pub reading_direction: Option<Option<String>>,
}

fn deserialize_some_i64<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(d).map(Some)
}

fn deserialize_some_string<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

#[utoipa::path(
    operation_id = "series_update_series",    patch,
    path = "/series/{slug}",
    params(("slug" = String, Path,)),
    request_body = UpdateSeriesReq,
    responses(
        (status = 200, body = SeriesView),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn update_series(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpdateSeriesReq>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = row.id;

    let normalized_key = req.match_key.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_owned())
        }
    });

    // Validate the publication status if the caller is changing it. Empty
    // string clears nothing — status is NOT NULL so we reject empty input;
    // callers wanting "no status" must pick `continuing`.
    let normalized_status = if let Some(s) = req.status.as_ref() {
        let t = s.trim().to_ascii_lowercase();
        if !matches!(
            t.as_str(),
            "continuing" | "ended" | "cancelled" | "hiatus" | "limited"
        ) {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.status",
                "status must be continuing, ended, cancelled, hiatus, or limited",
            );
        }
        Some(t)
    } else {
        None
    };

    // Validate the reading-direction override if the caller is
    // setting it. `null` clears (back to "Auto"); a recognized value
    // pins. Unknown values are rejected so a typo doesn't silently
    // disable the cascade.
    let normalized_reading_dir = if let Some(v) = req.reading_direction.as_ref() {
        match v.as_deref() {
            None => Some(None),
            Some(s) => {
                let t = s.trim().to_ascii_lowercase();
                if !matches!(t.as_str(), "ltr" | "rtl" | "ttb") {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation.reading_direction",
                        "reading_direction must be ltr, rtl, ttb, or null",
                    );
                }
                Some(Some(t))
            }
        }
    } else {
        None
    };

    // Validate + slugify any admin-supplied slug.
    let new_slug = if let Some(input) = req.slug.as_deref() {
        let s = crate::slug::slugify_segment(input);
        use crate::slug::SlugAllocator;
        let allocator = crate::slug::SeriesSlugAllocator {
            db: &app.db,
            excluding: Some(uuid),
        };
        match allocator.is_taken(&s).await {
            Ok(true) => {
                return error(StatusCode::CONFLICT, "conflict.slug", "slug already in use");
            }
            Ok(false) => Some(s),
            Err(e) => {
                tracing::error!(error = %e, "slug uniqueness check failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    let mut am: series::ActiveModel = row.into();
    am.match_key = Set(normalized_key);
    if let Some(s) = new_slug.clone() {
        am.slug = Set(s);
    }
    if let Some(s) = normalized_status.clone() {
        am.status = Set(s);
        // Stamp the sticky-override timestamp so the scanner's
        // post-scan `reconcile_series_status` skips this row's status
        // write on subsequent scans. The total_issues refresh is
        // deliberately independent — see reconcile_status.rs for the
        // rationale.
        am.status_user_set_at = Set(Some(Utc::now().fixed_offset()));
    }
    // comicvine_id / metron_id live in external_ids now; applied
    // via writers::set_external_id after the row update commits
    // (see the Ok(updated) branch below). M5's <ExternalIdsCard>
    // is the richer user surface; this PATCH stays for backwards
    // compat with the existing series edit drawer.
    let pending_cv = req.comicvine_id;
    let pending_metron = req.metron_id;
    let normalized_summary = req.summary.as_ref().map(|v| {
        v.as_ref().and_then(|s| {
            let t = s.trim().to_owned();
            if t.is_empty() { None } else { Some(t) }
        })
    });
    if let Some(v) = normalized_summary.clone() {
        am.summary = Set(v);
    }
    if let Some(v) = normalized_reading_dir.clone() {
        am.reading_direction = Set(v);
    }
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    match am.update(&app.db).await {
        Ok(updated) => {
            // Apply external-ID edits the user touched. Mirrors the
            // per-issue PATCH flow in api/issues.rs.
            use crate::metadata::writers::{self as writers, SetBy};
            use crate::metadata::{Identifier, Source};
            if let Some(opt) = pending_cv {
                let res = match opt {
                    Some(v) => {
                        writers::set_external_id(
                            &app.db,
                            "series",
                            &updated.id.to_string(),
                            &Identifier::new(Source::ComicVine, v.to_string()),
                            SetBy::User,
                        )
                        .await
                    }
                    None => {
                        writers::delete_external_id(
                            &app.db,
                            "series",
                            &updated.id.to_string(),
                            Source::ComicVine,
                        )
                        .await
                    }
                };
                if let Err(e) = res {
                    tracing::error!(series_id = %updated.id, error = %e, "series external_id (comicvine) write failed");
                }
            }
            if let Some(opt) = pending_metron {
                let res = match opt {
                    Some(v) => {
                        writers::set_external_id(
                            &app.db,
                            "series",
                            &updated.id.to_string(),
                            &Identifier::new(Source::Metron, v.to_string()),
                            SetBy::User,
                        )
                        .await
                    }
                    None => {
                        writers::delete_external_id(
                            &app.db,
                            "series",
                            &updated.id.to_string(),
                            Source::Metron,
                        )
                        .await
                    }
                };
                if let Err(e) = res {
                    tracing::error!(series_id = %updated.id, error = %e, "series external_id (metron) write failed");
                }
            }
            if let Some(s) = new_slug {
                crate::audit::record(
                    &app.db,
                    crate::audit::AuditEntry {
                        actor_id: user.id,
                        action: "admin.series.slug.set",
                        target_type: Some("series"),
                        target_id: Some(uuid.to_string()),
                        payload: serde_json::json!({ "slug": s }),
                        ip: ctx.ip_string(),
                        user_agent: ctx.user_agent.clone(),
                    },
                )
                .await;
            }
            // Single combined audit row for status / external IDs since the
            // user can flip several at once from the issue drawer.
            let mut diff = serde_json::Map::new();
            if let Some(s) = normalized_status {
                diff.insert("status".into(), serde_json::json!(s));
            }
            if let Some(v) = req.comicvine_id {
                diff.insert("comicvine_id".into(), serde_json::json!(v));
            }
            if let Some(v) = req.metron_id {
                diff.insert("metron_id".into(), serde_json::json!(v));
            }
            if let Some(v) = normalized_summary {
                diff.insert("summary".into(), serde_json::json!(v));
            }
            if let Some(v) = normalized_reading_dir {
                diff.insert("reading_direction".into(), serde_json::json!(v));
            }
            if !diff.is_empty() {
                crate::audit::record(
                    &app.db,
                    crate::audit::AuditEntry {
                        actor_id: user.id,
                        action: "admin.series.update",
                        target_type: Some("series"),
                        target_id: Some(uuid.to_string()),
                        payload: serde_json::Value::Object(diff),
                        ip: ctx.ip_string(),
                        user_agent: ctx.user_agent.clone(),
                    },
                )
                .await;
            }
            let updated_id_text = updated.id.to_string();
            let mut view = SeriesView::from(updated);
            enrich_series_view_legacy_ids(&app.db, &mut view, &updated_id_text).await;
            Json(view).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "update series failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SeriesView {
    pub id: String,
    pub library_id: String,
    pub name: String,
    pub slug: String,
    pub year: Option<i32>,
    pub volume: Option<i32>,
    pub publisher: Option<String>,
    pub status: String,
    pub total_issues: Option<i32>,
    pub age_rating: Option<String>,
    pub summary: Option<String>,
    pub language_code: String,
    /// External-database IDs (ComicVine volume id, Metron series id). Set by
    /// the scanner from ComicInfo or by admins via `PATCH /series/{slug}`.
    pub comicvine_id: Option<i64>,
    pub metron_id: Option<i64>,
    pub issue_count: Option<i64>,
    /// URL of the first issue's cover thumbnail. Null when no active issue exists.
    pub cover_url: Option<String>,
    /// RFC3339 timestamps from the series row.
    pub created_at: String,
    pub updated_at: String,
    /// Aggregated CSV-style ComicInfo fields, frequency-ordered (most frequent
    /// first). Empty on the list endpoint to keep payloads small; populated
    /// only by `GET /series/{id}`. The `From<Model>` impl initializes these
    /// to empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pencillers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inkers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colorists: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub letterers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cover_artists: Vec<String>,
    /// Name → creator slug map covering every credit listed in the
    /// `writers / pencillers / inkers / colorists / letterers /
    /// cover_artists` arrays above (and the editor/translator roles
    /// when they're surfaced). Populated server-side from a single
    /// JOIN onto `person` via `series_credits.person_id`, so the
    /// web's `ChipList` can build `/creators/<slug>` links in O(1)
    /// per chip without a per-pill name-resolution round-trip.
    /// Missing entries fall back to the legacy
    /// `/?library=all&credits=<name>` filter — typically only
    /// happens for credits scanned between rollups.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub creator_slugs: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub characters: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<String>,
    /// Sum of `page_count` across active, on-disk issues. Detail-only.
    pub total_page_count: Option<i64>,
    /// Most-recent `created_at` / `updated_at` across active, on-disk issues.
    /// Drives the "Recently Added/Updated" rails on the home page (sorted at
    /// the series row level today; these fields are informational on detail).
    pub last_issue_added_at: Option<String>,
    pub last_issue_updated_at: Option<String>,
    /// Earliest / latest publication year across the series's issues. The
    /// series-level `year` is "first release"; `latest_year` lets the UI
    /// render a range (e.g. "2012–2018"). Both `None` when no active issue
    /// has a parsed year.
    pub earliest_year: Option<i32>,
    pub latest_year: Option<i32>,
    /// Per-user read progress across the entire series — server-computed
    /// so the UI doesn't have to paginate the issue list to compute "X of N
    /// read". `None` on the list endpoint; populated only on `get_one`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_summary: Option<SeriesProgressSummary>,
    /// Calling user's rating for this series, 0..=5 in half-star steps.
    /// `None` means "no rating set". Detail-only.
    pub user_rating: Option<f64>,
    /// Per-series reading-direction override. `"ltr"` / `"rtl"` /
    /// `"ttb"` or `None` meaning "Auto — inherit from user pref /
    /// library default at read time". The reader consults this above
    /// the user and library defaults but below ComicInfo `<Manga>`.
    /// Editable via `PATCH /series/{slug}` and set automatically by
    /// the M3 scanner heuristic when ≥80% of a series's issues
    /// declare manga.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_direction: Option<String>,
    /// Search-result excerpt with `<mark>…</mark>` tags around matched
    /// terms. Populated only on search-mode list responses; `None`
    /// everywhere else. The frontend renders this as HTML with a small
    /// allowlist sanitiser so future field changes can't introduce
    /// XSS surface area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Metadata-completeness assessment ("does this need metadata pulled?").
    /// Detail-only — populated by `get_one`; `None` on the list endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_completeness: Option<crate::metadata::completeness::CompletenessReport>,
    /// At-a-glance completeness tier (`"complete"` | `"partial"` |
    /// `"needs_metadata"`) rolled up across the series' active issues. The
    /// lightweight card-badge counterpart to [`Self::metadata_completeness`];
    /// populated by `hydrate_series` on list endpoints. `None` when no signal
    /// (e.g. zero active issues) — the card shows no badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_completeness_tier: Option<String>,
    /// Per-issue completeness rollup across the series (`complete` of `total`
    /// active issues whose metadata is [`CompletenessTier::Complete`]). Drives
    /// the Collection tab's "N of M issues have complete metadata" line.
    /// Detail-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_completeness_summary: Option<MetadataCompletenessSummary>,
}

/// Issue-level completeness rollup for a series. `complete` counts active
/// issues meeting the issue core criteria; `total` is the active-issue count.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetadataCompletenessSummary {
    pub complete: i64,
    pub total: i64,
}

/// Per-user, server-computed read progress for the whole series. Sidesteps
/// the client-side cap on the issues page (which fetches 100 at a time).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SeriesProgressSummary {
    /// Active issues in the series (excludes removed / soft-deleted).
    pub total: i64,
    /// Active issues the user has finished (`progress.finished == true`).
    pub finished: i64,
    /// Active issues the user has started but not finished.
    pub in_progress: i64,
    /// Sum of `issue.page_count` over the active issues the user has
    /// finished. Used by the series page's "Reading load" stat to estimate
    /// how many pages — and therefore minutes — remain on the series.
    /// `0` when `page_count` is null on every finished issue.
    pub finished_pages: i64,
}

impl From<series::Model> for SeriesView {
    fn from(m: series::Model) -> Self {
        Self {
            id: m.id.to_string(),
            library_id: m.library_id.to_string(),
            name: m.name,
            slug: m.slug,
            year: m.year,
            volume: m.volume,
            publisher: m.publisher,
            status: m.status,
            total_issues: m.total_issues,
            age_rating: m.age_rating,
            summary: m.summary,
            language_code: m.language_code,
            // CV/Metron IDs live in external_ids — the From impl
            // can't reach the DB, so call sites populate via
            // [`enrich_series_view_legacy_ids`] below. The full
            // per-source set is exposed via M5's <ExternalIdsCard>
            // endpoint; these two scalars stay for backwards compat.
            comicvine_id: None,
            metron_id: None,
            issue_count: None,
            cover_url: None,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
            writers: Vec::new(),
            pencillers: Vec::new(),
            inkers: Vec::new(),
            colorists: Vec::new(),
            letterers: Vec::new(),
            cover_artists: Vec::new(),
            creator_slugs: std::collections::HashMap::new(),
            genres: Vec::new(),
            tags: Vec::new(),
            characters: Vec::new(),
            teams: Vec::new(),
            locations: Vec::new(),
            total_page_count: None,
            last_issue_added_at: None,
            last_issue_updated_at: None,
            earliest_year: None,
            latest_year: None,
            progress_summary: None,
            user_rating: None,
            reading_direction: m.reading_direction,
            snippet: None,
            metadata_completeness: None,
            metadata_completeness_summary: None,
            metadata_completeness_tier: None,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueSummaryView {
    pub id: String,
    pub slug: String,
    pub series_id: String,
    pub series_slug: String,
    /// Parent series name, denormalized so card components can fall back
    /// to `"<series> #<number>"` when the issue has no title. Populated
    /// by endpoints that have the series row in scope (rails, CBL
    /// window, collections, search); `None` on endpoints where adding
    /// the join would cost more than the user-visible benefit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    pub title: Option<String>,
    pub number: Option<String>,
    pub sort_number: Option<f64>,
    pub year: Option<i32>,
    pub page_count: Option<i32>,
    pub state: String,
    pub cover_url: Option<String>,
    /// Spec §6.5 classification: `"Special"`, `"Annual"`, `"OneShot"`,
    /// `"TPB"`, or `None` for ordinary numbered issues. Drives the
    /// reader's Specials & Extras section on the series detail page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special_type: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueDetailView {
    pub id: String,
    /// URL-safe identifier, unique within the parent series.
    pub slug: String,
    /// Slug of the parent series — handy for nested URL construction.
    pub series_slug: String,
    pub series_id: String,
    pub library_id: String,
    pub file_path: String,
    pub state: String,
    pub title: Option<String>,
    pub number: Option<String>,
    pub sort_number: Option<f64>,
    pub volume: Option<i32>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub summary: Option<String>,
    pub notes: Option<String>,
    pub publisher: Option<String>,
    pub writer: Option<String>,
    pub penciller: Option<String>,
    pub inker: Option<String>,
    pub colorist: Option<String>,
    pub letterer: Option<String>,
    pub cover_artist: Option<String>,
    pub editor: Option<String>,
    pub translator: Option<String>,
    pub imprint: Option<String>,
    pub characters: Option<String>,
    pub teams: Option<String>,
    pub locations: Option<String>,
    pub alternate_series: Option<String>,
    pub tags: Option<String>,
    pub genre: Option<String>,
    pub language_code: Option<String>,
    pub age_rating: Option<String>,
    pub manga: Option<String>,
    pub format: Option<String>,
    pub black_and_white: Option<bool>,
    pub page_count: Option<i32>,
    pub story_arc: Option<String>,
    pub story_arc_number: Option<String>,
    pub web_url: Option<String>,
    pub gtin: Option<String>,
    /// External-database IDs. ComicVine encodes the ID as `4000-N` for issues;
    /// the parser strips the prefix so callers see the bare integer.
    pub comicvine_id: Option<i64>,
    pub metron_id: Option<i64>,
    /// Parent series' `reading_direction` override. Consulted by the
    /// reader above the user and library defaults but below ComicInfo
    /// `<Manga>`. `None` = "Auto" (series has no override). Plumbed by
    /// the issue-detail handler; not stored on the issue row itself.
    /// See `manga-and-bulk-metadata-1.0` M2.
    pub series_reading_direction: Option<String>,
    /// Parent library's `default_reading_direction` (`"ltr"` | `"rtl"`).
    /// The reader consults this as a fallback below ComicInfo
    /// `<Manga>` and the user's per-account preference but above the
    /// hard-coded LTR default. Plumbed by the issue-detail handler;
    /// not stored on the issue row itself.
    pub library_default_reading_direction: Option<String>,
    /// Calling user's 0..=5 rating for this issue. `None` when unset.
    pub user_rating: Option<f64>,
    /// File size in bytes from the disk row at the last scan. Surfaced in the
    /// admin "Details" tab alongside `file_path`.
    pub file_size: i64,
    pub created_at: String,
    pub updated_at: String,
    /// User-curated extra links beyond `web_url` (which mirrors ComicInfo).
    /// Each entry has a required `url` and optional `label`.
    pub additional_links: Vec<IssueLink>,
    /// Names of fields the user has overridden via `PATCH /issues/{id}`. The
    /// scanner skips these on a rescan. Surfaced so the UI can flag rows as
    /// "edited" and offer a "revert to ComicInfo" affordance later.
    pub user_edited: Vec<String>,
    /// Per-page metadata, deserialized from the issue's stored JSON. Empty when
    /// the parse failed or the source had no `<Pages>` block.
    /// `value_type` keeps the parsers crate framework-free; the actual schema
    /// fields (image, type, double_page, image_width, image_height, …) are
    /// stable and documented in `crates/parsers/src/comicinfo.rs::PageInfo`.
    #[schema(value_type = Vec<serde_json::Value>)]
    pub pages: Vec<parsers::comicinfo::PageInfo>,
    pub comic_info_raw: serde_json::Value,
    /// Creator-name → slug map covering every credit name listed in
    /// the per-role CSV fields above (writer/penciller/inker/…). Built
    /// from `issue_credits.person_id` joined to `person`; missing
    /// entries fall back to the legacy library-grid filter in the UI.
    /// Empty until the get_one handler populates it.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub creator_slugs: std::collections::HashMap<String, String>,
    /// ISO-8601 timestamp of the last in-place rewrite of this issue's
    /// archive bytes (from `metadata-sidecar-writeback-1.0` M3+ or
    /// `archive-rewrite-1.0` M2+). `None` when Folio has never
    /// rewritten the file. Surfaces in the UI as a "Metadata last
    /// written {kind} on {date}" badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rewrite_at: Option<String>,
    /// `"sidecar"` (XML refresh) or `"edit"` (page bytes touched).
    /// Paired with [`Self::last_rewrite_at`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rewrite_kind: Option<String>,
    /// ISO-8601 timestamp of the last provider metadata sync for this issue
    /// (`issue.last_metadata_sync_at`). `None` when never synced. Surfaced in
    /// the Metadata tab's freshness section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_metadata_sync_at: Option<String>,
    /// Whether the parent library has archive writeback enabled. Gates
    /// the admin "Edit archive…" affordance in the UI
    /// (`archive-rewrite-1.0` M3). Populated by the issue-detail handler;
    /// defaults false (not stored on the issue row).
    #[serde(default)]
    pub allow_archive_writeback: bool,
    /// 0-based index of the page used as the cover. Surfaced so the page
    /// editor can highlight the current cover. `archive-rewrite-1.0`.
    #[serde(default)]
    pub cover_page_index: i32,
    /// Whether the parent library has acknowledged the CBR→CBZ conversion
    /// at least once (`library.cbr_convert_confirmed_at` is set). The page
    /// editor uses this to decide whether to show the one-time conversion
    /// explainer when editing a `.cbr`. `archive-rewrite-1.0` M6.
    #[serde(default)]
    pub library_cbr_convert_confirmed: bool,
    /// Metadata-completeness assessment ("does this issue need metadata
    /// pulled?"). Populated by the issue-detail handler; `None` on list
    /// endpoints (the cheaper `IssueSummaryView.metadata_completeness_tier`
    /// carries the at-a-glance signal there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_completeness: Option<crate::metadata::completeness::CompletenessReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IssueLink {
    /// Display label. `null` falls back to the URL host on the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub url: String,
}

impl IssueSummaryView {
    /// `series_slug` is the parent series' slug, populated by the caller
    /// (which has the series row in scope). `From` is removed because the
    /// slug isn't on the issue::Model itself.
    pub fn from_model(m: issue::Model, series_slug: &str) -> Self {
        let cover_url = (m.state == "active").then(|| format!("/issues/{}/pages/0/thumb", m.id));
        Self {
            id: m.id,
            slug: m.slug,
            series_id: m.series_id.to_string(),
            series_slug: series_slug.to_owned(),
            series_name: None,
            title: m.title,
            number: m.number_raw,
            sort_number: m.sort_number,
            year: m.year,
            page_count: m.page_count,
            state: m.state,
            cover_url,
            special_type: m.special_type,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }

    /// Attach the parent series name. Use everywhere we want card
    /// components to fall back to `"<series> #<number>"` on issues with
    /// no title. Builder shape so call sites read as
    /// `from_model(m, slug).with_series_name(&series.name)` without
    /// duplicating the existing slug-only constructor.
    pub fn with_series_name(mut self, name: impl Into<String>) -> Self {
        self.series_name = Some(name.into());
        self
    }
}

impl IssueDetailView {
    /// See [`IssueSummaryView::from_model`] — same reason for taking the
    /// parent series slug as a separate argument.
    pub fn from_model(m: issue::Model, series_slug: &str) -> Self {
        Self {
            id: m.id,
            slug: m.slug,
            series_slug: series_slug.to_owned(),
            series_id: m.series_id.to_string(),
            library_id: m.library_id.to_string(),
            file_path: m.file_path,
            state: m.state,
            title: m.title,
            number: m.number_raw,
            sort_number: m.sort_number,
            volume: m.volume,
            year: m.year,
            month: m.month,
            day: m.day,
            summary: m.summary,
            notes: m.notes,
            publisher: m.publisher,
            writer: m.writer,
            penciller: m.penciller,
            inker: m.inker,
            colorist: m.colorist,
            letterer: m.letterer,
            cover_artist: m.cover_artist,
            editor: m.editor,
            translator: m.translator,
            imprint: m.imprint,
            characters: m.characters,
            teams: m.teams,
            locations: m.locations,
            alternate_series: m.alternate_series,
            tags: m.tags,
            genre: m.genre,
            language_code: m.language_code,
            age_rating: m.age_rating,
            manga: m.manga,
            format: m.format,
            black_and_white: m.black_and_white,
            page_count: m.page_count,
            story_arc: m.story_arc,
            story_arc_number: m.story_arc_number,
            web_url: m.web_url,
            // CV / Metron / GTIN are in external_ids — populated by
            // [`enrich_issue_detail_legacy_ids`] at the call sites.
            // M5's <ExternalIdsCard> is the canonical full surface;
            // these three stay on the response for backwards compat.
            gtin: None,
            comicvine_id: None,
            metron_id: None,
            series_reading_direction: None,
            library_default_reading_direction: None,
            user_rating: None,
            file_size: m.file_size,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
            additional_links: serde_json::from_value(m.additional_links).unwrap_or_default(),
            user_edited: serde_json::from_value(m.user_edited).unwrap_or_default(),
            // The scanner persists `Vec<PageInfo>` via `serde_json::to_value`; round-trip back.
            // Tolerate broken / empty JSON by falling back to an empty list — the reader
            // already copes with no per-page metadata.
            pages: serde_json::from_value(m.pages).unwrap_or_default(),
            comic_info_raw: m.comic_info_raw,
            creator_slugs: std::collections::HashMap::new(),
            last_rewrite_at: m.last_rewrite_at.map(|t| t.to_rfc3339()),
            last_rewrite_kind: m.last_rewrite_kind,
            last_metadata_sync_at: m.last_metadata_sync_at.map(|t| t.to_rfc3339()),
            allow_archive_writeback: false,
            cover_page_index: m.cover_page_index,
            library_cbr_convert_confirmed: false,
            metadata_completeness: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SeriesSort {
    #[default]
    Name,
    CreatedAt,
    UpdatedAt,
    /// Release year (`series.year`). Nullable column — NULLs sort
    /// last on ASC, first on DESC, then ID tiebreaks.
    Year,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueSort {
    /// `sort_number` ASC (NULLS LAST), tie-break on `id`. Default.
    #[default]
    Number,
    CreatedAt,
    UpdatedAt,
    /// Release year (`issue.year`). Nullable; NULLs sort last on ASC.
    Year,
    /// Page count — proxy for "time to read". Nullable.
    PageCount,
    /// Calling user's rating from `user_ratings` (per-issue scope).
    /// Issues without a rating from this user sort last on ASC.
    UserRating,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Deserialize)]
pub struct ListSeriesQuery {
    pub library: Option<Uuid>,
    pub q: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub sort: Option<SeriesSort>,
    #[serde(default)]
    pub order: Option<SortOrder>,
    #[serde(default)]
    pub cursor: Option<String>,
    /// Single status enum (`continuing` | `ended` | `cancelled` | `hiatus`).
    #[serde(default)]
    pub status: Option<String>,
    /// Inclusive lower / upper bounds on `series.year`. NULL years are
    /// excluded when either bound is set.
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    /// Comma-separated lists for the metadata facet filters used by the
    /// library grid. Series-direct columns (`publisher`, `language`,
    /// `age_rating`) are IN-set; `genres` / `tags` and the credit-role
    /// fields are includes-any against their junction tables.
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub genres: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub age_rating: Option<String>,
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
    /// Any-role credit filter — CSV of person names. Matches series
    /// where the person has at least one credit in `series_credits`,
    /// regardless of role. Used by the people-search click-through so
    /// landing on a creator surfaces every series they touched, not
    /// just the slice under one role. Use this rather than stacking
    /// per-role facets, which AND-combine and produce empty result
    /// sets for creators who hold different roles on different series.
    #[serde(default)]
    pub credits: Option<String>,
    /// Cast / setting facets — includes-any against the CSV columns on
    /// the issues table. Series-level lists (`SeriesView.characters`,
    /// etc.) are aggregated from these issue rows, so filtering on
    /// issues yields the right series set.
    #[serde(default)]
    pub characters: Option<String>,
    #[serde(default)]
    pub teams: Option<String>,
    #[serde(default)]
    pub locations: Option<String>,
    /// Inclusive bounds on the calling user's stored series rating
    /// (0..=5, half-star steps). When either bound is set, series with
    /// no rating from this user are excluded.
    #[serde(default)]
    pub user_rating_min: Option<f64>,
    #[serde(default)]
    pub user_rating_max: Option<f64>,
}

const VALID_STATUSES: &[&str] = &["continuing", "ended", "cancelled", "hiatus"];

pub(crate) fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct ListIssuesQuery {
    pub q: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub sort: Option<IssueSort>,
    #[serde(default)]
    pub order: Option<SortOrder>,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn default_limit() -> u64 {
    50
}

pub(crate) fn clamp_limit(limit: u64) -> u64 {
    limit.clamp(1, 100)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesListView {
    pub items: Vec<SeriesView>,
    pub next_cursor: Option<String>,
    /// Total matching rows across all pages — populated only on the
    /// first page (no cursor). `None` on subsequent pages where the
    /// caller already has this number from page zero, so we don't pay
    /// the `COUNT(*)` cost on every page fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueListView {
    pub items: Vec<IssueSummaryView>,
    pub next_cursor: Option<String>,
    /// See [`SeriesListView::total`] — same first-page-only semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

/// Cursors are opaque base64 strings encoding `"{sort_value}|{id}"`. The
/// caller never needs to know the format — the endpoint that issued the
/// cursor is the only thing that interprets it. Empty `sort_value` is valid
/// (used when sorting by a nullable column and the boundary row's sort
/// value was NULL). The `id` segment is opaque to this helper; series
/// callers parse it as `Uuid`, issue callers consume it as a `String`.
pub(crate) fn encode_cursor(sort_value: &str, id: &str) -> String {
    use base64::Engine;
    let s = format!("{}|{}", sort_value, id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

pub(crate) fn parse_cursor(s: &str) -> Result<(String, String), ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let decoded = String::from_utf8(bytes).map_err(|_| ())?;
    let (value, id) = decoded.rsplit_once('|').ok_or(())?;
    Ok((value.to_string(), id.to_string()))
}

/// Opaque offset cursor for text-ranked search results. Search ordering is
/// rank-based and not all callers can express a stable keyset boundary, so
/// this keeps pagination incremental without exposing raw offsets in URLs.
pub(crate) fn encode_offset_cursor(offset: u64) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("offset:{offset}"))
}

pub(crate) fn parse_offset_cursor(s: &str) -> Result<u64, ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let decoded = String::from_utf8(bytes).map_err(|_| ())?;
    decoded
        .strip_prefix("offset:")
        .ok_or(())?
        .parse::<u64>()
        .map_err(|_| ())
}

// ───── /series list helpers ─────
//
// Pattern mirrors `issues::list` (M7.1): validation up front, then
// visibility → static filters → search-mode early return → count →
// cursor → sort → fetch → hydrate. Each helper threads
// sea_orm::Select<series::Entity> through; validation helpers return
// `Result<_, &'static str>` so the handler can wrap with the canonical
// `error()` builder (keeps clippy::result_large_err happy).

fn validate_list_series_query_params(q: &ListSeriesQuery) -> Result<(), &'static str> {
    if let Some(s) = q.q.as_ref()
        && s.len() > MAX_QUERY_LEN
    {
        return Err("q too long");
    }
    if let Some(s) = q.status.as_deref()
        && !VALID_STATUSES.contains(&s)
    {
        return Err("unknown status");
    }
    if q.user_rating_min.is_some() || q.user_rating_max.is_some() {
        let min = q.user_rating_min.unwrap_or(0.0);
        let max = q.user_rating_max.unwrap_or(5.0);
        if !(0.0..=5.0).contains(&min) || !(0.0..=5.0).contains(&max) || min > max {
            return Err("user_rating bounds must be 0..=5 with min <= max");
        }
    }
    Ok(())
}

fn apply_series_visibility(
    mut select: sea_orm::Select<series::Entity>,
    visible: &VisibleLibs,
    library: Option<Uuid>,
) -> Option<sea_orm::Select<series::Entity>> {
    if let Some(lib) = library {
        if !visible.unrestricted && !visible.allowed.contains(&lib) {
            return None;
        }
        select = select.filter(series::Column::LibraryId.eq(lib));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return None;
        }
        select = select.filter(
            series::Column::LibraryId.is_in(visible.allowed.iter().copied().collect::<Vec<_>>()),
        );
    }
    Some(select)
}

fn apply_series_direct_column_filters(
    mut select: sea_orm::Select<series::Entity>,
    q: &ListSeriesQuery,
) -> sea_orm::Select<series::Entity> {
    if let Some(s) = q.status.as_deref() {
        select = select.filter(series::Column::Status.eq(s));
    }
    if let Some(y) = q.year_from {
        select = select.filter(series::Column::Year.gte(y));
    }
    if let Some(y) = q.year_to {
        select = select.filter(series::Column::Year.lte(y));
    }
    let direct_facets: [(Option<&str>, series::Column); 3] = [
        (q.publisher.as_deref(), series::Column::Publisher),
        (q.language.as_deref(), series::Column::LanguageCode),
        (q.age_rating.as_deref(), series::Column::AgeRating),
    ];
    for (raw, column) in direct_facets {
        let Some(raw) = raw else { continue };
        let values = split_csv(raw);
        if values.is_empty() {
            continue;
        }
        select = select.filter(column.is_in(values));
    }
    select
}

/// Genres + tags via the per-series junction tables.
fn apply_series_junction_facet_filters(
    mut select: sea_orm::Select<series::Entity>,
    q: &ListSeriesQuery,
) -> sea_orm::Select<series::Entity> {
    if let Some(raw) = q.genres.as_deref() {
        let values = split_csv(raw);
        if !values.is_empty() {
            select = select.filter(Expr::cust_with_values(
                "EXISTS (SELECT 1 FROM series_genres sg WHERE sg.series_id = series.id AND sg.genre = ANY($1))",
                [values],
            ));
        }
    }
    if let Some(raw) = q.tags.as_deref() {
        let values = split_csv(raw);
        if !values.is_empty() {
            select = select.filter(Expr::cust_with_values(
                "EXISTS (SELECT 1 FROM series_tags st WHERE st.series_id = series.id AND st.tag = ANY($1))",
                [values],
            ));
        }
    }
    select
}

/// One includes-any against `series_credits` per credit role.
fn apply_series_credit_role_filters(
    mut select: sea_orm::Select<series::Entity>,
    q: &ListSeriesQuery,
) -> sea_orm::Select<series::Entity> {
    let credit_facets: [(Option<&str>, &'static str); 8] = [
        (q.writers.as_deref(), "writer"),
        (q.pencillers.as_deref(), "penciller"),
        (q.inkers.as_deref(), "inker"),
        (q.colorists.as_deref(), "colorist"),
        (q.letterers.as_deref(), "letterer"),
        (q.cover_artists.as_deref(), "cover_artist"),
        (q.editors.as_deref(), "editor"),
        (q.translators.as_deref(), "translator"),
    ];
    for (raw, role) in credit_facets {
        let Some(raw) = raw else { continue };
        let values = split_csv(raw);
        if values.is_empty() {
            continue;
        }
        select = select.filter(Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM series_credits sc WHERE sc.series_id = series.id AND sc.role = $1 AND sc.person = ANY($2))",
            [Value::from(role), Value::from(values)],
        ));
    }
    // Any-role credits filter (the `credits` query param). Matches
    // series where the person has *any* credit role, sidestepping the
    // AND-combining problem that the per-role facets above create when
    // a creator holds different roles on different series.
    if let Some(raw) = q.credits.as_deref() {
        let values = split_csv(raw);
        if !values.is_empty() {
            select = select.filter(Expr::cust_with_values(
                "EXISTS (SELECT 1 FROM series_credits sc WHERE sc.series_id = series.id AND sc.person = ANY($1))",
                [Value::from(values)],
            ));
        }
    }
    select
}

/// Cast / setting facets — characters, teams, locations live as CSV
/// strings on `issues`, not in junction tables. Per-value split rule
/// mirrors `aggregate_csv` / `split_csv`: prefer `;` when the column
/// value contains one (so names like `"Capes, Inc."` survive),
/// otherwise split on `,`. Lowercased on both sides for case-
/// insensitive matching.
fn apply_series_cast_setting_filters(
    mut select: sea_orm::Select<series::Entity>,
    q: &ListSeriesQuery,
) -> sea_orm::Select<series::Entity> {
    let cast_facets: [(Option<&str>, &'static str); 3] = [
        (q.characters.as_deref(), "characters"),
        (q.teams.as_deref(), "teams"),
        (q.locations.as_deref(), "locations"),
    ];
    for (raw, column) in cast_facets {
        let Some(raw) = raw else { continue };
        let values = split_csv(raw);
        if values.is_empty() {
            continue;
        }
        let lowered: Vec<String> = values.iter().map(|s| s.to_lowercase()).collect();
        let sql = format!(
            "EXISTS (SELECT 1 FROM issues i \
               WHERE i.series_id = series.id \
                 AND i.removed_at IS NULL \
                 AND i.state = 'active' \
                 AND EXISTS ( \
                   SELECT 1 FROM unnest( \
                     regexp_split_to_array( \
                       coalesce(i.{column}, ''), \
                       CASE WHEN coalesce(i.{column}, '') LIKE '%;%' THEN ';' ELSE ',' END \
                     ) \
                   ) AS piece \
                   WHERE lower(trim(piece)) = ANY($1) \
                 ))",
        );
        select = select.filter(Expr::cust_with_values(&sql, [Value::from(lowered)]));
    }
    select
}

/// EXISTS-subquery filter on the calling user's per-series rating.
/// Caller must call `validate_list_series_query_params` first — this
/// helper trusts the bounds.
fn apply_series_user_rating_filter(
    mut select: sea_orm::Select<series::Entity>,
    q: &ListSeriesQuery,
    user_id: Uuid,
) -> sea_orm::Select<series::Entity> {
    if q.user_rating_min.is_some() || q.user_rating_max.is_some() {
        let min = q.user_rating_min.unwrap_or(0.0);
        let max = q.user_rating_max.unwrap_or(5.0);
        select = select.filter(Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM user_ratings ur \
             WHERE ur.user_id = $1 \
               AND ur.target_type = 'series' \
               AND ur.target_id = series.id::text \
               AND ur.rating BETWEEN $2 AND $3)",
            [Value::from(user_id), Value::from(min), Value::from(max)],
        ));
    }
    select
}

/// Decode the opaque cursor and dispatch to the per-sort
/// `apply_*_cursor` helper. Static `Err` keeps the `Result` variant
/// small (clippy::result_large_err); caller maps to 400 `validation`.
fn apply_series_cursor(
    select: sea_orm::Select<series::Entity>,
    cursor: &str,
    sort: SeriesSort,
    asc: bool,
) -> Result<sea_orm::Select<series::Entity>, &'static str> {
    let (c_value, c_id_str) = parse_cursor(cursor).map_err(|_| "invalid cursor")?;
    let c_id = Uuid::parse_str(&c_id_str).map_err(|_| "invalid cursor")?;
    Ok(match sort {
        SeriesSort::Name => apply_string_cursor(
            select,
            series::Column::NormalizedName,
            series::Column::Id,
            &c_value,
            c_id,
            asc,
        ),
        SeriesSort::CreatedAt => {
            let ts =
                chrono::DateTime::parse_from_rfc3339(&c_value).map_err(|_| "invalid cursor")?;
            apply_ts_cursor(
                select,
                series::Column::CreatedAt,
                series::Column::Id,
                ts,
                c_id,
                asc,
            )
        }
        SeriesSort::UpdatedAt => {
            let ts =
                chrono::DateTime::parse_from_rfc3339(&c_value).map_err(|_| "invalid cursor")?;
            apply_ts_cursor(
                select,
                series::Column::UpdatedAt,
                series::Column::Id,
                ts,
                c_id,
                asc,
            )
        }
        SeriesSort::Year => {
            // Empty `c_value` encodes a NULL year on the boundary row;
            // otherwise parse as i32 (validate explicitly so a garbled
            // cursor 400s instead of silently page-shifting).
            let parsed = if c_value.is_empty() {
                None
            } else {
                Some(c_value.parse::<i32>().map_err(|_| "invalid cursor")?)
            };
            apply_i32_cursor(
                select,
                series::Column::Year,
                series::Column::Id,
                parsed,
                c_id,
                asc,
            )
        }
    })
}

/// Final ORDER BY chain. Year is nullable; we emulate `NULLS LAST` on
/// both ASC and DESC so undated series consistently sort to the bottom
/// regardless of direction (Postgres defaults are NULLS LAST on ASC,
/// NULLS FIRST on DESC).
fn apply_series_sort_ordering(
    select: sea_orm::Select<series::Entity>,
    sort: SeriesSort,
    asc: bool,
) -> sea_orm::Select<series::Entity> {
    match sort {
        SeriesSort::Name => {
            if asc {
                select
                    .order_by_asc(series::Column::NormalizedName)
                    .order_by_asc(series::Column::Id)
            } else {
                select
                    .order_by_desc(series::Column::NormalizedName)
                    .order_by_desc(series::Column::Id)
            }
        }
        SeriesSort::CreatedAt => {
            if asc {
                select
                    .order_by_asc(series::Column::CreatedAt)
                    .order_by_asc(series::Column::Id)
            } else {
                select
                    .order_by_desc(series::Column::CreatedAt)
                    .order_by_desc(series::Column::Id)
            }
        }
        SeriesSort::UpdatedAt => {
            if asc {
                select
                    .order_by_asc(series::Column::UpdatedAt)
                    .order_by_asc(series::Column::Id)
            } else {
                select
                    .order_by_desc(series::Column::UpdatedAt)
                    .order_by_desc(series::Column::Id)
            }
        }
        SeriesSort::Year => {
            let nulls_last = Expr::cust("year IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(series::Column::Year)
                    .order_by_asc(series::Column::Id)
            } else {
                s.order_by_desc(series::Column::Year)
                    .order_by_desc(series::Column::Id)
            }
        }
    }
}

/// Compute the opaque cursor encoding for the boundary row when the
/// fetched window overflows the page limit. Empty string for Year
/// = NULL year on the boundary; the cursor decoder uses that as the
/// signal to switch to id-only filtering inside the NULL bucket.
fn compute_next_series_cursor(
    rows: &[series::Model],
    limit: u64,
    sort: SeriesSort,
) -> Option<String> {
    if rows.len() as u64 <= limit {
        return None;
    }
    let r = rows.get(limit as usize - 1)?;
    let value = match sort {
        SeriesSort::Name => r.normalized_name.clone(),
        SeriesSort::CreatedAt => r.created_at.to_rfc3339(),
        SeriesSort::UpdatedAt => r.updated_at.to_rfc3339(),
        SeriesSort::Year => r.year.map(|y| y.to_string()).unwrap_or_default(),
    };
    Some(encode_cursor(&value, &r.id.to_string()))
}

#[utoipa::path(
    operation_id = "series_list",    get,
    path = "/series",
    responses((status = 200, body = SeriesListView))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListSeriesQuery>,
) -> impl IntoResponse {
    if let Err(msg) = validate_list_series_query_params(&q) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", msg);
    }

    let visible_libs = visible_libraries(&app, &user).await;
    let empty = || {
        Json(SeriesListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response()
    };
    let Some(mut select) =
        apply_series_visibility(series::Entity::find(), &visible_libs, q.library)
    else {
        return empty();
    };
    select = apply_series_direct_column_filters(select, &q);
    select = apply_series_junction_facet_filters(select, &q);
    select = apply_series_credit_role_filters(select, &q);
    select = apply_series_cast_setting_filters(select, &q);
    select = apply_series_user_rating_filter(select, &q, user.id);

    let limit = clamp_limit(q.limit);
    let q_text = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());

    // Search mode: filter by tsvector match (or trigram similarity
    // fallback for fuzzy spellings). Pagination uses an opaque offset
    // cursor so relevance-ranked result order can stay intact without
    // exposing page numbers. The sort path is chosen by the caller:
    //
    // - `?sort` absent → relevance (ts_rank_cd), the modal default.
    // - `?sort=name` / `year` / `created_at` / `updated_at` → respect
    //   the explicit ordering. Used by the dedicated `/search` page
    //   when the user picks a sort option (M4 of the search-
    //   improvements plan).
    if let Some(text) = q_text {
        let offset = match q.cursor.as_deref() {
            Some(cursor) => match parse_offset_cursor(cursor) {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation",
                        "invalid cursor",
                    );
                }
            },
            None => 0,
        };
        let filtered = select.filter(
            Condition::any()
                .add(Expr::cust_with_values(
                    "search_doc @@ websearch_to_tsquery('simple', $1)",
                    [text],
                ))
                .add(Expr::cust_with_values(
                    "normalized_name % $1",
                    [entity::series::normalize_name(text)],
                )),
        );
        let total = if q.cursor.is_none() {
            match filtered.clone().count(&app.db).await {
                Ok(n) => Some(n as i64),
                Err(e) => {
                    tracing::error!(error = %e, "list series search count failed");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
            }
        } else {
            None
        };
        let ranked = if let Some(sort) = q.sort {
            // Explicit sort overrides relevance. Honour the order
            // param too (defaults match the non-search branch:
            // Name ASC, others DESC). `apply_series_sort_ordering`
            // handles the NULL-aware order semantics for `year`.
            let order = q.order.unwrap_or(match sort {
                SeriesSort::Name => SortOrder::Asc,
                SeriesSort::CreatedAt | SeriesSort::UpdatedAt | SeriesSort::Year => SortOrder::Desc,
            });
            let asc = matches!(order, SortOrder::Asc);
            apply_series_sort_ordering(filtered, sort, asc)
                .offset(offset)
                .limit(limit + 1)
        } else {
            filtered
                .order_by_desc(Expr::cust_with_values(
                    "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
                    [text],
                ))
                .order_by_asc(series::Column::NormalizedName)
                .order_by_asc(series::Column::Id)
                .offset(offset)
                .limit(limit + 1)
        };
        let rows = match ranked.all(&app.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "list series search failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let has_more = rows.len() as u64 > limit;
        let page_rows: Vec<series::Model> = rows.into_iter().take(limit as usize).collect();
        let next_cursor = has_more.then(|| encode_offset_cursor(offset + limit));
        // Second pass: fetch per-row `ts_headline` excerpts so result
        // rows render with a snippet showing *why* the row matched.
        // Cheap PK lookup on already-fetched IDs, scoped to whichever
        // free-text fields we want to highlight (currently summary —
        // name is in the row title, publisher/year already show as
        // metadata badges, so summary is the high-signal target).
        let snippets = match fetch_series_snippets(&app, &page_rows, text).await {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(error = %e, "series snippet fetch failed; continuing without");
                std::collections::HashMap::new()
            }
        };
        let mut items = hydrate_series(&app, page_rows).await;
        for item in items.iter_mut() {
            if let Ok(uuid) = Uuid::parse_str(&item.id)
                && let Some(s) = snippets.get(&uuid).cloned()
            {
                item.snippet = Some(s);
            }
        }
        return Json(SeriesListView {
            items,
            next_cursor,
            total,
        })
        .into_response();
    }

    // Sort + cursor mode. Defaults: name ASC, timestamps DESC
    // (recently-updated/added rails), year DESC.
    let sort = q.sort.unwrap_or_default();
    let order = q.order.unwrap_or(match sort {
        SeriesSort::Name => SortOrder::Asc,
        SeriesSort::CreatedAt | SeriesSort::UpdatedAt | SeriesSort::Year => SortOrder::Desc,
    });
    let asc = matches!(order, SortOrder::Asc);

    // Count once on the first page only (no cursor). Postgres's
    // COUNT(*) over the filtered set is fast enough at typical Folio
    // scale and the client uses this for the header subtitle.
    let total: Option<i64> = if q.cursor.is_none() {
        match select.clone().count(&app.db).await {
            Ok(n) => Some(n as i64),
            Err(e) => {
                tracing::error!(error = %e, "list series count failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    if let Some(cursor) = q.cursor.as_deref() {
        select = match apply_series_cursor(select, cursor, sort, asc) {
            Ok(s) => s,
            Err(msg) => return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", msg),
        };
    }
    select = apply_series_sort_ordering(select, sort, asc);

    let rows: Vec<series::Model> = match select.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list series failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let next_cursor = compute_next_series_cursor(&rows, limit, sort);
    let page: Vec<series::Model> = rows.into_iter().take(limit as usize).collect();

    Json(SeriesListView {
        items: hydrate_series(&app, page).await,
        next_cursor,
        total,
    })
    .into_response()
}

/// Attach `issue_count` + `cover_url` to a batch of series rows. Issue count
/// excludes soft-deleted and confirmed-removed issues so the UI doesn't see
/// stale rows.
pub(crate) async fn hydrate_series(app: &AppState, rows: Vec<series::Model>) -> Vec<SeriesView> {
    if rows.is_empty() {
        return Vec::new();
    }

    let series_ids: Vec<Uuid> = rows.iter().map(|s| s.id).collect();
    let counts = issue::Entity::find()
        .filter(issue::Column::SeriesId.is_in(series_ids.clone()))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::SeriesId)
        .column_as(Expr::col(issue::Column::Id).count(), "issue_count")
        .group_by(issue::Column::SeriesId)
        .into_model::<SeriesIssueCountRow>()
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.series_id, row.issue_count))
        .collect::<HashMap<_, _>>();

    let mut covers: HashMap<Uuid, String> = HashMap::new();
    // Prefer issue #1 as the series cover even when a preceding
    // entry (e.g. #1/2, #0, "Free Comic Book Day" specials with
    // sort_number < 1) exists. Those preludes stay in the issue
    // listing — only the cover anchor changes. CASE-WHEN pushes
    // `sort_number = 1` to the top of the per-series order; the
    // rest falls back to natural ascending so series without a
    // canonical #1 (start at #0 or skip numbering entirely) still
    // pick a sensible cover. NULLS-LAST on `sort_number` avoids an
    // unnumbered ALWAYS-LAST file dragging in front of real
    // numbered issues. M5/series-cover-issue-1 (2026-05-23 ask).
    let cover_rows = issue::Entity::find()
        .filter(issue::Column::SeriesId.is_in(series_ids))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::SeriesId)
        .column(issue::Column::Id)
        .order_by_asc(issue::Column::SeriesId)
        .order_by_asc(Expr::cust("CASE WHEN sort_number = 1 THEN 0 ELSE 1 END"))
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::FilePath)
        .into_model::<SeriesCoverRow>()
        .all(&app.db)
        .await
        .unwrap_or_default();
    for row in cover_rows {
        covers.entry(row.series_id).or_insert(row.id);
    }

    // Batch-fetch external IDs once for the whole page so the
    // per-row enrichment doesn't N+1.
    let series_ids_text: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    let id_rows = if series_ids_text.is_empty() {
        Vec::new()
    } else {
        entity::external_id::Entity::find()
            .filter(entity::external_id::Column::EntityType.eq("series"))
            .filter(entity::external_id::Column::EntityId.is_in(series_ids_text))
            .all(&app.db)
            .await
            .unwrap_or_default()
    };
    let mut by_series: HashMap<String, (Option<i64>, Option<i64>)> = HashMap::new();
    for row in id_rows {
        let entry = by_series.entry(row.entity_id.clone()).or_default();
        match row.source.as_str() {
            "comicvine" => entry.0 = row.external_id.parse::<i64>().ok(),
            "metron" => entry.1 = row.external_id.parse::<i64>().ok(),
            _ => {}
        }
    }

    // Batched metadata-completeness tier for the card badge — one aggregate
    // over the whole page, same FILTER predicate as the saved-view filter and
    // the series rollup so all three agree.
    let tiers = fetch_metadata_completeness_tiers(app, &rows).await;

    rows.into_iter()
        .map(|s| {
            let series_id = s.id;
            let mut v = SeriesView::from(s);
            v.issue_count = counts.get(&series_id).copied();
            v.cover_url = covers
                .get(&series_id)
                .map(|id| format!("/issues/{id}/pages/0/thumb"));
            if let Some((cv, metron)) = by_series.get(&series_id.to_string()) {
                v.comicvine_id = *cv;
                v.metron_id = *metron;
            }
            v.metadata_completeness_tier = tiers.get(&series_id).cloned();
            v
        })
        .collect()
}

/// Batched per-series metadata-completeness tier for card badges. One
/// aggregate over every series on the page; the `complete_count` FILTER
/// matches [`compute_metadata_completeness_summary`] and the saved-view
/// `metadata_completeness` predicate exactly. Series with zero active issues
/// are omitted (no badge). Returns `series_id → tier` where tier is
/// `"complete"` | `"partial"` | `"needs_metadata"`.
async fn fetch_metadata_completeness_tiers(
    app: &AppState,
    rows: &[series::Model],
) -> HashMap<Uuid, String> {
    use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
    if rows.is_empty() {
        return HashMap::new();
    }
    #[derive(FromQueryResult)]
    struct TierRow {
        series_id: Uuid,
        active_count: i64,
        complete_count: i64,
    }
    let mut params: Vec<Value> = Vec::with_capacity(rows.len());
    let placeholders: Vec<String> = rows
        .iter()
        .map(|r| {
            params.push(Value::from(r.id));
            format!("${}", params.len())
        })
        .collect();
    let sql = format!(
        r#"
        SELECT i.series_id AS series_id,
               COUNT(*)::bigint AS active_count,
               COUNT(*) FILTER (
                 -- `title` intentionally excluded: optional for comic issues.
                 WHERE i.year IS NOT NULL AND i.year >= 1800
                   AND COALESCE(btrim(i.summary), '') <> ''
                   AND i.page_count IS NOT NULL AND i.page_count > 0
                   AND (
                     COALESCE(i.writer, '') <> '' OR COALESCE(i.penciller, '') <> '' OR
                     COALESCE(i.inker, '') <> '' OR COALESCE(i.colorist, '') <> '' OR
                     COALESCE(i.letterer, '') <> '' OR COALESCE(i.cover_artist, '') <> '' OR
                     COALESCE(i.editor, '') <> '' OR COALESCE(i.translator, '') <> ''
                   )
                   AND EXISTS (
                     SELECT 1 FROM external_ids x
                     WHERE x.entity_type = 'issue' AND x.entity_id = i.id
                       AND x.source IN ('comicvine', 'metron')
                   )
               )::bigint AS complete_count
          FROM issues i
         WHERE i.state = 'active' AND i.removed_at IS NULL
           AND i.series_id IN ({})
         GROUP BY i.series_id
    "#,
        placeholders.join(",")
    );
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    TierRow::find_by_statement(stmt)
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            if r.active_count == 0 {
                return None;
            }
            let tier = if r.complete_count >= r.active_count {
                "complete"
            } else if r.complete_count == 0 {
                "needs_metadata"
            } else {
                "partial"
            };
            Some((r.series_id, tier.to_owned()))
        })
        .collect()
}

/// Populate `SeriesView.comicvine_id` + `.metron_id` from the
/// `external_ids` table for a single series. Used by call sites that
/// build a single `SeriesView` (PATCH response, single GET).
async fn enrich_series_view_legacy_ids(
    db: &DatabaseConnection,
    view: &mut SeriesView,
    series_id_text: &str,
) {
    if let Ok((cv, metron, _gtin)) =
        crate::metadata::writers::fetch_legacy_id_trio(db, "series", series_id_text).await
    {
        view.comicvine_id = cv;
        view.metron_id = metron;
    }
}

/// Companion of [`enrich_series_view_legacy_ids`] for issue
/// responses. Populates the legacy `gtin` / `comicvine_id` /
/// `metron_id` fields on [`IssueDetailView`] from `external_ids`.
pub(crate) async fn enrich_issue_detail_legacy_ids(
    db: &DatabaseConnection,
    view: &mut IssueDetailView,
    issue_id: &str,
) {
    if let Ok((cv, metron, gtin)) =
        crate::metadata::writers::fetch_legacy_id_trio(db, "issue", issue_id).await
    {
        view.comicvine_id = cv;
        view.metron_id = metron;
        view.gtin = gtin;
    }
}

/// Assess a hydrated [`SeriesView`]'s metadata completeness. Pure — reads only
/// fields already on the view, so call it *after* `enrich_series_view_legacy_ids`
/// (so the provider-match signal sees the CV/Metron ids) and after the facet
/// rollups populate `genres`. Zero extra queries.
pub(crate) fn assess_series_view(
    view: &SeriesView,
) -> crate::metadata::completeness::CompletenessReport {
    use crate::metadata::completeness::{
        SeriesCompletenessInput, assess_series, non_empty, plausible_year,
    };
    assess_series(&SeriesCompletenessInput {
        // CV + Metron are the shipped metadata providers; matching either is
        // "matched to a provider". Barcode-only external_ids (GTIN/ISBN/UPC)
        // are intentionally excluded — they aren't provider matches.
        has_external_id: view.comicvine_id.is_some() || view.metron_id.is_some(),
        has_summary: non_empty(view.summary.as_deref()),
        has_publisher: non_empty(view.publisher.as_deref()),
        // `status` is non-null and defaults to "continuing" on creation, so
        // it's effectively always present — kept in the criteria set for
        // documentation symmetry, contributes no false "needs metadata".
        has_status: non_empty(Some(view.status.as_str())),
        has_total_issues: view.total_issues.is_some(),
        has_year_began: plausible_year(view.year),
        has_genres: !view.genres.is_empty(),
    })
}

/// Assess a hydrated [`IssueDetailView`]'s metadata completeness. Pure — call
/// *after* `enrich_issue_detail_legacy_ids`. Credits come from the per-role
/// CSV columns already on the view (a non-empty role implies ≥1 credit), so
/// there's no extra junction query.
pub(crate) fn assess_issue_view(
    view: &IssueDetailView,
) -> crate::metadata::completeness::CompletenessReport {
    use crate::metadata::completeness::{
        IssueCompletenessInput, assess_issue, non_empty, plausible_year,
    };
    let has_credits = [
        &view.writer,
        &view.penciller,
        &view.inker,
        &view.colorist,
        &view.letterer,
        &view.cover_artist,
        &view.editor,
        &view.translator,
    ]
    .iter()
    .any(|f| non_empty(f.as_deref()));
    assess_issue(&IssueCompletenessInput {
        has_external_id: view.comicvine_id.is_some() || view.metron_id.is_some(),
        has_title: non_empty(view.title.as_deref()),
        has_cover_date: plausible_year(view.year),
        has_summary: non_empty(view.summary.as_deref()),
        has_page_count: view.page_count.is_some_and(|p| p > 0),
        has_credits,
        // An on-disk active issue always has a renderable cover (page 0). No
        // separate extract-failed flag exists today.
        has_cover: view.state == "active",
        has_characters: non_empty(view.characters.as_deref()),
        has_story_arcs: non_empty(view.story_arc.as_deref()),
        has_genres: non_empty(view.genre.as_deref()),
    })
}

#[derive(Debug, FromQueryResult)]
struct SeriesIssueCountRow {
    series_id: Uuid,
    issue_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct SeriesCoverRow {
    series_id: Uuid,
    id: String,
}

pub(crate) fn apply_string_cursor<E, C, IdC, V>(
    select: sea_orm::Select<E>,
    sort_col: C,
    id_col: IdC,
    c_value: &str,
    c_id: V,
    asc: bool,
) -> sea_orm::Select<E>
where
    E: EntityTrait,
    C: ColumnTrait,
    IdC: ColumnTrait,
    V: Clone + Into<sea_orm::Value>,
{
    if asc {
        select.filter(
            Condition::any().add(sort_col.gt(c_value)).add(
                Condition::all()
                    .add(sort_col.eq(c_value))
                    .add(id_col.gt(c_id)),
            ),
        )
    } else {
        select.filter(
            Condition::any().add(sort_col.lt(c_value)).add(
                Condition::all()
                    .add(sort_col.eq(c_value))
                    .add(id_col.lt(c_id)),
            ),
        )
    }
}

pub(crate) fn apply_ts_cursor<E, C, IdC, V>(
    select: sea_orm::Select<E>,
    sort_col: C,
    id_col: IdC,
    c_value: chrono::DateTime<chrono::FixedOffset>,
    c_id: V,
    asc: bool,
) -> sea_orm::Select<E>
where
    E: EntityTrait,
    C: ColumnTrait,
    IdC: ColumnTrait,
    V: Clone + Into<sea_orm::Value>,
{
    if asc {
        select.filter(
            Condition::any().add(sort_col.gt(c_value)).add(
                Condition::all()
                    .add(sort_col.eq(c_value))
                    .add(id_col.gt(c_id)),
            ),
        )
    } else {
        select.filter(
            Condition::any().add(sort_col.lt(c_value)).add(
                Condition::all()
                    .add(sort_col.eq(c_value))
                    .add(id_col.lt(c_id)),
            ),
        )
    }
}

/// Apply a `(numeric, id)` cursor where the numeric column is `f64` (used
/// for `issue.sort_number`). The boundary `c_value` may be empty when the
/// boundary row had a NULL sort value; in that case we filter on id only.
pub(crate) fn apply_f64_cursor<E, C, IdC, V>(
    select: sea_orm::Select<E>,
    sort_col: C,
    id_col: IdC,
    c_value: Option<f64>,
    c_id: V,
    asc: bool,
) -> sea_orm::Select<E>
where
    E: EntityTrait,
    C: ColumnTrait,
    IdC: ColumnTrait,
    V: Clone + Into<sea_orm::Value>,
{
    match c_value {
        Some(v) => {
            if asc {
                select.filter(
                    Condition::any()
                        .add(sort_col.gt(v))
                        .add(Condition::all().add(sort_col.eq(v)).add(id_col.gt(c_id))),
                )
            } else {
                select.filter(
                    Condition::any()
                        .add(sort_col.lt(v))
                        .add(Condition::all().add(sort_col.eq(v)).add(id_col.lt(c_id))),
                )
            }
        }
        // NULL boundary: keep within the NULLs bucket, paginate on id.
        None => {
            let s = select.filter(sort_col.is_null());
            if asc {
                s.filter(id_col.gt(c_id))
            } else {
                s.filter(id_col.lt(c_id))
            }
        }
    }
}

/// Apply a `(integer, id)` cursor where the integer column is nullable
/// (used for sort by `series.year` or `issue.year`). NULL boundary
/// keeps the page within the NULLs bucket, ordered by id.
pub(crate) fn apply_i32_cursor<E, C, IdC, V>(
    select: sea_orm::Select<E>,
    sort_col: C,
    id_col: IdC,
    c_value: Option<i32>,
    c_id: V,
    asc: bool,
) -> sea_orm::Select<E>
where
    E: EntityTrait,
    C: ColumnTrait,
    IdC: ColumnTrait,
    V: Clone + Into<sea_orm::Value>,
{
    match c_value {
        Some(v) => {
            if asc {
                select.filter(
                    Condition::any()
                        .add(sort_col.gt(v))
                        .add(Condition::all().add(sort_col.eq(v)).add(id_col.gt(c_id))),
                )
            } else {
                select.filter(
                    Condition::any()
                        .add(sort_col.lt(v))
                        .add(Condition::all().add(sort_col.eq(v)).add(id_col.lt(c_id))),
                )
            }
        }
        None => {
            let s = select.filter(sort_col.is_null());
            if asc {
                s.filter(id_col.gt(c_id))
            } else {
                s.filter(id_col.lt(c_id))
            }
        }
    }
}

/// Collection-completeness report for a series: how many issues are owned
/// versus expected, and **which** main-run issue numbers are missing.
///
/// The denominator (`total_expected`) comes from `series.total_issues`, which
/// the scanner resolves from a `series.json` sidecar or the max ComicInfo
/// `<Count>`. series.json carries only the *count*, never a per-issue
/// manifest — so interior `missing` numbers are *inferred* by interpolating
/// the integer run between the lowest and highest owned issue. `expected_source`
/// records this provenance; a future provider-backed exact manifest will flip
/// it to `"provider_manifest"` and make `missing` exact.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CollectionReportView {
    /// Active (non-removed) issues owned. Matches the saved-view
    /// `collection_completeness` `active_count`, so this report and the
    /// SeriesCard collection dot never disagree.
    pub total_owned: i64,
    /// Publisher-claimed total from `series.total_issues`. `None` when the
    /// scanner couldn't resolve a count.
    pub total_expected: Option<i32>,
    /// `total_owned / total_expected * 100`, `None` when `total_expected` is
    /// `None`. Capped at 100 (over-collection still reads as complete).
    pub completeness_pct: Option<f64>,
    /// `"complete"` | `"incomplete"` | `"unknown"` — same vocabulary and CASE
    /// semantics as the `collection_completeness` saved-view predicate.
    pub completeness_state: String,
    pub main_run: MainRunReport,
    /// Annuals, one-shots, TPBs, point issues (`#2.5`), and unnumbered files —
    /// listed but excluded from the integer gap math.
    pub specials: Vec<SpecialEntry>,
    /// `"series_total"` today (count-only). `"provider_manifest"` once an
    /// exact provider issue list backs the report.
    pub expected_source: String,
    /// Every owned active issue with its metadata-completeness status, ordered
    /// by `sort_number`. Lets the UI color each issue chip by status
    /// (complete / partial / needs-metadata) and reveal the missing fields on
    /// click — no separate per-issue request. Uses the same scorer as the
    /// issue detail page.
    pub issues: Vec<CollectionIssueEntry>,
}

/// One owned issue's identity + metadata-completeness status, for the
/// collection grid. `metadata_tier` is `"complete"` | `"partial"` |
/// `"needs_metadata"`; `missing_core` lists the absent core fields (empty when
/// complete).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CollectionIssueEntry {
    pub slug: String,
    pub number_raw: Option<String>,
    pub title: Option<String>,
    pub sort_number: Option<f64>,
    /// Spec §6.5 tag (`"Annual"`, `"OneShot"`, …) when classified; `None` for
    /// ordinary main-run issues.
    pub special_type: Option<String>,
    pub metadata_tier: String,
    /// Missing core field keys ([`crate::metadata::completeness`] keys).
    pub missing_core: Vec<String>,
}

/// The integer-numbered backbone of a series and its inferred gaps.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MainRunReport {
    /// Owned main-run `sort_number`s, ascending (e.g. `[0.0, 1.0, 2.0, 4.0]`).
    pub present: Vec<f64>,
    /// `number_raw` labels aligned 1:1 with [`Self::present`] for display.
    pub present_labels: Vec<String>,
    /// Integers in `min..=max` not owned (e.g. `[3]`). **Inferred** — see
    /// [`CollectionReportView::expected_source`].
    pub missing: Vec<i64>,
    /// Lowest / highest owned main-run number (as f64), `None` when the run is
    /// empty.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Count expected beyond `max` when `total_expected > max` (e.g. own up to
    /// #4 with `total_expected = 6` → `2`).
    pub trailing_missing: i64,
}

/// A non-main-run issue: special_type-tagged, fractional, or unnumbered.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SpecialEntry {
    pub number_raw: Option<String>,
    pub sort_number: Option<f64>,
    /// Spec §6.5 tag (`"Annual"`, `"OneShot"`, `"TPB"`, `"Special"`) when the
    /// scanner classified it; `None` for plain point/unnumbered issues.
    pub special_type: Option<String>,
}

/// Minimal per-issue projection for the collection report — only the three
/// columns the gap algorithm needs, so the query stays cheap even for a
/// 1000-issue run and never truncates (no `.limit()`).
#[derive(Debug, FromQueryResult)]
struct CollectionNumberRow {
    sort_number: Option<f64>,
    number_raw: Option<String>,
    special_type: Option<String>,
}

/// How a single issue participates in the collection report.
enum NumberClass {
    /// Integral `sort_number`, not special-tagged — part of the gap-detected
    /// backbone. Carries the rounded integer.
    MainRun(i64),
    /// Special-tagged, fractional (`#2.5`), or unnumbered — listed under
    /// `specials`, excluded from gap math.
    Special,
}

/// Classify an issue for the collection report. A stored `special_type`
/// (scanner-assigned at scan time) takes precedence over the numeric shape,
/// so an annual parsed to `sort_number = 1.0` never pollutes the main run.
fn classify_issue_number(sort_number: Option<f64>, special_type: Option<&str>) -> NumberClass {
    if special_type.is_some() {
        return NumberClass::Special;
    }
    match sort_number {
        Some(n) if n >= 0.0 && (n - n.round()).abs() < 1e-9 => {
            NumberClass::MainRun(n.round() as i64)
        }
        _ => NumberClass::Special,
    }
}

/// Derive `completeness_state` with the exact CASE semantics of the
/// `collection_completeness` saved-view predicate in `views/compile.rs`.
fn collection_state(total_owned: i64, total_expected: Option<i32>) -> &'static str {
    match total_expected {
        None => "unknown",
        Some(t) if total_owned >= t as i64 => "complete",
        Some(_) => "incomplete",
    }
}

/// Pure builder — kept free of DB access so the gap algorithm is unit-testable.
fn build_collection_report(
    rows: Vec<CollectionNumberRow>,
    total_expected: Option<i32>,
) -> CollectionReportView {
    let total_owned = rows.len() as i64;
    let mut present: Vec<f64> = Vec::new();
    let mut present_labels: Vec<String> = Vec::new();
    let mut present_ints: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    let mut specials: Vec<SpecialEntry> = Vec::new();

    for r in rows {
        match classify_issue_number(r.sort_number, r.special_type.as_deref()) {
            NumberClass::MainRun(n) => {
                present.push(r.sort_number.unwrap_or(n as f64));
                present_labels.push(r.number_raw.unwrap_or_else(|| n.to_string()));
                present_ints.insert(n);
            }
            NumberClass::Special => specials.push(SpecialEntry {
                number_raw: r.number_raw,
                sort_number: r.sort_number,
                special_type: r.special_type,
            }),
        }
    }

    let min = present_ints.iter().next().copied();
    let max = present_ints.iter().next_back().copied();
    let mut missing: Vec<i64> = Vec::new();
    if let (Some(lo), Some(hi)) = (min, max) {
        missing.extend((lo..=hi).filter(|k| !present_ints.contains(k)));
    }
    let max_owned = max.unwrap_or(0);
    let trailing_missing = match total_expected {
        Some(t) if (t as i64) > max_owned => (t as i64) - max_owned,
        _ => 0,
    };

    let completeness_pct = total_expected.map(|t| {
        if t <= 0 {
            0.0
        } else {
            ((total_owned as f64 / t as f64) * 100.0).min(100.0)
        }
    });

    CollectionReportView {
        total_owned,
        total_expected,
        completeness_pct,
        completeness_state: collection_state(total_owned, total_expected).to_owned(),
        main_run: MainRunReport {
            present,
            present_labels,
            missing,
            min: min.map(|n| n as f64),
            max: max.map(|n| n as f64),
            trailing_missing,
        },
        specials,
        expected_source: "series_total".to_owned(),
        // Populated by the handler (needs DB access for per-issue scoring).
        issues: Vec::new(),
    }
}

/// Per-issue projection for the collection report's metadata-completeness
/// scan. Carries exactly the columns [`assess_issue_view`]'s logic needs.
#[derive(Debug, FromQueryResult)]
struct IssueCompletenessRow {
    id: String,
    slug: String,
    number_raw: Option<String>,
    title: Option<String>,
    sort_number: Option<f64>,
    special_type: Option<String>,
    year: Option<i32>,
    summary: Option<String>,
    page_count: Option<i32>,
    writer: Option<String>,
    penciller: Option<String>,
    inker: Option<String>,
    colorist: Option<String>,
    letterer: Option<String>,
    cover_artist: Option<String>,
    editor: Option<String>,
    translator: Option<String>,
    state: String,
}

/// Score every active issue in a series for metadata completeness, ordered by
/// `sort_number`. Two queries total (issue rows + a batched provider-id
/// lookup), then a pure per-issue scoring loop — no N+1.
async fn collect_issue_completeness(app: &AppState, series_id: Uuid) -> Vec<CollectionIssueEntry> {
    use crate::metadata::completeness::{
        CompletenessTier, IssueCompletenessInput, assess_issue, non_empty, plausible_year,
    };
    let rows = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::Slug)
        .column(issue::Column::NumberRaw)
        .column(issue::Column::Title)
        .column(issue::Column::SortNumber)
        .column(issue::Column::SpecialType)
        .column(issue::Column::Year)
        .column(issue::Column::Summary)
        .column(issue::Column::PageCount)
        .column(issue::Column::Writer)
        .column(issue::Column::Penciller)
        .column(issue::Column::Inker)
        .column(issue::Column::Colorist)
        .column(issue::Column::Letterer)
        .column(issue::Column::CoverArtist)
        .column(issue::Column::Editor)
        .column(issue::Column::Translator)
        .column(issue::Column::State)
        .order_by_asc(issue::Column::SortNumber)
        .into_model::<IssueCompletenessRow>()
        .all(&app.db)
        .await
        .unwrap_or_default();
    if rows.is_empty() {
        return Vec::new();
    }

    // Batched provider-match lookup: which of these issues have a CV/Metron
    // external_ids row. One query, not one-per-issue.
    let issue_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let matched: std::collections::HashSet<String> = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("issue"))
        .filter(entity::external_id::Column::EntityId.is_in(issue_ids))
        .filter(entity::external_id::Column::Source.is_in(["comicvine", "metron"]))
        .select_only()
        .column(entity::external_id::Column::EntityId)
        .into_tuple::<String>()
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    rows.into_iter()
        .map(|r| {
            let has_credits = [
                &r.writer,
                &r.penciller,
                &r.inker,
                &r.colorist,
                &r.letterer,
                &r.cover_artist,
                &r.editor,
                &r.translator,
            ]
            .iter()
            .any(|f| non_empty(f.as_deref()));
            let report = assess_issue(&IssueCompletenessInput {
                has_external_id: matched.contains(&r.id),
                has_title: non_empty(r.title.as_deref()),
                has_cover_date: plausible_year(r.year),
                has_summary: non_empty(r.summary.as_deref()),
                has_page_count: r.page_count.is_some_and(|p| p > 0),
                has_credits,
                has_cover: r.state == "active",
                ..Default::default()
            });
            let tier = match report.tier {
                CompletenessTier::Complete => "complete",
                CompletenessTier::NeedsMetadata => "needs_metadata",
                CompletenessTier::Partial => "partial",
            };
            CollectionIssueEntry {
                slug: r.slug,
                number_raw: r.number_raw,
                title: r.title,
                sort_number: r.sort_number,
                special_type: r.special_type,
                metadata_tier: tier.to_owned(),
                missing_core: report.missing_core,
            }
        })
        .collect()
}

#[utoipa::path(
    operation_id = "series_collection_report",    get,
    path = "/series/{slug}/collection",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = CollectionReportView),
        (status = 404)
    )
)]
#[handler]
pub async fn collection_report(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }
    // No `.limit()` — the whole run must be visible for gap detection. The
    // three-column projection keeps this cheap even at 1000+ issues. Filter
    // matches `active_issue_count_subquery` so `total_owned` lines up with the
    // saved-view `collection_completeness` count.
    let rows = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::SortNumber)
        .column(issue::Column::NumberRaw)
        .column(issue::Column::SpecialType)
        .order_by_asc(issue::Column::SortNumber)
        .into_model::<CollectionNumberRow>()
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(series_id = %row.id, error = %e, "collection report query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut report = build_collection_report(rows, row.total_issues);
    report.issues = collect_issue_completeness(&app, row.id).await;
    Json(report).into_response()
}

#[utoipa::path(
    operation_id = "series_get_one",    get,
    path = "/series/{slug}",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = SeriesView),
        (status = 404)
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }
    // `RemovedAt.is_null()` keeps soft-deleted and confirmed-removed issues
    // out of the count, cover and writer aggregation — they belong on the
    // library's Removed tab, not on the series detail page.
    let count = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.id))
        .filter(issue::Column::RemovedAt.is_null())
        .count(&app.db)
        .await
        .ok();
    // Match `hydrate_series`'s cover-pick rule: prefer issue #1,
    // fall back to natural sort_number ascending. Keeps the
    // detail-page hero cover consistent with what the grid card on
    // /library and home rails shows. See `hydrate_series` for the
    // full rationale.
    let cover_issue = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("CASE WHEN sort_number = 1 THEN 0 ELSE 1 END"))
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::FilePath)
        .one(&app.db)
        .await
        .ok()
        .flatten();
    // Pull the per-issue stats columns + the still-CSV-shaped fields
    // (characters / teams / locations) we don't yet normalize. Capped at 500
    // to bound the work for absurdly long series. Genres / tags / credits
    // come from the junction tables below.
    let agg_rows = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::Characters)
        .column(issue::Column::Teams)
        .column(issue::Column::Locations)
        .column(issue::Column::PageCount)
        .column(issue::Column::Summary)
        .column(issue::Column::Year)
        .column(issue::Column::CreatedAt)
        .column(issue::Column::UpdatedAt)
        .limit(500)
        .into_model::<AggregateRow>()
        .all(&app.db)
        .await
        .unwrap_or_default();

    // Frequency-ranked top-12 per metadata facet, sourced from the
    // `issue_*` junction tables joined with this series's active issues.
    // One query per facet; counts are computed by Postgres so the API
    // doesn't have to split / dedupe CSVs in Rust.
    let metadata_facets = aggregate_series_metadata(&app, row.id).await;

    let mut total_pages: i64 = 0;
    let mut last_added: Option<chrono::DateTime<chrono::FixedOffset>> = None;
    let mut last_updated: Option<chrono::DateTime<chrono::FixedOffset>> = None;
    // Earliest / latest publication year — drives the "Released" stat's
    // range display ("2012–2018"). Only consider plausibly-valid years
    // (>= 1800) so a stray 0 from a malformed ComicInfo doesn't pull the
    // range to the year of the dinosaurs.
    let mut earliest_year: Option<i32> = None;
    let mut latest_year: Option<i32> = None;
    for r in &agg_rows {
        if let Some(p) = r.page_count {
            total_pages += i64::from(p.max(0));
        }
        last_added = Some(last_added.map_or(r.created_at, |x| x.max(r.created_at)));
        last_updated = Some(last_updated.map_or(r.updated_at, |x| x.max(r.updated_at)));
        if let Some(y) = r.year
            && y >= 1800
        {
            earliest_year = Some(earliest_year.map_or(y, |x| x.min(y)));
            latest_year = Some(latest_year.map_or(y, |x| x.max(y)));
        }
    }
    // The series-level `year` column is the editorial "first release" — fall
    // back to it if no issue has a parsed year, and prefer it over a
    // higher-than-expected aggregate floor (admin-set first year wins).
    if let Some(start) = row.year
        && start >= 1800
    {
        earliest_year = Some(earliest_year.map_or(start, |x| x.min(start)));
        latest_year = Some(latest_year.map_or(start, |x| x.max(start)));
    }

    // Series-level summary fallback: if the series row has no summary, the
    // first active issue's summary stands in. Editing the series later
    // promotes the value to the series row directly.
    let series_id_for_lookups = row.id;
    let mut v = SeriesView::from(row);
    if v.summary.as_deref().is_none_or(str::is_empty) {
        v.summary = agg_rows
            .iter()
            .find_map(|r| r.summary.clone().filter(|s| !s.trim().is_empty()));
    }
    v.issue_count = count.map(|c| c as i64);
    v.cover_url = cover_issue.map(|i| format!("/issues/{}/pages/0/thumb", i.id));
    v.writers = metadata_facets.credits_for("writer");
    v.pencillers = metadata_facets.credits_for("penciller");
    v.inkers = metadata_facets.credits_for("inker");
    v.colorists = metadata_facets.credits_for("colorist");
    v.letterers = metadata_facets.credits_for("letterer");
    v.cover_artists = metadata_facets.credits_for("cover_artist");
    v.creator_slugs = metadata_facets.creator_slugs;
    v.genres = metadata_facets.genres;
    v.tags = metadata_facets.tags;
    v.characters = aggregate_csv(agg_rows.iter().map(|r| r.characters.as_deref()));
    v.teams = aggregate_csv(agg_rows.iter().map(|r| r.teams.as_deref()));
    v.locations = aggregate_csv(agg_rows.iter().map(|r| r.locations.as_deref()));
    v.total_page_count = (!agg_rows.is_empty()).then_some(total_pages);
    v.last_issue_added_at = last_added.map(|t| t.to_rfc3339());
    v.last_issue_updated_at = last_updated.map(|t| t.to_rfc3339());
    v.earliest_year = earliest_year;
    v.latest_year = latest_year;
    enrich_series_view_legacy_ids(&app.db, &mut v, &series_id_for_lookups.to_string()).await;

    // Metadata-completeness: assessed from the fully-hydrated view (provider
    // ids + facet rollups already populated), so it costs no extra query.
    v.metadata_completeness = Some(assess_series_view(&v));
    // Per-issue rollup ("N of M issues have complete metadata") via one
    // aggregate query — drives the Collection tab's metadata-health line.
    v.metadata_completeness_summary =
        Some(compute_metadata_completeness_summary(&app, series_id_for_lookups).await);

    // Per-user read-progress summary — computed against the *full* series,
    // not the 100-issue page the client typically pulls. Two cheap counts
    // (finished / in-progress) plus the active-issue count we already
    // have, so the UI can render "n / N" without paginating.
    v.progress_summary = Some(
        compute_progress_summary(
            &app,
            series_id_for_lookups,
            user.id,
            count.unwrap_or(0) as i64,
        )
        .await,
    );

    // Calling user's series rating, if any. A miss returns None — the
    // widget shows an empty 5-star control.
    v.user_rating =
        lookup_user_rating(&app, user.id, "series", &series_id_for_lookups.to_string()).await;

    Json(v).into_response()
}

/// Fetch `ts_headline()` excerpts for a set of series rows that matched
/// a search query. Returns a `(series_id → snippet)` map; series whose
/// summary doesn't yield any highlightable fragment are simply omitted
/// (caller leaves `snippet` as `None`).
///
/// We highlight against `summary` only — the series name already shows
/// in the row title, and publisher / year render as separate metadata
/// chips, so summary is the high-signal target. Markup is constrained
/// to `<mark>…</mark>` so the frontend sanitiser can use a tight
/// allowlist.
pub(crate) async fn fetch_series_snippets(
    app: &AppState,
    rows: &[series::Model],
    q_text: &str,
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, Statement, Value};
    if rows.is_empty() {
        return Ok(HashMap::new());
    }

    #[derive(Debug, FromQueryResult)]
    struct SnippetRow {
        id: Uuid,
        snippet: Option<String>,
    }

    // $1 holds the query; the remaining placeholders ($2..) bind each
    // ID individually. This mirrors `filter_options.rs`'s pattern —
    // sea_orm's raw-statement layer doesn't expose Postgres array
    // params nicely, and result-set sizes here are bounded by
    // `clamp_limit` so the placeholder count stays small.
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
                    COALESCE(summary, ''),
                    websearch_to_tsquery('simple', $1),
                    'MaxFragments=1, MaxWords=18, MinWords=5, ShortWord=2, StartSel=<mark>, StopSel=</mark>, HighlightAll=false'
                  ) AS snippet
             FROM series
             WHERE id IN ({})"#,
        id_placeholders.join(",")
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    let rows: Vec<SnippetRow> = SnippetRow::find_by_statement(stmt).all(&app.db).await?;
    // Keep only snippets that actually contain a highlight; otherwise
    // we'd render the first 18 words of the summary unchanged, which
    // adds noise without conveying "why this matched".
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let s = r.snippet?;
            if s.contains("<mark>") {
                Some((r.id, s))
            } else {
                None
            }
        })
        .collect())
}

/// Count finished / in-progress active issues for `user_id` within
/// `series_id`. The two counts come from the same join so we read the
/// progress table once.
async fn compute_progress_summary(
    app: &AppState,
    series_id: Uuid,
    user_id: Uuid,
    total: i64,
) -> SeriesProgressSummary {
    use entity::progress_record;
    // Two-step lookup avoids needing a SeaORM `Related<Issue>` impl on
    // progress_record (which we don't have, and shouldn't add just for
    // this read path). Pulling the (id, page_count) pairs lets the
    // progress join compute `finished_pages` without a second SQL hop.
    #[derive(FromQueryResult)]
    struct IssueIdAndPages {
        id: String,
        page_count: Option<i32>,
    }
    let issue_rows: Vec<IssueIdAndPages> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::PageCount)
        .into_model::<IssueIdAndPages>()
        .all(&app.db)
        .await
        .unwrap_or_default();
    if issue_rows.is_empty() {
        return SeriesProgressSummary {
            total,
            finished: 0,
            in_progress: 0,
            finished_pages: 0,
        };
    }
    let pages_by_id: std::collections::HashMap<String, i64> = issue_rows
        .iter()
        .map(|r| (r.id.clone(), i64::from(r.page_count.unwrap_or(0).max(0))))
        .collect();
    let issue_ids: Vec<String> = issue_rows.into_iter().map(|r| r.id).collect();
    let progress_rows = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let mut finished: i64 = 0;
    let mut in_progress: i64 = 0;
    let mut finished_pages: i64 = 0;
    for r in progress_rows {
        if r.finished {
            finished += 1;
            finished_pages += pages_by_id.get(&r.issue_id).copied().unwrap_or(0);
        } else if r.last_page > 0 {
            in_progress += 1;
        }
    }
    SeriesProgressSummary {
        total,
        finished,
        in_progress,
        finished_pages,
    }
}

/// Per-issue metadata-completeness rollup for a series: count of active
/// issues meeting the issue **core** criteria (provider-matched + title +
/// cover date + summary + page count + ≥1 credit; cover is implicit for an
/// active issue) over the active-issue count.
///
/// One aggregate query — no per-issue scoring loop. The FILTER predicate
/// mirrors [`assess_issue_view`] exactly so the rollup and the per-issue
/// detail report never disagree: credits come from the per-role CSV columns,
/// the provider match from an `external_ids` row with a provider source
/// (CV/Metron — barcode sources excluded).
async fn compute_metadata_completeness_summary(
    app: &AppState,
    series_id: Uuid,
) -> MetadataCompletenessSummary {
    use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct Row {
        active_count: i64,
        complete_count: i64,
    }
    let sql = r#"
        SELECT
          COUNT(*)::bigint AS active_count,
          COUNT(*) FILTER (
            -- `title` intentionally excluded: optional for comic issues.
            WHERE i.year IS NOT NULL AND i.year >= 1800
              AND COALESCE(btrim(i.summary), '') <> ''
              AND i.page_count IS NOT NULL AND i.page_count > 0
              AND (
                COALESCE(i.writer, '') <> '' OR COALESCE(i.penciller, '') <> '' OR
                COALESCE(i.inker, '') <> '' OR COALESCE(i.colorist, '') <> '' OR
                COALESCE(i.letterer, '') <> '' OR COALESCE(i.cover_artist, '') <> '' OR
                COALESCE(i.editor, '') <> '' OR COALESCE(i.translator, '') <> ''
              )
              AND EXISTS (
                SELECT 1 FROM external_ids x
                WHERE x.entity_type = 'issue' AND x.entity_id = i.id
                  AND x.source IN ('comicvine', 'metron')
              )
          )::bigint AS complete_count
        FROM issues i
        WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
    "#;
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, [series_id.into()]);
    Row::find_by_statement(stmt)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|r| MetadataCompletenessSummary {
            complete: r.complete_count,
            total: r.active_count,
        })
        .unwrap_or(MetadataCompletenessSummary {
            complete: 0,
            total: 0,
        })
}

/// One-row lookup against `user_ratings`. Returns `None` when no row
/// exists or the query fails — the widget treats both as "not rated yet".
pub(crate) async fn lookup_user_rating(
    app: &AppState,
    user_id: Uuid,
    target_type: &str,
    target_id: &str,
) -> Option<f64> {
    use entity::user_rating;
    user_rating::Entity::find()
        .filter(user_rating::Column::UserId.eq(user_id))
        .filter(user_rating::Column::TargetType.eq(target_type))
        .filter(user_rating::Column::TargetId.eq(target_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|r| r.rating)
}

/// Row shape used by `get_one` to read the still-CSV-shaped issue fields
/// (characters / teams / locations) plus the per-issue stat columns. The
/// junction-backed facets (genre / tags / credits) come from
/// [`aggregate_series_metadata`] instead.
#[derive(Debug, FromQueryResult)]
struct AggregateRow {
    characters: Option<String>,
    teams: Option<String>,
    locations: Option<String>,
    page_count: Option<i32>,
    summary: Option<String>,
    year: Option<i32>,
    created_at: chrono::DateTime<chrono::FixedOffset>,
    updated_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Frequency-ranked top-12 per metadata facet, sourced from the
/// `issue_genres` / `issue_tags` / `issue_credits` junction tables joined
/// with this series's active, non-removed issues. Reproduces the existing
/// API contract: ordered by occurrence count desc, alpha asc, capped at 12.
struct SeriesMetadataFacets {
    genres: Vec<String>,
    tags: Vec<String>,
    /// Indexed by role — one entry per `CREDIT_ROLES`. Lookup via
    /// [`SeriesMetadataFacets::credits_for`].
    credits_by_role: std::collections::HashMap<String, Vec<String>>,
    /// Creator name → canonical slug. Built from the same JOIN that
    /// produces the per-role buckets; populated only for credits whose
    /// `series_credits.person_id` resolves on `person`. Names without a
    /// slug fall back to the legacy library-grid filter in the UI.
    creator_slugs: std::collections::HashMap<String, String>,
}

impl SeriesMetadataFacets {
    fn credits_for(&self, role: &str) -> Vec<String> {
        self.credits_by_role.get(role).cloned().unwrap_or_default()
    }
}

const FACET_RESULT_CAP: u64 = 12;

async fn aggregate_series_metadata(app: &AppState, series_id: Uuid) -> SeriesMetadataFacets {
    use sea_orm::{ConnectionTrait, Statement};
    let backend = app.db.get_database_backend();

    #[derive(Debug, FromQueryResult)]
    struct ValueRow {
        value: String,
    }
    #[derive(Debug, FromQueryResult)]
    struct CreditRow {
        role: String,
        person: String,
        // NULL when the credit's `person_id` hasn't been backfilled
        // yet (a freshly-scanned issue between rollups). The UI falls
        // back to a legacy library-grid filter in that case.
        slug: Option<String>,
        // Used by the SQL `ORDER BY ... cnt DESC`; not surfaced in Rust.
        #[allow(dead_code)]
        cnt: i64,
    }

    // Genres + tags share a shape: `(value, count)` ordered by count desc.
    let genres = ValueRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        r"SELECT ig.genre AS value
            FROM issue_genres ig
            JOIN issues i ON i.id = ig.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            GROUP BY ig.genre
            ORDER BY COUNT(*) DESC, ig.genre ASC
            LIMIT $2",
        [series_id.into(), (FACET_RESULT_CAP as i64).into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.value)
    .collect();

    let tags = ValueRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        r"SELECT it.tag AS value
            FROM issue_tags it
            JOIN issues i ON i.id = it.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            GROUP BY it.tag
            ORDER BY COUNT(*) DESC, it.tag ASC
            LIMIT $2",
        [series_id.into(), (FACET_RESULT_CAP as i64).into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.value)
    .collect();

    // One credits query, bucket by role in Rust. The 8*12=96-row cap below
    // matches the per-role display cap; over-fetch is tiny so a single
    // query is cheaper than eight role-specific ones.
    // LEFT JOIN on `person` via the FK so the slug rides along
    // alongside the display name. `person_id` is normally populated by
    // the series rollup right after each scan; the LEFT JOIN
    // gracefully degrades to a NULL slug for credits that haven't
    // been rolled up yet.
    let credit_rows: Vec<CreditRow> = CreditRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        r"SELECT ic.role, ic.person, p.slug AS slug, COUNT(*)::bigint AS cnt
            FROM issue_credits ic
            JOIN issues i ON i.id = ic.issue_id
            LEFT JOIN person p ON p.id = ic.person_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            GROUP BY ic.role, ic.person, p.slug
            ORDER BY ic.role ASC, cnt DESC, ic.person ASC",
        [series_id.into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default();

    let mut credits_by_role: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut creator_slugs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for row in credit_rows {
        if let Some(slug) = &row.slug
            && !creator_slugs.contains_key(&row.person)
        {
            creator_slugs.insert(row.person.clone(), slug.clone());
        }
        let bucket = credits_by_role.entry(row.role).or_default();
        if (bucket.len() as u64) < FACET_RESULT_CAP {
            bucket.push(row.person);
        }
    }

    SeriesMetadataFacets {
        genres,
        tags,
        credits_by_role,
        creator_slugs,
    }
}

/// Split each value on `,`/`;`, trim, dedupe (case-insensitive), then order by
/// occurrence frequency (most-credited first). Cap at 12 entries. Used for
/// every CSV-shaped ComicInfo aggregate (writers, genres, tags, etc.).
fn aggregate_csv<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, (String, usize)> = HashMap::new();
    for raw in values.into_iter().flatten() {
        // Same per-value rule as `metadata_rollup::split_csv`: prefer
        // `;` when present so names containing commas (e.g.
        // `"Capes, Inc."`) survive aggregation.
        let sep: char = if raw.contains(';') { ';' } else { ',' };
        for piece in raw.split(sep) {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = trimmed.to_lowercase();
            let entry = counts
                .entry(key)
                .or_insert_with(|| (trimmed.to_string(), 0));
            entry.1 += 1;
        }
    }
    let mut by_count: Vec<(String, usize)> = counts.into_values().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    by_count
        .into_iter()
        .take(12)
        .map(|(name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::aggregate_csv;
    use super::{CollectionNumberRow, build_collection_report};

    fn run(values: &[&str]) -> Vec<String> {
        aggregate_csv(values.iter().map(|s| Some(*s)))
    }

    /// Build a projection row: `(sort_number, number_raw, special_type)`.
    fn row(
        sort_number: Option<f64>,
        number_raw: Option<&str>,
        special_type: Option<&str>,
    ) -> CollectionNumberRow {
        CollectionNumberRow {
            sort_number,
            number_raw: number_raw.map(str::to_owned),
            special_type: special_type.map(str::to_owned),
        }
    }

    fn main_run(nums: &[i64]) -> Vec<CollectionNumberRow> {
        nums.iter()
            .map(|n| row(Some(*n as f64), Some(&n.to_string()), None))
            .collect()
    }

    #[test]
    fn detects_interior_gap_and_trailing_missing() {
        // Own #1, #2, #4 with a publisher total of 6 → #3 missing, +2 trailing.
        let mut rows = main_run(&[1, 2, 4]);
        rows.reverse(); // builder must not assume input ordering
        let r = build_collection_report(rows, Some(6));
        assert_eq!(r.total_owned, 3);
        assert_eq!(r.total_expected, Some(6));
        assert_eq!(r.main_run.missing, vec![3]);
        assert_eq!(r.main_run.trailing_missing, 2);
        assert_eq!(r.main_run.min, Some(1.0));
        assert_eq!(r.main_run.max, Some(4.0));
        assert_eq!(r.completeness_state, "incomplete");
        assert_eq!(r.completeness_pct, Some(50.0));
    }

    #[test]
    fn issue_zero_is_part_of_the_main_run() {
        // #0 is a legit prelude; it anchors the run and is gap-detected from.
        let r = build_collection_report(main_run(&[0, 1, 3]), None);
        assert_eq!(r.main_run.min, Some(0.0));
        assert_eq!(r.main_run.missing, vec![2]);
        assert!(r.specials.is_empty());
    }

    #[test]
    fn point_issues_are_specials_not_gaps() {
        // #2.5 must NOT make #3 look missing — it's a point issue, listed
        // under specials, excluded from the integer run.
        let mut rows = main_run(&[1, 2, 3]);
        rows.push(row(Some(2.5), Some("2.5"), None));
        let r = build_collection_report(rows, None);
        assert!(r.main_run.missing.is_empty());
        assert_eq!(r.specials.len(), 1);
        assert_eq!(r.specials[0].sort_number, Some(2.5));
    }

    #[test]
    fn special_type_beats_integral_sort_number() {
        // An annual parsed to sort_number = 1.0 must land in specials, not
        // collide with main-run #1.
        let mut rows = main_run(&[1, 2]);
        rows.push(row(Some(1.0), Some("Annual 1"), Some("Annual")));
        let r = build_collection_report(rows, None);
        assert_eq!(r.main_run.present.len(), 2);
        assert_eq!(r.specials.len(), 1);
        assert_eq!(r.specials[0].special_type.as_deref(), Some("Annual"));
    }

    #[test]
    fn unnumbered_issue_is_special() {
        let rows = vec![
            row(None, Some("Preview"), None),
            row(Some(1.0), Some("1"), None),
        ];
        let r = build_collection_report(rows, None);
        assert_eq!(r.main_run.present, vec![1.0]);
        assert_eq!(r.specials.len(), 1);
        assert!(r.main_run.missing.is_empty());
    }

    #[test]
    fn no_total_yields_unknown_state() {
        let r = build_collection_report(main_run(&[1, 2, 3]), None);
        assert_eq!(r.completeness_state, "unknown");
        assert_eq!(r.completeness_pct, None);
        assert_eq!(r.main_run.trailing_missing, 0);
    }

    #[test]
    fn over_collection_is_complete_and_capped() {
        // Own more than the publisher claim → complete, pct capped at 100.
        let r = build_collection_report(main_run(&[1, 2, 3, 4, 5]), Some(4));
        assert_eq!(r.completeness_state, "complete");
        assert_eq!(r.completeness_pct, Some(100.0));
        assert_eq!(r.main_run.trailing_missing, 0);
    }

    #[test]
    fn aggregates_and_ranks_values() {
        let out = run(&[
            "Brian K. Vaughan",
            "Brian K. Vaughan",
            "Fiona Staples; Brian K. Vaughan",
            "",
        ]);
        assert_eq!(
            out,
            vec!["Brian K. Vaughan".to_string(), "Fiona Staples".to_string()]
        );
    }

    #[test]
    fn splits_on_comma_and_semicolon() {
        // ComicInfo writes Genre/Tags as comma-separated; some scanners use ';'.
        let out = run(&["Action, Adventure", "Action; Sci-Fi", "Adventure"]);
        // "action" appears twice, "adventure" twice, "sci-fi" once.
        assert_eq!(out[0..2], ["Action".to_string(), "Adventure".to_string()]);
        assert!(out.contains(&"Sci-Fi".to_string()));
    }

    #[test]
    fn empty_input_yields_empty() {
        let out: Vec<String> = aggregate_csv(std::iter::empty::<Option<&str>>())
            .into_iter()
            .collect();
        assert!(out.is_empty());
    }

    #[test]
    fn skips_none_values() {
        let inputs: Vec<Option<&str>> = vec![None, Some("Spider-Man"), None];
        let out: Vec<String> = aggregate_csv(inputs).into_iter().collect();
        assert_eq!(out, vec!["Spider-Man".to_string()]);
    }

    #[test]
    fn case_insensitive_dedup_keeps_first_casing() {
        let out = run(&["chip zdarsky", "Chip Zdarsky"]);
        assert_eq!(out.len(), 1);
        // First-seen casing wins (the `or_insert_with` only fires once).
        assert_eq!(out[0], "chip zdarsky");
    }

    use super::{encode_cursor, parse_cursor};

    #[test]
    fn cursor_round_trips_string_and_uuid() {
        let id = uuid::Uuid::nil().to_string();
        let cursor = encode_cursor("Saga", &id);
        let (v, parsed) = parse_cursor(&cursor).unwrap();
        assert_eq!(v, "Saga");
        assert_eq!(parsed, id);
    }

    #[test]
    fn cursor_round_trips_with_pipe_in_value() {
        // `rsplit_once('|')` keeps everything before the last '|' as the value,
        // so embedded pipes survive the round trip. Important because RFC3339
        // timestamps don't contain '|' but normalized series names theoretically
        // could (we still want to be resilient).
        let cursor = encode_cursor("foo|bar", "issue-1");
        let (v, id) = parse_cursor(&cursor).unwrap();
        assert_eq!(v, "foo|bar");
        assert_eq!(id, "issue-1");
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(parse_cursor("not-base64!!!").is_err());
        // Valid base64 but no separator.
        assert!(parse_cursor("aGVsbG8").is_err());
    }
}

#[utoipa::path(
    operation_id = "series_list_issues",    get,
    path = "/series/{slug}/issues",
    params(("slug" = String, Path,)),
    responses((status = 200, body = IssueListView))
)]
#[handler]
pub async fn list_issues(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<ListIssuesQuery>,
) -> impl IntoResponse {
    if let Some(s) = q.q.as_ref()
        && s.len() > MAX_QUERY_LEN
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", "q too long");
    }
    let s = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, s.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }

    let limit = clamp_limit(q.limit);
    let q_text = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());

    // Hide soft-deleted and confirmed-removed issues — both have `removed_at`
    // set by `library::reconcile`. The series page wants currently-on-disk
    // issues only; removed ones live under the library's Removed tab.
    let mut select = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::RemovedAt.is_null());

    // Search mode: rank by ts_rank_cd and paginate with opaque offset
    // cursors so filtered issue searches can continue past the first page.
    if let Some(text) = q_text {
        let offset = match q.cursor.as_deref() {
            Some(cursor) => match parse_offset_cursor(cursor) {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation",
                        "invalid cursor",
                    );
                }
            },
            None => 0,
        };
        let select = select.filter(Expr::cust_with_values(
            "search_doc @@ websearch_to_tsquery('simple', $1)",
            [text],
        ));
        let total = if q.cursor.is_none() {
            match select.clone().count(&app.db).await {
                Ok(n) => Some(n as i64),
                Err(e) => {
                    tracing::error!(error = %e, "list issues search count failed");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
            }
        } else {
            None
        };
        let select = select
            .order_by_desc(Expr::cust_with_values(
                "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
                [text],
            ))
            .order_by_asc(issue::Column::Id)
            .offset(offset)
            .limit(limit + 1);
        let rows: Vec<issue::Model> = match select.all(&app.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "list issues search failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let has_more = rows.len() as u64 > limit;
        let next_cursor = has_more.then(|| encode_offset_cursor(offset + limit));
        let series_slug = s.slug.clone();
        let items: Vec<IssueSummaryView> = rows
            .into_iter()
            .take(limit as usize)
            .map(|m| IssueSummaryView::from_model(m, &series_slug))
            .collect();
        return Json(IssueListView {
            items,
            next_cursor,
            total,
        })
        .into_response();
    }

    let sort = q.sort.unwrap_or_default();
    // Per-series listing only supports the original three sorts. Year /
    // page count / user rating are cross-library discovery sorts (see
    // `api::issues::list`) — reject them here so a stale client doesn't
    // get silently mis-sorted results.
    match sort {
        IssueSort::Year | IssueSort::PageCount | IssueSort::UserRating => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "sort not supported on per-series listing",
            );
        }
        _ => {}
    }
    let order = q.order.unwrap_or(match sort {
        IssueSort::Number => SortOrder::Asc,
        IssueSort::CreatedAt | IssueSort::UpdatedAt => SortOrder::Desc,
        // Unreachable thanks to the validation above, but keeps the
        // match exhaustive without an `_` arm that would silently
        // accept a future variant.
        IssueSort::Year | IssueSort::PageCount | IssueSort::UserRating => SortOrder::Desc,
    });
    let asc = matches!(order, SortOrder::Asc);

    // Count once on the first page only — see `series::list` for the
    // shape rationale; the per-series UI uses this for the issue
    // total in the header.
    let total: Option<i64> = if q.cursor.is_none() {
        match select.clone().count(&app.db).await {
            Ok(n) => Some(n as i64),
            Err(e) => {
                tracing::error!(error = %e, "list_issues count failed");
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
            IssueSort::Year | IssueSort::PageCount | IssueSort::UserRating => {
                // Already 400'd above.
                unreachable!("rejected at top of handler")
            }
        };
    }

    select = match sort {
        IssueSort::Number => {
            // Default order: sort_number ASC NULLS LAST → emulate via custom
            // ORDER BY expression so cursor pagination stays stable for issues
            // that lack a parsed sort number.
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
        IssueSort::Year | IssueSort::PageCount | IssueSort::UserRating => {
            unreachable!("rejected at top of handler")
        }
    };

    let rows: Vec<issue::Model> = match select.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list issues failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get(limit as usize - 1).map(|r| {
            let value = match sort {
                IssueSort::Number => r.sort_number.map(|n| n.to_string()).unwrap_or_default(),
                IssueSort::CreatedAt => r.created_at.to_rfc3339(),
                IssueSort::UpdatedAt => r.updated_at.to_rfc3339(),
                IssueSort::Year | IssueSort::PageCount | IssueSort::UserRating => {
                    unreachable!("rejected at top of handler")
                }
            };
            encode_cursor(&value, &r.id)
        })
    } else {
        None
    };
    let page: Vec<issue::Model> = rows.into_iter().take(limit as usize).collect();

    let series_slug = s.slug.clone();
    Json(IssueListView {
        items: page
            .into_iter()
            .map(|m| IssueSummaryView::from_model(m, &series_slug))
            .collect(),
        next_cursor,
        total,
    })
    .into_response()
}

// ───────── ACL helpers ─────────

struct VisibleLibs {
    /// Admin users see all libraries — bypass any filtering.
    unrestricted: bool,
    /// Library IDs the user has explicit access to (only used when not admin).
    allowed: std::collections::HashSet<Uuid>,
}

async fn visible_libraries(app: &AppState, user: &CurrentUser) -> VisibleLibs {
    if user.role == "admin" {
        return VisibleLibs {
            unrestricted: true,
            allowed: Default::default(),
        };
    }
    let granted: Vec<library_user_access::Model> = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    VisibleLibs {
        unrestricted: false,
        allowed: granted.into_iter().map(|g| g.library_id).collect(),
    }
}

async fn visible_in_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    let v = visible_libraries(app, user).await;
    v.unrestricted || v.allowed.contains(&lib_id)
}

/// Response for `GET /series/{slug}/resume` — the issue (and page) the user
/// should land on when they hit "Read" without picking a specific issue.
/// Mirrors the client-side `pickNextIssue` algorithm in
/// [`web/lib/reading-state.ts`]: prefer the most-recently-updated in-progress
/// issue; else the first unfinished issue in sort order; else (everything
/// read) restart from issue #1. `state` lets clients label the action
/// (Continue reading / Read / Read again).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesResumeView {
    pub series_slug: String,
    /// `null` when the series has no readable issues (every issue is
    /// soft-deleted / encrypted / state != 'active').
    pub issue_slug: Option<String>,
    pub issue_id: Option<String>,
    /// 0-based page index to resume from. `0` for unread / re-read paths.
    pub page: i32,
    /// One of `'unread' | 'in_progress' | 'finished'`. `'finished'` means
    /// the user already read every issue — `issue_slug` points at #1 so a
    /// "Read again" CTA is honored.
    pub state: String,
}

#[utoipa::path(
    operation_id = "series_resume",    get,
    path = "/series/{slug}/resume",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = SeriesResumeView),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn resume(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    use entity::progress_record;
    let srow = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, srow.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }
    // Active, non-removed issues in canonical sort order. Empty series →
    // 200 with null issue (clients should disable the play CTA).
    let issues: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(srow.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "series resume: issues lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if issues.is_empty() {
        return Json(SeriesResumeView {
            series_slug: srow.slug,
            issue_slug: None,
            issue_id: None,
            page: 0,
            state: "unread".into(),
        })
        .into_response();
    }
    let issue_ids: Vec<String> = issues.iter().map(|i| i.id.clone()).collect();
    let progress: Vec<progress_record::Model> = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user.id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let progress_by_id: HashMap<String, progress_record::Model> = progress
        .into_iter()
        .map(|p| (p.issue_id.clone(), p))
        .collect();

    // 1. Most-recently-updated in-progress issue → "Continue reading".
    let mut best_in_progress: Option<(&issue::Model, &progress_record::Model)> = None;
    for iss in &issues {
        let Some(p) = progress_by_id.get(&iss.id) else {
            continue;
        };
        if p.finished || p.last_page <= 0 {
            continue;
        }
        match best_in_progress {
            None => best_in_progress = Some((iss, p)),
            Some((_, best)) if p.updated_at > best.updated_at => {
                best_in_progress = Some((iss, p));
            }
            _ => {}
        }
    }
    if let Some((iss, p)) = best_in_progress {
        return Json(SeriesResumeView {
            series_slug: srow.slug,
            issue_slug: Some(iss.slug.clone()),
            issue_id: Some(iss.id.clone()),
            page: p.last_page,
            state: "in_progress".into(),
        })
        .into_response();
    }

    let is_main = |i: &&issue::Model| i.sort_number.map(|n| n >= 1.0).unwrap_or(true);
    let is_unread = |i: &&issue::Model| {
        progress_by_id
            .get(&i.id)
            .map(|p| !p.finished)
            .unwrap_or(true)
    };

    // 2. First unfinished main-run issue → "Read". Preludes (#0,
    // #1/2, FCBD specials) stay visible in the issue list, but the
    // primary series CTA should anchor on #1 when a main issue exists.
    if let Some(iss) = issues.iter().find(|i| is_main(i) && is_unread(i)) {
        return Json(SeriesResumeView {
            series_slug: srow.slug,
            issue_slug: Some(iss.slug.clone()),
            issue_id: Some(iss.id.clone()),
            page: 0,
            state: "unread".into(),
        })
        .into_response();
    }

    // 2b. Every main issue is finished. Fall back to unread preludes so
    // the user can mop up specials before the series is truly done.
    if let Some(iss) = issues.iter().find(|i| is_unread(i)) {
        return Json(SeriesResumeView {
            series_slug: srow.slug,
            issue_slug: Some(iss.slug.clone()),
            issue_id: Some(iss.id.clone()),
            page: 0,
            state: "unread".into(),
        })
        .into_response();
    }

    // 3. Every active issue is finished — restart from #1 ("Read again").
    let first = issues
        .iter()
        .find(|i| is_main(i))
        .unwrap_or_else(|| &issues[0]);
    Json(SeriesResumeView {
        series_slug: srow.slug,
        issue_slug: Some(first.slug.clone()),
        issue_id: Some(first.id.clone()),
        page: 0,
        state: "finished".into(),
    })
    .into_response()
}
