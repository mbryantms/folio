//! `/admin/metadata/*` — operator surface for the metadata-providers
//! integration (metadata-providers-1.0).
//!
//! M1 ships the two endpoints needed before any other surface can light
//! up:
//! - `GET /admin/metadata/providers` — lists configured providers,
//!   whether each is enabled, and the current Redis-backed quota
//!   snapshot.
//! - `POST /admin/metadata/providers/{id}/test` — runs `health_check`
//!   against the provider; audit-logged as `admin.metadata.providers.test`.
//!
//! Both routes are gated by [`RequireAdmin`]. M5+ add the Dashboard
//! and Runs tabs on top of the same module.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{audit_log, issue, library, metadata_run, metadata_run_candidate, series};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::jobs::backfill::{self, BackfillKind};
use crate::metadata::comicvine::ComicVineClient;
use crate::metadata::identifier::Source;
use crate::metadata::metron::MetronClient;
use crate::metadata::provider::{MetadataProvider, ProviderError};
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_providers))
        .routes(routes!(test_provider))
        .routes(routes!(dashboard))
        .routes(routes!(match_quality))
        .routes(routes!(list_runs))
        .routes(routes!(get_run))
        .routes(routes!(recent_applies))
        .routes(routes!(run_phash_backfill))
        .routes(routes!(run_variant_cover_backfill))
        .routes(routes!(list_auto_synced))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderView {
    /// Stable identifier — `"comicvine"` | `"metron"` (M2).
    pub id: String,
    pub label: String,
    /// `true` when an API key / credentials are set AND the master
    /// `metadata.<provider>.enabled` toggle is on.
    pub enabled: bool,
    /// `true` when the credential is set but the master toggle is off
    /// — UI surfaces a "Enable to test" hint in that state.
    pub configured: bool,
    pub quota: Option<QuotaView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QuotaView {
    pub remaining_hour: Option<u32>,
    pub remaining_day: Option<u32>,
    pub seconds_until_reset: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProvidersListResp {
    pub providers: Vec<ProviderView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TestProviderResp {
    pub ok: bool,
    pub quota: QuotaView,
    pub duration_ms: u64,
}

#[utoipa::path(
    operation_id = "admin_metadata_providers_list",    get,
    path = "/admin/metadata/providers",
    responses(
        (status = 200, body = ProvidersListResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list_providers(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let cfg = app.cfg();
    let mut providers = Vec::new();

    let cv_key_set = cfg
        .comicvine_api_key
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let cv_enabled = cfg.comicvine_enabled && cv_key_set;
    let cv_quota = if cv_key_set {
        comicvine_client(&app)
            .quota()
            .await
            .ok()
            .map(snapshot_to_view)
    } else {
        None
    };
    providers.push(ProviderView {
        id: Source::ComicVine.as_str().to_owned(),
        label: Source::ComicVine.label().to_owned(),
        enabled: cv_enabled,
        configured: cv_key_set,
        quota: cv_quota,
    });

    let metron_set = cfg
        .metron_username
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        && cfg
            .metron_password
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    let metron_enabled = cfg.metron_enabled && metron_set;
    let metron_quota = if metron_set {
        metron_client(&app).quota().await.ok().map(snapshot_to_view)
    } else {
        None
    };
    providers.push(ProviderView {
        id: Source::Metron.as_str().to_owned(),
        label: Source::Metron.label().to_owned(),
        enabled: metron_enabled,
        configured: metron_set,
        quota: metron_quota,
    });

    Json(ProvidersListResp { providers }).into_response()
}

#[utoipa::path(
    operation_id = "admin_metadata_providers_test",    post,
    path = "/admin/metadata/providers/{id}/test",
    params(
        ("id" = String, Path, description = "Provider id (`comicvine` | `metron`)"),
    ),
    responses(
        (status = 200, body = TestProviderResp),
        (status = 400, description = "credentials missing"),
        (status = 403, description = "admin only"),
        (status = 404, description = "unknown provider"),
        (status = 409, description = "provider disabled"),
        (status = 502, description = "provider responded with an error"),
    )
)]
#[handler]
pub async fn test_provider(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Response {
    let Ok(source) = id.parse::<Source>() else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.unknown_provider",
            "unknown provider id",
        );
    };
    let cfg = app.cfg();

    let result = match source {
        Source::ComicVine => {
            let Some(key) = cfg
                .comicvine_api_key
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "metadata.no_credentials",
                    "set the ComicVine API key before testing",
                );
            };
            if !cfg.comicvine_enabled {
                return error(
                    StatusCode::CONFLICT,
                    "metadata.disabled",
                    "ComicVine integration is disabled; enable it before testing",
                );
            }
            let _ = key; // value already loaded into the client below
            let client = comicvine_client(&app);
            let started = std::time::Instant::now();
            let outcome = client.health_check().await;
            (started.elapsed().as_millis() as u64, outcome)
        }
        Source::Metron => {
            let username_set = cfg
                .metron_username
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let password_set = cfg
                .metron_password
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !(username_set && password_set) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "metadata.no_credentials",
                    "set the Metron username and password before testing",
                );
            }
            if !cfg.metron_enabled {
                return error(
                    StatusCode::CONFLICT,
                    "metadata.disabled",
                    "Metron integration is disabled; enable it before testing",
                );
            }
            let client = metron_client(&app);
            let started = std::time::Instant::now();
            let outcome = client.health_check().await;
            (started.elapsed().as_millis() as u64, outcome)
        }
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                "metadata.provider_not_supported",
                "this provider isn't supported yet",
            );
        }
    };
    let (duration_ms, outcome) = result;

    let (status_code, payload, body): (StatusCode, serde_json::Value, Response) = match outcome {
        Ok(snap) => {
            let view = snapshot_to_view(snap);
            let body = Json(TestProviderResp {
                ok: true,
                quota: view.clone(),
                duration_ms,
            })
            .into_response();
            (
                StatusCode::OK,
                serde_json::json!({
                    "ok": true,
                    "duration_ms": duration_ms,
                    "quota": view,
                }),
                body,
            )
        }
        Err(e) => {
            let (status, code) = classify(&e);
            let payload = serde_json::json!({
                "ok": false,
                "duration_ms": duration_ms,
                "error": e.to_string(),
            });
            (status, payload, error(status, code, &e.to_string()))
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.metadata.providers.test",
            target_type: Some("metadata_provider"),
            target_id: Some(source.as_str().to_owned()),
            payload,
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let _ = status_code; // status already baked into `body`
    body
}

fn classify(err: &ProviderError) -> (StatusCode, &'static str) {
    match err {
        ProviderError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "metadata.unauthorized"),
        ProviderError::QuotaExceeded { .. } => {
            (StatusCode::TOO_MANY_REQUESTS, "metadata.quota_exceeded")
        }
        ProviderError::NotFound(_) => (StatusCode::NOT_FOUND, "metadata.not_found"),
        ProviderError::Transport(_) => (StatusCode::BAD_GATEWAY, "metadata.transport"),
        ProviderError::InvalidResponse(_) => (StatusCode::BAD_GATEWAY, "metadata.invalid_response"),
        ProviderError::Upstream(_) => (StatusCode::BAD_GATEWAY, "metadata.upstream"),
    }
}

