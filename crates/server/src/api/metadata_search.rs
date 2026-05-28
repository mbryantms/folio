//! `/series/{slug}/metadata/*` and per-issue variants — search-and-poll
//! surface that feeds the `<MetadataMatchDialog>` (M5).
//!
//! Two endpoint shapes per scope:
//! - `POST .../metadata/search` enqueues a SearchSeries / SearchIssue
//!   apalis job (coalescing per-entity in Redis) and returns the
//!   `run_id`.
//! - `GET .../metadata/candidates` polls the run row + per-candidate
//!   detail. `?run_id=...` pins a specific run; without it, the
//!   latest run for the scope/entity is returned.
//!
//! ACL: callers need read access to the parent library. Admin users
//! have unconditional access (the visibility helper already
//! short-circuits for them).

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, library_user_access, metadata_run, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::jobs::{metadata_apply, metadata_search};
use crate::metadata::apply::{self, ApplyArgs, ApplyMode};
use crate::metadata::diff::{self, DiffResp};
use crate::metadata::matcher::{IssueQueryFacts, SeriesQueryFacts};
use crate::metadata::orchestrator;
use crate::metadata::refresh::{self, RefreshOutcome, RefreshScope};
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(search_series))
        .routes(routes!(candidates_series))
        .routes(routes!(proposed_diff_series))
        .routes(routes!(composite_diff_series))
        .routes(routes!(composite_diff_issue))
        .routes(routes!(composite_apply_series))
        .routes(routes!(composite_apply_issue))
        .routes(routes!(apply_series))
        .routes(routes!(pause_series))
        .routes(routes!(resume_series))
        .routes(routes!(sync_status_series))
        .routes(routes!(search_issue))
        .routes(routes!(candidates_issue))
        .routes(routes!(proposed_diff_issue))
        .routes(routes!(apply_issue))
        .routes(routes!(refresh_library_metadata))
}

// ───────── response shapes ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SearchStartedResp {
    pub run_id: Uuid,
    /// `true` when an in-flight run for the same target was reused
    /// instead of enqueueing a fresh one — UI can swallow the "Started
    /// fetching" toast in this case.
    pub coalesced: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema, Clone)]
