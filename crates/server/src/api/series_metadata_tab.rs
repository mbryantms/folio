//! The series page's **Metadata** tab: everything administrative about a
//! series' metadata in one place, so the reading surfaces stay clean.
//!
//! - `GET /series/{slug}/metadata/overview` — the summary strip and the
//!   cards: the series' provider ids, issue-id counts per provider, the
//!   completeness tiers (the Collection grid's scorer), provider sync and
//!   sidecar state, what the folder holds (archives, sidecars, series.json
//!   / cvinfo, backups), the latest run, and series field provenance.
//! - `GET /series/{slug}/metadata/issues` — one row per active issue for
//!   the audit table: tier, missing core fields, the three provider ids,
//!   the tagging tool that wrote the file's sidecar, and the latest run.
//!   Filters are server-side (`tier`, `without`, `kind`); the list is the
//!   whole series, like the Collection report — a series is domain-bounded.
//! - `POST /series/{slug}/metadata/issues/accept` — the operator's
//!   "metadata complete enough" acknowledgement over a selection (the
//!   per-issue endpoint, batched).
//!
//! Everything here reads; the fetch actions on the tab are the existing
//! batch / refresh / match endpoints.

use std::collections::{BTreeMap, HashMap};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{external_id, field_provenance, issue, metadata_run, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use super::external_ids::{ExternalIdRow, fetch_rows};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::metadata::completeness::CompletenessTier;
use crate::metadata::tag_tool;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(overview))
        .routes(routes!(issues))
        .routes(routes!(accept_issues))
}

const SOURCES: [&str; 3] = ["comicvine", "metron", "gcd"];