fn snapshot_to_view(snap: crate::metadata::provider::QuotaSnapshot) -> QuotaView {
    QuotaView {
        remaining_hour: snap.remaining_hour,
        remaining_day: snap.remaining_day,
        seconds_until_reset: snap.seconds_until_reset,
    }
}

fn comicvine_client(app: &AppState) -> ComicVineClient {
    let key = app.cfg().comicvine_api_key.clone().unwrap_or_default();
    ComicVineClient::new(key, app.jobs.redis.clone())
}

fn metron_client(app: &AppState) -> MetronClient {
    let cfg = app.cfg();
    let username = cfg.metron_username.clone().unwrap_or_default();
    let password = cfg.metron_password.clone().unwrap_or_default();
    MetronClient::new(&username, &password, app.jobs.redis.clone())
}

impl Clone for QuotaView {
    fn clone(&self) -> Self {
        Self {
            remaining_hour: self.remaining_hour,
            remaining_day: self.remaining_day,
            seconds_until_reset: self.seconds_until_reset,
        }
    }
}

// ───────── GET /admin/metadata/dashboard ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardResp {
    /// Total series rows the operator has — denominator for the
    /// "matched / unmatched" headline.
    pub series_total: i64,
    /// Series with at least one row in `external_ids` from a
    /// configured provider source.
    pub series_matched: i64,
    /// `series_total - series_matched` (precomputed so the UI can
    /// render directly).
    pub series_unmatched: i64,
    /// Count of successful `metadata_apply` audit rows in the last
    /// 7 days.
    pub applies_last_7_days: i64,
    /// Per-provider quota snapshots — only populated when the
    /// provider is configured + enabled.
    pub providers: Vec<ProviderView>,
}

