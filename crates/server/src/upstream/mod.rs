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
//! - `X-Forwarded-{For,Proto,Host}` set on the outbound request by the
//!   server, never copied from an untrusted client. `X-Forwarded-For` /
//!   `X-Real-IP` carry the resolved client IP from `RequestContext`
//!   (post-trusted-proxy walk). `X-Forwarded-Host` / `X-Forwarded-Proto`
//!   are kept only when the immediate peer is a trusted proxy
//!   (`RequestContext::peer_trusted`) and the value is well-formed (one
//!   valid `host[:port]`; proto exactly `http` or `https`); otherwise they
//!   are the inbound `Host` and `http`. So a TLS-terminating proxy in
//!   `COMIC_TRUSTED_PROXIES` must set `X-Forwarded-Proto`, and should pass
//!   `Host` through or set `X-Forwarded-Host`. Next builds absolute
//!   redirect URLs from these, so they follow the host the client used
//!   (LAN IP, extra host names) rather than one configured name.
//! - Stripping every `x-middleware-*` header in both directions: those
//!   names are Next.js-internal, so a client may not send them to Next and
//!   Next's own may not reach the client.
//! - Streaming response body back to the caller without buffering.
//! - 502 on upstream connection failure or timeout, with the standard
//!   `{"error": {"code", "message"}}` envelope and a fixed message; the
//!   transport detail goes to the server log only.
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
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri, header},
    response::Response,
};

use crate::middleware::request_context::RequestContext;
use crate::middleware::security_headers::CspHeader;
use crate::state::AppState;

/// Per-request upstream timeout. SSR pages are usually fast (<1 s)
/// but allow up to 60 s for cold-start re-renders or slow data
/// fetches inside `getServerSideProps`. M2 may parameterize this if
/// any specific route needs a different bound; for the catch-all
/// fallback, a single value is fine.
pub const PROXY_TIMEOUT: Duration = Duration::from_secs(60);

/// Request-body cap on everything the fallback forwards to Next (N-A2).
/// What Next receives through here is page navigations (no body), Server
/// Actions and the odd form post: kilobytes, not megabytes. 64 MiB is far
/// above any of that and well below what lets one client tie up the
/// proxy with an endless upload inside [`PROXY_TIMEOUT`]. A declared
/// `Content-Length` over the cap is refused before anything reaches Next;
/// a chunked body is counted as it streams and the request is cut off at
/// the cap. Both answer 413 with the canonical envelope (`too_large`).
pub const MAX_PROXY_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Marker error for a streamed request body that crossed the cap; found
/// again in the reqwest error's source chain to answer 413, not 502.
#[derive(Debug)]
struct ProxyBodyTooLarge;

impl std::fmt::Display for ProxyBodyTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request body exceeds the proxy limit")
    }
}

impl std::error::Error for ProxyBodyTooLarge {}

fn body_too_large() -> Response {
    crate::api::respond(
        StatusCode::PAYLOAD_TOO_LARGE,
        shared::error::ApiErrorCode::TooLarge,
        "request body exceeds the size limit",
    )
}

fn is_body_too_large(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if e.is::<ProxyBodyTooLarge>() {
            return true;
        }
        cur = e.source();
    }
    false
}

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
    let (client_ip, peer_trusted) = req
        .extensions()
        .get::<RequestContext>()
        .map_or((None, false), |c| (c.client_ip, c.peer_trusted));
    if is_websocket_upgrade(req.headers()) {
        return proxy_websocket(&cfg.web_upstream_url, req, client_ip, peer_trusted).await;
    }
    let csp = req.extensions().get::<CspHeader>().map(|c| c.0.clone());
    forward(
        &app.web_proxy_client,
        &cfg.web_upstream_url,
        req,
        PROXY_TIMEOUT,
        client_ip,
        peer_trusted,
        csp,
    )
    .await
}

