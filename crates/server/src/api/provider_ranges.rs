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
//!
//! `POST …/provider-ranges/detect` runs detection for every provider
//! that can enumerate issues ([`crate::metadata::series_link`]): it
//! resolves the series' Metron / GCD id even when the series was only
//! matched through ComicVine, then maps the issue runs those providers
//! file under a different series ([`crate::metadata::auto_split`]).

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
use crate::metadata::auto_split::GapStatus;
use crate::metadata::identifier::Source;
use crate::metadata::matcher::canonical_issue_number;
use crate::metadata::range_map::ranges_overlap;
use crate::metadata::series_link::{self, LinkCandidate, LinkMethod, SourceStatus};
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

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
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
    if !crate::library::access::series_visible(&app, &user, &s).await {
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
    if !crate::library::access::series_visible(&app, &user, &s).await {
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
            // Detail cache first, then the coverage issue-list cache (which
            // carries a GCD series' name even when only a link found it).
            let meta = match crate::metadata::cache::series_display_meta(
                &app.db,
                src,
                &seg.provider_series_id,
            )
            .await
            {
                Some((Some(n), y)) => Some((Some(n), y)),
                other => crate::metadata::coverage::cached_series_label(
                    &app.jobs.redis,
                    src,
                    &seg.provider_series_id,
                )
                .await
                .or(other),
            };
            if let Some((name, year)) = meta {
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
    /// Set when two or more providers were scanned: whether they found the
    /// same uncovered runs. Disagreement is normal (each provider routes
    /// its own issues through its own ranges); it's shown so the admin
    /// knows the providers split the run differently.
    pub agreement: Option<DetectAgreement>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectAgreement {
    pub agree: bool,
    pub summary: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectSourceResult {
    pub source: String,
    pub source_label: String,
    pub status: SourceStatus,
    /// The provider series the detector scanned against (or, for a
    /// provider that can't enumerate, the series it is linked to).
    pub provider_series_id: Option<String>,
    pub provider_series_name: Option<String>,
    pub provider_series_year: Option<i32>,
    pub provider_series_url: Option<String>,
    /// How that provider series was found.
    pub resolved_via: Option<LinkMethod>,
    /// The provider series id was recorded on the series' external ids
    /// during this run (a cross-reference or a strict search match).
    pub id_recorded: bool,
    /// Distinct issue numbers the provider series lists. `0` ⇒ not
    /// enumerated.
    pub covered_count: u32,
    /// Local numbered issues the provider series lists.
    pub matched_local: u32,
    /// Local issue runs the provider series didn't cover ("600..611").
    pub gaps: Vec<String>,
    /// Per-run outcome, aligned with `gaps`.
    pub gap_details: Vec<DetectGap>,
    /// Range mappings created this run.
    pub created: Vec<ProviderRangeRow>,
    /// Automated range mappings the provider series now covers itself —
    /// likely stale (e.g. after a re-match). Reported only; never removed
    /// automatically.
    pub stale_ranges: Vec<ProviderRangeRow>,
    /// Uncovered issues with a non-numeric number (annuals, `14AU`) —
    /// excluded from range detection.
    pub uncovered_specials: u32,
    /// Possible provider series for the admin to confirm
    /// (`needs_confirmation`). Never written automatically.
    pub candidates: Vec<LinkCandidate>,
    /// What failed for this provider (`error` / `rate_limited` / a note
    /// for `no_series`).
    pub error: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectGap {
    pub low: String,
    pub high: String,
    pub issue_count: u32,
    pub status: GapStatus,
    pub provider_series_id: Option<String>,
    pub provider_series_name: Option<String>,
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

    // Reconcile the applied series-level linkage first — under writeback
    // it isn't persisted at apply time, so this is what makes the matched
    // Metron/CV id show in the External IDs card + header.
    for (source, provider_series_id) in series_link::applied_series_targets(&app.db, s.id).await {
        let identifier = crate::metadata::identifier::Identifier::with_canonical_url(
            source,
            provider_series_id,
            "series",
        );
        if let Ok((_, promoted_pairs)) = crate::metadata::writers::set_external_id_promoting(
            &app.db,
            "series",
            &s.id.to_string(),
            &identifier,
            crate::metadata::writers::SetBy::Provider(source),
        )
        .await
            && promoted_pairs > 0
        {
            // WP-8.2: a promoted external link is a similar-series signal.
            app.similarity.invalidate_all();
        }
    }

    let detected = series_link::detect_series(&app, &s).await;
    let all_rows = fetch_rows(&app, s.id).await;
    let results: Vec<DetectSourceResult> = detected
        .into_iter()
        .map(|d| to_result(d, &all_rows))
        .collect();
    let agreement = agreement(&results);

    let total_created: usize = results.iter().map(|r| r.created.len()).sum();
    let ids_recorded: Vec<serde_json::Value> = results
        .iter()
        .filter(|r| r.id_recorded)
        .map(|r| {
            serde_json::json!({
                "source": r.source,
                "provider_series_id": r.provider_series_id,
                "via": r.resolved_via,
            })
        })
        .collect();
    let per_source: Vec<serde_json::Value> = results
        .iter()
        .map(|r| serde_json::json!({ "source": r.source, "status": r.status }))
        .collect();
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.series.provider_range_detect",
            target_type: Some("series"),
            target_id: Some(s.id.to_string()),
            payload: serde_json::json!({
                "created": total_created,
                "ids_recorded": ids_recorded,
                "sources": per_source,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(DetectResp { results, agreement }).into_response()
}

fn to_result(d: series_link::SourceDetect, all_rows: &[ProviderRangeRow]) -> DetectSourceResult {
    let source = d.source;
    let outcome = d.outcome.unwrap_or_default();
    let created_keys: Vec<String> = outcome
        .created
        .iter()
        .map(|c| format!("{}|{}|{}", c.provider_series_id, c.range_low, c.range_high))
        .collect();
    let row_key = |r: &ProviderRangeRow| {
        format!(
            "{}|{}|{}",
            r.provider_series_id,
            r.range_low.clone().unwrap_or_default(),
            r.range_high.clone().unwrap_or_default()
        )
    };
    let created: Vec<ProviderRangeRow> = all_rows
        .iter()
        .filter(|r| r.source == source.as_str() && created_keys.contains(&row_key(r)))
        .cloned()
        .collect();
    let stale_ids: Vec<String> = outcome
        .stale_range_ids
        .iter()
        .map(|id| id.to_string())
        .collect();
    let stale_ranges: Vec<ProviderRangeRow> = all_rows
        .iter()
        .filter(|r| stale_ids.contains(&r.id))
        .cloned()
        .collect();
    let link = d.link.as_ref();
    DetectSourceResult {
        source: source.as_str().to_owned(),
        source_label: source.label().to_owned(),
        status: d.status,
        provider_series_id: link.map(|l| l.external_id.clone()),
        provider_series_name: link.and_then(|l| l.name.clone()),
        provider_series_year: link.and_then(|l| l.year),
        provider_series_url: link.and_then(|l| {
            crate::metadata::identifier::canonical_url(source, "series", &l.external_id)
        }),
        resolved_via: link.map(|l| l.method),
        id_recorded: link.is_some_and(|l| l.written),
        covered_count: outcome.covered_count as u32,
        matched_local: outcome.matched_local as u32,
        gaps: outcome
            .gaps
            .iter()
            .map(|(lo, hi)| format!("{lo}..{hi}"))
            .collect(),
        gap_details: outcome
            .gap_outcomes
            .into_iter()
            .map(|g| DetectGap {
                low: g.low,
                high: g.high,
                issue_count: g.issue_count as u32,
                status: g.status,
                provider_series_id: g.provider_series_id,
                provider_series_name: g.provider_series_name,
                error: g.error,
            })
            .collect(),
        created,
        stale_ranges,
        uncovered_specials: outcome.uncovered_specials as u32,
        candidates: d.candidates,
        error: d.error,
    }
}

/// Compare the uncovered runs of every scanned provider.
fn agreement(results: &[DetectSourceResult]) -> Option<DetectAgreement> {
    let scanned: Vec<&DetectSourceResult> = results
        .iter()
        .filter(|r| r.status == SourceStatus::Scanned)
        .collect();
    if scanned.len() < 2 {
        return None;
    }
    let describe = |r: &DetectSourceResult| {
        if r.gap_details.is_empty() {
            "no split".to_owned()
        } else {
            r.gap_details
                .iter()
                .map(|g| {
                    if g.low == g.high {
                        format!("#{}", g.low)
                    } else {
                        format!("#{}–{}", g.low, g.high)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let first = &scanned[0].gaps;
    let agree = scanned.iter().all(|r| &r.gaps == first);
    let labels: Vec<&str> = scanned.iter().map(|r| r.source_label.as_str()).collect();
    let summary = if agree {
        if first.is_empty() {
            format!(
                "{} agree: no issues outside the matched series.",
                labels.join(" and ")
            )
        } else {
            format!(
                "{} agree: {} sit outside the matched series.",
                labels.join(" and "),
                describe(scanned[0])
            )
        }
    } else {
        let parts: Vec<String> = scanned
            .iter()
            .map(|r| format!("{} {}", r.source_label, describe(r)))
            .collect();
        format!(
            "Providers split this run differently ({}). Each provider routes only its own issues, so both mappings can coexist.",
            parts.join("; ")
        )
    };
    Some(DetectAgreement { agree, summary })
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

    fn scanned(label: &str, gaps: &[(&str, &str)]) -> DetectSourceResult {
        DetectSourceResult {
            source: label.to_lowercase(),
            source_label: label.into(),
            status: SourceStatus::Scanned,
            provider_series_id: Some("1".into()),
            provider_series_name: None,
            provider_series_year: None,
            provider_series_url: None,
            resolved_via: Some(LinkMethod::Linked),
            id_recorded: false,
            covered_count: 10,
            matched_local: 4,
            gaps: gaps.iter().map(|(l, h)| format!("{l}..{h}")).collect(),
            gap_details: gaps
                .iter()
                .map(|(l, h)| DetectGap {
                    low: (*l).into(),
                    high: (*h).into(),
                    issue_count: 2,
                    status: GapStatus::Mapped,
                    provider_series_id: None,
                    provider_series_name: None,
                    error: None,
                })
                .collect(),
            created: Vec::new(),
            stale_ranges: Vec::new(),
            uncovered_specials: 0,
            candidates: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn agreement_needs_two_scanned_providers_and_compares_gaps() {
        let one = vec![scanned("Metron", &[("600", "611")])];
        assert!(agreement(&one).is_none());

        let same = vec![
            scanned("Metron", &[("600", "611")]),
            scanned("GCD", &[("600", "611")]),
        ];
        let a = agreement(&same).unwrap();
        assert!(a.agree);
        assert!(a.summary.contains("#600–611"), "{}", a.summary);

        let differ = vec![
            scanned("Metron", &[("600", "611")]),
            scanned("GCD", &[("417", "611")]),
        ];
        let a = agreement(&differ).unwrap();
        assert!(!a.agree);
        assert!(a.summary.contains("GCD #417–611"), "{}", a.summary);
    }
}
