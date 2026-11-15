//! `POST /integrations/mylar/snapshot` — the one door Mylar data comes
//! through (upcoming-releases 1.0, part B).
//!
//! A script on the operator's Mylar host pushes a read-only snapshot
//! here; Folio never calls Mylar (see `crate::upcoming::mylar`). The
//! route lives in the `bare` group because an external client calls it,
//! and alone in this module so nothing else can share its credential.
//!
//! **Auth before body.** [`MylarIngestAuth`] is a parts extractor placed
//! ahead of the body extractor, so a caller without the token gets its
//! 401 or 403 before a byte of the body is parsed. The token is a
//! single-purpose secret, not a user: it carries no session and opens no
//! other route. A non-`app_` Bearer is already CSRF-exempt
//! (`auth::csrf::token_exemption`), so the route is not in
//! `CSRF_EXEMPT_PATHS`; a POST with no Bearer at all is refused by the
//! CSRF middleware (403 `auth.csrf`) before it gets here.
//!
//! There is deliberately no failed-auth lockout here. Behind a reverse
//! proxy every caller shares one address, so a lockout would let anyone
//! block the pusher; the token is 256 random bits and gains nothing from
//! one. The per-IP bucket bounds the work a guesser can cause.

use axum::{
    Extension,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use shared::error::ApiErrorCode;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::extractors::Validated;
use super::respond;
use crate::middleware::{RequestContext, rate_limit};
use crate::state::AppState;
use crate::upcoming::mylar::{self, MylarSnapshot, Stored};
use server_macros::handler;

/// The `bare` group has no timeout of its own (it carries streams); this
/// small JSON POST gets one.
const INGEST_TIMEOUT: Duration = Duration::from_secs(30);

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(ingest))
        .route_layer(rate_limit::MYLAR_INGEST.build())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            INGEST_TIMEOUT,
        ))
}

/// Proof that the request carries the current ingest token and that the
/// integration is switched on.
pub struct MylarIngestAuth;

fn unauthorized(message: &str) -> Response {
    let mut resp = respond(StatusCode::UNAUTHORIZED, ApiErrorCode::AuthInvalid, message);
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    resp
}

impl FromRequestParts<AppState> for MylarIngestAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let cfg = app.cfg();
        if !cfg.upcoming_enabled || !cfg.integrations_mylar_enabled {
            return Err(respond(
                StatusCode::FORBIDDEN,
                ApiErrorCode::PermissionDenied,
                "Mylar snapshots are turned off on this server",
            ));
        }
        let presented = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let Some(presented) = presented else {
            return Err(unauthorized("an ingest token is required"));
        };
        match mylar::verify_token(&app.db, presented).await {
            Ok(true) => Ok(Self),
            Ok(false) => Err(unauthorized("the ingest token is not valid")),
            Err(e) => {
                tracing::error!(error = %e, "mylar ingest: token lookup failed");
                Err(respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiErrorCode::Internal,
                    "internal",
                ))
            }
        }
    }
}

#[utoipa::path(
    operation_id = "mylar_snapshot_ingest", post,
    path = "/integrations/mylar/snapshot",
    request_body = MylarSnapshot,
    responses(
        (status = 204, description = "snapshot stored (or an unchanged snapshot acknowledged)"),
        (status = 401, description = "missing or wrong ingest token"),
        (status = 403, description = "Mylar snapshots are turned off"),
        (status = 413, description = "body over the limit"),
        (status = 422, description = "unsupported schema version or a value out of range"),
        (status = 429, description = "too many pushes"),
    )
)]
#[handler]
pub async fn ingest(
    State(app): State<AppState>,
    _auth: MylarIngestAuth,
    Extension(ctx): Extension<RequestContext>,
    Validated(snapshot): Validated<MylarSnapshot>,
) -> Response {
    let received_at = chrono::Utc::now().fixed_offset();
    match mylar::store(&app.db, &snapshot, received_at, ctx.ip_string()).await {
        Ok(stored) => {
            tracing::info!(
                heartbeat = stored == Stored::Heartbeat,
                pull = snapshot.pull_list.len(),
                watch = snapshot.watchlist.len(),
                wanted = snapshot.wanted.len(),
                "mylar ingest: snapshot accepted"
            );
            metrics::gauge!(mylar::LAST_SNAPSHOT_GAUGE).set(received_at.timestamp() as f64);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "mylar ingest: storing the snapshot failed");
            respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "internal",
            )
        }
    }
}
