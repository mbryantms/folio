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
//! Both routes are gated by [`RequireAdmin`]. M5+ add the Dashboard,
//! Review queue, and Runs tabs on top of the same module.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

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
pub async fn list_providers(
    State(app): State<AppState>,
    _admin: RequireAdmin,
) -> Response {
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
            (
                status,
                payload,
                error(status, code, &e.to_string()),
            )
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
        ProviderError::QuotaExceeded { .. } => (StatusCode::TOO_MANY_REQUESTS, "metadata.quota_exceeded"),
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
    let key = app
        .cfg()
        .comicvine_api_key
        .clone()
        .unwrap_or_default();
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
