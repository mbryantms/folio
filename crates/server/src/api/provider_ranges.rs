//! Per-(series, provider) issue-range mapping CRUD — provider
//! series-boundary divergence support.
//!
//! Surfaces the `series_provider_range` table to the
//! `<SeriesProviderRangesCard>` UI so an operator can declare that a
//! contiguous issue range of a local series belongs to a DIFFERENT
//! provider series than the rest of the run (e.g. Fantastic Four
//! #600–611 → Metron "Fantastic Four (2012)"). The mapping then drives
//! issue-search routing ([`crate::metadata::range_map`]) and the apply
//! path's series-identity override.
//!
//! Visibility (GET) is granted to anyone who can see the library;
//! editing (POST/DELETE) is admin-only and audited. Manual rows land
//! `set_by='user'`.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::series_provider_range as range_entity;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::metadata::identifier::Source;
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::range_map::ranges_overlap;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_series))
        .routes(routes!(coverage_series))
        .routes(routes!(add_series))
        .routes(routes!(delete_series))
        .routes(routes!(detect_series))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderRangeRow {
    pub id: String,
    pub source: String,
    pub source_label: String,
    pub provider_series_id: String,
    pub provider_series_url: Option<String>,
    pub provider_series_name: Option<String>,
    /// Inclusive lower bound (canonical issue number). `null` = open-ended.
    pub range_low: Option<String>,
    /// Inclusive upper bound (canonical issue number). `null` = open-ended.
    pub range_high: Option<String>,
    /// The mapped sub-series' start year (used by the issue-search year gate).
    pub declared_year: Option<i32>,
    pub set_by: String,
    pub first_set_at: String,
    pub last_synced_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderRangesListResp {
    pub series_id: String,
    pub rows: Vec<ProviderRangeRow>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddProviderRangeReq {
    /// `"comicvine" | "metron" | "gcd" | …`. Aliases accepted.
    pub source: String,
    pub provider_series_id: String,
    pub provider_series_url: Option<String>,
    pub provider_series_name: Option<String>,
    /// Inclusive lower bound; canonicalized server-side. Empty / omitted
    /// ⇒ open-ended.
    pub range_low: Option<String>,
    /// Inclusive upper bound; canonicalized server-side. Empty / omitted
    /// ⇒ open-ended.
    pub range_high: Option<String>,
    pub declared_year: Option<i32>,
}

#[utoipa::path(
    operation_id = "provider_ranges_list_series", get,
    path = "/series/{slug}/provider-ranges",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = ProviderRangesListResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn list_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let rows = fetch_rows(&app, s.id).await;
    Json(ProviderRangesListResp {
        series_id: s.id.to_string(),
        rows,
    })
    .into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderCoverageResp {
    pub providers: Vec<ProviderCoverage>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderCoverage {
    pub source: String,
    pub source_label: String,
    /// Issue-range segments across this local series, in reading order.
    pub segments: Vec<CoverageSegment>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CoverageSegment {
    pub low: String,
    pub high: String,
    pub issue_count: u32,
    pub provider_series_id: String,
    pub provider_series_name: Option<String>,
    pub provider_series_url: Option<String>,
    pub declared_year: Option<i32>,
    /// `true` ⇒ a range-override sub-series; `false` ⇒ the series-level
    /// default mapping.
    pub via_range: bool,
    /// The `series_provider_range` row id for an override segment, so the
    /// UI can offer a delete affordance.
    pub range_id: Option<String>,
}

#[utoipa::path(
    operation_id = "provider_ranges_coverage_series", get,
    path = "/series/{slug}/provider-coverage",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = ProviderCoverageResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn coverage_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> Response {
    use entity::{external_id, issue};
    use sea_orm::{QueryOrder, QuerySelect};

    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }

    // Project only the issue number (in reading order) — loading full
    // issue rows would drag the large `comic_info_raw` / `pages` JSON.
    let issue_numbers: Vec<String> = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::State.eq("active"))
        .order_by_asc(issue::Column::SortNumber)
        .select_only()
        .column(issue::Column::NumberRaw)
        .into_tuple::<Option<String>>()
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| {
            let raw = n.as_deref()?.trim();
            (!raw.is_empty()).then(|| canonical_issue_number(raw))
        })
        .collect();
    let ranges = range_entity::Entity::find()
        .filter(range_entity::Column::SeriesId.eq(s.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let series_ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(s.id.to_string()))
        .all(&app.db)
        .await
        .unwrap_or_default();

    let mut providers = build_provider_coverage(&issue_numbers, &ranges, &series_ids);

    // Default segments come from `external_ids`, which carries only the
    // id — fill their display name + start year from the matched series'
    // cached detail so they read "Fantastic Four (1998)", not "series
    // 1711". Range-override segments already carry the name we stored.
    for p in &mut providers {
        for seg in &mut p.segments {
            if seg.provider_series_name.is_some() && seg.declared_year.is_some() {
                continue;
            }
            let Ok(src) = Source::from_str(&p.source) else {
                continue;
            };
            if let Some((name, year)) =
                crate::metadata::cache::series_display_meta(&app.db, src, &seg.provider_series_id)
                    .await
            {
                if seg.provider_series_name.is_none() {
                    seg.provider_series_name = name;
                }
                if seg.declared_year.is_none() {
                    seg.declared_year = year;
                }
            }
        }
    }

    Json(ProviderCoverageResp { providers }).into_response()
}

/// Fold each issue (canonical numbers, reading order) to its effective
/// provider series and run-length-encode into per-provider segments. A
/// pure, DB-free function so it's unit-testable; default-segment display
/// names stay unset here for the caller to enrich from the metadata cache.
fn build_provider_coverage(
    issue_numbers: &[String],
    ranges: &[range_entity::Model],
    series_ids: &[entity::external_id::Model],
) -> Vec<ProviderCoverage> {
    use crate::metadata::range_map::{EffectiveTarget, fold_targets, issue_in_range};

    // Per source, the effective target for each issue in reading order.
    let mut per_source: Vec<(Source, Vec<(String, EffectiveTarget)>)> = Vec::new();
    for canon in issue_numbers {
        for t in fold_targets(ranges, series_ids, canon) {
            match per_source.iter_mut().find(|(src, _)| *src == t.source) {
                Some((_, v)) => v.push((canon.clone(), t)),
                None => per_source.push((t.source, vec![(canon.clone(), t)])),
            }
        }
    }

    // Run-length-encode consecutive same-series issues into segments.
    per_source
        .into_iter()
        .map(|(source, list)| {
            let mut segments: Vec<CoverageSegment> = Vec::new();
            for (canon, t) in list {
                let extend = matches!(segments.last(), Some(last) if last.provider_series_id == t.provider_series_id);
                if extend {
                    let last = segments.last_mut().unwrap();
                    last.high = canon.clone();
                    last.issue_count += 1;
                } else {
                    let range_id = if t.via_range {
                        ranges
                            .iter()
                            .find(|r| {
                                Source::from_str(&r.source).ok() == Some(source)
                                    && r.provider_series_id == t.provider_series_id
                                    && issue_in_range(
                                        &canon,
                                        r.range_low.as_deref(),
                                        r.range_high.as_deref(),
                                    )
                            })
                            .map(|r| r.id.to_string())
                    } else {
                        None
                    };
                    segments.push(CoverageSegment {
                        low: canon.clone(),
                        high: canon,
                        issue_count: 1,
                        provider_series_id: t.provider_series_id,
                        provider_series_name: t.provider_series_name,
                        provider_series_url: t.provider_series_url,
                        declared_year: t.declared_year,
                        via_range: t.via_range,
                        range_id,
                    });
                }
            }
            ProviderCoverage {
                source: source.as_str().to_owned(),
                source_label: source.label().to_owned(),
                segments,
            }
        })
        .collect()
}

#[utoipa::path(
    operation_id = "provider_ranges_add_series", post,
    path = "/series/{slug}/provider-ranges",
    params(("slug" = String, Path)),
    request_body = AddProviderRangeReq,
    responses(
        (status = 201, body = ProviderRangeRow),
        (status = 400, description = "invalid source / range"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "range overlaps an existing mapping"),
    )
)]
#[handler]
pub async fn add_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Json(req): Json<AddProviderRangeReq>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let Ok(source) = req.source.parse::<Source>() else {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_source",
            "unknown source",
        );
    };
    let provider_series_id = req.provider_series_id.trim().to_owned();
    if provider_series_id.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_provider_series_id",
            "provider_series_id required",
        );
    }
    let range_low = canon_bound(req.range_low.as_deref());
    let range_high = canon_bound(req.range_high.as_deref());
    // Reject an inverted numeric range (low > high). Non-numeric bounds
    // pass through — the overlap guard treats them conservatively.
    if let (Some(lo), Some(hi)) = (
        range_low.as_deref().and_then(|s| s.parse::<f64>().ok()),
        range_high.as_deref().and_then(|s| s.parse::<f64>().ok()),
    ) && lo > hi
    {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_range",
            "range_low must be ≤ range_high",
        );
    }

    // Overlap guard: a series + source can't carry two ranges that
    // cover the same issue — routing would be ambiguous.
    let existing = range_entity::Entity::find()
        .filter(range_entity::Column::SeriesId.eq(s.id))
        .filter(range_entity::Column::Source.eq(source.as_str()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    if existing.iter().any(|r| {
        ranges_overlap(
            range_low.as_deref(),
            range_high.as_deref(),
            r.range_low.as_deref(),
            r.range_high.as_deref(),
        )
    }) {
        return error(
            StatusCode::CONFLICT,
            "metadata.range_overlap",
            "that issue range overlaps an existing mapping for this provider",
        );
    }

    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    let model = range_entity::ActiveModel {
        id: Set(id),
        series_id: Set(s.id),
        source: Set(source.as_str().to_owned()),
        provider_series_id: Set(provider_series_id.clone()),
        provider_series_url: Set(req
            .provider_series_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)),
        provider_series_name: Set(req
            .provider_series_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)),
        range_low: Set(range_low.clone()),
        range_high: Set(range_high.clone()),
        declared_year: Set(req.declared_year),
        set_by: Set("user".to_owned()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    };
    if let Err(e) = model.insert(&app.db).await {
        tracing::warn!(error = %e, "provider_range insert failed");
        return error(StatusCode::BAD_GATEWAY, "internal", "range write failed");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.series.provider_range_set",
            target_type: Some("series"),
            target_id: Some(s.id.to_string()),
            payload: serde_json::json!({
                "source": source.as_str(),
                "provider_series_id": provider_series_id,
                "range_low": range_low,
                "range_high": range_high,
                "declared_year": req.declared_year,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let rows = fetch_rows(&app, s.id).await;
    let Some(row) = rows.into_iter().find(|r| r.id == id.to_string()) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "internal",
            "range write succeeded but readback failed",
        );
    };
    (StatusCode::CREATED, Json(row)).into_response()
}

#[utoipa::path(
    operation_id = "provider_ranges_delete_series", delete,
    path = "/series/{slug}/provider-ranges/{id}",
    params(("slug" = String, Path), ("id" = String, Path)),
    responses(
        (status = 204, description = "removed"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series / range not found"),
    )
)]
#[handler]
pub async fn delete_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, id)): Path<(String, String)>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Ok(range_id) = Uuid::parse_str(&id) else {
        return error(StatusCode::BAD_REQUEST, "metadata.invalid_id", "invalid id");
    };
    // Scope the delete to this series so a stray id can't reach across.
    let existing = range_entity::Entity::find()
        .filter(range_entity::Column::Id.eq(range_id))
        .filter(range_entity::Column::SeriesId.eq(s.id))
        .one(&app.db)
        .await
        .ok()
        .flatten();
    let Some(row) = existing else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.range_not_found",
            "no such range for this series",
        );
    };
    if let Err(e) = range_entity::Entity::delete_by_id(range_id)
        .exec(&app.db)
        .await
    {
        tracing::warn!(error = %e, "provider_range delete failed");
        return error(StatusCode::BAD_GATEWAY, "internal", "delete failed");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.series.provider_range_delete",
            target_type: Some("series"),
            target_id: Some(s.id.to_string()),
            payload: serde_json::json!({
                "id": range_id.to_string(),
                "source": row.source,
                "provider_series_id": row.provider_series_id,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

// ───────── on-demand detection ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectResp {
    pub results: Vec<DetectSourceResult>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectSourceResult {
    pub source: String,
    pub source_label: String,
    /// The matched provider series the detector scanned against.
    pub provider_series_id: String,
    /// Issue numbers that series reported. `0` ⇒ the provider couldn't
    /// enumerate it (e.g. ComicVine, or an empty/failed response).
    pub covered_count: u32,
    /// Local issue ranges the matched series didn't cover ("600..611").
    pub gaps: Vec<String>,
    /// Range mappings created this run.
    pub created: Vec<ProviderRangeRow>,
    /// Set when the detector errored for this source (provider call failed).
    pub error: Option<String>,
}

#[utoipa::path(
    operation_id = "provider_ranges_detect_series", post,
    path = "/series/{slug}/provider-ranges/detect",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = DetectResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn detect_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    // The provider series this run was matched to come from the run's
    // *applied candidates* (recorded synchronously on apply) — under
    // writeback the series-level `external_ids` aren't written until a
    // later rescan, so they're unreliable here. Latest applied per source.
    let targets = applied_series_targets(&app, s.id).await;

    let mut results = Vec::new();
    for (source, provider_series_id) in targets {
        // Reconcile the main series-level linkage too — under writeback it
        // isn't persisted at apply time, so this is what makes the matched
        // Metron/CV id show in the External IDs card + header.
        let identifier = crate::metadata::identifier::Identifier::with_canonical_url(
            source,
            provider_series_id.clone(),
            "series",
        );
        let _ = crate::metadata::writers::set_external_id(
            &app.db,
            "series",
            &s.id.to_string(),
            &identifier,
            crate::metadata::writers::SetBy::Provider(source),
        )
        .await;

        let Some(provider) = crate::metadata::apply::build_provider(&app, source) else {
            continue;
        };
        let (covered_count, gaps, created, error) =
            match crate::metadata::auto_split::detect_and_map(
                &app.db,
                &s,
                source,
                &provider_series_id,
                &*provider,
            )
            .await
            {
                Ok(o) => (
                    o.covered_count as u32,
                    o.gaps
                        .into_iter()
                        .map(|(lo, hi)| format!("{lo}..{hi}"))
                        .collect(),
                    o.created,
                    None,
                ),
                Err(e) => (0, Vec::new(), Vec::new(), Some(e.to_string())),
            };
        // Re-read the freshly-written rows for the response.
        let created_rows: Vec<ProviderRangeRow> = if created.is_empty() {
            Vec::new()
        } else {
            let ids: Vec<String> = created
                .iter()
                .map(|c| format!("{}|{}|{}", c.provider_series_id, c.range_low, c.range_high))
                .collect();
            fetch_rows(&app, s.id)
                .await
                .into_iter()
                .filter(|r| {
                    ids.contains(&format!(
                        "{}|{}|{}",
                        r.provider_series_id,
                        r.range_low.clone().unwrap_or_default(),
                        r.range_high.clone().unwrap_or_default()
                    ))
                })
                .collect()
        };
        results.push(DetectSourceResult {
            source: source.as_str().to_owned(),
            source_label: source.label().to_owned(),
            provider_series_id,
            covered_count,
            gaps,
            created: created_rows,
            error,
        });
    }

    let total_created: usize = results.iter().map(|r| r.created.len()).sum();
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.series.provider_range_detect",
            target_type: Some("series"),
            target_id: Some(s.id.to_string()),
            payload: serde_json::json!({ "created": total_created }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(DetectResp { results }).into_response()
}

/// Latest applied provider series per source for `series_id`, from the
/// run candidates (most-recent `applied_at` wins).
async fn applied_series_targets(app: &AppState, series_id: Uuid) -> Vec<(Source, String)> {
    use entity::{metadata_run, metadata_run_candidate};
    use sea_orm::QueryOrder;

    let run_ids: Vec<Uuid> = metadata_run::Entity::find()
        .filter(metadata_run::Column::Scope.eq("series"))
        .filter(metadata_run::Column::ScopeEntityId.eq(series_id.to_string()))
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.id)
        .collect();
    if run_ids.is_empty() {
        return Vec::new();
    }
    let cands = metadata_run_candidate::Entity::find()
        .filter(metadata_run_candidate::Column::RunId.is_in(run_ids))
        .filter(metadata_run_candidate::Column::AppliedAt.is_not_null())
        .order_by_desc(metadata_run_candidate::Column::AppliedAt)
        .all(&app.db)
        .await
        .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut targets = Vec::new();
    for c in cands {
        if let Ok(src) = Source::from_str(&c.source)
            && seen.insert(src)
        {
            targets.push((src, c.external_id));
        }
    }
    targets
}

// ───────── shared ─────────

pub(crate) async fn fetch_rows(app: &AppState, series_id: Uuid) -> Vec<ProviderRangeRow> {
    let rows = range_entity::Entity::find()
        .filter(range_entity::Column::SeriesId.eq(series_id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            let source = Source::from_str(&r.source).ok()?;
            Some(ProviderRangeRow {
                id: r.id.to_string(),
                source: source.as_str().to_owned(),
                source_label: source.label().to_owned(),
                provider_series_id: r.provider_series_id,
                provider_series_url: r.provider_series_url,
                provider_series_name: r.provider_series_name,
                range_low: r.range_low,
                range_high: r.range_high,
                declared_year: r.declared_year,
                set_by: r.set_by,
                first_set_at: r.first_set_at.to_rfc3339(),
                last_synced_at: r.last_synced_at.to_rfc3339(),
            })
        })
        .collect()
}

/// Trim + canonicalize an issue-number bound; empty ⇒ `None` (open-ended).
fn canon_bound(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(canonical_issue_number)
}

async fn user_can_see_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    use entity::library_user_access;
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn range_row(source: &str, id: &str, low: &str, high: &str) -> range_entity::Model {
        range_entity::Model {
            id: Uuid::nil(),
            series_id: Uuid::nil(),
            source: source.into(),
            provider_series_id: id.into(),
            provider_series_url: None,
            provider_series_name: Some("Fantastic Four (2012)".into()),
            range_low: Some(low.into()),
            range_high: Some(high.into()),
            declared_year: Some(2012),
            set_by: "cross_reference".into(),
            first_set_at: Utc::now().into(),
            last_synced_at: Utc::now().into(),
        }
    }

    fn series_ext(source: &str, id: &str) -> entity::external_id::Model {
        entity::external_id::Model {
            entity_type: "series".into(),
            entity_id: Uuid::nil().to_string(),
            source: source.into(),
            external_id: id.into(),
            external_url: None,
            set_by: "metron".into(),
            first_set_at: Utc::now().into(),
            last_synced_at: Utc::now().into(),
        }
    }

    #[test]
    fn coverage_segments_split_metron_and_collapse_comicvine() {
        let issues: Vec<String> = ["1", "2", "600", "601", "611"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let ranges = vec![range_row("metron", "1713", "600", "611")];
        let series_ids = vec![
            series_ext("metron", "1711"),
            series_ext("comicvine", "6211"),
        ];

        let providers = build_provider_coverage(&issues, &ranges, &series_ids);

        let metron = providers.iter().find(|p| p.source == "metron").unwrap();
        assert_eq!(metron.segments.len(), 2, "main run + 2012 split");
        // Default main-run segment first.
        assert_eq!(metron.segments[0].provider_series_id, "1711");
        assert!(!metron.segments[0].via_range);
        assert_eq!(metron.segments[0].low, "1");
        assert_eq!(metron.segments[0].high, "2");
        assert_eq!(metron.segments[0].issue_count, 2);
        // Override segment for the relaunch block.
        assert_eq!(metron.segments[1].provider_series_id, "1713");
        assert!(metron.segments[1].via_range);
        assert_eq!(metron.segments[1].low, "600");
        assert_eq!(metron.segments[1].high, "611");
        assert_eq!(metron.segments[1].issue_count, 3);
        assert!(metron.segments[1].range_id.is_some());
        assert_eq!(
            metron.segments[1].provider_series_name.as_deref(),
            Some("Fantastic Four (2012)")
        );

        // ComicVine is a lumper here — one segment spanning the whole run.
        let cv = providers.iter().find(|p| p.source == "comicvine").unwrap();
        assert_eq!(cv.segments.len(), 1);
        assert_eq!(cv.segments[0].provider_series_id, "6211");
        assert!(!cv.segments[0].via_range);
        assert_eq!(cv.segments[0].low, "1");
        assert_eq!(cv.segments[0].high, "611");
        assert_eq!(cv.segments[0].issue_count, 5);
    }
}