// ───────── overview ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesMetadataOverview {
    pub series_id: String,
    /// The series' provider ids (`external_ids`, series level).
    pub series_ids: Vec<ExternalIdRow>,
    pub issue_ids: IssueIdCounts,
    pub tiers: SeriesTierCounts,
    pub sync: SeriesSyncState,
    pub files: SeriesFilesState,
    /// The latest metadata run on the series or any of its issues.
    pub last_run: Option<SeriesLastRun>,
    pub runs_total: i64,
    /// `field_provenance` rows on the series row.
    pub series_provenance: Vec<ProvenanceRow>,
    /// Distinct issues with at least one provenance row of each strength.
    pub issue_provenance: IssueProvenanceCounts,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct IssueIdCounts {
    pub total: i64,
    pub comicvine: i64,
    pub metron: i64,
    pub gcd: i64,
    pub all_sources: i64,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct SeriesTierCounts {
    pub complete: i64,
    pub partial: i64,
    pub needs_metadata: i64,
    pub accepted: i64,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct SeriesSyncState {
    /// `series.last_metadata_sync_at` (RFC 3339).
    pub series_last_sync_at: Option<String>,
    pub metadata_sync_paused: bool,
    /// Issues ever synced with a provider through Folio.
    pub issues_synced: i64,
    /// Issues whose sidecars Folio has rewritten, and the latest rewrite.
    pub sidecars_rewritten: i64,
    pub last_rewrite_at: Option<String>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct SeriesFilesState {
    pub archives: i64,
    /// Archive count per extension (`cbz`, `cbr`, …), largest first.
    pub by_extension: Vec<ExtensionCount>,
    pub with_comicinfo: i64,
    pub with_metroninfo: i64,
    pub series_json_present: bool,
    /// A Mylar `cvinfo` file sits in the folder.
    pub cvinfo_present: bool,
    /// `.bak` backups in the folder; `None` when the folder can't be read.
    pub backups: Option<i64>,
    pub folder_path: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExtensionCount {
    pub extension: String,
    pub count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesLastRun {
    pub id: String,
    /// `series` | `issue`.
    pub scope: String,
    pub status: String,
    pub started_at: String,
    pub items_total: i32,
    pub items_matched_high: i32,
    pub items_applied: i32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProvenanceRow {
    pub field: String,
    pub set_by: String,
    pub set_at: String,
    pub source_external_id: Option<String>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct IssueProvenanceCounts {
    /// `comicinfo` / `metroninfo` / `series_json` / `scanner_inference`.
    pub file: i64,
    /// `comicvine` / `metron` / `gcd`.
    pub provider: i64,
    pub user: i64,
}

#[utoipa::path(
    operation_id = "series_metadata_overview",
    get,
    path = "/series/{slug}/metadata/overview",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = SeriesMetadataOverview),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn overview(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> Response {
    let Some(s) = visible_series(&app, &user, &slug).await else {
        return not_found();
    };
    let series_ids = fetch_rows(&app, "series", &s.id.to_string()).await;
    let scored = super::series::score_series_issues(&app, s.id).await;
    let issue_ids_list: Vec<String> = scored.iter().map(|(r, _)| r.id.clone()).collect();
    let ids = issue_ids_for(&app, &issue_ids_list).await;

    let mut issue_ids = IssueIdCounts {
        total: scored.len() as i64,
        ..Default::default()
    };
    for id in &issue_ids_list {
        let Some(m) = ids.get(id) else { continue };
        if m.contains_key("comicvine") {
            issue_ids.comicvine += 1;
        }
        if m.contains_key("metron") {
            issue_ids.metron += 1;
        }
        if m.contains_key("gcd") {
            issue_ids.gcd += 1;
        }
        if SOURCES.iter().all(|src| m.contains_key(*src)) {
            issue_ids.all_sources += 1;
        }
    }
    let mut tiers = SeriesTierCounts::default();
    for (_, report) in &scored {
        match report.tier {
            CompletenessTier::Complete => tiers.complete += 1,
            CompletenessTier::Partial => tiers.partial += 1,
            CompletenessTier::NeedsMetadata => tiers.needs_metadata += 1,
            CompletenessTier::Accepted => tiers.accepted += 1,
        }
    }

    #[derive(FromQueryResult)]
    struct SyncFiles {
        issues_synced: i64,
        sidecars_rewritten: i64,
        last_rewrite_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
        archives: i64,
        with_comicinfo: i64,
        with_metroninfo: i64,
    }
    let sf = SyncFiles::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "SELECT count(*) FILTER (WHERE last_metadata_sync_at IS NOT NULL)::bigint AS issues_synced, \
                count(*) FILTER (WHERE last_sidecar_rewrite_at IS NOT NULL)::bigint AS sidecars_rewritten, \
                max(last_sidecar_rewrite_at) AS last_rewrite_at, \
                count(*)::bigint AS archives, \
                count(*) FILTER (WHERE COALESCE(comicinfo_count, 0) > 0)::bigint AS with_comicinfo, \
                count(*) FILTER (WHERE metroninfo_present)::bigint AS with_metroninfo \
           FROM issues WHERE series_id = $1 AND removed_at IS NULL AND state = 'active'",
        [s.id.into()],
    ))
    .one(&app.db)
    .await
    .ok()
    .flatten();

    #[derive(FromQueryResult)]
    struct ExtRow {
        extension: String,
        count: i64,
    }
    let by_extension = ExtRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "SELECT lower(regexp_replace(file_path, '^.*\\.', '')) AS extension, count(*)::bigint AS count \
           FROM issues WHERE series_id = $1 AND removed_at IS NULL AND state = 'active' \
          GROUP BY 1 ORDER BY count DESC, extension ASC",
        [s.id.into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| ExtensionCount {
        extension: r.extension,
        count: r.count,
    })
    .collect();

    let (cvinfo_present, backups) = match s.folder_path.as_deref() {
        Some(folder) => inspect_folder(folder).await,
        None => (false, None),
    };

    let files = SeriesFilesState {
        archives: sf.as_ref().map(|r| r.archives).unwrap_or(0),
        by_extension,
        with_comicinfo: sf.as_ref().map(|r| r.with_comicinfo).unwrap_or(0),
        with_metroninfo: sf.as_ref().map(|r| r.with_metroninfo).unwrap_or(0),
        series_json_present: s.series_json_present.unwrap_or(false),
        cvinfo_present,
        backups,
        folder_path: s.folder_path.clone(),
    };
    let sync = SeriesSyncState {
        series_last_sync_at: s.last_metadata_sync_at.map(|t| t.to_rfc3339()),
        metadata_sync_paused: s.metadata_sync_paused,
        issues_synced: sf.as_ref().map(|r| r.issues_synced).unwrap_or(0),
        sidecars_rewritten: sf.as_ref().map(|r| r.sidecars_rewritten).unwrap_or(0),
        last_rewrite_at: sf
            .as_ref()
            .and_then(|r| r.last_rewrite_at)
            .map(|t| t.to_rfc3339()),
    };

    // Runs: the series' own and its issues'.
    let mut scope_ids: Vec<String> = issue_ids_list.clone();
    scope_ids.push(s.id.to_string());
    let runs_total = {
        use sea_orm::PaginatorTrait;
        metadata_run::Entity::find()
            .filter(metadata_run::Column::ScopeEntityId.is_in(scope_ids.clone()))
            .count(&app.db)
            .await
            .map(|n| n as i64)
            .unwrap_or(0)
    };
    let last_run = metadata_run::Entity::find()
        .filter(metadata_run::Column::ScopeEntityId.is_in(scope_ids))
        .order_by_desc(metadata_run::Column::StartedAt)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|r| SeriesLastRun {
            id: r.id.to_string(),
            scope: r.scope,
            status: r.status,
            started_at: r.started_at.to_rfc3339(),
            items_total: r.items_total,
            items_matched_high: r.items_matched_high,
            items_applied: r.items_applied,
        });

    let series_provenance = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("series"))
        .filter(field_provenance::Column::EntityId.eq(s.id.to_string()))
        .order_by_asc(field_provenance::Column::Field)
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| ProvenanceRow {
            field: r.field,
            set_by: r.set_by,
            set_at: r.set_at.to_rfc3339(),
            source_external_id: r.source_external_id,
        })
        .collect();

    #[derive(FromQueryResult)]
    struct ProvRow {
        strength: String,
        issues: i64,
    }
    let mut issue_provenance = IssueProvenanceCounts::default();
    if let Ok(rows) = ProvRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "SELECT CASE WHEN fp.set_by = 'user' THEN 'user' \
                     WHEN fp.set_by IN ('comicvine', 'metron', 'gcd') THEN 'provider' \
                     ELSE 'file' END AS strength, \
                count(DISTINCT fp.entity_id)::bigint AS issues \
           FROM field_provenance fp JOIN issues i ON i.id = fp.entity_id \
          WHERE fp.entity_type = 'issue' AND i.series_id = $1 AND i.removed_at IS NULL \
          GROUP BY 1",
        [s.id.into()],
    ))
    .all(&app.db)
    .await
    {
        for r in rows {
            match r.strength.as_str() {
                "user" => issue_provenance.user = r.issues,
                "provider" => issue_provenance.provider = r.issues,
                _ => issue_provenance.file = r.issues,
            }
        }
    }

    Json(SeriesMetadataOverview {
        series_id: s.id.to_string(),
        series_ids,
        issue_ids,
        tiers,
        sync,
        files,
        last_run,
        runs_total,
        series_provenance,
        issue_provenance,
    })
    .into_response()
}

