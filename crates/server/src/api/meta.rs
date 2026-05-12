//! Meta endpoints: `/`, `/openapi.json`, `/metrics` (Prometheus).

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct Hello {
    pub app: &'static str,
    pub version: &'static str,
    pub auth_mode: String,
    pub docs: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(hello))
        .route("/openapi.json", get(openapi_json))
        .route("/metrics", get(metrics))
}

async fn hello(State(state): State<AppState>) -> impl IntoResponse {
    Json(Hello {
        app: "comic-reader",
        version: env!("CARGO_PKG_VERSION"),
        auth_mode: state.cfg.auth_mode.to_string(),
        docs: "/openapi.json",
    })
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
