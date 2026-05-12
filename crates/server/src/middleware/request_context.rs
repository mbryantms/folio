//! Per-request client context (§17.7, audit-log enrichment).
//!
//! Captures the real client IP (via [`crate::auth::xff::client_ip`]) and the
//! `User-Agent` once, stashes them in `Request::extensions` so handlers can
//! pull them out without reparsing for every audit-log row or session insert.

use axum::{extract::Request, middleware::Next, response::Response};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::auth::xff;

/// Resolved request context. Cloned cheaply (small struct).
#[derive(Clone, Debug, Default)]
pub struct RequestContext {
    /// Best-effort client IP. `None` only if `ConnectInfo` extraction failed,
    /// which shouldn't happen given how we configured the listener.
    pub client_ip: Option<IpAddr>,
    /// Verbatim `User-Agent` header. `None` if the client didn't send one.
    pub user_agent: Option<String>,
}

impl RequestContext {
    /// String form of the client IP for the `audit_log.ip` column.
    pub fn ip_string(&self) -> Option<String> {
        self.client_ip.map(|ip| ip.to_string())
    }
}

/// Per-process trusted-proxies set. Built once at boot from
/// `Config::trusted_proxies` and shared via an `Arc`. Storing it here rather
/// than on `AppState` keeps the middleware layer self-contained.
#[derive(Clone, Default)]
pub struct TrustedProxies(pub Arc<Vec<IpNet>>);

impl TrustedProxies {
    pub fn from_config(raw: &str) -> Self {
        Self(Arc::new(xff::parse_trusted_proxies(raw)))
    }

    pub fn as_slice(&self) -> &[IpNet] {
        &self.0
    }
}

/// Axum middleware: resolves `RequestContext` and inserts it into the
/// request's extensions. Must be installed *outside* any handler/extractor
/// that needs to read it.
///
/// `ConnectInfo<SocketAddr>` is read from request extensions directly rather
/// than via the extractor pattern — the production listener is started with
/// `into_make_service_with_connect_info` so it's always present at runtime,
/// but unit tests dispatch via `tower::ServiceExt::oneshot` which doesn't
/// populate it. We fall back to `127.0.0.1` in that case so the middleware
/// (and any handler reading from the extension) doesn't 500 the request.
pub async fn set_context(
    axum::extract::State(trusted): axum::extract::State<TrustedProxies>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer_ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or_else(|| IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let ip = xff::client_ip(req.headers(), peer_ip, trusted.as_slice());
    let ua = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(512).collect::<String>());

    let ctx = RequestContext {
        client_ip: Some(ip),
        user_agent: ua,
    };
    req.extensions_mut().insert(ctx);
    next.run(req).await
}