/// `(cvinfo present, .bak count)` for a series folder; a folder that can't
/// be read reports `(false, None)` rather than failing the overview.
async fn inspect_folder(folder: &str) -> (bool, Option<i64>) {
    let mut dir = match tokio::fs::read_dir(folder).await {
        Ok(d) => d,
        Err(_) => return (false, None),
    };
    let mut cvinfo = false;
    let mut backups = 0i64;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.eq_ignore_ascii_case("cvinfo") {
            cvinfo = true;
        } else if name.contains(".bak") {
            backups += 1;
        }
    }
    (cvinfo, Some(backups))
}

// ───────── issues ─────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct IssuesQuery {
    /// `complete` | `partial` | `needs_metadata` | `accepted` | `incomplete`
    /// (partial + needs metadata).
    #[serde(default)]
    pub tier: Option<String>,
    /// `comicvine` | `metron` | `gcd`: only issues lacking that id.
    #[serde(default)]
    pub without: Option<String>,
    /// `specials` (annuals, specials, one-shots, …) | `main`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesMetadataIssuesResp {
    pub items: Vec<SeriesMetadataIssueRow>,
    /// Rows after the filters (what `items` holds).
    pub total: i64,
    /// Active issues in the series, before any filter.
    pub series_total: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesMetadataIssueRow {
    pub id: String,
    pub slug: String,
    /// `#12`, `Annual 3`, `Special 1` — what the chip grids show.
    pub label: String,
    pub number_raw: Option<String>,
    pub special_type: Option<String>,
    pub title: Option<String>,
    pub sort_number: Option<f64>,
    /// `YYYY-MM` or `YYYY` when known.
    pub cover_date: Option<String>,
    pub tier: CompletenessTier,
    pub accepted: bool,
    /// Missing core field keys (`external_id`, `summary`, …).
    pub missing_core: Vec<String>,
    /// Provider issue ids by source.
    pub ids: IssueSourceIds,
    /// [`tag_tool`] key for the file's ComicInfo notes.
    pub tagged_by: String,
    pub last_run: Option<IssueLastRun>,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct IssueSourceIds {
    pub comicvine: Option<String>,
    pub metron: Option<String>,
    pub gcd: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueLastRun {
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub matched_high: i32,
    pub applied: i32,
}

#[utoipa::path(
    operation_id = "series_metadata_issues",
    get,
    path = "/series/{slug}/metadata/issues",
    params(("slug" = String, Path,), IssuesQuery),
    responses(
        (status = 200, body = SeriesMetadataIssuesResp),
        (status = 404, description = "series not found"),
        (status = 422, description = "unknown filter value"),
    )
)]
#[handler]
pub async fn issues(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(q): Query<IssuesQuery>,
) -> Response {
    let tier = match q.tier.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(v @ ("complete" | "partial" | "needs_metadata" | "accepted" | "incomplete")) => {
            Some(v.to_owned())
        }
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "series.invalid_filter",
                "tier must be complete, partial, needs_metadata, accepted or incomplete",
            );
        }
    };
    let without = match q
        .without
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(v) if SOURCES.contains(&v) => Some(v.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "series.invalid_filter",
                "without must be comicvine, metron or gcd",
            );
        }
    };
    let kind = match q.kind.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(v @ ("specials" | "main")) => Some(v.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "series.invalid_filter",
                "kind must be specials or main",
            );
        }
    };
    let Some(s) = visible_series(&app, &user, &slug).await else {
        return not_found();
    };

    let scored = super::series::score_series_issues(&app, s.id).await;
    let series_total = scored.len() as i64;
    let issue_ids_list: Vec<String> = scored.iter().map(|(r, _)| r.id.clone()).collect();
    let ids = issue_ids_for(&app, &issue_ids_list).await;

    // Per-issue extras in one query: cover month, the tagging trace, and
    // the latest run.
    #[derive(FromQueryResult)]
    struct Extra {
        id: String,
        month: Option<i32>,
        notes: String,
        genre: String,
        web: String,
        run_id: Option<uuid::Uuid>,
        run_status: Option<String>,
        run_started_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
        run_matched_high: Option<i32>,
        run_applied: Option<i32>,
    }
    let extras: HashMap<String, Extra> = Extra::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "SELECT i.id, i.month, \
                COALESCE(i.comic_info_raw->>'notes', '') AS notes, \
                COALESCE(i.comic_info_raw->>'genre', '') AS genre, \
                COALESCE(i.comic_info_raw->>'web', '') AS web, \
                r.id AS run_id, r.status AS run_status, r.started_at AS run_started_at, \
                r.items_matched_high AS run_matched_high, r.items_applied AS run_applied \
           FROM issues i \
           LEFT JOIN LATERAL (SELECT id, status, started_at, items_matched_high, items_applied \
                                FROM metadata_run mr WHERE mr.scope = 'issue' AND mr.scope_entity_id = i.id \
                               ORDER BY mr.started_at DESC LIMIT 1) r ON true \
          WHERE i.series_id = $1 AND i.removed_at IS NULL AND i.state = 'active'",
        [s.id.into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|e| (e.id.clone(), e))
    .collect();

    let mut items = Vec::with_capacity(scored.len());
    for (r, report) in scored {
        let tier_key = match report.tier {
            CompletenessTier::Complete => "complete",
            CompletenessTier::Partial => "partial",
            CompletenessTier::NeedsMetadata => "needs_metadata",
            CompletenessTier::Accepted => "accepted",
        };
        if let Some(t) = tier.as_deref() {
            let keep = if t == "incomplete" {
                matches!(tier_key, "partial" | "needs_metadata")
            } else {
                tier_key == t
            };
            if !keep {
                continue;
            }
        }
        let own = ids.get(&r.id);
        if let Some(src) = without.as_deref()
            && own.is_some_and(|m| m.contains_key(src))
        {
            continue;
        }
        let is_special = r.special_type.as_deref().is_some_and(|t| !t.is_empty());
        if let Some(k) = kind.as_deref()
            && (k == "specials") != is_special
        {
            continue;
        }
        let extra = extras.get(&r.id);
        let cover_date = match (r.year, extra.and_then(|e| e.month)) {
            (Some(y), Some(m)) if (1..=12).contains(&m) => Some(format!("{y:04}-{m:02}")),
            (Some(y), _) => Some(format!("{y:04}")),
            _ => None,
        };
        let label = match (r.special_type.as_deref(), r.number_raw.as_deref()) {
            (Some(t), Some(n)) if !t.is_empty() => format!("{t} {n}"),
            (Some(t), None) if !t.is_empty() => t.to_owned(),
            (_, Some(n)) => format!("#{n}"),
            (_, None) => r.title.clone().unwrap_or_else(|| "Issue".to_owned()),
        };
        let tagged_by = extra
            .map(|e| tag_tool::classify(&e.notes, &e.genre, &e.web))
            .unwrap_or("untagged")
            .to_owned();
        let last_run = extra.and_then(|e| {
            Some(IssueLastRun {
                id: e.run_id?.to_string(),
                status: e.run_status.clone()?,
                started_at: e.run_started_at?.to_rfc3339(),
                matched_high: e.run_matched_high.unwrap_or(0),
                applied: e.run_applied.unwrap_or(0),
            })
        });
        items.push(SeriesMetadataIssueRow {
            id: r.id.clone(),
            slug: r.slug.clone(),
            label,
            number_raw: r.number_raw.clone(),
            special_type: r.special_type.clone(),
            title: r.title.clone(),
            sort_number: r.sort_number,
            cover_date,
            tier: report.tier,
            accepted: r.metadata_review_accepted_at.is_some(),
            missing_core: report.missing_core,
            ids: IssueSourceIds {
                comicvine: own.and_then(|m| m.get("comicvine").cloned()),
                metron: own.and_then(|m| m.get("metron").cloned()),
                gcd: own.and_then(|m| m.get("gcd").cloned()),
            },
            tagged_by,
            last_run,
        });
    }
    let total = items.len() as i64;
    Json(SeriesMetadataIssuesResp {
        items,
        total,
        series_total,
    })
    .into_response()
}

