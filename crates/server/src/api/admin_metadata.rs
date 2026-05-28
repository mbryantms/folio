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
use entity::{audit_log, library, metadata_run, metadata_run_candidate, series};
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
    pub dismissed_at: Option<String>,
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
            dismissed_at: c.dismissed_at.map(|t| t.to_rfc3339()),
        })
        .collect();
    Json(RunDetailResp {
        run: run_to_row(run),
        query,
        candidates,
    })
    .into_response()
}

// ───────── /admin/metadata/phash-backfill ─────────

/// Optional `?limit=` for [`run_phash_backfill`]. The admin UI drives the
/// drain in small batches so each request returns well before a reverse
/// proxy timeout; omit for the legacy 500-per-call default.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct PhashBackfillQuery {
    #[serde(default)]
    pub limit: Option<u32>,
}

#[utoipa::path(
    operation_id = "metadata_phash_backfill",
    post,
    path = "/admin/metadata/phash-backfill",
    params(PhashBackfillQuery),
    responses(
        (status = 200, body = crate::metadata::phash::BackfillOutcome),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn run_phash_backfill(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<PhashBackfillQuery>,
) -> Response {
    let archive_limits = app.cfg().archive_limits();
    let batch = q
        .limit
        .unwrap_or(crate::metadata::phash::BACKFILL_BATCH_CAP as u32)
        .clamp(1, crate::metadata::phash::BACKFILL_BATCH_CAP as u32) as u64;
    let outcome = match crate::metadata::phash::run_backfill(&app.db, archive_limits, batch).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!(error = %e, "phash backfill: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.metadata.phash_backfill",
            target_type: None,
            target_id: None,
            payload: serde_json::json!({
                "considered": outcome.considered,
                "hashed": outcome.hashed,
                "skipped": outcome.skipped,
                "errored": outcome.errored,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    Json(outcome).into_response()
}

// ───────── /admin/metadata/variant-cover-backfill ─────────

#[utoipa::path(
    operation_id = "metadata_variant_cover_backfill",
    post,
    path = "/admin/metadata/variant-cover-backfill",
    responses(
        (status = 200, body = crate::metadata::writers::VariantCoverBackfillOutcome),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn run_variant_cover_backfill(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    let data_path = app.cfg().data_path.clone();
    let outcome =
        match crate::metadata::writers::run_variant_cover_backfill(&app.db, &data_path).await {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "variant cover backfill: query failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.metadata.variant_cover_backfill",
            target_type: None,
            target_id: None,
            payload: serde_json::json!({
                "considered": outcome.considered,
                "stored": outcome.stored,
                "skipped": outcome.skipped,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    Json(outcome).into_response()
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