pub struct CandidateView {
    pub source: String,
    pub external_id: String,
    pub bucket: String,
    pub score: f32,
    pub score_breakdown: serde_json::Value,
    pub candidate: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CandidatesResp {
    pub run_id: Uuid,
    pub status: String,
    pub providers: Vec<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub items_total: i32,
    pub items_matched_high: i32,
    pub items_matched_medium: i32,
    pub items_matched_low: i32,
    pub error_summary: Option<String>,
    pub candidates: Vec<CandidateView>,
    /// Match-outcome classification (matching-accuracy-1.0 M8).
    /// `None` while the run is still searching; populated once it
    /// completes. Drives the MetadataMatchDialog state — one-click
    /// apply on `SingleGoodMatch`, warning copy on `SingleBadCover`,
    /// flat list on `MultiGood` / `MultiBadCover`, empty state on
    /// `NoMatches`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_outcome: Option<MatchOutcomeView>,
}

/// Discriminated view of the matcher's outcome classification.
/// Vocabulary mirrors [`crate::metadata::match_outcome::MatchOutcomeKind`]:
/// `single_good`, `multi_good`, `single_bad_cover`, `multi_bad_cover`,
/// `no_match`.
///
/// `top_hamming` is the top candidate's cover-pHash Hamming distance
/// when a phash pair was available, else `null`. `matched_via_alternate`
/// tells the UI whether the top match came from a variant (drives the
/// "via alternate cover" badge).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MatchOutcomeView {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_hamming: Option<u32>,
    pub matched_via_alternate: bool,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct CandidatesQuery {
    /// Pin a specific run; defaults to the latest run for the
    /// scope/entity.
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplyRequest {
    /// The run row produced by `POST .../metadata/search`. Required so
    /// the apply job can read the chosen candidate back from
    /// `metadata_run_candidate` rather than re-fetching from the
    /// provider.
    pub run_id: Uuid,
    /// 0-based rank from the orchestrator (lower = higher score).
    pub ordinal: i32,
    /// `fill_missing` (default) only writes fields that are currently
    /// empty; `replace_all` overwrites non-user fields. User-set
    /// fields stay sacred regardless unless `override_user_edits=true`
    /// (admin-only).
    #[serde(default = "default_fill_missing")]
    pub mode: ApplyMode,
    /// Pull + write the cover image. Defaults to true.
    #[serde(default = "default_true")]
    pub apply_cover: bool,
    /// `never` / `when_missing` (default) / `always`. Only applies to
    /// the primary cover; variants are always additive.
    #[serde(default = "default_when_missing")]
    pub cover_overwrite_policy: ApplyCoverPolicy,
    /// Bypass the user-precedence rule. Admin-only; non-admin callers
    /// get 403 if they request it.
    #[serde(default)]
    pub override_user_edits: bool,
    /// M5 preview-pane opt-in: when present, only the named fields
    /// (by `MetadataField::key()`) are applied; everything else is
    /// skipped. When absent, the legacy "apply every eligible field"
    /// behaviour applies (preserves backward compat for older
    /// clients).
    #[serde(default)]
    pub selected_fields: Option<Vec<String>>,
    /// M5 conflict-resolution: per-source list of external-ID rows
    /// where the user has opted to "Use theirs". The candidate's
    /// value replaces the user-set row for these sources. Other
    /// conflicts stay sacred.
    #[serde(default)]
    pub override_external_id_sources: Vec<String>,
}

fn default_fill_missing() -> ApplyMode {
    ApplyMode::FillMissing
}
fn default_true() -> bool {
    true
}
fn default_when_missing() -> ApplyCoverPolicy {
    ApplyCoverPolicy::WhenMissing
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyCoverPolicy {
    Never,
    WhenMissing,
    Always,
}

impl From<ApplyCoverPolicy> for crate::jobs::metadata_apply::CoverPolicy {
    fn from(p: ApplyCoverPolicy) -> Self {
        match p {
            ApplyCoverPolicy::Never => crate::jobs::metadata_apply::CoverPolicy::Never,
            ApplyCoverPolicy::WhenMissing => crate::jobs::metadata_apply::CoverPolicy::WhenMissing,
            ApplyCoverPolicy::Always => crate::jobs::metadata_apply::CoverPolicy::Always,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApplyAcceptedResp {
    pub run_id: Uuid,
    pub ordinal: i32,
    /// `queued` — the apply job is in flight; the candidate row's
    /// `applied_at` flips once it completes. The Runs / Review-Queue
    /// surfaces (M6) reflect the new state via the same polling
    /// endpoint.
    pub status: String,
}

// ───────── /series/{slug}/metadata/search ─────────

#[utoipa::path(
    operation_id = "metadata_search_series",    post,
    path = "/series/{slug}/metadata/search",
    params(("slug" = String, Path)),
    responses(
        (status = 202, body = SearchStartedResp),
        (status = 400, description = "no providers configured"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
        (status = 502, description = "queue error"),
    )
)]
#[handler]
pub async fn search_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(_ctx): Extension<RequestContext>,
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

    let facts = SeriesQueryFacts {
        name: s.name.clone(),
        year: s.year,
        publisher: s.publisher.clone(),
        volume: s.volume,
    };

    let providers = orchestrator::build_providers(&app.cfg(), app.jobs.redis.clone());
    if providers.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.no_providers",
            "no metadata providers are configured + enabled",
        );
    }
    let providers_listed: Vec<_> = providers.iter().map(|p| p.id()).collect();

    // Start a run row first so we have an id to reserve the slot
    // against. If the slot was already taken, roll back the run row
    // and surface the coalesced id.
    let new_run_id = match orchestrator::start_run(
        &app.db,
        orchestrator::StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: Some(s.id.to_string()),
            library_id: Some(s.library_id),
            triggered_by: Some(user.id),
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &providers_listed,
            query: metadata_search::series_stored_query(&facts),
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "metadata_search: start_run failed");
            return error(
                StatusCode::BAD_GATEWAY,
                "metadata.queue",
                "run insert failed",
            );
        }
    };

    let winner_run_id = match metadata_search::reserve_series_slot(&app, s.id, new_run_id).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "metadata_search: slot reservation failed");
            return error(StatusCode::BAD_GATEWAY, "metadata.queue", "redis error");
        }
    };

    if winner_run_id != new_run_id {
        // Existing run already in flight — discard the speculative
        // row we just inserted.
        let _ = metadata_run::Entity::delete_by_id(new_run_id)
            .exec(&app.db)
            .await;
        return (
            StatusCode::ACCEPTED,
            Json(SearchStartedResp {
                run_id: winner_run_id,
                coalesced: true,
            }),
        )
            .into_response();
    }

    use apalis::prelude::Storage;
    let mut storage = app.jobs.metadata_search_series_storage.clone();
    if let Err(e) = storage
        .push(metadata_search::SearchSeriesJob {
            run_id: new_run_id,
            series_id: s.id,
            library_id: Some(s.library_id),
            facts,
        })
        .await
    {
        tracing::error!(error = %e, "metadata_search: push to queue failed");
        let _ = orchestrator::fail_run(&app.db, new_run_id, "queue push failed").await;
        return error(
            StatusCode::BAD_GATEWAY,
            "metadata.queue",
            "queue push failed",
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(SearchStartedResp {
            run_id: new_run_id,
            coalesced: false,
        }),
    )
        .into_response()
}

// ───────── /series/{slug}/metadata/candidates ─────────