// ───────── accept (bulk acknowledgement) ─────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AcceptIssuesReq {
    pub issue_ids: Vec<String>,
    /// `true` marks the issues complete enough; `false` reverts.
    pub accepted: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AcceptIssuesResp {
    /// Issues of this series the request changed.
    pub updated: i64,
}

#[utoipa::path(
    operation_id = "series_metadata_accept_issues",
    post,
    path = "/series/{slug}/metadata/issues/accept",
    params(("slug" = String, Path,)),
    request_body = AcceptIssuesReq,
    responses(
        (status = 200, body = AcceptIssuesResp),
        (status = 404, description = "series not found"),
        (status = 422, description = "empty selection"),
    )
)]
#[handler]
pub async fn accept_issues(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Json(req): Json<AcceptIssuesReq>,
) -> Response {
    if req.issue_ids.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "issue_ids must not be empty",
        );
    }
    let Some(s) = visible_series(&app, &user, &slug).await else {
        return not_found();
    };
    // Only this series' issues, whatever the request named.
    let rows = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::Id.is_in(req.issue_ids.clone()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let now = chrono::Utc::now().fixed_offset();
    let mut updated = 0i64;
    let mut touched: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        if row.metadata_review_accepted_at.is_some() == req.accepted {
            continue;
        }
        let id = row.id.clone();
        let mut am: issue::ActiveModel = row.into();
        am.metadata_review_accepted_at = sea_orm::Set(req.accepted.then_some(now));
        am.metadata_review_accepted_by = sea_orm::Set(req.accepted.then_some(user.id));
        am.updated_at = sea_orm::Set(now);
        match am.update(&app.db).await {
            Ok(_) => {
                updated += 1;
                touched.push(id);
            }
            Err(e) => {
                tracing::error!(error = %e, issue_id = %id, "bulk metadata accept update failed");
            }
        }
    }
    if updated > 0 {
        crate::audit::record(
            &app.db,
            crate::audit::AuditEntry {
                actor_id: user.id,
                action: if req.accepted {
                    "metadata.issue.accept_bulk"
                } else {
                    "metadata.issue.unaccept_bulk"
                },
                target_type: Some("series"),
                target_id: Some(s.id.to_string()),
                payload: serde_json::json!({
                    "accepted": req.accepted,
                    "updated": updated,
                    "issue_ids": touched,
                }),
                ip: ctx.ip_string(),
                user_agent: ctx.user_agent.clone(),
            },
        )
        .await;
    }
    Json(AcceptIssuesResp { updated }).into_response()
}

// ───────── helpers ─────────

async fn visible_series(app: &AppState, user: &CurrentUser, slug: &str) -> Option<series::Model> {
    let s = super::series::find_by_slug(&app.db, slug).await.ok()?;
    access::series_visible(app, user, &s).await.then_some(s)
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "series not found")
}

/// `issue id → (source → external id)` for the given issues, one query.
async fn issue_ids_for(
    app: &AppState,
    issue_ids: &[String],
) -> HashMap<String, BTreeMap<String, String>> {
    let mut out: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    if issue_ids.is_empty() {
        return out;
    }
    let rows = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("issue"))
        .filter(external_id::Column::EntityId.is_in(issue_ids.to_vec()))
        .filter(external_id::Column::Source.is_in(SOURCES))
        .select_only()
        .column(external_id::Column::EntityId)
        .column(external_id::Column::Source)
        .column(external_id::Column::ExternalId)
        .into_tuple::<(String, String, String)>()
        .all(&app.db)
        .await
        .unwrap_or_default();
    for (entity_id, source, ext) in rows {
        out.entry(entity_id).or_default().insert(source, ext);
    }
    out
}
