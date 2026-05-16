//! Per-request CSP nonce middleware.
//!
//! Generates a 16-byte random value per request, base64url-encodes it,
//! and stashes the result in `Request::extensions` as [`Nonce`]. Two
//! downstream consumers read it:
//!
//!   - [`security_headers::set_headers`](super::security_headers) builds
//!     the per-response CSP with `'nonce-XXX' 'strict-dynamic'` slotted
//!     into `script-src` / `style-src`.
//!   - [`upstream::proxy`](crate::upstream::proxy) forwards the same
//!     value as `x-csp-nonce` to Next.js so the SSR runtime can stamp
//!     the nonce onto every inline `<script>` tag it emits.
//!
//! The middleware MUST run outside both consumers in the layer stack:
//! `set_nonce` → `security_headers::set_headers` → `upstream::proxy`.
//! Wired in [`crate::app::router`].
//!
//! Why 16 bytes? 128 bits of entropy, comfortably above the
//! [CSP spec recommendation](https://www.w3.org/TR/CSP3/#security-nonces)
//! of "at least 128 bits". Base64url with no padding gives 22 chars,
//! short enough to add to every CSP header without bloat.
//!
//! Why per-request rather than per-response? `Response::extensions`
//! doesn't survive the proxy hop — `upstream::forward` builds a new
//! `Response` from the upstream stream. Storing on the request keeps
//! the nonce visible to both the proxy (pre-upstream) and the headers
//! middleware (post-handler) within the same axum invocation.

use axum::{extract::Request, middleware::Next, response::Response};
use base64::Engine;

/// Per-request CSP nonce. Stored in `Request::extensions`; read via
/// `req.extensions().get::<Nonce>()`. Opaque wrapper around the
/// base64url-encoded string so a misuse like `req.extensions::get::<String>()`
/// doesn't accidentally match some other string extension.
#[derive(Clone, Debug)]
pub struct Nonce(pub String);

/// Length in bytes of the raw random value backing each nonce.
/// 16 bytes = 128 bits of entropy, the W3C-recommended floor.
const NONCE_BYTES: usize = 16;

/// Axum middleware. Generates a fresh nonce and inserts it into the
/// request's extensions before delegating to `next`.
pub async fn set_nonce(mut req: Request, next: Next) -> Response {
    let mut bytes = [0u8; NONCE_BYTES];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes[..]);
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    req.extensions_mut().insert(Nonce(encoded));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request as HttpRequest, routing::get};
    use tower::ServiceExt;

    fn test_router() -> Router {
        Router::new()
            .route(
                "/",
                get(|req: HttpRequest<Body>| async move {
                    req.extensions()
                        .get::<Nonce>()
                        .map(|n| n.0.clone())
                        .unwrap_or_default()
                }),
            )
            .layer(axum::middleware::from_fn(set_nonce))
    }

    /// 16 bytes base64url-no-pad encodes to ceil(16 * 4 / 3) = 22 chars.
    /// Every char must be in the base64url alphabet:
    /// `A-Z`, `a-z`, `0-9`, `-`, `_`.
    #[tokio::test]
    async fn nonce_shape_is_22_char_base64url() {
        let resp = test_router()
            .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert_eq!(s.len(), 22, "expected 22-char nonce, got {} chars", s.len());
        for c in s.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-base64url char in nonce: {c:?}",
            );
        }
    }

    /// Two requests must produce different nonces. 128 bits of entropy
    /// means the practical collision risk is astronomically small;
    /// this test is really a guard against accidentally using a
    /// constant or a per-process value.
    #[tokio::test]
    async fn nonces_differ_across_requests() {
        let r = test_router();
        let a = body_string(
            r.clone()
                .oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .into_body(),
        )
        .await;
        let b = body_string(
            r.oneshot(HttpRequest::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .into_body(),
        )
        .await;
        assert_ne!(a, b);
    }

    async fn body_string(b: Body) -> String {
        let bytes = axum::body::to_bytes(b, 1024).await.unwrap();
        std::str::from_utf8(&bytes).unwrap().to_owned()
    }
}
