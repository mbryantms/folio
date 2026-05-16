//! Reverse proxy to the Next.js SSR upstream.
//!
//! M1 + M2 + M3 of the rust-public-origin plan
//! (`~/.claude/plans/rust-public-origin-1.0.md`). M1 built the
//! streaming reverse-proxy; M2 wired `proxy()` as the axum router's
//! `Router::fallback` in `app.rs`; M3 added WebSocket-upgrade
//! passthrough. The design intent is documented in
//! `web/next.config.ts:26,40-41`: Rust is the public origin and Next
//! is an internal SSR upstream.
//!
//! What this module owns
//!
//! - Streaming request body forward to `${cfg.web_upstream_url}{path}`.
//! - Hop-by-hop header stripping (RFC 7230 §6.1).
//! - `X-Forwarded-{For,Proto,Host}` injection on the outbound request.
//!   `X-Forwarded-For` honors the resolved client IP from
//!   `RequestContext` (post-trusted-proxy walk).
//! - Streaming response body back to the caller without buffering.
//! - 502 on upstream connection failure or timeout, with the standard
//!   `{"error": {"code", "message"}}` envelope.
//! - Raw byte-level WebSocket-upgrade passthrough via
//!   [`proxy_websocket`]. The proxy completes the HTTP/1.1 upgrade
//!   handshake on both sides, then bridges the two TCP streams with
//!   `tokio::io::copy_bidirectional` so any negotiated subprotocol or
//!   extension flows end-to-end without being decoded in the middle.
//!   Existing `/ws/*` routes (e.g. scan-events) still terminate at
//!   their explicit handlers; the fallback only handles WS upgrades
//!   for paths Next.js owns.
//!
//! What this module does NOT own (yet)
//!
//! - `/api/*` prefix-strip. M4 drops the Next.js `/api/:path*` rewrite
//!   once Rust owns the public origin in prod; until then `/api/foo`
//!   loops back through Next, which costs one extra hop but preserves
//!   the JSON 404 envelope on unmatched paths.

use std::net::IpAddr;
use std::time::Duration;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::middleware::request_context::RequestContext;
use crate::state::AppState;

/// Per-request upstream timeout. SSR pages are usually fast (<1 s)
/// but allow up to 60 s for cold-start re-renders or slow data
/// fetches inside `getServerSideProps`. M2 may parameterize this if
/// any specific route needs a different bound; for the catch-all
/// fallback, a single value is fine.
pub const PROXY_TIMEOUT: Duration = Duration::from_secs(60);

/// Hop-by-hop headers per RFC 7230 §6.1 — never forwarded in either
/// direction. The Connection header itself may also name additional
/// headers to drop; we accept that tiny gap for now since modern
/// HTTP/1.1 clients almost never use that mechanism and HTTP/2
/// forbids it entirely.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Axum handler. Use as `Router::fallback(upstream::proxy)` to catch
/// every request not matched by an explicit route. Reads the resolved
/// client IP from `RequestContext` (populated by the
/// `request_context::set_context` middleware) so it can be appended to
/// the outbound `X-Forwarded-For` chain. Dispatches WebSocket upgrades
/// to [`proxy_websocket`] and everything else to [`forward`].
pub async fn proxy(State(app): State<AppState>, req: Request<Body>) -> Response {
    let cfg = app.cfg();
    if is_websocket_upgrade(req.headers()) {
        return proxy_websocket(&cfg.web_upstream_url, req).await;
    }
    let client_ip = req
        .extensions()
        .get::<RequestContext>()
        .and_then(|c| c.client_ip);
    forward(
        &app.web_proxy_client,
        &cfg.web_upstream_url,
        req,
        PROXY_TIMEOUT,
        client_ip,
    )
    .await
}