#[utoipa::path(
    operation_id = "admin_metadata_dashboard",    get,
    path = "/admin/metadata/dashboard",
    responses(
        (status = 200, body = DashboardResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn dashboard(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let cfg = app.cfg();

    let series_total = series::Entity::find()
        .filter(series::Column::RemovedAt.is_null())
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;

    let series_matched = matched_series_count(&app).await;
    let series_unmatched = (series_total - series_matched).max(0);

    let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let applies_last_7_days = audit_log::Entity::find()
        .filter(audit_log::Column::Action.is_in([
            "admin.series.metadata_apply",
            "admin.series.metadata_apply_force",
            "admin.issue.metadata_apply",
            "admin.issue.metadata_apply_force",
        ]))
        .filter(audit_log::Column::CreatedAt.gte(seven_days_ago.fixed_offset()))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;

    // Reuse `list_providers`' provider-view builder for consistency.
    let mut providers = Vec::new();
    let cv_key_set = cfg
        .comicvine_api_key
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let cv_enabled = cfg.comicvine_enabled && cv_key_set;
    let cv_quota = if cv_key_set {
        comicvine_client(&app)
            .quota()
            .await
            .ok()
            .map(snapshot_to_view)
    } else {
        None
    };
    providers.push(ProviderView {
        id: Source::ComicVine.as_str().to_owned(),
        label: Source::ComicVine.label().to_owned(),
        enabled: cv_enabled,
        configured: cv_key_set,
        quota: cv_quota,
    });
    let metron_set = cfg
        .metron_username
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        && cfg
            .metron_password
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    let metron_enabled = cfg.metron_enabled && metron_set;
    let metron_quota = if metron_set {
        metron_client(&app).quota().await.ok().map(snapshot_to_view)
    } else {
        None
    };
    providers.push(ProviderView {
        id: Source::Metron.as_str().to_owned(),
        label: Source::Metron.label().to_owned(),
        enabled: metron_enabled,
        configured: metron_set,
        quota: metron_quota,
    });

    Json(DashboardResp {
        series_total,
        series_matched,
        series_unmatched,
        applies_last_7_days,
        providers,
    })
    .into_response()
}

async fn matched_series_count(app: &AppState) -> i64 {
    // SeaORM doesn't model "exists in external_ids" naively; raw SQL
    // is the cleanest way to count distinct series with at least one
    // provider-source identifier.
    #[derive(FromQueryResult)]
    struct Count {
        c: i64,
    }
    let stmt = Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        r#"
        SELECT COUNT(DISTINCT s.id)::bigint AS c
        FROM series s
        WHERE s.removed_at IS NULL
          AND EXISTS (
            SELECT 1 FROM external_ids e
            WHERE e.entity_type = 'series'
              AND e.entity_id = s.id::text
              AND e.source IN ('comicvine','metron','gcd','marvel','locg')
          )
        "#
        .to_owned(),
    );
    Count::find_by_statement(stmt)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|r| r.c)
        .unwrap_or(0)
}

