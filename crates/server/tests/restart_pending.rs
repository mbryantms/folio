//! Integration test for `GET /admin/server/restart-pending`.
//!
//! Boot-only settings (worker pools, ZIP LRU, the metadata weekly-refresh
//! cron) are read once at startup. A PATCH updates the live `Config` so the
//! admin form reflects the new value, but the running process keeps the boot
//! value until restart. The endpoint diffs a boot-time `Config` snapshot
//! against the live one and reports exactly those changed keys, so the admin
//! shell can surface a "needs restart" banner.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "registration failed");
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn patch_settings(app: &TestApp, auth: &Authed, body: Value) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_restart_pending(app: &TestApp, auth: &Authed) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/server/restart-pending")
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn restart_pending_reflects_boot_only_changes() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Nothing changed since boot → empty list.
    let resp = get_restart_pending(&app, &admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(
        body["pending"].as_array().unwrap().len(),
        0,
        "a fresh boot has nothing pending"
    );

    // Change a boot-only worker count + the metadata refresh cron. A live
    // key (the rate-limit toggle) is changed too, to prove it does NOT show
    // up as restart-pending.
    let resp = patch_settings(
        &app,
        &admin,
        json!({
            "workers.scan_count": 8,
            "metadata.weekly_refresh_cron": "0 0 6 * * 1",
            "auth.rate_limit_enabled": false,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Both boot-only keys now show as pending with boot → current values;
    // the live key is absent.
    let resp = get_restart_pending(&app, &admin).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let pending = body["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 2, "exactly the two boot-only keys changed");

    let by_key: std::collections::HashMap<&str, &Value> = pending
        .iter()
        .map(|p| (p["key"].as_str().unwrap(), p))
        .collect();

    let scan = by_key
        .get("workers.scan_count")
        .expect("workers.scan_count pending");
    assert_eq!(scan["current_value"], "8");
    assert_ne!(
        scan["boot_value"], scan["current_value"],
        "boot value must differ from the new value"
    );

    let cron = by_key
        .get("metadata.weekly_refresh_cron")
        .expect("metadata.weekly_refresh_cron pending");
    assert_eq!(cron["current_value"], "0 0 6 * * 1");

    assert!(
        !by_key.contains_key("auth.rate_limit_enabled"),
        "a live (non-boot) setting must never be restart-pending"
    );

    // Reverting the value back to the boot value clears it from the list.
    let boot_scan = scan["boot_value"].as_str().unwrap().to_owned();
    let resp = patch_settings(
        &app,
        &admin,
        json!({ "workers.scan_count": boot_scan.parse::<u64>().unwrap() }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(get_restart_pending(&app, &admin).await.into_body()).await;
    let pending = body["pending"].as_array().unwrap();
    assert_eq!(
        pending.len(),
        1,
        "reverting scan_count drops it from pending"
    );
    assert_eq!(pending[0]["key"], "metadata.weekly_refresh_cron");
}

#[tokio::test]
async fn restart_pending_requires_admin() {
    let app = TestApp::spawn().await;
    // First registered user is the admin; the second is a regular user.
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = get_restart_pending(&app, &user).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