#[utoipa::path(
    operation_id = "metadata_candidates_series",    get,
    path = "/series/{slug}/metadata/candidates",
    params(("slug" = String, Path), CandidatesQuery),
    responses(
        (status = 200, body = CandidatesResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found / no run"),
    )
)]
#[handler]
pub async fn candidates_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(q): Query<CandidatesQuery>,
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

    let run = match q.run_id {
        Some(id) => match orchestrator::fetch_run(&app.db, id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "metadata.run_not_found",
                    "no such run",
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "metadata_search: fetch_run failed");
                return error(StatusCode::BAD_GATEWAY, "internal", "internal");
            }
        },
        None => {
            match latest_run_for_scope(&app, orchestrator::scope::SERIES, &s.id.to_string()).await {
                Some(r) => r,
                None => {
                    return error(
                        StatusCode::NOT_FOUND,
                        "metadata.run_not_found",
                        "no run yet for this series",
                    );
                }
            }
        }
    };

    // Guard cross-entity poking via ?run_id= — only return runs that
    // belong to *this* series.
    if run.scope != orchestrator::scope::SERIES
        || run.scope_entity_id.as_deref() != Some(s.id.to_string().as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    }
    Json(build_candidates_resp(&app, run).await).into_response()
}

// ───────── /series/{slug}/metadata/pause + resume + status ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncStatusResp {
    pub series_slug: String,
    pub paused: bool,
    pub last_metadata_sync_at: Option<String>,
    /// `external_ids` row count for this series (UI uses it to render
    /// "matched against 2 sources" without a second round-trip).
    pub linked_source_count: i64,
}

#[utoipa::path(
    operation_id = "metadata_sync_status_series",    get,
    path = "/series/{slug}/metadata/status",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = SyncStatusResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn sync_status_series(
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
    let linked = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(s.id.to_string()))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;
    Json(SyncStatusResp {
        series_slug: s.slug.clone(),
        paused: s.metadata_sync_paused,
        last_metadata_sync_at: s.last_metadata_sync_at.map(|t| t.to_rfc3339()),
        linked_source_count: linked,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "metadata_pause_series",    post,
    path = "/series/{slug}/metadata/pause",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = SyncStatusResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn pause_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
) -> Response {
    toggle_metadata_sync_paused(&app, &user, &ctx, &slug, true).await
}

#[utoipa::path(
    operation_id = "metadata_resume_series",    post,
    path = "/series/{slug}/metadata/resume",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = SyncStatusResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn resume_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
) -> Response {
    toggle_metadata_sync_paused(&app, &user, &ctx, &slug, false).await
}

async fn toggle_metadata_sync_paused(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    slug: &str,
    paused: bool,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(app, user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let was = s.metadata_sync_paused;
    let mut am: entity::series::ActiveModel = s.clone().into();
    am.metadata_sync_paused = sea_orm::Set(paused);
    am.updated_at = sea_orm::Set(chrono::Utc::now().fixed_offset());
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "metadata pause/resume update failed");
        return error(StatusCode::BAD_GATEWAY, "internal", "internal");
    }
    let action: &'static str = if paused {
        "admin.series.metadata_pause"
    } else {
        "admin.series.metadata_resume"
    };
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action,
            target_type: Some("series"),
            target_id: Some(s.id.to_string()),
            payload: serde_json::json!({ "was": was, "now": paused }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    let linked = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(s.id.to_string()))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;
    Json(SyncStatusResp {
        series_slug: s.slug.clone(),
        paused,
        last_metadata_sync_at: s.last_metadata_sync_at.map(|t| t.to_rfc3339()),
        linked_source_count: linked,
    })
    .into_response()
}

// ───────── per-issue ─────────

#[utoipa::path(
    operation_id = "metadata_search_issue",    post,
    path = "/series/{slug}/issues/{issue_slug}/metadata/search",
    params(("slug" = String, Path), ("issue_slug" = String, Path)),
    responses(
        (status = 202, body = SearchStartedResp),
        (status = 400, description = "no providers configured"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue not found"),
        (status = 502, description = "queue error"),
    )
)]
#[handler]
pub async fn search_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(_ctx): Extension<RequestContext>,
    Path((slug, issue_slug)): Path<(String, String)>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }

    let Some(issue_number) = i.number_raw.clone().filter(|s| !s.trim().is_empty()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.no_issue_number",
            "issue has no number_raw; can't search without it",
        );
    };

    let facts = IssueQueryFacts {
        series_name: s.name.clone(),
        series_year: s.year,
        publisher: s.publisher.clone(),
        volume: s.volume,
        issue_number,
        issue_year: i.year,
    };

    let providers = orchestrator::build_providers(&app.cfg(), app.jobs.redis.clone());
    if providers.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.no_providers",
            "no metadata providers are configured + enabled",
        );
    }
    let providers_listed: Vec<_> = providers.iter().map(|p| p.id()).collect();

    let series_external_ids = fetch_series_external_ids(&app, &s).await;

    let new_run_id = match orchestrator::start_run(
        &app.db,
        orchestrator::StartRunArgs {
            scope: orchestrator::scope::ISSUE,
            scope_entity_id: Some(i.id.clone()),
            library_id: Some(s.library_id),
            triggered_by: Some(user.id),
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &providers_listed,
            query: metadata_search::issue_stored_query(&facts),
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "metadata_search issue: start_run failed");
            return error(
                StatusCode::BAD_GATEWAY,
                "metadata.queue",
                "run insert failed",
            );
        }
    };

    let winner_run_id = match metadata_search::reserve_issue_slot(&app, &i.id, new_run_id).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "metadata_search issue: slot reservation failed");
            return error(StatusCode::BAD_GATEWAY, "metadata.queue", "redis error");
        }
    };
    if winner_run_id != new_run_id {
        let _ = metadata_run::Entity::delete_by_id(new_run_id)
            .exec(&app.db)
            .await;
        return (
            StatusCode::ACCEPTED,
            Json(SearchStartedResp {
                run_id: winner_run_id,
                coalesced: true,
            }),
        )
            .into_response();
    }

    use apalis::prelude::Storage;
    let mut storage = app.jobs.metadata_search_issue_storage.clone();
    if let Err(e) = storage
        .push(metadata_search::SearchIssueJob {
            run_id: new_run_id,
            issue_id: i.id.clone(),
            library_id: Some(s.library_id),
            facts,
            series_external_ids,
        })
        .await
    {
        tracing::error!(error = %e, "metadata_search issue: push failed");
        let _ = orchestrator::fail_run(&app.db, new_run_id, "queue push failed").await;
        return error(
            StatusCode::BAD_GATEWAY,
            "metadata.queue",
            "queue push failed",
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(SearchStartedResp {
            run_id: new_run_id,
            coalesced: false,
        }),
    )
        .into_response()
}