// ───────── GET /admin/metadata/match-quality ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MatchQualityWindow {
    /// `single_good | multi_good | single_bad_cover | multi_bad_cover | no_match`
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MatchQualityResp {
    /// Distribution of outcomes within the trailing 7 days. The
    /// shape mirrors what the M8 dialog will use, so the dashboard
    /// can speak the same vocabulary once cover-decides ships in M4.
    pub last_7d: Vec<MatchQualityWindow>,
    pub last_28d: Vec<MatchQualityWindow>,
    /// Total rows in each window — denominator for any percentage UI.
    pub total_7d: i64,
    pub total_28d: i64,
}

#[utoipa::path(
    operation_id = "admin_metadata_match_quality",
    get,
    path = "/admin/metadata/match-quality",
    responses(
        (status = 200, body = MatchQualityResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn match_quality(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let now = chrono::Utc::now();
    let cutoff_7d = now - chrono::Duration::days(7);
    let cutoff_28d = now - chrono::Duration::days(28);

    let last_7d = match outcome_distribution(&app, cutoff_7d).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "match_quality 7d query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let last_28d = match outcome_distribution(&app, cutoff_28d).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "match_quality 28d query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let total_7d = last_7d.iter().map(|w| w.count).sum();
    let total_28d = last_28d.iter().map(|w| w.count).sum();

    Json(MatchQualityResp {
        last_7d,
        last_28d,
        total_7d,
        total_28d,
    })
    .into_response()
}

async fn outcome_distribution(
    app: &AppState,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<MatchQualityWindow>, sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        kind: String,
        c: i64,
    }
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT outcome_kind AS kind, COUNT(*)::bigint AS c \
         FROM metadata_match_outcome \
         WHERE created_at >= $1 \
         GROUP BY outcome_kind",
        [sea_orm::Value::from(since.fixed_offset())],
    );
    let rows = Row::find_by_statement(stmt).all(&app.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| MatchQualityWindow {
            kind: r.kind,
            count: r.c,
        })
        .collect())
}

// ───────── GET /admin/metadata/runs ─────────

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct RunsListQuery {
    pub library_id: Option<Uuid>,
    pub scope: Option<String>,
    pub status: Option<String>,
    /// Hard cap of 100; default 25.
    pub limit: Option<u64>,
    /// ISO-8601 timestamp; returns rows older than this for
    /// cursor-style pagination.
    pub before: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunRow {
    pub id: Uuid,
    pub scope: String,
    pub scope_entity_id: Option<String>,
    pub library_id: Option<Uuid>,
    pub trigger_kind: String,
    pub providers: Vec<String>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub items_total: i32,
    pub items_matched_high: i32,
    pub items_matched_medium: i32,
    pub items_matched_low: i32,
    pub items_applied: i32,
    pub items_skipped: i32,
    pub error_summary: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunsListResp {
    pub runs: Vec<RunRow>,
    /// `started_at` of the last row — caller passes back as `before=`
    /// for the next page. `None` when no more rows.
    pub next_cursor: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_metadata_runs_list",    get,
    path = "/admin/metadata/runs",
    params(RunsListQuery),
    responses(
        (status = 200, body = RunsListResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list_runs(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<RunsListQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(25).clamp(1, 100);
    let mut query = metadata_run::Entity::find()
        .order_by_desc(metadata_run::Column::StartedAt)
        .limit(limit + 1);
    if let Some(lib) = q.library_id {
        query = query.filter(metadata_run::Column::LibraryId.eq(lib));
    }
    if let Some(scope) = q.scope.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(metadata_run::Column::Scope.eq(scope));
    }
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(metadata_run::Column::Status.eq(status));
    }
    if let Some(before) = q.before {
        query = query.filter(metadata_run::Column::StartedAt.lt(before.fixed_offset()));
    }
    let mut rows = match query.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "admin_metadata.list_runs db error");
            return error(StatusCode::BAD_GATEWAY, "internal", "internal");
        }
    };
    let next_cursor = if rows.len() as u64 > limit {
        let extra = rows.pop().unwrap();
        Some(extra.started_at.to_rfc3339())
    } else {
        None
    };
    let runs = rows.into_iter().map(run_to_row).collect();
    Json(RunsListResp { runs, next_cursor }).into_response()
}

