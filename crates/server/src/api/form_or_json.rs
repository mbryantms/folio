//! `FormOrJson<T>` — accept JSON or `application/x-www-form-urlencoded` bodies
//! with a single extractor, and remember which one it was so the handler can
//! reply with the matching response shape.
//!
//! Used by progressive-enhancement auth endpoints (login, register,
//! request-password-reset, reset-password, account update). The JS happy path
//! POSTs JSON via `fetch`; the no-JS fallback path POSTs form-encoded via the
//! browser's native `<form method="POST">` submission. Same handler, two
//! response shapes:
//!   - JSON in → JSON out (200 / 4xx envelope).
//!   - Form in → 303 redirect (success) or 303 redirect with `?error=…`
//!     (failure). Browsers follow the redirect; cookies travel along.
//!
//! Why progressive enhancement matters here: every credential `<form>` used
//! to ship without `method`/`action`, so a pre-hydration submit leaked
//! `?email=&password=` into the URL bar, history, and access logs. With
//! this extractor + the matching client-side wiring, the form *always* has
//! a real POST target — JS speeds it up, but its absence can't ever leak
//! credentials into a GET.

use axum::{
    Json,
    body::Bytes,
    extract::{FromRequest, Request},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    Json,
    Form,
}

#[derive(Debug)]
pub struct FormOrJson<T> {
    pub data: T,
    pub format: ResponseFormat,
}

impl<T, S> FromRequest<S> for FormOrJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_form = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .map(|c| c.starts_with("application/x-www-form-urlencoded"))
            .unwrap_or(false);

        let body = Bytes::from_request(req, state)
            .await
            .map_err(IntoResponse::into_response)?;

        if is_form {
            let data = serde_urlencoded::from_bytes::<T>(&body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": {"code": "validation", "message": e.to_string()}})),
                )
                    .into_response()
            })?;
            Ok(FormOrJson {
                data,
                format: ResponseFormat::Form,
            })
        } else {
            let data = serde_json::from_slice::<T>(&body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": {"code": "validation", "message": e.to_string()}})),
                )
                    .into_response()
            })?;
            Ok(FormOrJson {
                data,
                format: ResponseFormat::Json,
            })
        }
    }
}

/// Build a redirect URL by appending `error`/`message`/`next` query params
/// to a base path. Used by handlers that need to bounce a form-fallback
/// failure back to its origin page with a banner. Each component is
/// urlencoded; the base is taken verbatim.
pub fn redirect_with_error(base: &str, code: &str, message: &str, next: Option<&str>) -> String {
    let mut out = format!(
        "{base}?error={code}&message={msg}",
        base = base,
        code = urlencoding::encode(code),
        msg = urlencoding::encode(message),
    );
    if let Some(n) = next.filter(|n| !n.is_empty()) {
        out.push_str("&next=");
        out.push_str(&urlencoding::encode(n));
    }
    out
}
