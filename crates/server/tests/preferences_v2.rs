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
async fn page_animation_round_trips_and_validates() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-anim@example.com").await;

    // Newly registered users have no preference set — null means
    // "use the reader's built-in default" (currently slide).
    let me = get_me(&app, &auth).await;
    assert!(
        me["default_page_animation"].is_null(),
        "fresh user defaults to null page animation: {me}"
    );

    // Accept all three supported values. `fade` was added in
    // v0.3.45 alongside the original `off` + `slide`.
    for value in ["off", "slide", "fade"] {
        let body_str = format!(r#"{{ "default_page_animation": "{value}" }}"#);
        let (status, body) = patch_pref(&app, &auth, &body_str).await;
        assert_eq!(status, StatusCode::OK, "value {value} accepted");
        assert_eq!(body["default_page_animation"], value);
        let me = get_me(&app, &auth).await;
        assert_eq!(me["default_page_animation"], value);
    }

    // Null clears the preference.
    let (status, body) = patch_pref(&app, &auth, r#"{ "default_page_animation": null }"#).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["default_page_animation"].is_null());

    // Unknown values rejected with 400. `curl` was considered for
    // v0.3.45 but the CSS-only rotation didn't match the corner-peel
    // semantics the visual implies; deferred to a future release
    // backed by react-pageflip integration.
    let (status, _) = patch_pref(&app, &auth, r#"{ "default_page_animation": "curl" }"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
            "density": "compact",
            "corner_radius": "round",
            "motion": "reduced"
        }"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["theme"], "dark");
    assert_eq!(body["accent_color"], "blue");
    assert_eq!(body["density"], "compact");
    assert_eq!(body["corner_radius"], "round");
    assert_eq!(body["motion"], "reduced");
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn keybinds_value_must_be_string() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kb-num@example.com").await;
    let (status, body) = patch_pref(&app, &auth, r#"{"keybinds": {"nextPage": 42}}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
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
async fn contain_fit_mode_round_trips() {
    // WP-4.2: the reader's "Fit screen" mode is a valid default.
    let app = TestApp::spawn().await;
    let auth = register(&app, "fit-contain@example.com").await;
    let (status, body) = patch_pref(&app, &auth, r#"{"default_fit_mode":"contain"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["default_fit_mode"], "contain");
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

/// Cover-stack style: defaults to `pickup`, round-trips a valid value,
/// rejects an unknown one with 422 and leaves the stored value alone.
#[tokio::test]
async fn cover_stack_style_round_trip_and_validation() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "stack@example.com").await;
    assert_eq!(get_me(&app, &auth).await["cover_stack_style"], "pickup");

    let (status, body) = patch_pref(&app, &auth, r#"{"cover_stack_style":"arc"}"#).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["cover_stack_style"], "arc");
    assert_eq!(get_me(&app, &auth).await["cover_stack_style"], "arc");

    let (status, _) = patch_pref(&app, &auth, r#"{"cover_stack_style":"spiral"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(get_me(&app, &auth).await["cover_stack_style"], "arc");
}

/// Effect switches: start empty (web defaults apply), merge per key across
/// PATCHes, `null` clears one back to its default, and an unknown key or a
/// non-boolean value 400s (serde rejection) without touching the stored object.
#[tokio::test]
async fn effects_merge_clear_and_validation() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "effects@example.com").await;
    assert_eq!(get_me(&app, &auth).await["effects"], serde_json::json!({}));

    let (status, body) = patch_pref(&app, &auth, r#"{"effects":{"cover_tilt":true}}"#).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["effects"], serde_json::json!({"cover_tilt": true}));

    // A second PATCH with a different key keeps the first.
    let (status, _) = patch_pref(&app, &auth, r#"{"effects":{"page_curl":false}}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        get_me(&app, &auth).await["effects"],
        serde_json::json!({"cover_tilt": true, "page_curl": false})
    );

    // null resets one effect to its default.
    let (status, _) = patch_pref(&app, &auth, r#"{"effects":{"cover_tilt":null}}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        get_me(&app, &auth).await["effects"],
        serde_json::json!({"page_curl": false})
    );

    for bad in [
        r#"{"effects":{"sparkles":true}}"#,
        r#"{"effects":{"page_curl":"yes"}}"#,
    ] {
        let (status, _) = patch_pref(&app, &auth, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(
        get_me(&app, &auth).await["effects"],
        serde_json::json!({"page_curl": false})
    );
}

#[tokio::test]
async fn keybinds_are_bounded_with_field_details() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kb-cap@example.com").await;
    let max = server::auth::local::MAX_KEYBINDS;
    let mut map = serde_json::Map::new();
    for i in 0..=max {
        map.insert(format!("action{i}"), serde_json::json!("k"));
    }
    let too_many = serde_json::json!({ "keybinds": map }).to_string();
    let long = "x".repeat(server::auth::local::MAX_KEYBIND_LEN + 1);
    let too_long = serde_json::json!({ "keybinds": { "nextPage": long } }).to_string();
    for body in [too_many, too_long] {
        let (status, resp) = patch_pref(&app, &auth, &body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
        assert_eq!(resp["error"]["code"], "validation");
        assert_eq!(resp["error"]["details"][0]["field"], "keybinds", "{resp}");
    }
    // At the cap is fine.
    let mut ok = serde_json::Map::new();
    for i in 0..max {
        ok.insert(format!("action{i}"), serde_json::json!("k"));
    }
    let (status, resp) = patch_pref(
        &app,
        &auth,
        &serde_json::json!({ "keybinds": ok }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
}