fn run_to_row(m: metadata_run::Model) -> RunRow {
    RunRow {
        id: m.id,
        scope: m.scope,
        scope_entity_id: m.scope_entity_id,
        library_id: m.library_id,
        trigger_kind: m.trigger_kind,
        providers: m.providers,
        status: m.status,
        started_at: m.started_at.to_rfc3339(),
        finished_at: m.finished_at.map(|t| t.to_rfc3339()),
        items_total: m.items_total,
        items_matched_high: m.items_matched_high,
        items_matched_medium: m.items_matched_medium,
        items_matched_low: m.items_matched_low,
        items_applied: m.items_applied,
        items_skipped: m.items_skipped,
        error_summary: m.error_summary,
    }
}

// ───────── GET /admin/metadata/runs/{id} ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunDetailResp {
    pub run: RunRow,
    pub query: Option<serde_json::Value>,
    pub candidates: Vec<CandidateRow>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CandidateRow {
    pub ordinal: i32,
    pub source: String,
    pub external_id: String,
    pub bucket: String,
    pub score: f32,
    pub score_breakdown: serde_json::Value,
    pub candidate: serde_json::Value,
    pub applied_at: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_metadata_run_detail",    get,
    path = "/admin/metadata/runs/{id}",
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, body = RunDetailResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "run not found"),
    )
)]
#[handler]
pub async fn get_run(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(run) = metadata_run::Entity::find_by_id(id)
        .one(&app.db)
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
    let query = run.query.clone();
    let candidates = metadata_run_candidate::Entity::find()
        .filter(metadata_run_candidate::Column::RunId.eq(id))
        .order_by_asc(metadata_run_candidate::Column::Ordinal)
        .all(&app.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|c| CandidateRow {
            ordinal: c.ordinal,
            source: c.source,
            external_id: c.external_id,
            bucket: c.bucket,
            score: c.score,
            score_breakdown: c.score_breakdown,
            candidate: c.candidate,
            applied_at: c.applied_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Json(RunDetailResp {
        run: run_to_row(run),
        query,
        candidates,
    })
    .into_response()
}

// ───────── GET /admin/metadata/recent-applies ─────────

/// One recent metadata-apply event for the dashboard feed (audit B14).
/// Sourced from `metadata_run` rows that actually wrote changes
/// (`items_applied > 0`) — the only place that captures **automatic**
/// (weekly-refresh) applies, which emit no audit_log row. `automatic`
/// flags the server-side runs an operator otherwise never sees.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecentApplyRow {
    pub run_id: Uuid,
    pub batch_id: Option<Uuid>,
    /// `series` | `issue` | `library` | `bulk_refresh`.
    pub scope: String,
    /// Human label — series name, `"<series> #<n>"`, or library name.
    /// Falls back to the raw scope id when the row has since been removed.
    pub entity_label: String,
    /// Series slug for linking to the affected page, when resolvable.
    pub series_slug: Option<String>,
    pub library_id: Option<Uuid>,
    /// `true` when the run had no `triggered_by` — a server-side
    /// automatic (weekly-refresh) apply, the silent case B14 surfaces.
    pub automatic: bool,
    pub trigger_kind: String,
    pub items_applied: i32,
    pub providers: Vec<String>,
    pub applied_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecentAppliesResp {
    pub applies: Vec<RecentApplyRow>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct RecentAppliesQuery {
    /// Max rows (default 10, capped 50). This is a dashboard *summary*
    /// — the full, filterable history lives in the Runs tab.
    #[serde(default)]
    pub limit: Option<u64>,
}

#[utoipa::path(
    operation_id = "admin_metadata_recent_applies",
    get,
    path = "/admin/metadata/recent-applies",
    params(RecentAppliesQuery),
    responses(
        (status = 200, body = RecentAppliesResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn recent_applies(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<RecentAppliesQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(10).clamp(1, 50);
    // Runs that actually changed data, newest finish first. A run is only
    // `finished_at`-stamped once it finalizes, so applied runs always carry
    // one — order on it for true apply-recency.
    let rows = match metadata_run::Entity::find()
        .filter(metadata_run::Column::ItemsApplied.gt(0))
        .filter(metadata_run::Column::FinishedAt.is_not_null())
        .order_by_desc(metadata_run::Column::FinishedAt)
        .limit(limit)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "admin_metadata.recent_applies db error");
            return error(StatusCode::BAD_GATEWAY, "internal", "internal");
        }
    };

    // Batch-resolve entity labels: collect the series + issue ids referenced
    // by the page, fetch each set once, then map per row.
    let mut series_ids: Vec<Uuid> = Vec::new();
    let mut issue_ids: Vec<String> = Vec::new();
    for r in &rows {
        match (r.scope.as_str(), r.scope_entity_id.as_deref()) {
            ("series", Some(id)) => {
                if let Ok(u) = id.parse::<Uuid>() {
                    series_ids.push(u);
                }
            }
            ("issue", Some(id)) => issue_ids.push(id.to_owned()),
            _ => {}
        }
    }

    let series_rows = series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids.clone()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let series_by_id: std::collections::HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();

    let issue_rows = issue::Entity::find()
        .filter(issue::Column::Id.is_in(issue_ids.clone()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    // Issue → its series (for the label + slug), fetched in one more pass.
    let issue_series_ids: Vec<Uuid> = issue_rows.iter().map(|i| i.series_id).collect();
    let extra_series = series::Entity::find()
        .filter(series::Column::Id.is_in(issue_series_ids))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let mut series_lookup = series_by_id;
    for s in extra_series {
        series_lookup.entry(s.id).or_insert(s);
    }
    let issue_by_id: std::collections::HashMap<String, issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    let library_rows = library::Entity::find()
        .all(&app.db)
        .await
        .unwrap_or_default();
    let library_name: std::collections::HashMap<Uuid, String> =
        library_rows.into_iter().map(|l| (l.id, l.name)).collect();

    let applies = rows
        .into_iter()
        .map(|r| {
            let (entity_label, series_slug) =
                resolve_apply_label(&r, &series_lookup, &issue_by_id, &library_name);
            RecentApplyRow {
                run_id: r.id,
                batch_id: r.batch_id,
                scope: r.scope,
                entity_label,
                series_slug,
                library_id: r.library_id,
                automatic: r.triggered_by.is_none(),
                trigger_kind: r.trigger_kind,
                items_applied: r.items_applied,
                providers: r.providers,
                applied_at: r.finished_at.map(|t| t.to_rfc3339()),
            }
        })
        .collect();

    Json(RecentAppliesResp { applies }).into_response()
}

/// Resolve a run's `(entity_label, series_slug)` for the recent-applies
/// feed. Soft-falls to the raw scope id when the referenced row is gone.
fn resolve_apply_label(
    run: &metadata_run::Model,
    series_by_id: &std::collections::HashMap<Uuid, series::Model>,
    issue_by_id: &std::collections::HashMap<String, issue::Model>,
    library_name: &std::collections::HashMap<Uuid, String>,
) -> (String, Option<String>) {
    match (run.scope.as_str(), run.scope_entity_id.as_deref()) {
        ("series", Some(id)) => {
            if let Some(s) = id.parse::<Uuid>().ok().and_then(|u| series_by_id.get(&u)) {
                return (s.name.clone(), Some(s.slug.clone()));
            }
            (format!("Series {id}"), None)
        }
        ("issue", Some(id)) => {
            if let Some(i) = issue_by_id.get(id) {
                let series = series_by_id.get(&i.series_id);
                let series_name = series
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "Issue".to_owned());
                let label = match i.number_raw.as_deref() {
                    Some(n) if !n.is_empty() => format!("{series_name} #{n}"),
                    _ => i.title.clone().unwrap_or_else(|| series_name.clone()),
                };
                return (label, series.map(|s| s.slug.clone()));
            }
            ("Issue".to_owned(), None)
        }
        (_, _) => {
            // library / bulk_refresh scopes name the library when set.
            let label = run
                .library_id
                .and_then(|id| library_name.get(&id).cloned())
                .map(|n| format!("{n} (library refresh)"))
                .unwrap_or_else(|| "Library refresh".to_owned());
            (label, None)
        }
    }
}

// ───────── /admin/metadata/phash-backfill ─────────

/// Response for the backfill-trigger endpoints (audit B17). The sweep now
/// runs as a background apalis job; `enqueued` reports that the job was
/// accepted. Progress + the final tally arrive over the scan-events WS as a
/// `backfill.completed` event, and the queue depth surfaces it while pending.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackfillEnqueuedResp {
    pub enqueued: bool,
    /// `cover_phash` | `variant_cover`.
    pub kind: String,
}

async fn enqueue_backfill(
    app: &AppState,
    actor_id: Uuid,
    ctx: &RequestContext,
    kind: BackfillKind,
    action: &'static str,
) -> Response {
    let enqueued = backfill::enqueue(app, kind).await;
    if !enqueued {
        return error(
            StatusCode::BAD_GATEWAY,
            "internal",
            "could not enqueue backfill",
        );
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id,
            action,
            target_type: None,
            target_id: None,
            payload: serde_json::json!({ "kind": kind.as_str(), "enqueued": true }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    (
        StatusCode::ACCEPTED,
        Json(BackfillEnqueuedResp {
            enqueued: true,
            kind: kind.as_str().to_owned(),
        }),
    )
        .into_response()
}

#[utoipa::path(
    operation_id = "metadata_phash_backfill",
    post,
    path = "/admin/metadata/phash-backfill",
    responses(
        (status = 202, body = BackfillEnqueuedResp, description = "backfill job enqueued"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn run_phash_backfill(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    enqueue_backfill(
        &app,
        actor.id,
        &ctx,
        BackfillKind::CoverPhash,
        "admin.metadata.phash_backfill",
    )
    .await
}

// ───────── /admin/metadata/variant-cover-backfill ─────────

#[utoipa::path(
    operation_id = "metadata_variant_cover_backfill",
    post,
    path = "/admin/metadata/variant-cover-backfill",
    responses(
        (status = 202, body = BackfillEnqueuedResp, description = "backfill job enqueued"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn run_variant_cover_backfill(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    enqueue_backfill(
        &app,
        actor.id,
        &ctx,
        BackfillKind::VariantCover,
        "admin.metadata.variant_cover_backfill",
    )
    .await
}

// ───────── GET /admin/metadata/auto-synced ─────────

/// One auto-synced series row for the admin "Auto-synced" tab.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AutoSyncedSeriesRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub library_name: String,
    pub year: Option<i32>,
    /// RFC3339 of the last provider sync, or `null` if never synced.
    pub last_metadata_sync_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AutoSyncedResp {
    pub series: Vec<AutoSyncedSeriesRow>,
}

/// Series with auto-sync enabled (`metadata_sync_paused = false`) — the
/// opt-in set the weekly refresh cron will touch. Auto-sync is
/// series-level, so issues inherit their series' setting. The list is
/// operator-curated (opt-in), so it's intentionally bounded and returned
/// in full, ordered by name.
#[utoipa::path(
    operation_id = "metadata_auto_synced",
    get,
    path = "/admin/metadata/auto-synced",
    responses(
        (status = 200, body = AutoSyncedResp),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list_auto_synced(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let rows = match series::Entity::find()
        .filter(series::Column::MetadataSyncPaused.eq(false))
        .filter(series::Column::RemovedAt.is_null())
        .order_by_asc(series::Column::Name)
        .find_also_related(library::Entity)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "auto-synced series query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series = rows
        .into_iter()
        .map(|(s, lib)| AutoSyncedSeriesRow {
            id: s.id.to_string(),
            slug: s.slug,
            name: s.name,
            library_name: lib.map(|l| l.name).unwrap_or_default(),
            year: s.year,
            last_metadata_sync_at: s.last_metadata_sync_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Json(AutoSyncedResp { series }).into_response()
}
