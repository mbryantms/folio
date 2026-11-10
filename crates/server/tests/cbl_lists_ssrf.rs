//! SSRF guard for `POST /me/cbl-lists` (auth-hardening Phase B B1).
//!
//! Closes [security-audit.md H-1](../../../docs/dev/security-audit.md):
//! authenticated callers must not be able to make the server fetch
//! loopback / RFC-1918 / cloud-metadata addresses. The guard rejects
//! before the outbound `reqwest` call so we can assert behaviour without
//! standing up an actual HTTP target.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
    #[allow(dead_code)]
    user_id: Uuid,
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
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn post_create_from_url(
    app: &TestApp,
    auth: &Authed,
    url: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "kind": "url", "url": url }).to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/cbl-lists")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_loopback_url() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-loopback@example.com").await;
    let (status, body) = post_create_from_url(&app, &auth, "https://127.0.0.1/list.cbl").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["error"]["code"], "invalid_url");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_aws_metadata_url() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-metadata@example.com").await;
    // 169.254.169.254 — AWS / GCP / Azure / OpenStack instance metadata.
    // Single most-cited SSRF target.
    let (status, body) = post_create_from_url(
        &app,
        &auth,
        "https://169.254.169.254/latest/meta-data/iam/security-credentials/",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["error"]["code"], "invalid_url");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_rfc1918_private_ranges() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-private@example.com").await;
    for raw in [
        "https://10.0.0.1/list.cbl",
        "https://172.16.5.5/list.cbl",
        "https://192.168.1.1/list.cbl",
        "https://[::1]/list.cbl",
        "https://[fc00::1]/list.cbl",
    ] {
        let (status, body) = post_create_from_url(&app, &auth, raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw} body={body}");
        assert_eq!(body["error"]["code"], "invalid_url", "{raw}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_non_https_schemes() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-scheme@example.com").await;
    for raw in [
        "http://example.com/list.cbl",
        "ftp://example.com/list.cbl",
        "file:///etc/passwd",
    ] {
        let (status, body) = post_create_from_url(&app, &auth, raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw} body={body}");
        assert_eq!(body["error"]["code"], "invalid_url", "{raw}");
    }
}

/// A URL list with stored validators takes the conditional refresh path.
/// When its host now resolves to an internal address (here `localhost`),
/// refresh and check are refused before any connection is made, and the
/// message names no address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conditional_refresh_to_a_host_resolving_internally_is_refused() {
    use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-conditional@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("ssrf-conditional@example.com"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();

    // Count every TCP connection the server might make to the target.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = connections.clone();
    tokio::spawn(async move {
        while let Ok((_sock, _)) = listener.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });

    let now = chrono::Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    entity::cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(Some(user.id)),
        source_kind: Set("url".into()),
        source_url: Set(Some(format!("https://localhost:{port}/list.cbl"))),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(Some("\"v1\"".into())),
        source_last_modified: Set(Some("Mon, 02 Nov 2026 00:00:00 GMT".into())),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList><Name>x</Name><Books/></ReadingList>".into()),
        parsed_name: Set("conditional".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(None),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    for (path, code) in [("refresh", "refresh_failed"), ("check", "check_failed")] {
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/me/cbl-lists/{list_id}/{path}"))
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    auth.session, auth.csrf
                ),
            )
            .header("X-CSRF-Token", &auth.csrf)
            .body(Body::empty())
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{path}: {body}");
        assert_eq!(body["error"]["code"], code, "{path}");
        let msg = body["error"]["message"].as_str().unwrap();
        assert!(msg.contains("not allowed"), "{path}: {msg}");
        for leaked in ["127.0.0.1", "::1", "loopback", "private"] {
            assert!(!msg.contains(leaked), "{path} leaked {leaked}: {msg}");
        }
    }
    assert_eq!(connections.load(Ordering::SeqCst), 0, "no connection made");
}

/// The refusal at import names no address either; the code stays
/// `invalid_url`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusal_message_names_no_address() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ssrf-message@example.com").await;
    for (raw, leaked) in [
        ("https://169.254.169.254/latest/meta-data/", "169.254"),
        ("https://[fd00::1]/list.cbl", "fd00"),
        ("https://localhost/list.cbl", "127.0.0.1"),
        ("https://[64:ff9b::7f00:1]/list.cbl", "7f00"),
    ] {
        let (status, body) = post_create_from_url(&app, &auth, raw).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw} body={body}");
        assert_eq!(body["error"]["code"], "invalid_url", "{raw}");
        let msg = body["error"]["message"].as_str().unwrap();
        assert!(!msg.contains(leaked), "{raw}: {msg}");
        assert!(!msg.contains("::1"), "{raw}: {msg}");
    }
}
