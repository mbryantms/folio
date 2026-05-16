//! End-to-end test for the catch-all `upstream::proxy` fallback wired
//! into the axum router (M2 of the rust-public-origin plan,
//! `~/.claude/plans/rust-public-origin-1.0.md`).
//!
//! The lightweight unit tests for the proxy module itself live in
//! `tests/upstream.rs` — they drive `upstream::forward()` directly
//! without booting the server. This file boots a full `TestApp`
//! (Postgres + Redis via testcontainers) with the SSR upstream URL
//! pointed at a wiremock instance, and verifies routing precedence:
//!
//! 1. Requests to explicit Rust routes (e.g. `/healthz`) are handled
//!    by Rust, never proxied.
//! 2. Requests to unknown paths fall through to the SSR proxy and
//!    reach the wiremock upstream with path + query + body intact.
//! 3. The upstream's response (status, headers, body) is forwarded
//!    back to the originating client verbatim.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn explicit_rust_route_does_not_hit_fallback() {
    // Upstream URL points at a guaranteed-dead address — if the
    // fallback fired we'd get a 502 instead of healthz's 200.
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/healthz is owned by Rust and must skip the proxy"
    );
}

#[tokio::test]
async fn unmatched_path_falls_through_to_ssr_upstream() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/some/spa-route"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("x-from-upstream", "yes")
                .set_body_string("<html><body>hello from next</body></html>"),
        )
        .mount(&upstream)
        .await;

    let app = TestApp::spawn_with_web_upstream(upstream.uri()).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/some/spa-route?foo=bar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("x-from-upstream").map(|v| v.as_bytes()),
        Some(&b"yes"[..])
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(
        body.starts_with(b"<html"),
        "fallback must forward the upstream body verbatim, got: {:?}",
        std::str::from_utf8(&body[..body.len().min(80)]).unwrap_or("<binary>"),
    );
}

#[tokio::test]
async fn fallback_returns_502_envelope_when_upstream_is_dead() {
    // Bind a port then drop the listener so it's guaranteed-dead.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let app = TestApp::spawn_with_web_upstream(dead).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // The fallback uses the standard error envelope so client code
    // parsing JSON errors won't choke on an HTML body.
    assert_eq!(v["error"]["code"], "upstream_unreachable");
}

#[tokio::test]
async fn ws_upgrade_via_oneshot_returns_400_no_upgrade_extension() {
    // `tower::ServiceExt::oneshot` bypasses hyper's server layer, so
    // the `OnUpgrade` extension hyper normally adds for `Upgrade:`
    // requests is absent. `upstream::proxy_websocket` detects that
    // and bails with a 400 instead of trying to bridge. The actual
    // byte-bridge path is exercised by
    // `ws_upgrade_byte_bridge_round_trips_payload` below, which runs
    // the router behind a real `axum::serve` so hyper populates the
    // upgrade extension.
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/some/ws-target")
                .header(header::UPGRADE, "websocket")
                .header(header::CONNECTION, "upgrade")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "ws_no_upgrade_extension");
}

#[tokio::test]
async fn ws_upgrade_byte_bridge_round_trips_payload() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    // 1. Stand up a tiny "upstream" listener that:
    //    - Accepts one connection.
    //    - Reads the HTTP/1.1 upgrade request (until \r\n\r\n).
    //    - Writes back a valid 101 Switching Protocols response with
    //      a placeholder Sec-WebSocket-Accept (hyper doesn't validate
    //      the digest against the request key — it only checks for
    //      the upgrade-completion shape).
    //    - Then byte-echoes anything that comes in until EOF.
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = upstream.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let mut read = 0;
        // Drain the HTTP request.
        loop {
            let n = sock.read(&mut buf[read..]).await.unwrap();
            if n == 0 {
                return;
            }
            read += n;
            if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let response = "HTTP/1.1 101 Switching Protocols\r\n\
                        Connection: upgrade\r\n\
                        Upgrade: websocket\r\n\
                        Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
                        \r\n";
        sock.write_all(response.as_bytes()).await.unwrap();
        // Echo loop.
        let mut echo_buf = vec![0u8; 1024];
        loop {
            let n = match sock.read(&mut echo_buf).await {
                Ok(0) => return,
                Ok(n) => n,
                Err(_) => return,
            };
            if sock.write_all(&echo_buf[..n]).await.is_err() {
                return;
            }
        }
    });

    // 2. Boot the app pointed at the upstream listener, behind a
    // real TCP socket so hyper populates the upgrade extension.
    let upstream_url = format!("http://{upstream_addr}");
    let app = TestApp::spawn_with_web_upstream(upstream_url).await;
    let app_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app_addr = app_listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(app_listener, router).await;
    });

    // 3. Connect as a raw TCP client, send a WS upgrade request, then
    // a tiny payload. Verify the echo.
    let mut client = TcpStream::connect(app_addr).await.unwrap();
    let request = format!(
        "GET /some/ws-target HTTP/1.1\r\n\
         Host: {app_addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    client.write_all(request.as_bytes()).await.unwrap();

    // Read the response headers (until \r\n\r\n).
    let mut buf = vec![0u8; 4096];
    let mut read = 0;
    let header_end = loop {
        let n = client.read(&mut buf[read..]).await.unwrap();
        assert!(n > 0, "upstream closed before 101");
        read += n;
        if let Some(pos) = buf[..read].windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = std::str::from_utf8(&buf[..header_end]).unwrap();
    assert!(
        head.starts_with("HTTP/1.1 101 "),
        "expected 101 Switching Protocols, got: {head:?}"
    );
    assert!(head.to_ascii_lowercase().contains("upgrade: websocket"));

    // Anything past `header_end` is bridge data, but nothing was sent
    // before our request so the buffer is exactly the headers.
    assert_eq!(read, header_end);

    // 4. Send a payload through the bridge and read it back.
    client.write_all(b"PING-PROXY").await.unwrap();
    let mut echo = [0u8; 10];
    client.read_exact(&mut echo).await.unwrap();
    assert_eq!(&echo, b"PING-PROXY");

    // 5. Send EOF; the bridge should close cleanly.
    client.shutdown().await.unwrap();
}
