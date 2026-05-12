//! Integration coverage for the slug-allocation pipeline (M1 of the
//! human-readable-URLs plan, `~/.claude/plans/let-s-create-a-new-merry-finch.md`).
//!
//! Verifies:
//!   - `POST /libraries` allocates a slug derived from the library name.
//!   - A second `POST /libraries` with the same name gets a `-2` numeric
//!     suffix (libraries have no natural disambiguator, so we fall through
//!     to the numeric path).
//!   - `PATCH /libraries/{id}` with a fresh slug succeeds and writes an
//!     audit row.
//!   - `PATCH /libraries/{id}` with a slug that's already in use returns
//!     `409 conflict.slug` and does NOT mutate the row.
//!   - `identity::resolve_or_create` allocates distinct slugs for two
//!     identically-named series in different libraries (using year
//!     disambiguator first, then numeric fallback).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{audit_log, library, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::Value;
use server::library::identity::{SeriesIdentityHint, resolve_or_create};
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

async fn register_admin(app: &TestApp) -> Authed {
    // The first registered local user becomes admin (per CLAUDE.md
    // bootstrap rule).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery-staple"}"#,
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
    let extract = |prefix: &str| {
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

async fn create_library(
    app: &TestApp,
    auth: &Authed,
    name: &str,
    root: &str,
) -> (StatusCode, Value) {
    let body = serde_json::json!({"name": name, "root_path": root}).to_string();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = body_json(resp.into_body()).await;
    (status, json)
}

async fn patch_library(app: &TestApp, auth: &Authed, id: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/libraries/{id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = body_json(resp.into_body()).await;
    (status, json)
}

#[tokio::test]
async fn library_create_allocates_slug_from_name() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let (status, json) = create_library(&app, &auth, "Main Collection", "/tmp/main").await;
    assert_eq!(status, StatusCode::CREATED, "{json:?}");
    assert_eq!(json["slug"], "main-collection");
}

#[tokio::test]
async fn library_create_disambiguates_on_collision() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let (s1, j1) = create_library(&app, &auth, "Saga", "/tmp/saga-a").await;
    assert_eq!(s1, StatusCode::CREATED);
    assert_eq!(j1["slug"], "saga");

    let (s2, j2) = create_library(&app, &auth, "Saga", "/tmp/saga-b").await;
    assert_eq!(s2, StatusCode::CREATED);
    assert_eq!(
        j2["slug"], "saga-2",
        "second library with same name must get numeric suffix",
    );
}

#[tokio::test]
async fn library_patch_slug_override_succeeds_and_audits() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let (_, j) = create_library(&app, &auth, "Original Name", "/tmp/orig").await;
    let id = j["id"].as_str().unwrap().to_owned();
    let slug = j["slug"].as_str().unwrap().to_owned();

    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "slug": "custom-slug" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["slug"], "custom-slug");

    // Audit row should be present, keyed off the canonical UUID.
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.slug.set"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "expected one slug-set audit row");
    assert_eq!(rows[0].target_id.as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn library_patch_slug_collision_returns_409() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let (_, a) = create_library(&app, &auth, "Alpha", "/tmp/alpha").await;
    let (_, b) = create_library(&app, &auth, "Bravo", "/tmp/bravo").await;
    let alpha_slug = a["slug"].as_str().unwrap().to_owned();
    let bravo_id = b["id"].as_str().unwrap().to_owned();
    let bravo_slug = b["slug"].as_str().unwrap().to_owned();

    // Try to rename Bravo → Alpha's slug. Must 409 and not mutate.
    let (status, body) = patch_library(
        &app,
        &auth,
        &bravo_slug,
        serde_json::json!({ "slug": &alpha_slug }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "conflict.slug");

    // Bravo's slug is unchanged.
    let bravo_uuid = uuid::Uuid::parse_str(&bravo_id).unwrap();
    let row = library::Entity::find_by_id(bravo_uuid)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.slug, "bravo");
}

#[tokio::test]
async fn library_patch_slug_self_no_collision() {
    // Renaming a library to its CURRENT slug is a valid no-op (the
    // allocator's `excluding` clause exempts the row's own slug from the
    // uniqueness check).
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;

    let (_, j) = create_library(&app, &auth, "Self", "/tmp/self").await;
    let slug = j["slug"].as_str().unwrap().to_owned();

    let (status, body) =
        patch_library(&app, &auth, &slug, serde_json::json!({ "slug": "self" })).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["slug"], "self");
}

#[tokio::test]
async fn series_identity_allocates_distinct_slugs_for_same_name() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let db = &app.state().db;

    // Two libraries.
    let (_, lib_a) = create_library(&app, &auth, "Lib A", "/tmp/lib-a").await;
    let (_, lib_b) = create_library(&app, &auth, "Lib B", "/tmp/lib-b").await;
    let id_a = uuid::Uuid::parse_str(lib_a["id"].as_str().unwrap()).unwrap();
    let id_b = uuid::Uuid::parse_str(lib_b["id"].as_str().unwrap()).unwrap();

    // Same series name in both, with different years to exercise the
    // year-disambiguator path.
    let hint_2018 = SeriesIdentityHint {
        series_name: "Spider-Man".into(),
        year: Some(2018),
        ..Default::default()
    };
    let hint_2022 = SeriesIdentityHint {
        series_name: "Spider-Man".into(),
        year: Some(2022),
        ..Default::default()
    };

    let m_a = resolve_or_create(
        db,
        id_a,
        std::path::Path::new("/tmp/lib-a/spider"),
        &hint_2018,
        "eng",
    )
    .await
    .unwrap();
    let m_b = resolve_or_create(
        db,
        id_b,
        std::path::Path::new("/tmp/lib-b/spider"),
        &hint_2022,
        "eng",
    )
    .await
    .unwrap();

    let row_a = series::Entity::find_by_id(m_a.id())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let row_b = series::Entity::find_by_id(m_b.id())
        .one(db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(row_a.slug, "spider-man");
    assert_eq!(
        row_b.slug, "spider-man-2022",
        "second series with same name must use the year disambiguator before falling back to numeric",
    );
}
