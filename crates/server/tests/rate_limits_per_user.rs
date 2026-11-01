//! E3 (audit 2026-11 N-A5) — per-user rate limits on free-text search,
//! metadata PATCH, collection writes and collection reorder.
//!
//! For one representative route of each group: driving one user past the
//! bucket gives a 429 in the canonical envelope, and a second user (same
//! client IP, as every `oneshot` request is) still gets through, because
//! the key is the user id of a verified access token, not the IP. The
//! buckets are compile-time constants (like every other bucket), so each
//! test drains the real burst with cheap requests that miss: the governor
//! runs before the handler, so a 404 spends a token the same as a 200.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    Authed {
        session: extract_cookie(&resp, "__Host-comic_session"),
        csrf: extract_cookie(&resp, "__Host-comic_csrf"),
    }
}

/// One request; returns status, `Retry-After`, and the JSON body.
async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let req = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        retry_after,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Send the request as `auth` until the bucket answers 429 and return the
/// 429 body. At least `burst` requests must get through first (the bucket
/// refills while the loop runs, so usually a few more do). Panics if it
/// never trips.
async fn drain(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<Value>,
    burst: usize,
) -> Value {
    // `allowed` = requests that got through before this one.
    for allowed in 0..(burst * 10) {
        let (s, retry_after, b) = http(app, method.clone(), uri, auth, body.clone()).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            assert!(
                allowed >= burst,
                "{method} {uri} tripped after only {allowed} requests (burst {burst})"
            );
            assert!(retry_after.is_some(), "429 carries Retry-After");
            return b;
        }
    }
    panic!("{method} {uri}: bucket never tripped");
}

fn assert_envelope(body: &Value, bucket: &str) {
    assert_eq!(body["error"]["code"], "rate_limited", "{body:#}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains(bucket), "message names {bucket}: {msg}");
    assert!(
        body["error"]["retry_after_seconds"].as_u64().unwrap_or(0) >= 1,
        "{body:#}"
    );
}

async fn two_users() -> (TestApp, Authed, Authed) {
    let app = TestApp::spawn().await;
    // First user becomes the admin, so the PATCH case reaches the handler
    // for the first user and stops at the admin gate (or 404) for the
    // second; either way neither status is a 429 until the bucket says so.
    let a = register(&app, "first@example.com").await;
    let b = register(&app, "second@example.com").await;
    (app, a, b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_bucket_is_per_user() {
    let (app, a, b) = two_users().await;
    let uri = "/api/issues/search?q=batman";
    let body = drain(&app, &a, Method::GET, uri, None, 60).await;
    assert_envelope(&body, "search");

    // The second user has a budget of their own.
    let (s, _, _) = http(&app, Method::GET, uri, &b, None).await;
    assert_eq!(s, StatusCode::OK, "second user is not throttled");

    // `/issues` shares the `/issues/search` instance; another family
    // (`/people`) has its own.
    let (s, _, _) = http(&app, Method::GET, "/api/people?q=moore", &a, None).await;
    assert_eq!(s, StatusCode::OK, "other search families keep their budget");
    // Reads outside the bucket are untouched.
    let (s, _, _) = http(&app, Method::GET, "/api/me/collections", &a, None).await;
    assert_eq!(s, StatusCode::OK);
}

/// A forged or missing token keys to the client IP, never to a user: an
/// anonymous flood from the same address can't spend a signed-in user's
/// budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_tokens_do_not_drain_a_users_bucket() {
    let (app, a, _b) = two_users().await;
    let forged = Authed {
        session: "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJ4In0.c2ln".into(),
        csrf: a.csrf.clone(),
    };
    let uri = "/api/issues/search?q=batman";
    let body = drain(&app, &forged, Method::GET, uri, None, 60).await;
    assert_envelope(&body, "search");
    let (s, _, _) = http(&app, Method::GET, uri, &a, None).await;
    assert_eq!(s, StatusCode::OK, "the real user keeps their own bucket");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_patch_bucket_is_per_user() {
    let (app, a, b) = two_users().await;
    // Missing series: 404 before the trip, so no row is ever written.
    let uri = "/api/series/does-not-exist";
    let patch = Some(json!({"summary": "x"}));
    let body = drain(&app, &a, Method::PATCH, uri, patch.clone(), 30).await;
    assert_envelope(&body, "metadata_write");

    let (s, _, _) = http(&app, Method::PATCH, uri, &b, patch).await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "second user is not throttled"
    );
    // The series GET on the same path is outside the bucket.
    let (s, _, _) = http(&app, Method::GET, uri, &a, None).await;
    assert_ne!(s, StatusCode::TOO_MANY_REQUESTS, "GET is not bucketed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_write_bucket_is_per_user() {
    let (app, a, b) = two_users().await;
    let uri = format!("/api/me/collections/{}", Uuid::now_v7());
    let body = drain(&app, &a, Method::DELETE, &uri, None, 60).await;
    assert_envelope(&body, "collection_write");

    let (s, _, _) = http(&app, Method::DELETE, &uri, &b, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "second user reaches the handler");

    // Reads and the reorder bucket are separate.
    let (s, _, _) = http(&app, Method::GET, "/api/me/collections", &a, None).await;
    assert_eq!(s, StatusCode::OK, "collection reads are not bucketed");
    let (s, _, _) = http(
        &app,
        Method::POST,
        &format!("{uri}/entries/reorder"),
        &a,
        Some(json!({"entry_ids": []})),
    )
    .await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "reorder has its own bucket"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_reorder_bucket_is_per_user() {
    let (app, a, b) = two_users().await;
    let uri = format!("/api/me/collections/{}/entries/reorder", Uuid::now_v7());
    let order = Some(json!({"entry_ids": []}));
    let body = drain(&app, &a, Method::POST, &uri, order.clone(), 30).await;
    assert_envelope(&body, "collection_reorder");

    let (s, _, _) = http(&app, Method::POST, &uri, &b, order).await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "second user is not throttled"
    );
    // Writes keep their budget while reorders are throttled.
    let (s, _, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/collections/{}", Uuid::now_v7()),
        &a,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "writes have their own bucket");
}
