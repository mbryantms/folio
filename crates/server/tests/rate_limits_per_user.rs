//! E3 (audit 2026-11 N-A5) — per-user rate limits on free-text search,
//! metadata PATCH, collection writes and collection reorder; E12 adds the
//! saved-view PATCH / DELETE and the admin metadata writes (field
//! provenance, cover pick, external ids).
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
/// refills while the loop runs, so usually a few more do). Requests go out
/// in concurrent waves of [`WAVE`], so the drain outpaces the refill even
/// when each request is slow on a loaded CI shard. Panics if it never trips.
async fn drain(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<Value>,
    burst: usize,
) -> Value {
    const WAVE: usize = 20;
    let mut allowed = 0;
    for _ in 0..(burst * 10 / WAVE) {
        let wave = (0..WAVE).map(|_| http(app, method.clone(), uri, auth, body.clone()));
        let mut limited = None;
        for (s, retry_after, b) in futures::future::join_all(wave).await {
            if s == StatusCode::TOO_MANY_REQUESTS {
                assert!(retry_after.is_some(), "429 carries Retry-After");
                limited.get_or_insert(b);
            } else {
                allowed += 1;
            }
        }
        if let Some(b) = limited {
            assert!(
                allowed >= burst,
                "{method} {uri} tripped after only {allowed} requests (burst {burst})"
            );
            return b;
        }
    }
    panic!("{method} {uri}: bucket never tripped");
}

/// After a [`drain`], whether another route on the same instance answers
/// 429. The bucket refills while this runs, so a token can appear between
/// the trip and the request; a shared store still answers 429 within a few
/// immediate requests, while a separate one would let all of them through.
async fn shares_drained_bucket(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> bool {
    for _ in 0..20 {
        let (s, _, _) = http(app, method.clone(), uri, auth, body.clone()).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            return true;
        }
    }
    false
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

/// An expired token we signed still names its user: it spends that user's
/// budget (not the shared IP one) and, before the trip, gets the 401 that
/// makes the web client refresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_tokens_spend_their_own_users_budget() {
    use sea_orm::{ColumnTrait, Database, EntityTrait, QueryFilter};

    let (app, a, b) = two_users().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_a = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("first@example.com"))
        .one(&db)
        .await
        .unwrap()
        .expect("first user");
    let expired = Authed {
        session: app
            .state()
            .jwt_keys
            .issue_access(user_a.id, "admin", 0, chrono::Duration::hours(-2))
            .unwrap(),
        csrf: a.csrf.clone(),
    };
    let uri = "/api/issues/search?q=batman";
    let (s, _, _) = http(&app, Method::GET, uri, &expired, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "expired token reaches auth");

    let body = drain(&app, &expired, Method::GET, uri, None, 60).await;
    assert_envelope(&body, "search");
    // The bucket refills at 10/s, so a token can appear between the trip
    // and the next request; the shared bucket must still answer 429 within
    // a few immediate requests.
    let mut shared = false;
    for _ in 0..20 {
        let (s, _, _) = http(&app, Method::GET, uri, &a, None).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            shared = true;
            break;
        }
        assert_eq!(s, StatusCode::OK, "a refilled token reaches the handler");
    }
    assert!(shared, "the fresh token shares the same user's bucket");
    let (s, _, _) = http(&app, Method::GET, uri, &b, None).await;
    assert_eq!(s, StatusCode::OK, "other users are unaffected");
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

/// E12: `PATCH`/`DELETE /me/saved-views/{id}` also edit and delete
/// collection rows, so they get the collection-write budget in a sibling
/// `saved_view_write` bucket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_view_write_bucket_is_per_user() {
    let (app, a, b) = two_users().await;
    // Missing view: 404 before the trip, so no row is ever written.
    let uri = format!("/api/me/saved-views/{}", Uuid::now_v7());
    let patch = Some(json!({"name": "renamed"}));
    let body = drain(&app, &a, Method::PATCH, &uri, patch.clone(), 60).await;
    assert_envelope(&body, "saved_view_write");

    assert!(
        shares_drained_bucket(&app, &a, Method::DELETE, &uri, None).await,
        "the saved-view DELETE shares the PATCH's bucket"
    );

    let (s, _, _) = http(&app, Method::PATCH, &uri, &b, patch).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "second user reaches the handler");

    // The list on the same router is outside the bucket, and the
    // collections instance keeps its own budget.
    let (s, _, _) = http(&app, Method::GET, "/api/me/saved-views", &a, None).await;
    assert_eq!(s, StatusCode::OK, "saved-view reads are not bucketed");
    let (s, _, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/collections/{}", Uuid::now_v7()),
        &a,
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "collection writes have their own bucket"
    );
}

