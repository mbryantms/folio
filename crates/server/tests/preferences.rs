//! `PATCH /me/preferences` round-trip — set, read back via /auth/me, and
//! validation of the direction enum.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"prefs@example.com","password":"correctly-horse-battery"}"#,
                ))
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn patch_pref(app: &TestApp, auth: &Authed, body: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/me/preferences")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn get_me(app: &TestApp, auth: &Authed) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/me")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

#[tokio::test]
async fn round_trip_rtl() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (status, body) = patch_pref(&app, &auth, r#"{"default_reading_direction":"rtl"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["default_reading_direction"], "rtl");

    let me = get_me(&app, &auth).await;
    assert_eq!(me["default_reading_direction"], "rtl");
}

#[tokio::test]
async fn null_clears_preference() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let _ = patch_pref(&app, &auth, r#"{"default_reading_direction":"ltr"}"#).await;
    let (status, body) = patch_pref(&app, &auth, r#"{"default_reading_direction":null}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["default_reading_direction"].is_null());
}

#[tokio::test]
async fn invalid_direction_rejected() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (status, body) =
        patch_pref(&app, &auth, r#"{"default_reading_direction":"sideways"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

/// Audit 2026-11 F6/F7: the seven reader display preferences round-trip
/// through PATCH and `/auth/me`, start `null` (never set on the account),
/// clear back to `null`, and reject values outside the slider ranges.
#[tokio::test]
async fn reader_display_prefs_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let fresh = get_me(&app, &auth).await;
    for key in [
        "reader_brightness",
        "reader_sepia",
        "reader_grayscale",
        "reader_invert",
        "reader_warmth",
        "reader_contrast",
        "reader_zoom_persist",
    ] {
        assert!(fresh[key].is_null(), "{key} should start unset");
    }

    let (status, body) = patch_pref(
        &app,
        &auth,
        r#"{"reader_brightness":1.2,"reader_sepia":0.35,"reader_grayscale":40,
            "reader_invert":true,"reader_warmth":-20,"reader_contrast":110,
            "reader_zoom_persist":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reader_brightness"], 1.2);
    assert_eq!(body["reader_sepia"], 0.35);
    assert_eq!(body["reader_grayscale"], 40);
    assert_eq!(body["reader_invert"], true);
    assert_eq!(body["reader_warmth"], -20);
    assert_eq!(body["reader_contrast"], 110);
    assert_eq!(body["reader_zoom_persist"], true);

    let me = get_me(&app, &auth).await;
    assert_eq!(me["reader_brightness"], 1.2);
    assert_eq!(me["reader_warmth"], -20);
    assert_eq!(me["reader_zoom_persist"], true);

    // A partial PATCH leaves the other slices alone; `null` clears one.
    let (status, body) = patch_pref(
        &app,
        &auth,
        r#"{"reader_grayscale":null,"reader_contrast":90}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["reader_grayscale"].is_null());
    assert_eq!(body["reader_contrast"], 90);
    assert_eq!(body["reader_brightness"], 1.2);
    assert_eq!(body["reader_invert"], true);
}

#[tokio::test]
async fn reader_display_prefs_out_of_range_rejected() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    for (body, field) in [
        (r#"{"reader_brightness":2}"#, "reader_brightness"),
        (r#"{"reader_sepia":1.5}"#, "reader_sepia"),
        (r#"{"reader_grayscale":101}"#, "reader_grayscale"),
        (r#"{"reader_warmth":51}"#, "reader_warmth"),
        (r#"{"reader_contrast":49}"#, "reader_contrast"),
    ] {
        let (status, resp) = patch_pref(&app, &auth, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {resp}");
        assert_eq!(resp["error"]["code"], "validation");
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(field),
            "{field}: {resp}"
        );
    }
    // Nothing was written by the rejected bodies.
    let me = get_me(&app, &auth).await;
    assert!(me["reader_brightness"].is_null());
}
