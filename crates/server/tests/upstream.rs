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
    upstream::forward(client, upstream_url, req, timeout, None, false, None).await
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

type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// Mount a `/x` mock that records every X-Forwarded-Host / -Proto value
/// the upstream receives.
async fn recording_upstream() -> (MockServer, Seen) {
    let server = MockServer::start().await;
    let observed: Seen = Default::default();
    let seen = observed.clone();
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(move |req: &WmRequest| {
            let mut got = seen.lock().unwrap();
            for name in ["x-forwarded-host", "x-forwarded-proto"] {
                for v in req.headers.get_all(name).iter() {
                    got.push((name.to_owned(), v.to_str().unwrap_or("").to_owned()));
                }
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    (server, observed)
}

/// Send `/x` with `Host: host` plus `extra` headers; return the recorded
/// (sorted) X-Forwarded-Host / -Proto values.
async fn forwarded_seen(
    host: &str,
    extra: &[(&str, &str)],
    peer_trusted: bool,
) -> Vec<(String, String)> {
    let (server, observed) = recording_upstream().await;
    let mut b = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, host);
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let resp = upstream::forward(
        &proxy_client(),
        &server.uri(),
        b.body(Body::empty()).unwrap(),
        Duration::from_secs(5),
        None,
        peer_trusted,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let mut got = observed.lock().unwrap().clone();
    got.sort();
    got
}

fn pair(host: &str, proto: &str) -> Vec<(String, String)> {
    vec![
        ("x-forwarded-host".to_owned(), host.to_owned()),
        ("x-forwarded-proto".to_owned(), proto.to_owned()),
    ]
}

#[tokio::test]
async fn untrusted_peer_client_forwarded_host_and_proto_are_replaced() {
    let got = forwarded_seen(
        "folio.example.com",
        &[
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded-proto", "https"),
        ],
        false,
    )
    .await;
    assert_eq!(got, pair("folio.example.com", "http"));
}

#[tokio::test]
async fn trusted_peer_forwarded_host_and_proto_are_kept() {
    let got = forwarded_seen(
        "folio:8080",
        &[
            ("x-forwarded-host", "comics.example.com"),
            ("x-forwarded-proto", "https"),
        ],
        true,
    )
    .await;
    assert_eq!(got, pair("comics.example.com", "https"));
}

#[tokio::test]
async fn trusted_peer_without_forwarded_values_uses_host_and_http() {
    let got = forwarded_seen("192.168.1.20:8080", &[], true).await;
    assert_eq!(got, pair("192.168.1.20:8080", "http"));
}

#[tokio::test]
async fn trusted_peer_malformed_forwarded_values_fall_back() {
    let got = forwarded_seen(
        "folio.lan:8080",
        &[
            ("x-forwarded-host", "comics.example.com"),
            ("x-forwarded-proto", "javascript"),
        ],
        true,
    )
    .await;
    assert_eq!(got, pair("comics.example.com", "http"));

    let got = forwarded_seen(
        "folio.lan:8080",
        &[
            ("x-forwarded-host", "a.example, b.example"),
            ("x-forwarded-proto", "https"),
        ],
        true,
    )
    .await;
    assert_eq!(got, pair("folio.lan:8080", "https"));

    let got = forwarded_seen(
        "folio.lan:8080",
        &[
            ("x-forwarded-host", "a.example"),
            ("x-forwarded-host", "b.example"),
        ],
        true,
    )
    .await;
    assert_eq!(got, pair("folio.lan:8080", "http"));
}

#[tokio::test]
async fn strips_client_x_middleware_headers() {
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = observed.clone();
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(move |req: &WmRequest| {
            seen.lock()
                .unwrap()
                .extend(req.headers.keys().map(|k| k.as_str().to_owned()));
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/x")
        .header(header::HOST, "folio.example.com")
        .header(
            "x-middleware-subrequest",
            "middleware:middleware:middleware",
        )
        .header("x-middleware-rewrite", "/admin")
        .header("x-custom", "kept")
        .body(Body::empty())
        .unwrap();
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let names = observed.lock().unwrap().clone();
    assert!(
        names.iter().all(|n| !n.starts_with("x-middleware-")),
        "x-middleware-* reached the upstream: {names:?}"
    );
    assert!(names.iter().any(|n| n == "x-custom"), "{names:?}");
}

#[tokio::test]
async fn strips_upstream_x_middleware_response_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-middleware-rewrite", "/en/x")
                .insert_header("x-middleware-set-cookie", "a=b")
                .insert_header("x-nextjs-cache", "HIT"),
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
    assert!(
        resp.headers()
            .keys()
            .all(|k| !k.as_str().starts_with("x-middleware-")),
        "{:?}",
        resp.headers()
    );
    assert_eq!(resp.headers().get("x-nextjs-cache").unwrap(), "HIT");
}

#[tokio::test]
async fn strips_inbound_x_forwarded_for_when_unresolved() {
    // SEC-6: a client-supplied XFF must never reach the upstream. With an
    // anonymous forward (no resolved client IP), the inbound chain is dropped
    // and no XFF is injected — the upstream sees none.
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Some("sentinel".to_string())));
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
    let resp = forward_anon(&proxy_client(), &server.uri(), req, Duration::from_secs(5)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(observed.lock().unwrap().as_deref(), None);
}

#[tokio::test]
async fn replaces_inbound_x_forwarded_for_with_resolved_ip() {
    // SEC-6: the inbound (spoofable) chain is dropped and replaced by the single
    // resolved client IP — never the `inbound, hop` append form.
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
        false,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Only the resolved client survives — the spoofed `203.0.113.5` is gone.
    assert_eq!(observed.lock().unwrap().as_deref(), Some("198.51.100.7"));
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
        false,
        None,
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
    let text = String::from_utf8_lossy(&body);
    // The body names no internal address: not the upstream URL, its port,
    // nor the request path.
    assert!(!text.contains(&upstream_url), "{text}");
    assert!(!text.contains(&addr.port().to_string()), "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "upstream_unreachable");
    assert_eq!(v["error"]["message"], "The web app is not reachable");
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
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains(&server.uri()), "{text}");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "upstream_timeout");
    assert_eq!(v["error"]["message"], "The web app did not respond in time");
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

#[tokio::test]
async fn forwards_csp_to_upstream_as_request_header() {
    // `forward()` should set `content-security-policy` on the outbound
    // request when the caller supplies a CSP value (built per-request
    // from the nonce middleware + `CspTemplate`). Next.js's app-render
    // parses the `'nonce-XXX'` substring out of this request header
    // and stamps the same value onto every framework-emitted
    // `<script nonce=…>` tag.
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let captured = observed.clone();
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(move |req: &WmRequest| {
            let mut o = captured.lock().unwrap();
            *o = req
                .headers
                .get("content-security-policy")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    let csp = HeaderValue::from_static(
        "default-src 'self'; script-src 'self' 'nonce-rPRfMmebTC4PFD-fThYBVw' 'strict-dynamic'",
    );
    let resp = upstream::forward(
        &proxy_client(),
        &server.uri(),
        req_get("/page"),
        Duration::from_secs(5),
        None,
        false,
        Some(csp),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        observed
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|s| s.contains("'nonce-rPRfMmebTC4PFD-fThYBVw'")),
        "expected nonce to flow in the request CSP header",
    );
}

#[tokio::test]
async fn strips_inbound_csp_header_from_client() {
    // A malicious client must not be able to inject their own CSP via
    // an inbound `content-security-policy` request header — Next.js
    // would otherwise parse THEIR nonce instead of ours. forward()
    // must drop the inbound value before optionally setting our own.
    let server = MockServer::start().await;
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let captured = observed.clone();
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(move |req: &WmRequest| {
            let mut o = captured.lock().unwrap();
            *o = req
                .headers
                .get("content-security-policy")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;

    // Inbound request carries a forged CSP; we pass `None` to forward()
    // to simulate the case where the security-headers middleware
    // didn't run — the upstream must not see the forged value.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/page")
        .header(header::HOST, "folio.example.com")
        .header("content-security-policy", "script-src 'nonce-attacker'")
        .body(Body::empty())
        .unwrap();
    let resp = upstream::forward(
        &proxy_client(),
        &server.uri(),
        req,
        Duration::from_secs(5),
        None,
        false,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(observed.lock().unwrap().as_deref(), None);
}