#[utoipa::path(
    operation_id = "metadata_candidates_issue",    get,
    path = "/series/{slug}/issues/{issue_slug}/metadata/candidates",
    params(("slug" = String, Path), ("issue_slug" = String, Path), CandidatesQuery),
    responses(
        (status = 200, body = CandidatesResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue / run not found"),
    )
)]
#[handler]
pub async fn candidates_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Path((slug, issue_slug)): Path<(String, String)>,
    Query(q): Query<CandidatesQuery>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let run = match q.run_id {
        Some(id) => match orchestrator::fetch_run(&app.db, id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "metadata.run_not_found",
                    "no such run",
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "metadata_search issue: fetch_run failed");
                return error(StatusCode::BAD_GATEWAY, "internal", "internal");
            }
        },
        None => match latest_run_for_scope(&app, orchestrator::scope::ISSUE, &i.id).await {
            Some(r) => r,
            None => {
                return error(
                    StatusCode::NOT_FOUND,
                    "metadata.run_not_found",
                    "no run yet for this issue",
                );
            }
        },
    };
    if run.scope != orchestrator::scope::ISSUE
        || run.scope_entity_id.as_deref() != Some(i.id.as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    }
    Json(build_candidates_resp(&app, run).await).into_response()
}

// ───────── /series/{slug}/metadata/proposed-diff ─────────

/// Diff request — mirrors the [`ApplyRequest`] shape so the client
/// can preview exactly what the apply would do. M5 preview pane.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ProposedDiffQuery {
    pub run_id: Uuid,
    pub ordinal: i32,
    #[serde(default = "default_fill_missing")]
    pub mode: ApplyMode,
    #[serde(default)]
    pub override_user_edits: bool,
}

fn make_diff_args(q: &ProposedDiffQuery) -> ApplyArgs {
    ApplyArgs {
        run_id: q.run_id,
        ordinal: q.ordinal,
        mode: q.mode,
        apply_cover: false, // diff doesn't preview cover bytes — keep it cheap
        cover_overwrite_policy: crate::metadata::writers::CoverOverwritePolicy::WhenMissing,
        override_user_edits: q.override_user_edits,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

fn map_diff_err(e: apply::ApplyError) -> Response {
    use apply::ApplyError;
    match e {
        ApplyError::CandidateNotFound { .. } => error(
            StatusCode::NOT_FOUND,
            "metadata.candidate_not_found",
            "candidate not found",
        ),
        ApplyError::SeriesGone => error(
            StatusCode::NOT_FOUND,
            "series.not_found",
            "series no longer exists",
        ),
        ApplyError::IssueGone => error(
            StatusCode::NOT_FOUND,
            "issue.not_found",
            "issue no longer exists",
        ),
        ApplyError::InvalidScope(msg) => {
            error(StatusCode::BAD_REQUEST, "metadata.invalid_scope", &msg)
        }
        ApplyError::Provider(_) => error(
            StatusCode::BAD_GATEWAY,
            "metadata.provider",
            "upstream provider error",
        ),
        ApplyError::Db(_) | ApplyError::Io(_) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    operation_id = "metadata_proposed_diff_series",
    get,
    path = "/series/{slug}/metadata/proposed-diff",
    params(("slug" = String, Path), ProposedDiffQuery),
    responses(
        (status = 200, body = DiffResp),
        (status = 400, description = "invalid run scope"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series / run / candidate not found"),
        (status = 502, description = "provider error"),
    )
)]
#[handler]
pub async fn proposed_diff_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(q): Query<ProposedDiffQuery>,
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
    // Sanity-check run scope before paying the provider round trip.
    let Some(run) = orchestrator::fetch_run(&app.db, q.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::SERIES
        || run.scope_entity_id.as_deref() != Some(s.id.to_string().as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this series",
        );
    }
    match diff::compute_series_diff(&app, make_diff_args(&q)).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => map_diff_err(e),
    }
}

// ───────── /series/{slug}/metadata/apply ─────────

#[utoipa::path(
    operation_id = "metadata_apply_series",    post,
    path = "/series/{slug}/metadata/apply",
    params(("slug" = String, Path)),
    request_body = ApplyRequest,
    responses(
        (status = 202, body = ApplyAcceptedResp),
        (status = 400, description = "candidate not found / no providers"),
        (status = 403, description = "library access denied / override_user_edits requires admin"),
        (status = 404, description = "series / run not found"),
        (status = 502, description = "queue error"),
    )
)]
#[handler]
pub async fn apply_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    axum::Json(req): axum::Json<ApplyRequest>,
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
    if req.override_user_edits && user.role != "admin" {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "override_user_edits requires admin",
        );
    }
    // Validate the run belongs to this series + the candidate exists.
    let Some(run) = orchestrator::fetch_run(&app.db, req.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::SERIES
        || run.scope_entity_id.as_deref() != Some(s.id.to_string().as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this series",
        );
    }
    if entity::metadata_run_candidate::Entity::find_by_id((req.run_id, req.ordinal))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.candidate_not_found",
            "no candidate with that ordinal",
        );
    }

    use apalis::prelude::Storage;
    let mut storage = app.jobs.metadata_apply_series_storage.clone();
    if let Err(e) = storage
        .push(metadata_apply::ApplySeriesJob {
            run_id: req.run_id,
            ordinal: req.ordinal,
            series_id: s.id,
            mode: req.mode,
            apply_cover: req.apply_cover,
            cover_overwrite_policy: req.cover_overwrite_policy.into(),
            override_user_edits: req.override_user_edits,
            actor_id: Some(user.id),
            actor_ip: ctx.ip_string(),
            actor_ua: ctx.user_agent.clone(),
            selected_fields: req
                .selected_fields
                .clone()
                .map(std::collections::HashSet::from_iter),
            override_external_id_sources: req
                .override_external_id_sources
                .iter()
                .cloned()
                .collect(),
            is_auto: false,
        })
        .await
    {
        tracing::error!(error = %e, "metadata_apply series: push failed");
        return error(
            StatusCode::BAD_GATEWAY,
            "metadata.queue",
            "queue push failed",
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(ApplyAcceptedResp {
            run_id: req.run_id,
            ordinal: req.ordinal,
            status: "queued".into(),
        }),
    )
        .into_response()
}

