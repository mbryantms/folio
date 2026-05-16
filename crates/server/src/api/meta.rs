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
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/metrics", get(metrics))
}

async fn openapi_json() -> impl IntoResponse {
    Json(crate::app::openapi_spec())
}

async fn metrics(State(state): State<AppState>) -> Response {
    let body = state.prometheus.render();
    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    resp
}
