//! M6d: integration coverage for `GET /admin/logs`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use server::observability::LogEntry;
use std::collections::BTreeMap;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
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
    let _ = body_json(resp.into_body()).await;
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn seed_entry(level: &str, target: &str, message: &str) -> LogEntry {
    LogEntry {
        id: 0,
        timestamp: Utc::now(),
        level: level.into(),
        target: target.into(),
        message: message.into(),
        fields: BTreeMap::new(),
    }
}

#[tokio::test]
async fn rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let (s, _) = get(&app, &user, "/api/admin/logs").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lists_recent_entries_oldest_first() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry("info", "server::scan", "scan started"));
    buf.push(seed_entry("warn", "server::api::progress", "retry"));
    buf.push(seed_entry("error", "server::api::scan", "boom"));

    let (s, body) = get(&app, &admin, "/api/admin/logs").await;
    assert_eq!(s, StatusCode::OK);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["message"], "scan started");
    assert_eq!(entries[2]["message"], "boom");
    assert!(body["watermark"].as_u64().unwrap() >= entries[2]["id"].as_u64().unwrap());
    assert!(body["capacity"].as_u64().unwrap() >= 3);
}

#[tokio::test]
async fn level_filter_drops_lower_severity() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry("debug", "t", "debug msg"));
    buf.push(seed_entry("info", "t", "info msg"));
    buf.push(seed_entry("error", "t", "error msg"));

    let (_, body) = get(&app, &admin, "/api/admin/logs?level=info").await;
    let messages: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages, vec!["info msg", "error msg"]);

    let (s, _) = get(&app, &admin, "/api/admin/logs?level=garbage").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn since_filter_returns_only_newer() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    let buf = app.state().log_buffer.clone();
    for n in 0..5 {
        buf.push(seed_entry("info", "t", &format!("msg{n}")));
    }
    let (_, body_all) = get(&app, &admin, "/api/admin/logs").await;
    let mid = body_all["entries"][2]["id"].as_u64().unwrap();
    let (_, body_after) = get(&app, &admin, &format!("/api/admin/logs?since={mid}")).await;
    let after = body_after["entries"].as_array().unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0]["message"], "msg3");
    assert_eq!(after[1]["message"], "msg4");
}

#[tokio::test]
async fn q_substring_matches_target_or_message() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry("info", "server::scan", "ScanComplete!"));
    buf.push(seed_entry(
        "info",
        "server::api::progress",
        "progress upsert",
    ));
    buf.push(seed_entry("info", "server::thumbs", "thumb generated"));

    let (_, body) = get(&app, &admin, "/api/admin/logs?q=scan").await;
    let entries = body["entries"].as_array().unwrap();
    // 'scan' matches the first (target server::scan) and the message "ScanComplete!"
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["message"], "ScanComplete!");
}