// ───────── /series/{slug}/issues/{issue_slug}/metadata/proposed-diff ─────────

#[utoipa::path(
    operation_id = "metadata_proposed_diff_issue",
    get,
    path = "/series/{slug}/issues/{issue_slug}/metadata/proposed-diff",
    params(
        ("slug" = String, Path),
        ("issue_slug" = String, Path),
        ProposedDiffQuery,
    ),
    responses(
        (status = 200, body = DiffResp),
        (status = 400, description = "invalid run scope"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue / run / candidate not found"),
        (status = 502, description = "provider error"),
    )
)]
#[handler]
pub async fn proposed_diff_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Path((slug, issue_slug)): Path<(String, String)>,
    Query(q): Query<ProposedDiffQuery>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let Some(run) = orchestrator::fetch_run(&app.db, q.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::ISSUE
        || run.scope_entity_id.as_deref() != Some(i.id.as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this issue",
        );
    }
    match diff::compute_issue_diff(&app, make_diff_args(&q)).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => map_diff_err(e),
    }
}

// ───────── composite (multi-provider) diff ─────────

/// Query for the composite compare view. `include` is a comma-separated
/// list of candidate ordinals (`?include=0,2`); `serde_urlencoded`
/// (axum's `Query`) can't decode repeated keys into a `Vec`, so this is
/// a single string we split ourselves. Omitting it falls back to the
/// best (lowest-ordinal) candidate per provider.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct CompositeDiffQuery {
    pub run_id: Uuid,
    #[serde(default = "default_fill_missing")]
    pub mode: ApplyMode,
    #[serde(default)]
    pub override_user_edits: bool,
    #[serde(default)]
    pub include: Option<String>,
}

