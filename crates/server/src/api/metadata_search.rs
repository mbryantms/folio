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
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::jobs::{metadata_apply, metadata_search};
use crate::metadata::apply::ApplyMode;
use crate::metadata::matcher::{IssueQueryFacts, SeriesQueryFacts};
use crate::metadata::orchestrator;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(search_series))
        .routes(routes!(candidates_series))
        .routes(routes!(apply_series))
        .routes(routes!(search_issue))
        .routes(routes!(candidates_issue))
        .routes(routes!(apply_issue))
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
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
            return error(StatusCode::BAD_GATEWAY, "metadata.queue", "run insert failed");
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
        return error(StatusCode::BAD_GATEWAY, "metadata.queue", "queue push failed");
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
    }

    let run = match q.run_id {
        Some(id) => match orchestrator::fetch_run(&app.db, id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return error(StatusCode::NOT_FOUND, "metadata.run_not_found", "no such run");
            }
            Err(e) => {
                tracing::error!(error = %e, "metadata_search: fetch_run failed");
                return error(StatusCode::BAD_GATEWAY, "internal", "internal");
            }
        },
        None => match latest_run_for_scope(&app, orchestrator::scope::SERIES, &s.id.to_string()).await {
            Some(r) => r,
            None => {
                return error(
                    StatusCode::NOT_FOUND,
                    "metadata.run_not_found",
                    "no run yet for this series",
                );
            }
        },
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
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
            return error(StatusCode::BAD_GATEWAY, "metadata.queue", "run insert failed");
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
        return error(StatusCode::BAD_GATEWAY, "metadata.queue", "queue push failed");
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
    }
    let run = match q.run_id {
        Some(id) => match orchestrator::fetch_run(&app.db, id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return error(StatusCode::NOT_FOUND, "metadata.run_not_found", "no such run");
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
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
        return error(StatusCode::NOT_FOUND, "metadata.run_not_found", "no such run");
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
        })
        .await
    {
        tracing::error!(error = %e, "metadata_apply series: push failed");
        return error(StatusCode::BAD_GATEWAY, "metadata.queue", "queue push failed");
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
        return error(StatusCode::FORBIDDEN, "auth.forbidden", "library access denied");
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
        return error(StatusCode::NOT_FOUND, "metadata.run_not_found", "no such run");
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
        })
        .await
    {
        tracing::error!(error = %e, "metadata_apply issue: push failed");
        return error(StatusCode::BAD_GATEWAY, "metadata.queue", "queue push failed");
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

// ───────── helpers ─────────

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
