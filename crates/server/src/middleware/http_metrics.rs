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
//! `"<unmatched>"`. The `/metrics` scrape itself is not counted.

use std::time::Instant;

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};

/// Outermost request layer: times the full handling and records the
/// client-observed status. Reads `MatchedPath` on ingress (populated by axum's
/// router for layered middleware) so the `route` label stays bounded.
pub async fn track(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());

    // Don't let the scrape inflate its own counters.
    if route == "/metrics" {
        return next.run(req).await;
    }

    let method = req.method().as_str().to_owned();
    let start = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16();

    metrics::counter!(
        "folio_http_requests_total",
        "method" => method.clone(),
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
