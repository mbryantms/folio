//! `/csp-report` — CSP violation ingestion (§17.4).
//!
//! Browsers POST `application/csp-report` (legacy) or `application/reports+json` (modern).
//! We accept either by reading the raw body and parsing as JSON.
//! Logged structured at `warn`; increments `folio_csp_violations_total`.

use axum::{body::Bytes, extract::State, http::StatusCode, response::IntoResponse};
use utoipa_axum::router::OpenApiRouter;

use crate::middleware::rate_limit;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
    // CSP report endpoint isn't in the spec — the body shape is
    // browser-defined and varies by user-agent. Register via plain
    // `.route()` so we still apply the per-route rate-limit layer
    // without forcing a `#[utoipa::path]` over a polymorphic body.
    OpenApiRouter::new().route(
        "/csp-report",
        axum::routing::post(csp_report).route_layer(rate_limit::CSP_REPORT.build()),
    )
}

// Note: no #[utoipa::path] — the body shape is browser-defined and varies by user-agent.
pub async fn csp_report(State(_state): State<AppState>, body: Bytes) -> impl IntoResponse {
    metrics::counter!("folio_csp_violations_total").increment(1);
    let parsed: serde_json::Value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| serde_json::json!({"raw": String::from_utf8_lossy(&body)}));
    tracing::warn!(report = %parsed, "csp violation");
    StatusCode::NO_CONTENT
}