/// Parse the comma-separated `include` param into candidate ordinals,
/// dropping any non-integer token.
fn parse_include(raw: &Option<String>) -> Vec<i32> {
    raw.as_deref()
        .map(|s| {
            s.split(',')
                .filter_map(|t| t.trim().parse::<i32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[utoipa::path(
    operation_id = "metadata_composite_diff_series",
    get,
    path = "/series/{slug}/metadata/composite-diff",
    params(("slug" = String, Path), CompositeDiffQuery),
    responses(
        (status = 200, body = crate::metadata::composite::CompositeDiffResp),
        (status = 400, description = "invalid run scope"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series / run not found"),
    )
)]
#[handler]
pub async fn composite_diff_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(q): Query<CompositeDiffQuery>,
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
    let Some(run) = orchestrator::fetch_run(&app.db, q.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::SERIES
        || run.scope_entity_id.as_deref() != Some(s.id.to_string().as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this series",
        );
    }
    match crate::metadata::composite::compute_composite_diff(
        &app,
        q.run_id,
        q.mode,
        q.override_user_edits,
        &parse_include(&q.include),
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => map_diff_err(e),
    }
}

#[utoipa::path(
    operation_id = "metadata_composite_diff_issue",
    get,
    path = "/series/{slug}/issues/{issue_slug}/metadata/composite-diff",
    params(("slug" = String, Path), ("issue_slug" = String, Path), CompositeDiffQuery),
    responses(
        (status = 200, body = crate::metadata::composite::CompositeDiffResp),
        (status = 400, description = "invalid run scope"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue / run not found"),
    )
)]
#[handler]
pub async fn composite_diff_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Path((slug, issue_slug)): Path<(String, String)>,
    Query(q): Query<CompositeDiffQuery>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let Some(run) = orchestrator::fetch_run(&app.db, q.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::ISSUE
        || run.scope_entity_id.as_deref() != Some(i.id.as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this issue",
        );
    }
    match crate::metadata::composite::compute_composite_diff(
        &app,
        q.run_id,
        q.mode,
        q.override_user_edits,
        &parse_include(&q.include),
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => map_diff_err(e),
    }
}

// ───────── /series/{slug}/issues/{issue_slug}/metadata/apply ─────────

#[utoipa::path(
    operation_id = "metadata_apply_issue",    post,
    path = "/series/{slug}/issues/{issue_slug}/metadata/apply",
    params(("slug" = String, Path), ("issue_slug" = String, Path)),
    request_body = ApplyRequest,
    responses(
        (status = 202, body = ApplyAcceptedResp),
        (status = 400, description = "candidate not found / no providers"),
        (status = 403, description = "library access denied / override_user_edits requires admin"),
        (status = 404, description = "issue / run not found"),
        (status = 502, description = "queue error"),
    )
)]
#[handler]
pub async fn apply_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug)): Path<(String, String)>,
    axum::Json(req): axum::Json<ApplyRequest>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    if req.override_user_edits && user.role != "admin" {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "override_user_edits requires admin",
        );
    }
    let Some(run) = orchestrator::fetch_run(&app.db, req.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::ISSUE
        || run.scope_entity_id.as_deref() != Some(i.id.as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this issue",
        );
    }
    if entity::metadata_run_candidate::Entity::find_by_id((req.run_id, req.ordinal))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.candidate_not_found",
            "no candidate with that ordinal",
        );
    }

    use apalis::prelude::Storage;
    let mut storage = app.jobs.metadata_apply_issue_storage.clone();
    if let Err(e) = storage
        .push(metadata_apply::ApplyIssueJob {
            run_id: req.run_id,
            ordinal: req.ordinal,
            issue_id: i.id.clone(),
            mode: req.mode,
            apply_cover: req.apply_cover,
            cover_overwrite_policy: req.cover_overwrite_policy.into(),
            override_user_edits: req.override_user_edits,
            actor_id: Some(user.id),
            actor_ip: ctx.ip_string(),
            actor_ua: ctx.user_agent.clone(),
            selected_fields: req
                .selected_fields
                .clone()
                .map(std::collections::HashSet::from_iter),
            override_external_id_sources: req
                .override_external_id_sources
                .iter()
                .cloned()
                .collect(),
            is_auto: false,
        })
        .await
    {
        tracing::error!(error = %e, "metadata_apply issue: push failed");
        return error(
            StatusCode::BAD_GATEWAY,
            "metadata.queue",
            "queue push failed",
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(ApplyAcceptedResp {
            run_id: req.run_id,
            ordinal: req.ordinal,
            status: "queued".into(),
        }),
    )
        .into_response()
}

// ───────── /libraries/{slug}/metadata/refresh ─────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct RefreshLibraryQuery {
    /// `unmatched` | `stale` | `all` | `recent` (default `stale`).
    /// `unmatched` is the cheapest scope and the right default for
    /// "I just added a library, get me caught up". `stale` is what
    /// the weekly cron uses. `all` is the operator escape hatch.
    /// `recent` mirrors the Mylar "last N days" window.
    #[serde(default = "default_refresh_scope")]
    pub scope: String,
}

