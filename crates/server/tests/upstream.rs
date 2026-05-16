//! Integration tests for the `upstream::proxy` reverse proxy (M1 of
//! the rust-public-origin plan,
//! `~/.claude/plans/rust-public-origin-1.0.md`).
//!
//! These exercise `forward_anon()` directly against a wiremock
//! upstream — no DB, no Redis, no full `TestApp::spawn()`. Each test
//! spins up a fresh `MockServer` (random port), builds a minimal
//! `reqwest::Client` matching the production one (no redirect
//! following), constructs an axum `Request<Body>`, and asserts on
//! both the proxy's response and the upstream's observed request.

use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderValue, Method, Request, StatusCode, header},
};
use server::upstream;
use wiremock::matchers::{header as wm_header, method, path};
use wiremock::{Mock, MockServer, Request as WmRequest, ResponseTemplate};

fn proxy_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn req_get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "folio.example.com")
        .body(Body::empty())
        .unwrap()
}

/// Tests that don't care about the client-IP XFF append call through
/// this thin wrapper rather than passing `None` on every invocation.
async fn forward_anon(
    client: &reqwest::Client,
    upstream_url: &str,
    req: Request<Body>,
    timeout: Duration,
) -> axum::response::Response {
    upstream::forward(client, upstream_url, req, timeout, None).await
}

#[tokio::test]
async fn forwards_get_request_with_path_and_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sign-in"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/sign-in?ref=home"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"hello");
}

#[tokio::test]
async fn streams_large_response_body() {
    let server = MockServer::start().await;
    // 2 MiB body — confirms we don't buffer everything into memory
    // before responding. wiremock serves it as a single Vec<u8> so
    // the streaming win is on our side, but the size is enough to
    // demonstrate the path doesn't choke.
    let large: Vec<u8> = (0..(2 * 1024 * 1024)).map(|i| (i % 256) as u8).collect();
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(large.clone()))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/big"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.len(), large.len());
    assert_eq!(&body[..], &large[..]);
}

#[tokio::test]
async fn forwards_post_body_to_upstream() {
    let server = MockServer::start().await;
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let received_for_handler = received.clone();
    Mock::given(method("POST"))
        .and(path("/api/echo"))
        .respond_with(move |req: &WmRequest| {
            *received_for_handler.lock().unwrap() = req.body.clone();
            ResponseTemplate::new(201).set_body_string("created")
        })
        .mount(&server)
        .await;

    let payload = b"hello body".to_vec();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/echo")
        .header(header::HOST, "folio.example.com")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        // Set a Content-Length to confirm we strip it on outbound
        // (otherwise reqwest's chunked-encoded body would conflict).
        .header(header::CONTENT_LENGTH, "10")
        .body(Body::from(payload.clone()))
        .unwrap();
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"created");
    assert_eq!(*received.lock().unwrap(), payload);
}

#[tokio::test]
async fn injects_x_forwarded_proto_default_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(wm_header("x-forwarded-proto", "http"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/x"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn preserves_existing_x_forwarded_proto() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(wm_header("x-forwarded-proto", "https"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, "folio.example.com")
        .header("x-forwarded-proto", "https")
        .body(Body::empty())
        .unwrap();
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn injects_x_forwarded_host_from_inbound_host() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(wm_header("x-forwarded-host", "folio.example.com"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/x"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn preserves_inbound_x_forwarded_for() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(wm_header("x-forwarded-for", "203.0.113.5"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, "folio.example.com")
        .header("x-forwarded-for", "203.0.113.5")
        .body(Body::empty())
        .unwrap();
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn appends_client_ip_to_x_forwarded_for_chain() {
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let observed_for_handler = observed.clone();
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(move |req: &WmRequest| {
            *observed_for_handler.lock().unwrap() = req
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, "folio.example.com")
        .header("x-forwarded-for", "203.0.113.5")
        .body(Body::empty())
        .unwrap();
    let client_ip: std::net::IpAddr = "198.51.100.7".parse().unwrap();
    let resp = upstream::forward(
        &proxy_client(),
        &server.uri(),
        req,
        Duration::from_secs(5),
        Some(client_ip),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Inbound chain `203.0.113.5` + resolved client `198.51.100.7`
    // should arrive as the canonical `original, hop` form.
    assert_eq!(
        observed.lock().unwrap().as_deref(),
        Some("203.0.113.5, 198.51.100.7"),
    );
}

#[tokio::test]
async fn sets_x_forwarded_for_when_no_inbound_chain() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(wm_header("x-forwarded-for", "198.51.100.7"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client_ip: std::net::IpAddr = "198.51.100.7".parse().unwrap();
    let resp = upstream::forward(
        &proxy_client(),
        &server.uri(),
        req_get("/x"),
        Duration::from_secs(5),
        Some(client_ip),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn strips_hop_by_hop_headers_outbound() {
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let observed_for_handler = observed.clone();
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(move |req: &WmRequest| {
            let mut o = observed_for_handler.lock().unwrap();
            for (name, value) in req.headers.iter() {
                o.push((name.to_string(), value.to_str().unwrap_or("").to_string()));
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, "folio.example.com")
        .header(header::CONNECTION, "keep-alive")
        .header("keep-alive", "timeout=5")
        .header("te", "trailers")
        .body(Body::empty())
        .unwrap();
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let seen = observed.lock().unwrap();
    let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("keep-alive")),
        "keep-alive must not be forwarded, got headers: {names:?}",
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("te")),
        "te must not be forwarded, got headers: {names:?}",
    );
}

#[tokio::test]
async fn strips_hop_by_hop_headers_inbound_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("keep-alive", "timeout=5")
                .insert_header("connection", "keep-alive")
                .insert_header("x-app", "kept"),
        )
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/x"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("keep-alive").is_none());
    assert!(resp.headers().get("connection").is_none());
    assert_eq!(
        resp.headers().get("x-app"),
        Some(&HeaderValue::from_static("kept"))
    );
}

#[tokio::test]
async fn forwards_redirects_without_following() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect-me"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/destination"))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/redirect-me"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers().get("location"),
        Some(&HeaderValue::from_static("/destination")),
        "redirect Location header must reach the originating client verbatim",
    );
}

#[tokio::test]
async fn returns_502_on_connection_failure() {
    // Closed port — the bind-and-immediately-drop trick gives us a
    // guaranteed-dead address without taking a long timeout.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let upstream_url = format!("http://{addr}");

    let resp = forward_anon(
        &proxy_client(),
        &upstream_url,
        req_get("/x"),
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "upstream_unreachable");
}

#[tokio::test]
async fn returns_502_on_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;

    let resp = forward_anon(
        &proxy_client(),
        &server.uri(),
        req_get("/slow"),
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "upstream_timeout");
}

#[tokio::test]
async fn empty_upstream_url_returns_500_envelope() {
    let resp = forward_anon(
        &proxy_client(),
        "",
        req_get("/anywhere"),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "upstream_url_invalid");
}
