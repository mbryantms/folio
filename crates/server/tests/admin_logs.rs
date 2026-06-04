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
        // No library context → server stream (matches `classify_domain`).
        domain: "server".into(),
    }
}

/// Build a `LogEntry` with structured `fields` populated — used by the
/// library_id-filter test to mimic what the RingLayer's parent-span
/// walk produces in production. The scanner's `#[tracing::instrument]`
/// records `library_id` on the span; on_event copies it into the
/// event's `fields` map. Seeding directly here exercises the
/// downstream filter without needing a real scan.
fn seed_entry_with_fields(
    level: &str,
    target: &str,
    message: &str,
    fields: &[(&str, &str)],
) -> LogEntry {
    let mut map = BTreeMap::new();
    for (k, v) in fields {
        map.insert((*k).to_owned(), (*v).to_owned());
    }
    // Mirror `classify_domain`: library-scoped context ⇒ library stream.
    let domain = if map.contains_key("library_id") || map.contains_key("scan_id") {
        "library"
    } else {
        "server"
    };
    LogEntry {
        id: 0,
        timestamp: Utc::now(),
        level: level.into(),
        target: target.into(),
        message: message.into(),
        fields: map,
        domain: domain.into(),
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
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
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
async fn library_id_filter_scopes_to_one_library() {
    // Two seeded entries with `fields["library_id"] = <uuid>` and one
    // without. `?library_id=<uuid>` should return only the matching
    // pair; `?library_id=all` returns everything; an invalid UUID
    // returns 422.
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;

    let lib_a = "019e5c65-825f-7913-8d8d-35076c162065";
    let lib_b = "019e2665-9dd1-74b0-b393-624913f6f9e3";

    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry_with_fields(
        "info",
        "server::library::scanner",
        "scan complete (a)",
        &[("library_id", lib_a)],
    ));
    buf.push(seed_entry_with_fields(
        "warn",
        "server::library::scanner",
        "missing comicinfo (a)",
        &[("library_id", lib_a)],
    ));
    buf.push(seed_entry_with_fields(
        "info",
        "server::library::scanner",
        "scan complete (b)",
        &[("library_id", lib_b)],
    ));
    buf.push(seed_entry(
        "info",
        "server::api::progress",
        "no library context",
    ));

    // library_id=A → 2 entries.
    let (s, body) = get(&app, &admin, &format!("/api/admin/logs?library_id={lib_a}")).await;
    assert_eq!(s, StatusCode::OK);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "expected 2 lib_a entries, got: {body}");
    for e in entries {
        assert_eq!(e["fields"]["library_id"], lib_a);
    }

    // library_id=all → 4 (drops the filter).
    let (s, body) = get(&app, &admin, "/api/admin/logs?library_id=all").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 4);

    // No filter → also 4 (same as 'all').
    let (s, body) = get(&app, &admin, "/api/admin/logs").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 4);

    // Invalid UUID → 422 (not a silent empty list).
    let (s, _) = get(&app, &admin, "/api/admin/logs?library_id=not-a-uuid").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn domain_filter_separates_streams() {
    // observability-split M12: server vs library stream filter.
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry(
        "info",
        "server::api::reading",
        "request handled",
    ));
    buf.push(seed_entry_with_fields(
        "info",
        "server::library::scanner",
        "scan complete",
        &[("library_id", "019e5c65-825f-7913-8d8d-35076c162065")],
    ));

    let (_, server) = get(&app, &admin, "/api/admin/logs?domain=server").await;
    let s = server["entries"].as_array().unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["message"], "request handled");
    assert_eq!(s[0]["domain"], "server");

    let (_, library) = get(&app, &admin, "/api/admin/logs?domain=library").await;
    let l = library["entries"].as_array().unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0]["domain"], "library");

    // No domain filter → both.
    let (_, all) = get(&app, &admin, "/api/admin/logs").await;
    assert_eq!(all["entries"].as_array().unwrap().len(), 2);

    // Bad value → 422, not a silent empty list.
    let (s, _) = get(&app, &admin, "/api/admin/logs?domain=bogus").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn error_code_is_lifted_into_view() {
    // M12: `fields["error_code"]` is surfaced as a top-level `error_code`
    // for the Server-log error-code facet.
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let buf = app.state().log_buffer.clone();
    buf.push(seed_entry_with_fields(
        "error",
        "server::api::libraries",
        "api error: boom",
        &[("error_code", "internal"), ("status", "500")],
    ));

    let (_, body) = get(&app, &admin, "/api/admin/logs?level=error").await;
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["error_code"], "internal");
    assert_eq!(entries[0]["domain"], "server");
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