fn default_refresh_scope() -> String {
    "stale".to_owned()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshLibraryResp {
    pub library_id: Uuid,
    pub scope: String,
    pub series_eligible: usize,
    pub jobs_enqueued: usize,
    pub jobs_coalesced: usize,
    pub jobs_failed: usize,
}

#[utoipa::path(
    operation_id = "metadata_refresh_library",
    post,
    path = "/libraries/{slug}/metadata/refresh",
    params(("slug" = String, Path), RefreshLibraryQuery),
    responses(
        (status = 202, body = RefreshLibraryResp),
        (status = 400, description = "unknown scope"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn refresh_library_metadata(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Query(q): Query<RefreshLibraryQuery>,
) -> Response {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, lib.id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let Ok(scope) = q.scope.parse::<RefreshScope>() else {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_scope",
            "scope must be one of: unmatched, stale, all, recent",
        );
    };
    match refresh::fan_out_scope(&app, lib.id, scope, orchestrator::trigger_kind::BULK_ACTION).await
    {
        Ok(RefreshOutcome {
            series_eligible,
            jobs_enqueued,
            jobs_coalesced,
            jobs_failed,
        }) => (
            StatusCode::ACCEPTED,
            Json(RefreshLibraryResp {
                library_id: lib.id,
                scope: scope.as_str().to_owned(),
                series_eligible,
                jobs_enqueued,
                jobs_coalesced,
                jobs_failed,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, library_id = %lib.id, "metadata refresh fan-out failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

// ───────── helpers ─────────

// ───────── composite (multi-provider) apply ─────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CompositeFieldSource {
    /// `MetadataField::key()`.
    pub field: String,
    /// Candidate `ordinal` (unique within the run) whose value wins this
    /// field.
    pub ordinal: i32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CompositeApplyRequest {
    pub run_id: Uuid,
    /// Per-field candidate picks. A field absent here is not applied.
    pub field_sources: Vec<CompositeFieldSource>,
    /// The candidate `ordinal`s that contribute. Their `applied_at` is
    /// flipped.
    pub included: Vec<i32>,
    #[serde(default = "default_fill_missing")]
    pub mode: ApplyMode,
    #[serde(default = "default_true")]
    pub apply_cover: bool,
    #[serde(default = "default_when_missing")]
    pub cover_overwrite_policy: ApplyCoverPolicy,
    #[serde(default)]
    pub override_user_edits: bool,
    #[serde(default)]
    pub override_external_id_sources: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CompositeApplyResp {
    pub run_id: Uuid,
    pub status: String,
    pub applied_fields: Vec<String>,
    pub variants_written: u32,
}

fn cover_policy_to_writers(p: ApplyCoverPolicy) -> crate::metadata::writers::CoverOverwritePolicy {
    use crate::metadata::writers::CoverOverwritePolicy as W;
    match p {
        ApplyCoverPolicy::Never => W::Never,
        ApplyCoverPolicy::WhenMissing => W::WhenMissing,
        ApplyCoverPolicy::Always => W::Always,
    }
}

/// Translate a [`CompositeApplyRequest`] into the engine's
/// [`crate::metadata::composite::CompositeApplyArgs`]. Unknown source
/// tokens are dropped (validated against the run elsewhere).
fn make_composite_args(
    req: &CompositeApplyRequest,
    actor_id: Option<Uuid>,
) -> crate::metadata::composite::CompositeApplyArgs {
    let field_sources = req
        .field_sources
        .iter()
        .map(|fs| (fs.field.clone(), fs.ordinal))
        .collect();
    crate::metadata::composite::CompositeApplyArgs {
        run_id: req.run_id,
        field_sources,
        included: req.included.clone(),
        mode: req.mode,
        apply_cover: req.apply_cover,
        cover_overwrite_policy: cover_policy_to_writers(req.cover_overwrite_policy),
        override_user_edits: req.override_user_edits,
        override_external_id_sources: req.override_external_id_sources.iter().cloned().collect(),
        actor_id,
    }
}

async fn audit_composite(
    app: &AppState,
    ctx: &RequestContext,
    actor_id: Uuid,
    scope: &'static str,
    entity_id: String,
    req: &CompositeApplyRequest,
) {
    let action = if scope == "series" {
        "admin.series.metadata_composite_apply"
    } else {
        "admin.issue.metadata_composite_apply"
    };
    let per_field: Vec<serde_json::Value> = req
        .field_sources
        .iter()
        .map(|fs| serde_json::json!({ "field": fs.field, "ordinal": fs.ordinal }))
        .collect();
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id,
            action,
            target_type: Some(scope),
            target_id: Some(entity_id),
            payload: serde_json::json!({
                "run_id": req.run_id,
                "per_field_sources": per_field,
                "override_user_edits": req.override_user_edits,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
}

#[utoipa::path(
    operation_id = "metadata_composite_apply_series",
    post,
    path = "/series/{slug}/metadata/composite-apply",
    params(("slug" = String, Path)),
    request_body = CompositeApplyRequest,
    responses(
        (status = 200, body = CompositeApplyResp),
        (status = 403, description = "library access denied / override requires admin"),
        (status = 404, description = "series / run not found"),
    )
)]
#[handler]
pub async fn composite_apply_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    axum::Json(req): axum::Json<CompositeApplyRequest>,
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
    if req.override_user_edits && user.role != "admin" {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "override_user_edits requires admin",
        );
    }
    let Some(run) = orchestrator::fetch_run(&app.db, req.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::SERIES
        || run.scope_entity_id.as_deref() != Some(s.id.to_string().as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this series",
        );
    }
    match crate::metadata::composite::apply_composite(
        &app,
        make_composite_args(&req, Some(user.id)),
    )
    .await
    {
        Ok(outcome) => {
            audit_composite(&app, &ctx, user.id, "series", s.id.to_string(), &req).await;
            Json(CompositeApplyResp {
                run_id: req.run_id,
                status: "applied".into(),
                applied_fields: outcome.applied_fields,
                variants_written: outcome.variants_written,
            })
            .into_response()
        }
        Err(e) => map_diff_err(e),
    }
}

#[utoipa::path(
    operation_id = "metadata_composite_apply_issue",
    post,
    path = "/series/{slug}/issues/{issue_slug}/metadata/composite-apply",
    params(("slug" = String, Path), ("issue_slug" = String, Path)),
    request_body = CompositeApplyRequest,
    responses(
        (status = 200, body = CompositeApplyResp),
        (status = 403, description = "library access denied / override requires admin"),
        (status = 404, description = "issue / run not found"),
    )
)]
#[handler]
pub async fn composite_apply_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug)): Path<(String, String)>,
    axum::Json(req): axum::Json<CompositeApplyRequest>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    if req.override_user_edits && user.role != "admin" {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "override_user_edits requires admin",
        );
    }
    let Some(run) = orchestrator::fetch_run(&app.db, req.run_id)
        .await
        .ok()
        .flatten()
    else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "no such run",
        );
    };
    if run.scope != orchestrator::scope::ISSUE
        || run.scope_entity_id.as_deref() != Some(i.id.as_str())
    {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.run_not_found",
            "run does not belong to this issue",
        );
    }
    match crate::metadata::composite::apply_composite(
        &app,
        make_composite_args(&req, Some(user.id)),
    )
    .await
    {
        Ok(outcome) => {
            audit_composite(&app, &ctx, user.id, "issue", i.id.clone(), &req).await;
            Json(CompositeApplyResp {
                run_id: req.run_id,
                status: "applied".into(),
                applied_fields: outcome.applied_fields,
                variants_written: outcome.variants_written,
            })
            .into_response()
        }
        Err(e) => map_diff_err(e),
    }
}

