//! M4: extended `PATCH /me/preferences` coverage. Validates the new fit /
//! view / page-strip / theme / accent / density / keybinds slots round-trip
//! correctly and that PATCH semantics preserve untouched fields.

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

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(
        r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#,
        email = email,
    );
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
async fn fit_view_strip_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "fit@example.com").await;

    let (status, body) = patch_pref(
        &app,
        &auth,
        r#"{
            "default_fit_mode": "height",
            "default_view_mode": "webtoon",
            "default_page_strip": true
        }"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["default_fit_mode"], "height");
    assert_eq!(body["default_view_mode"], "webtoon");
    assert_eq!(body["default_page_strip"], true);

    // GET /auth/me must reflect the same values.
    let me = get_me(&app, &auth).await;
    assert_eq!(me["default_fit_mode"], "height");
    assert_eq!(me["default_view_mode"], "webtoon");
    assert_eq!(me["default_page_strip"], true);
}

#[tokio::test]
async fn cover_solo_default_and_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cover@example.com").await;

    // Newly registered users default to cover-solo on (printed-comic
    // convention).
    let me = get_me(&app, &auth).await;
    assert_eq!(me["default_cover_solo"], true);

    // Toggle off, then back on — both updates must round-trip via PATCH +
    // re-fetch through GET /auth/me.
    let (status, body) = patch_pref(&app, &auth, r#"{ "default_cover_solo": false }"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["default_cover_solo"], false);
    let me = get_me(&app, &auth).await;
    assert_eq!(me["default_cover_solo"], false);

    let (_, body) = patch_pref(&app, &auth, r#"{ "default_cover_solo": true }"#).await;
    assert_eq!(body["default_cover_solo"], true);
}

#[tokio::test]
async fn theme_accent_density_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "theme@example.com").await;

    let (status, body) = patch_pref(
        &app,
        &auth,
        r#"{
            "theme": "dark",
            "accent_color": "blue",
            "density": "compact"
        }"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["theme"], "dark");
    assert_eq!(body["accent_color"], "blue");
    assert_eq!(body["density"], "compact");
}

#[tokio::test]
async fn keybinds_object_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kb@example.com").await;

    let (status, body) = patch_pref(
        &app,
        &auth,
        r#"{"keybinds": {"nextPage": "j", "prevPage": "k"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["keybinds"]["nextPage"], "j");
    assert_eq!(body["keybinds"]["prevPage"], "k");

    // Sending a different object replaces the whole map (current spec).
    let (_, body2) = patch_pref(&app, &auth, r#"{"keybinds": {"cycleFit": "x"}}"#).await;
    assert_eq!(body2["keybinds"]["cycleFit"], "x");
    assert!(body2["keybinds"].get("nextPage").is_none());
}

#[tokio::test]
async fn keybinds_must_be_object() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kb-bad@example.com").await;
    let (status, body) = patch_pref(&app, &auth, r#"{"keybinds": "not-an-object"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn keybinds_value_must_be_string() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kb-num@example.com").await;
    let (status, body) = patch_pref(&app, &auth, r#"{"keybinds": {"nextPage": 42}}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn invalid_fit_mode_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "fit-bad@example.com").await;
    let (status, _) = patch_pref(&app, &auth, r#"{"default_fit_mode":"sideways"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invalid_theme_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "theme-bad@example.com").await;
    let (status, _) = patch_pref(&app, &auth, r#"{"theme":"neon-pink"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unspecified_fields_preserved() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "preserve@example.com").await;

    let _ = patch_pref(
        &app,
        &auth,
        r#"{
            "default_fit_mode": "width",
            "default_view_mode": "single",
            "theme": "dark"
        }"#,
    )
    .await;

    // PATCHing only the theme should not clear the other fields.
    let (status, body) = patch_pref(&app, &auth, r#"{"theme": "system"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["theme"], "system");
    assert_eq!(body["default_fit_mode"], "width");
    assert_eq!(body["default_view_mode"], "single");
}

#[tokio::test]
async fn null_clears_individual_fields() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "clear@example.com").await;

    let _ = patch_pref(&app, &auth, r#"{"default_fit_mode":"height"}"#).await;
    let (status, body) = patch_pref(&app, &auth, r#"{"default_fit_mode": null}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["default_fit_mode"].is_null());
}
