//! Custom axum extractors used by handlers in `crate::api`.
//!
//! Currently exports [`Validated<T>`] — an extractor that runs
//! [`garde::Validate::validate`] on a JSON request body and returns a
//! 422 with the canonical `{ "error": { ... } }` envelope on failure.
//!
//! Adopted by audit-remediation M9 (per decision #5). Handlers swap
//! `Json<CreateMarkerReq>` for `Validated<CreateMarkerReq>` and drop
//! the inline validation block.

use axum::{
    Json,
    extract::{FromRequest, Request, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use serde::de::DeserializeOwned;

use super::{respond, respond_with_field_errors};
use shared::error::{ApiErrorCode, FieldError};

/// Run `T::validate` after JSON deserialisation; reject with 422 on
/// validation failure, 400 on malformed JSON.
///
/// `T` must be `garde::Validate<Context = ()>` — i.e. validation
/// rules that don't need per-request runtime context. For context-
/// aware validation, deserialise into `Json<T>` and call
/// `t.validate_with(&ctx)` inside the handler.
#[derive(Debug, Clone, Copy, Default)]
pub struct Validated<T>(pub T);

impl<S, T> FromRequest<S> for Validated<T>
where
    S: Send + Sync,
    T: DeserializeOwned + garde::Validate<Context = ()> + 'static,
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(payload) = Json::<T>::from_request(req, state)
            .await
            .map_err(json_rejection_to_response)?;
        if let Err(report) = payload.validate() {
            return Err(from_garde(&report));
        }
        Ok(Validated(payload))
    }
}

/// Render a `garde::Report` as a 422 response. The human `message` is
/// every error as `path: message` joined with `; ` (a complete summary
/// on its own); `error.details` carries the same errors as a
/// [`FieldError`] list so a client form can bind each message to its
/// input. Use this from handlers that need to run validation manually
/// (e.g. with a per-request context) rather than letting [`Validated`]
/// do it implicitly.
pub fn from_garde(report: &garde::Report) -> Response {
    let mut parts: Vec<String> = Vec::new();
    let mut fields: Vec<FieldError> = Vec::new();
    for (path, error) in report.iter() {
        let path_str = path.to_string();
        let message = error.to_string();
        if path_str.is_empty() {
            parts.push(message.clone());
        } else {
            parts.push(format!("{path_str}: {message}"));
        }
        fields.push(FieldError {
            field: path_str,
            message,
        });
    }
    let message = if parts.is_empty() {
        "validation failed".to_owned()
    } else {
        parts.join("; ")
    };
    respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        message,
        fields,
    )
}

fn json_rejection_to_response(rej: JsonRejection) -> Response {
    let (status, message) = match &rej {
        JsonRejection::JsonDataError(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        JsonRejection::JsonSyntaxError(e) => (StatusCode::BAD_REQUEST, e.to_string()),
        JsonRejection::MissingJsonContentType(e) => (StatusCode::BAD_REQUEST, e.to_string()),
        _ => (StatusCode::BAD_REQUEST, rej.to_string()),
    };
    respond(status, ApiErrorCode::Validation, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request, routing::post};
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Debug, Deserialize, garde::Validate)]
    struct Sample {
        #[garde(length(min = 1, max = 10))]
        name: String,
        #[garde(range(min = 0, max = 100))]
        age: i32,
    }

    async fn handler(Validated(req): Validated<Sample>) -> String {
        format!("{} is {}", req.name, req.age)
    }

    #[tokio::test]
    async fn passes_through_when_valid() {
        let app = Router::new().route("/x", post(handler));
        let body = serde_json::json!({"name": "ok", "age": 42}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_with_422_on_validation_failure() {
        let app = Router::new().route("/x", post(handler));
        let body = serde_json::json!({"name": "", "age": 999}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msg = json["error"]["message"].as_str().unwrap();
        // Both rules should be reported, joined with `; `.
        assert!(msg.contains("name:"), "missing name path in {msg}");
        assert!(msg.contains("age:"), "missing age path in {msg}");

        // error.details carries the same failures as a [{field, message}]
        // list so a client form can bind each message to its input.
        let details = json["error"]["details"]
            .as_array()
            .expect("details is an array");
        let fields: Vec<&str> = details
            .iter()
            .map(|d| d["field"].as_str().unwrap())
            .collect();
        assert!(
            fields.contains(&"name"),
            "details missing name: {details:?}"
        );
        assert!(fields.contains(&"age"), "details missing age: {details:?}");
        for d in details {
            assert!(
                d["message"].as_str().is_some_and(|m| !m.is_empty()),
                "each detail has a non-empty message: {d:?}"
            );
        }
    }

    #[tokio::test]
    async fn malformed_json_returns_400() {
        let app = Router::new().route("/x", post(handler));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/x")
                    .header("content-type", "application/json")
                    .body(Body::from("{not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
