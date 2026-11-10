//! HTTP request metrics (the RED method) emitted through the global `metrics`
//! recorder — so they render from the same `/metrics` handle as every other
//! `folio_*` metric, no second recorder.
//!
//! - `folio_http_requests_total{method,route,status}` — counter
//! - `folio_http_request_duration_seconds{method,route}` — histogram
//!
//! The `route` label is the matched route *pattern* ([`MatchedPath`], e.g.
//! `/series/{series_slug}/issues/{issue_slug}`), never the raw URI — raw paths
//! carry unbounded IDs and would explode label cardinality. Requests with no
//! matched route (the Next.js upstream proxy fallback, 404s) bucket under
//! `"<unmatched>"`. The `method` label is one of a fixed set ([`method_label`]):
//! the request line can carry any method token, and each distinct token would
//! otherwise mint a new series. `status` is the numeric response status. The
//! `/metrics` scrape itself is not counted.

use std::time::Instant;

use axum::{
    extract::MatchedPath, extract::Request, http::Method, middleware::Next, response::Response,
};

/// Route label for a request no route claimed.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Method label for any method outside the standard set.
pub const OTHER_METHOD: &str = "OTHER";

/// Bounded `method` label: the standard methods by name, everything else
/// (`CONNECT`, `TRACE`, extension tokens) as [`OTHER_METHOD`].
pub fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        _ => OTHER_METHOD,
    }
}

/// Outermost request layer: times the full handling and records the
/// client-observed status. Reads `MatchedPath` on ingress (populated by axum's
/// router for layered middleware) so the `route` label stays bounded.
pub async fn track(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| UNMATCHED_ROUTE.to_owned());

    // Don't let the scrape inflate its own counters.
    if route == "/metrics" {
        return next.run(req).await;
    }

    let method = method_label(req.method());
    let start = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16();

    metrics::counter!(
        "folio_http_requests_total",
        "method" => method,
        "route" => route.clone(),
        "status" => status.to_string(),
    )
    .increment(1);
    metrics::histogram!(
        "folio_http_request_duration_seconds",
        "method" => method,
        "route" => route,
    )
    .record(start.elapsed().as_secs_f64());

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_methods_keep_their_name() {
        for m in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ] {
            assert_eq!(method_label(&m), m.as_str());
        }
    }

    #[test]
    fn other_methods_share_one_label() {
        for raw in ["CONNECT", "TRACE", "PROPFIND", "JUNK", "get", "X-1"] {
            let m = Method::from_bytes(raw.as_bytes()).unwrap();
            assert_eq!(method_label(&m), OTHER_METHOD, "{raw}");
        }
    }
}
