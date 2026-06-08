//! Meta endpoints: `/openapi.json`, `/metrics` (Prometheus).
//!
//! Pre-v0.2 this module also owned `GET /` and returned a metadata
//! JSON blob. That route was removed when the Rust binary became the
//! public origin (rust-public-origin plan, M2): `/` must fall through
//! the `Router::fallback` to the Next SSR upstream so the browser
//! lands on the actual homepage. The same liveness info is still
//! reachable from `/healthz`.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use constant_time_eq::constant_time_eq;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/metrics", get(metrics))
}

async fn openapi_json() -> impl IntoResponse {
    Json(crate::app::openapi_spec())
}

async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Machine bearer auth — a Prometheus scraper can't hold an admin session.
    // Release builds require a token unless COMIC_METRICS_OPEN=true.
    let cfg = state.cfg();
    if cfg.metrics_requires_token() {
        let Some(expected) = cfg
            .metrics_token
            .as_deref()
            .filter(|token| !token.is_empty())
        else {
            let mut resp = (StatusCode::UNAUTHORIZED, "metrics: token required\n").into_response();
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return resp;
        };
        let provided = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let ok = provided.is_some_and(|p| constant_time_eq(p.as_bytes(), expected.as_bytes()));
        if !ok {
            let mut resp = (StatusCode::UNAUTHORIZED, "metrics: unauthorized\n").into_response();
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return resp;
        }
    }

    // Sample process/runtime gauges (`folio_process_*`) fresh at scrape time.
    state.process_metrics.collect();

    let body = state.prometheus.render();
    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    resp
}