/// E12: the issue field-provenance lock shares the issue PATCH's
/// `metadata_write` instance; the GETs on the same router are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn field_provenance_lock_is_metadata_bucketed() {
    let (app, a, b) = two_users().await;
    let issue = "/api/series/does-not-exist/issues/nope";
    let uri = format!("{issue}/field-provenance/title");
    let body = drain(&app, &a, Method::PUT, &uri, None, 30).await;
    assert_envelope(&body, "metadata_write");
    // The pin clear, the issue PATCH and the bulk PATCH spend the same
    // budget.
    let siblings = [
        (Method::DELETE, uri.clone(), None),
        (
            Method::PATCH,
            issue.to_owned(),
            Some(json!({"summary": "x"})),
        ),
        (
            Method::PATCH,
            "/api/me/issues/bulk-metadata".to_owned(),
            Some(json!({"issue_ids": []})),
        ),
    ];
    for (method, sibling, body) in siblings {
        assert!(
            shares_drained_bucket(&app, &a, method.clone(), &sibling, body).await,
            "{method} {sibling} shares the lock's bucket"
        );
    }

    let (s, _, _) = http(&app, Method::PUT, &uri, &b, None).await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "second user is not throttled"
    );
    let (s, _, _) = http(&app, Method::GET, issue, &a, None).await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "issue GET is not bucketed"
    );
    let (s, _, _) = http(
        &app,
        Method::GET,
        &format!("{issue}/metadata-overview"),
        &a,
        None,
    )
    .await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "metadata overview is not bucketed"
    );
}

/// E12: the cover picks and the external-id writes each have a
/// `metadata_write` instance of their own (draining one leaves the other
/// its full burst), and the external-id list stays unbucketed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cover_pick_and_external_id_writes_are_metadata_bucketed() {
    let (app, a, _b) = two_users().await;
    let cover = "/api/series/does-not-exist/issues/nope/cover";
    let pick = Some(json!({"page_index": 0}));
    let body = drain(&app, &a, Method::PATCH, cover, pick, 30).await;
    assert_envelope(&body, "metadata_write");
    let variant = format!("{cover}/variant/{}", Uuid::now_v7());
    let cover_siblings = [
        (Method::POST, variant, None),
        (Method::DELETE, format!("{cover}/variant"), None),
        (
            Method::PATCH,
            "/api/series/does-not-exist/cover".to_owned(),
            Some(json!({"issue_id": Uuid::now_v7()})),
        ),
    ];
    for (method, sibling, body) in cover_siblings {
        assert!(
            shares_drained_bucket(&app, &a, method.clone(), &sibling, body).await,
            "{method} {sibling} shares the cover pick's bucket"
        );
    }

    let ids = "/api/series/does-not-exist/external-ids";
    let add = Some(json!({"source": "comicvine", "external_id": "4050-1"}));
    let body = drain(&app, &a, Method::POST, ids, add.clone(), 30).await;
    assert_envelope(&body, "metadata_write");
    let issue_ids = "/api/series/does-not-exist/issues/nope/external-ids";
    let id_siblings = [
        (Method::DELETE, format!("{ids}/comicvine"), None),
        (Method::POST, issue_ids.to_owned(), add),
        (Method::DELETE, format!("{issue_ids}/comicvine"), None),
    ];
    for (method, sibling, body) in id_siblings {
        assert!(
            shares_drained_bucket(&app, &a, method.clone(), &sibling, body).await,
            "{method} {sibling} shares the external-id add's bucket"
        );
    }

    let (s, _, _) = http(&app, Method::GET, ids, &a, None).await;
    assert_ne!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "external-id list is not bucketed"
    );
}