/// Forward a request to `upstream_url + path_and_query`. Pure helper
/// in the sense that it does not touch `AppState`, which makes it
/// drive-testable against a wiremock upstream without booting the
/// server. The state-bound `proxy()` handler is the only call site
/// in production; the function is `pub` so the integration suite at
/// `tests/upstream.rs` can exercise it directly.
pub async fn forward(
    client: &reqwest::Client,
    upstream_url: &str,
    req: Request<Body>,
    timeout: Duration,
    client_ip: Option<IpAddr>,
) -> Response {
    let (parts, body) = req.into_parts();

    let target = match build_target_url(upstream_url, &parts.uri) {
        Some(u) => u,
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upstream_url_invalid",
                "Could not construct upstream URL from configured web_upstream_url",
            );
        }
    };

    let orig_host = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(String::from);

    // Outbound header set. Strip hop-by-hop + Host + Content-Length;
    // keep everything else (Cookie, Authorization, Accept-*, custom
    // headers all flow through). Content-Length is dropped because
    // reqwest will re-encode the streaming body with chunked transfer
    // encoding — keeping the original Content-Length alongside chunked
    // is invalid HTTP and many servers (Next included) reject it.
    let mut outbound_headers = HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if is_hop_by_hop(name) || name == header::HOST || name == header::CONTENT_LENGTH {
            continue;
        }
        outbound_headers.append(name.clone(), value.clone());
    }

    // X-Forwarded-Proto: preserve if upstream proxy already set it
    // (Cloudflare always does when terminating TLS); else default to
    // "http" (we're being hit directly over plain HTTP in dev).
    if !outbound_headers.contains_key("x-forwarded-proto") {
        outbound_headers.insert(
            HeaderName::from_static("x-forwarded-proto"),
            HeaderValue::from_static("http"),
        );
    }
    // X-Forwarded-Host: the client-facing host as the upstream chain
    // saw it. The inbound `Host` header is the same value in nearly
    // every deployment (Cloudflare preserves it), so reusing it
    // avoids needing to plumb a separate `ConnectInfo`-derived host.
    if !outbound_headers.contains_key("x-forwarded-host")
        && let Some(host) = &orig_host
        && let Ok(v) = HeaderValue::from_str(host)
    {
        outbound_headers.insert(HeaderName::from_static("x-forwarded-host"), v);
    }
    // X-Forwarded-For: if the caller supplied the resolved client IP
    // (post-trusted-proxy walk), append it to the existing chain. The
    // canonical XFF format is `client, proxy1, proxy2`; we add our
    // immediate-sender entry to the right so downstream code that
    // honors the first entry as "real client" stays correct.
    if let Some(ip) = client_ip {
        let existing = outbound_headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(", ");
        let new_value = if existing.is_empty() {
            ip.to_string()
        } else {
            format!("{existing}, {ip}")
        };
        outbound_headers.remove("x-forwarded-for");
        if let Ok(v) = HeaderValue::from_str(&new_value) {
            outbound_headers.insert(HeaderName::from_static("x-forwarded-for"), v);
        }
    }

    // Build + stream.
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .expect("axum http::Method round-trips to reqwest");
    let body_stream = body.into_data_stream();
    let request = client
        .request(method, &target)
        .headers(outbound_headers)
        .timeout(timeout)
        .body(reqwest::Body::wrap_stream(body_stream));

    let upstream_resp = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            let code = if e.is_timeout() {
                "upstream_timeout"
            } else if e.is_connect() {
                "upstream_unreachable"
            } else {
                "upstream_error"
            };
            return error_response(StatusCode::BAD_GATEWAY, code, &e.to_string());
        }
    };

    // Translate upstream → axum response. Status, headers (minus
    // hop-by-hop), then stream the body.
    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream_resp.headers().iter() {
        if is_hop_by_hop(name) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let response_body = Body::from_stream(upstream_resp.bytes_stream());
    builder.body(response_body).expect("response builder")
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
}