async fn user_can_see_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    let row = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .unwrap_or(None);
    row.is_some()
}

async fn find_series_issue(
    app: &AppState,
    series_slug: &str,
    issue_slug: &str,
) -> Option<(series::Model, issue::Model)> {
    let s = crate::api::series::find_by_slug(&app.db, series_slug)
        .await
        .ok()?;
    let i = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::Slug.eq(issue_slug))
        .one(&app.db)
        .await
        .ok()
        .flatten()?;
    Some((s, i))
}

async fn latest_run_for_scope(
    app: &AppState,
    scope: &str,
    entity_id: &str,
) -> Option<metadata_run::Model> {
    metadata_run::Entity::find()
        .filter(metadata_run::Column::Scope.eq(scope))
        .filter(metadata_run::Column::ScopeEntityId.eq(entity_id))
        .order_by_desc(metadata_run::Column::StartedAt)
        .one(&app.db)
        .await
        .ok()
        .flatten()
}

async fn build_candidates_resp(app: &AppState, run: metadata_run::Model) -> CandidatesResp {
    let rows = orchestrator::fetch_candidates(&app.db, run.id)
        .await
        .unwrap_or_default();

    // M8: derive the outcome classification once the run is finished.
    // While the run is still in `queued` / `searching` we emit
    // `None` so the dialog renders its in-flight state instead of
    // a misleading "no match" snapshot.
    let match_outcome = if run.status == orchestrator::status::COMPLETED {
        Some(build_match_outcome_view(&rows))
    } else {
        None
    };

    let candidates = rows
        .into_iter()
        .map(|r| CandidateView {
            source: r.source,
            external_id: r.external_id,
            bucket: r.bucket,
            score: r.score,
            score_breakdown: r.score_breakdown,
            candidate: r.candidate,
        })
        .collect();
    CandidatesResp {
        run_id: run.id,
        status: run.status,
        providers: run.providers,
        started_at: run.started_at.to_rfc3339(),
        finished_at: run.finished_at.map(|t| t.to_rfc3339()),
        items_total: run.items_total,
        items_matched_high: run.items_matched_high,
        items_matched_medium: run.items_matched_medium,
        items_matched_low: run.items_matched_low,
        error_summary: run.error_summary,
        candidates,
        match_outcome,
    }
}

/// Classify the run's ranked-candidate list into a [`MatchOutcomeView`].
/// Reads `top_hamming` + `matched_via_alternate` from the per-row
/// `score_breakdown` JSON (populated by the orchestrator in M4 + M5).
///
/// Matching-accuracy-1.0 M8.
fn build_match_outcome_view(rows: &[entity::metadata_run_candidate::Model]) -> MatchOutcomeView {
    let kind = match (rows.len(), rows.first().map(|r| r.bucket.as_str())) {
        (0, _) => "no_match",
        (1, Some("high")) => "single_good",
        (1, _) => "single_bad_cover",
        (_, Some("high")) => "multi_good",
        _ => "multi_bad_cover",
    };
    let top_row = rows.first();
    let top_hamming = top_row
        .and_then(|r| r.score_breakdown.get("cover_hamming"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let matched_via_alternate = top_row
        .and_then(|r| r.score_breakdown.get("matched_via_alternate"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    MatchOutcomeView {
        kind: kind.to_owned(),
        top_hamming,
        matched_via_alternate,
    }
}

async fn fetch_series_external_ids(
    app: &AppState,
    s: &series::Model,
) -> Vec<(crate::metadata::identifier::Source, String)> {
    use entity::external_id;
    use std::str::FromStr;
    external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.eq(s.id.to_string()))
        .all(&app.db)
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            crate::metadata::identifier::Source::from_str(&row.source)
                .ok()
                .map(|s| (s, row.external_id))
        })
        .collect()
}