/// Forward a request to `upstream_url + path_and_query`. Pure helper
/// in the sense that it does not touch `AppState`, which makes it
/// drive-testable against a wiremock upstream without booting the
/// server. The state-bound `proxy()` handler is the only call site
/// in production; the function is `pub` so the integration suite at
/// `tests/upstream.rs` can exercise it directly. `peer_trusted` says
/// whether the immediate peer is a trusted proxy, whose
/// `X-Forwarded-Host` / `X-Forwarded-Proto` may then be kept.
pub async fn forward(
    client: &reqwest::Client,
    upstream_url: &str,
    req: Request<Body>,
    timeout: Duration,
    client_ip: Option<IpAddr>,
    peer_trusted: bool,
    csp: Option<HeaderValue>,
) -> Response {
    forward_with_body_limit(
        client,
        upstream_url,
        req,
        timeout,
        client_ip,
        peer_trusted,
        csp,
        MAX_PROXY_BODY_BYTES,
    )
    .await
}

/// [`forward`] with an explicit request-body cap in bytes. Production
/// always goes through [`forward`] ([`MAX_PROXY_BODY_BYTES`]); the
/// parameter exists so `tests/body_limits.rs` can exercise the streamed
/// cut-off without pushing 64 MiB through a mock.
#[allow(clippy::too_many_arguments)]
pub async fn forward_with_body_limit(
    client: &reqwest::Client,
    upstream_url: &str,
    req: Request<Body>,
    timeout: Duration,
    client_ip: Option<IpAddr>,
    peer_trusted: bool,
    csp: Option<HeaderValue>,
    max_body: u64,
) -> Response {
    let declared_len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared_len.is_some_and(|n| n > max_body) {
        return body_too_large();
    }

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

    // Build the outbound header set: inbound minus hop-by-hop / Host /
    // Content-Length / client-supplied forwarding and `x-middleware-*`
    // headers, with `X-Forwarded-*` set from the resolved client IP (SEC-6)
    // and, for host / proto, the trusted proxy's values or our own view.
    let mut outbound_headers = build_forwarded_headers(&parts.headers, client_ip, peer_trusted);

    // Content-Security-Policy: forward the per-request CSP value
    // (built in `security_headers::set_headers` with the nonce slot
    // populated) to Next.js as a *request* header. Next's app-render
    // parses `'nonce-XXX'` out of this header and auto-stamps the
    // same nonce onto every framework-emitted `<script>` tag — see
    // `node_modules/next/dist/server/app-render/get-script-nonce-
    // from-header.js`. Strip any inbound value first so a malicious
    // client can't smuggle their own policy.
    outbound_headers.remove("content-security-policy");
    outbound_headers.remove("content-security-policy-report-only");
    if let Some(v) = csp {
        outbound_headers.insert(HeaderName::from_static("content-security-policy"), v);
    }

    // Build + stream.
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .expect("axum http::Method round-trips to reqwest");
    // Count bytes as they stream; past the cap the stream yields
    // `ProxyBodyTooLarge`, which aborts the outbound request and is mapped
    // to 413 below. Nothing is buffered.
    let mut seen: u64 = 0;
    let body_stream = futures::StreamExt::map(body.into_data_stream(), move |chunk| {
        let chunk = chunk.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        seen = seen.saturating_add(chunk.len() as u64);
        if seen > max_body {
            return Err(Box::new(ProxyBodyTooLarge) as Box<dyn std::error::Error + Send + Sync>);
        }
        Ok(chunk)
    });
    let request = client
        .request(method, &target)
        .headers(outbound_headers)
        .timeout(timeout)
        .body(reqwest::Body::wrap_stream(body_stream));

    let upstream_resp = match request.send().await {
        Ok(r) => r,
        Err(e) if is_body_too_large(&e) => return body_too_large(),
        Err(e) => {
            let (code, message) = if e.is_timeout() {
                ("upstream_timeout", "The web app did not respond in time")
            } else if e.is_connect() {
                ("upstream_unreachable", "The web app is not reachable")
            } else {
                ("upstream_error", "The web app request failed")
            };
            // The detail stays in the server log. `without_url` drops the
            // request URL from the error: its query string can hold a
            // single-use token (a reset or verification link).
            tracing::warn!(
                code,
                upstream = %upstream_url,
                error = ?e.without_url(),
                "upstream proxy request failed"
            );
            return error_response(StatusCode::BAD_GATEWAY, code, message);
        }
    };

    // Translate upstream → axum response. Status, headers (minus
    // hop-by-hop), then stream the body.
    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream_resp.headers().iter() {
        if is_hop_by_hop(name) || is_next_internal(name) {
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

/// Next.js-internal header names (`x-middleware-subrequest`,
/// `x-middleware-rewrite`, `x-middleware-set-cookie`, ...). Next uses them
/// between its own layers; neither a client nor the outside world has any
/// business sending or seeing them, so the proxy drops them both ways.
fn is_next_internal(name: &HeaderName) -> bool {
    name.as_str().starts_with("x-middleware-")
}

/// Request headers the proxy never copies from the client because it sets
/// them itself (the forwarding set) or because they are Next-internal.
fn is_client_forwarding_or_internal(name: &HeaderName) -> bool {
    name == header::FORWARDED
        || matches!(
            name.as_str(),
            "x-forwarded-for" | "x-forwarded-host" | "x-forwarded-proto" | "x-real-ip"
        )
        || is_next_internal(name)
}

/// The single value of `name` when exactly one is present and it is
/// visible ASCII.
fn single_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut it = headers.get_all(name).iter();
    let v = it.next()?;
    if it.next().is_some() {
        return None;
    }
    v.to_str().ok().map(str::trim)
}

/// A well-formed `host[:port]` authority: parses as an `http::uri::Authority`
/// with no userinfo and no list separator.
fn valid_host(v: &str) -> bool {
    !v.is_empty()
        && !v.contains([',', '@', ' ', '/'])
        && v.parse::<axum::http::uri::Authority>().is_ok()
}

/// `(proto, host)` for `X-Forwarded-Proto` / `X-Forwarded-Host`.
///
/// A trusted proxy's own values are kept when well-formed (one value; a
/// valid `host[:port]`; proto exactly `http` or `https`). Anything else
/// (an untrusted peer, an absent or malformed value) falls back to the
/// inbound `Host` and `http`: the Rust server only listens on a plain TCP
/// socket (`axum::serve` over `TcpListener` in `app::serve`) and never
/// terminates TLS itself, so the scheme it received is always `http`.
fn forwarded_host_proto(inbound: &HeaderMap, peer_trusted: bool) -> (&str, Option<&str>) {
    let from_proxy = |name| {
        if peer_trusted {
            single_value(inbound, name)
        } else {
            None
        }
    };
    let proto = from_proxy("x-forwarded-proto")
        .filter(|p| matches!(*p, "http" | "https"))
        .unwrap_or("http");
    let host = from_proxy("x-forwarded-host")
        .filter(|h| valid_host(h))
        .or_else(|| single_value(inbound, "host").filter(|h| valid_host(h)));
    (proto, host)
}

/// Set the forwarding headers on `out`, which must already have had every
/// client-supplied copy removed. `inbound` is the original request's
/// header map (read for `Host` and a trusted proxy's values).
fn insert_forwarding(
    out: &mut HeaderMap,
    inbound: &HeaderMap,
    client_ip: Option<IpAddr>,
    peer_trusted: bool,
) {
    let (proto, host) = forwarded_host_proto(inbound, peer_trusted);
    if let Ok(v) = HeaderValue::from_str(proto) {
        out.insert(HeaderName::from_static("x-forwarded-proto"), v);
    }
    if let Some(v) = host.and_then(|h| HeaderValue::from_str(h).ok()) {
        out.insert(HeaderName::from_static("x-forwarded-host"), v);
    }
    if let Some(ip) = client_ip
        && let Ok(v) = HeaderValue::from_str(&ip.to_string())
    {
        out.insert(HeaderName::from_static("x-forwarded-for"), v.clone());
        out.insert(HeaderName::from_static("x-real-ip"), v);
    }
}

/// Construct the outbound header set for a proxied request.
///
/// Copies the inbound headers minus hop-by-hop, `Host`, `Content-Length`, the
/// client-supplied forwarding headers (`Forwarded`, `X-Forwarded-For`,
/// `X-Forwarded-Host`, `X-Forwarded-Proto`, `X-Real-IP`) and every
/// Next-internal `x-middleware-*` header, then sets the forwarding set
/// itself. `X-Forwarded-For` / `X-Real-IP` carry exactly the one client IP
/// resolved via the trusted-proxy walk (`xff::client_ip`, SEC-6), so a
/// client cannot hand Next a spoofed chain. `X-Forwarded-Host` /
/// `X-Forwarded-Proto` follow [`forwarded_host_proto`]: a trusted proxy's
/// well-formed values, else the inbound `Host` and `http`. CSP injection is
/// left to the caller (it depends on the per-request nonce value).
fn build_forwarded_headers(
    inbound: &HeaderMap,
    client_ip: Option<IpAddr>,
    peer_trusted: bool,
) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in inbound.iter() {
        if is_hop_by_hop(name)
            || name == header::HOST
            || name == header::CONTENT_LENGTH
            || is_client_forwarding_or_internal(name)
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    insert_forwarding(&mut out, inbound, client_ip, peer_trusted);
    out
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
    crate::api::error(status, code, message)
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
///
/// The upgrade request gets the same forwarding treatment as [`forward`]:
/// client-supplied forwarding and `x-middleware-*` headers are dropped and
/// the forwarding set is built from `client_ip` and `peer_trusted`. The
/// handshake-critical `Connection` / `Upgrade` / `Sec-WebSocket-*` headers
/// pass through untouched, which is all Next dev's hot-reload socket needs.
pub async fn proxy_websocket(
    upstream_url: &str,
    req: Request<Body>,
    client_ip: Option<IpAddr>,
    peer_trusted: bool,
) -> Response {
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
            tracing::warn!(upstream = %upstream_url, error = %e, "ws proxy: upstream connect failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                "The web app is not reachable",
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
            tracing::warn!(upstream = %upstream_url, error = %e, "ws proxy: upstream handshake failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_handshake_failed",
                "The web app connection failed",
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
    let mut outbound = HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if name == header::HOST
            || name == header::CONTENT_LENGTH
            || is_client_forwarding_or_internal(name)
        {
            continue;
        }
        outbound.append(name.clone(), value.clone());
    }
    insert_forwarding(&mut outbound, &parts.headers, client_ip, peer_trusted);
    let mut builder = Request::builder()
        .method(parts.method.clone())
        .uri(path_q)
        .version(hyper::Version::HTTP_11);
    for (name, value) in outbound.iter() {
        builder = builder.header(name, value);
    }
    builder = builder.header(header::HOST, &host_header);

    let upstream_req = match builder.body(Body::empty()) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "ws proxy: upgrade request build failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ws_request_build_failed",
                "Could not build the upgrade request",
            );
        }
    };

    // 6. Send and inspect the response.
    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(upstream = %upstream_url, error = %e, "ws proxy: upgrade request failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_send_failed",
                "The web app connection failed",
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
        if is_next_internal(name) {
            continue;
        }
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
    fn forwarded_headers_strip_client_supplied_chain() {
        // SEC-6: a direct client sends a spoofed XFF chain + X-Real-IP +
        // Forwarded. The outbound set must drop all of them and carry only the
        // single resolved client IP.
        let mut inbound = HeaderMap::new();
        inbound.insert(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("1.2.3.4, 5.6.7.8"),
        );
        inbound.insert(
            HeaderName::from_static("x-real-ip"),
            HeaderValue::from_static("1.2.3.4"),
        );
        inbound.insert(header::FORWARDED, HeaderValue::from_static("for=1.2.3.4"));
        inbound.insert(header::COOKIE, HeaderValue::from_static("session=abc"));

        let resolved: IpAddr = "203.0.113.7".parse().unwrap();
        let out = build_forwarded_headers(&inbound, Some(resolved), false);

        // Exactly one XFF entry, equal to the resolved IP — no spoofed hops.
        let xff: Vec<_> = out.get_all("x-forwarded-for").iter().collect();
        assert_eq!(xff.len(), 1);
        assert_eq!(xff[0], "203.0.113.7");
        assert_eq!(out.get("x-real-ip").unwrap(), "203.0.113.7");
        // Inbound Forwarded is dropped entirely; Cookie still flows through.
        assert!(out.get(header::FORWARDED).is_none());
        assert_eq!(out.get(header::COOKIE).unwrap(), "session=abc");
    }

    #[test]
    fn forwarded_headers_omit_xff_when_ip_unresolved() {
        // With no resolved client IP, no XFF/X-Real-IP is emitted (and any
        // inbound spoof was still stripped).
        let mut inbound = HeaderMap::new();
        inbound.insert(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("9.9.9.9"),
        );
        let out = build_forwarded_headers(&inbound, None, false);
        assert!(out.get("x-forwarded-for").is_none());
        assert!(out.get("x-real-ip").is_none());
    }

    fn hdrs(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        h
    }

    fn host_proto(out: &HeaderMap) -> (Vec<&str>, Vec<&str>) {
        let all = |n| {
            out.get_all(n)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>()
        };
        (all("x-forwarded-host"), all("x-forwarded-proto"))
    }

    #[test]
    fn untrusted_peer_gets_inbound_host_and_http() {
        let inbound = hdrs(&[
            ("host", "192.168.1.20:8080"),
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded-proto", "https"),
        ]);
        let out = build_forwarded_headers(&inbound, None, false);
        assert_eq!(host_proto(&out), (vec!["192.168.1.20:8080"], vec!["http"]));
    }

    #[test]
    fn trusted_peer_values_are_kept() {
        let inbound = hdrs(&[
            ("host", "folio:8080"),
            ("x-forwarded-host", "comics.example.com"),
            ("x-forwarded-proto", "https"),
        ]);
        let out = build_forwarded_headers(&inbound, None, true);
        assert_eq!(
            host_proto(&out),
            (vec!["comics.example.com"], vec!["https"])
        );
    }

    #[test]
    fn trusted_peer_without_values_falls_back() {
        let inbound = hdrs(&[("host", "comics.example.com")]);
        let out = build_forwarded_headers(&inbound, None, true);
        assert_eq!(host_proto(&out), (vec!["comics.example.com"], vec!["http"]));
    }

    #[test]
    fn trusted_peer_malformed_values_fall_back() {
        for (fh, fp) in [
            ("comics.example.com", "javascript"),
            ("comics.example.com", "HTTPS"),
            ("a.example, b.example", "https"),
            ("user@evil.example", "https"),
            ("evil.example/path", "https"),
            ("", "https"),
        ] {
            let mut inbound = hdrs(&[("host", "folio.lan:8080")]);
            inbound.insert(
                HeaderName::from_static("x-forwarded-host"),
                HeaderValue::from_str(fh).unwrap(),
            );
            inbound.insert(
                HeaderName::from_static("x-forwarded-proto"),
                HeaderValue::from_str(fp).unwrap(),
            );
            let (proto, host) = forwarded_host_proto(&inbound, true);
            let want_host = if valid_host(fh) { fh } else { "folio.lan:8080" };
            let want_proto = if fp == "https" { "https" } else { "http" };
            assert_eq!((host, proto), (Some(want_host), want_proto), "{fh} / {fp}");
        }
        // Two separate X-Forwarded-Host / -Proto lines are not one value.
        let inbound = hdrs(&[
            ("host", "folio.lan:8080"),
            ("x-forwarded-host", "a.example"),
            ("x-forwarded-host", "b.example"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-proto", "http"),
        ]);
        let out = build_forwarded_headers(&inbound, None, true);
        assert_eq!(host_proto(&out), (vec!["folio.lan:8080"], vec!["http"]));
    }

    #[test]
    fn next_internal_headers_are_stripped() {
        let mut inbound = HeaderMap::new();
        for n in [
            "x-middleware-subrequest",
            "x-middleware-rewrite",
            "x-middleware-next",
            "x-middleware-prefetch",
        ] {
            inbound.insert(HeaderName::from_static(n), HeaderValue::from_static("1"));
        }
        inbound.insert(
            HeaderName::from_static("x-nextjs-data"),
            HeaderValue::from_static("1"),
        );
        let out = build_forwarded_headers(&inbound, None, false);
        assert!(
            out.keys().all(|k| !k.as_str().starts_with("x-middleware-")),
            "{out:?}"
        );
        // Only the `x-middleware-` prefix is internal; other Next headers pass.
        assert_eq!(out.get("x-nextjs-data").unwrap(), "1");
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