fn build_target_url(upstream: &str, inbound: &Uri) -> Option<String> {
    if upstream.is_empty() {
        return None;
    }
    let base = upstream.trim_end_matches('/');
    let path_q = inbound
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    Some(format!("{base}{path_q}"))
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// Proxy a WebSocket upgrade to the upstream and bidirectionally bridge
/// the two TCP streams once both ends have flipped to 101. Raw byte-
/// level forwarding preserves any subprotocol or extension the client
/// and upstream negotiate without us having to parse WebSocket frames.
///
/// On any failure before the bridge is established, returns a JSON
/// error envelope. After bridging starts, the bridge runs in a
/// detached task; errors there are logged at `debug` (the most common
/// "error" is a normal close that surfaces as `BrokenPipe`).
pub async fn proxy_websocket(upstream_url: &str, req: Request<Body>) -> Response {
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpStream;

    let (mut parts, _body) = req.into_parts();

    // 1. Take the client-side `OnUpgrade` out of extensions. axum
    // populates this when an `Upgrade: websocket` request arrives;
    // awaiting it after we return a 101 response yields the raw
    // bidirectional `Upgraded` IO handle.
    let client_on_upgrade = match parts.extensions.remove::<hyper::upgrade::OnUpgrade>() {
        Some(u) => u,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "ws_no_upgrade_extension",
                "Inbound request marked as WS upgrade but no OnUpgrade extension present",
            );
        }
    };

    // 2. Parse the upstream URL to extract host + port.
    let url = match reqwest::Url::parse(upstream_url) {
        Ok(u) => u,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upstream_url_invalid",
                "Could not parse configured web_upstream_url",
            );
        }
    };
    let host = url.host_str().unwrap_or("localhost").to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let host_header = format!("{host}:{port}");

    // 3. Plain TCP. The upgrade handshake is a single HTTP/1.1
    // request/response over this socket, after which we own it as a
    // raw byte stream.
    let tcp = match TcpStream::connect((host.as_str(), port)).await {
        Ok(t) => t,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                &e.to_string(),
            );
        }
    };

    // 4. HTTP/1.1 client handshake on the socket. `with_upgrades()`
    // keeps the connection future alive across the protocol switch;
    // the IO handed back via `hyper::upgrade::on(response)` lives
    // independently afterwards.
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await {
        Ok(pair) => pair,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_handshake_failed",
                &e.to_string(),
            );
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(error = %e, "ws proxy: upstream connection task ended");
        }
    });

    // 5. Build the upgrade request. We retain Connection + Upgrade +
    // every Sec-WebSocket-* header (the WS handshake-critical set is
    // end-to-end despite Upgrade nominally being hop-by-hop). Rewrite
    // Host to upstream's authority so the upstream HTTP server routes
    // correctly; drop Content-Length (no body on the WS GET).
    let path_q = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let mut builder = Request::builder()
        .method(parts.method.clone())
        .uri(path_q)
        .version(hyper::Version::HTTP_11);
    for (name, value) in parts.headers.iter() {
        if name == header::HOST || name == header::CONTENT_LENGTH {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder = builder.header(header::HOST, &host_header);

    let upstream_req = match builder.body(Body::empty()) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ws_request_build_failed",
                &e.to_string(),
            );
        }
    };

    // 6. Send and inspect the response.
    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_send_failed",
                &e.to_string(),
            );
        }
    };

    let upstream_status = upstream_resp.status();
    if upstream_status != StatusCode::SWITCHING_PROTOCOLS {
        // Upstream refused the upgrade. For M3 we surface this as a
        // 502 with a clear code rather than buffering + forwarding
        // the upstream body — a non-101 reply to a WS handshake means
        // the client already lost; the body is rarely informative.
        return error_response(
            StatusCode::BAD_GATEWAY,
            "upstream_rejected_upgrade",
            &format!(
                "upstream returned {} instead of 101",
                upstream_status.as_u16()
            ),
        );
    }

    // 7. Build the 101 for the client, copying every upstream header
    // so the negotiated Sec-WebSocket-Accept / -Protocol / -Extensions
    // are preserved end-to-end.
    let mut response_builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in upstream_resp.headers().iter() {
        response_builder = response_builder.header(name, value);
    }

    // 8. Bridge task. Both sides have to complete their upgrades —
    // `tokio::try_join!` waits for the pair, then `copy_bidirectional`
    // runs until either direction closes.
    let upstream_on_upgrade = hyper::upgrade::on(upstream_resp);
    tokio::spawn(async move {
        match tokio::try_join!(client_on_upgrade, upstream_on_upgrade) {
            Ok((client_io, upstream_io)) => {
                let mut client_io = TokioIo::new(client_io);
                let mut upstream_io = TokioIo::new(upstream_io);
                if let Err(e) =
                    tokio::io::copy_bidirectional(&mut client_io, &mut upstream_io).await
                {
                    tracing::debug!(error = %e, "ws proxy: bridge ended");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "ws proxy: upgrade await failed");
            }
        }
    });

    response_builder
        .body(Body::empty())
        .expect("101 response builder")
}

#[cfg(test)]
mod tests {
    //! Fast unit tests of the URL-building + header-filtering helpers.
    //! The streaming integration tests live in `tests/upstream.rs` —
    //! they spin up a wiremock upstream and exercise `forward()`
    //! end-to-end.

    use super::*;

    #[test]
    fn build_target_url_basic() {
        let uri: Uri = "/sign-in?ref=home".parse().unwrap();
        assert_eq!(
            build_target_url("http://localhost:3000", &uri).as_deref(),
            Some("http://localhost:3000/sign-in?ref=home"),
        );
    }

    #[test]
    fn build_target_url_strips_trailing_slash_from_upstream() {
        let uri: Uri = "/foo".parse().unwrap();
        assert_eq!(
            build_target_url("http://web:3000/", &uri).as_deref(),
            Some("http://web:3000/foo"),
        );
    }

    #[test]
    fn build_target_url_handles_empty_path() {
        let uri: Uri = "/".parse().unwrap();
        assert_eq!(
            build_target_url("http://localhost:3000", &uri).as_deref(),
            Some("http://localhost:3000/"),
        );
    }

    #[test]
    fn build_target_url_rejects_empty_upstream() {
        let uri: Uri = "/anything".parse().unwrap();
        assert!(build_target_url("", &uri).is_none());
    }

    #[test]
    fn hop_by_hop_detection_is_case_insensitive() {
        assert!(is_hop_by_hop(&HeaderName::from_static("connection")));
        assert!(is_hop_by_hop(&HeaderName::from_static("transfer-encoding")));
        assert!(is_hop_by_hop(&HeaderName::from_static("upgrade")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("cookie")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("authorization")));
    }

    #[test]
    fn websocket_upgrade_detection() {
        let mut h = HeaderMap::new();
        h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        assert!(is_websocket_upgrade(&h));

        // Case insensitive.
        let mut h2 = HeaderMap::new();
        h2.insert(header::UPGRADE, HeaderValue::from_static("WebSocket"));
        assert!(is_websocket_upgrade(&h2));

        // Wrong protocol.
        let mut h3 = HeaderMap::new();
        h3.insert(header::UPGRADE, HeaderValue::from_static("h2c"));
        assert!(!is_websocket_upgrade(&h3));

        // No upgrade header.
        assert!(!is_websocket_upgrade(&HeaderMap::new()));
    }
}
