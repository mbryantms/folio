//! Series cover pick: the main run's lowest numbered issue anchors the
//! series cover — never an annual / one-shot / TPB that parsed to
//! `sort_number = 1` from a specials subfolder, and never a #0 / #½
//! prelude when the run has a real first issue (be it #1 or #424).
//! Both the `/series` grid rows (`hydrate_series`) and the `/series/{slug}`
//! detail hero must agree.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::Value;
use std::path::Path;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_authed(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
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

async fn get_json(app: &TestApp, auth: &Authed, path: &str) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
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
    assert_eq!(resp.status(), StatusCode::OK, "{path}");
    body_json(resp.into_body()).await
}

/// Seed one issue; `special_type` is stamped after insert the way the
/// scanner does for a file under an `Annuals/` subfolder.
async fn issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    dir: &Path,
    name: &str,
    sort_number: f64,
    special_type: Option<&str>,
) -> String {
    let path = dir.join(format!("{name}.cbz"));
    let id = IssueSeed::new(lib_id, series_id, &path, name.as_bytes(), sort_number)
        .insert(&app.state().db)
        .await;
    if let Some(kind) = special_type {
        let mut am: entity::issue::ActiveModel = entity::issue::Entity::find_by_id(id.clone())
            .one(&app.state().db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.special_type = Set(Some(kind.to_owned()));
        am.update(&app.state().db).await.unwrap();
    }
    id
}

fn cover_of(id: &str) -> String {
    format!("/issues/{id}/pages/0/thumb")
}

/// The grid row + the detail hero both anchor on `expected`, and neither
/// mentions `rejected`.
async fn assert_cover(
    app: &TestApp,
    auth: &Authed,
    series_id: Uuid,
    expected: &str,
    rejected: &[&str],
) {
    let detail = get_json(app, auth, &format!("/api/series/{series_id}")).await;
    assert_eq!(
        detail["cover_url"].as_str(),
        Some(cover_of(expected).as_str()),
        "detail cover"
    );
    let list = get_json(app, auth, "/api/series").await.to_string();
    assert!(list.contains(&cover_of(expected)), "grid row cover");
    for r in rejected {
        assert!(!list.contains(&cover_of(r)), "grid row must not use {r}");
    }
}

#[tokio::test]
async fn regular_number_one_beats_a_zero_prelude_and_an_annual_one() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let zero = issue(&app, lib_id, series_id, dir.path(), "Saga 000", 0.0, None).await;
    let one = issue(&app, lib_id, series_id, dir.path(), "Saga 001", 1.0, None).await;
    let _two = issue(&app, lib_id, series_id, dir.path(), "Saga 002", 2.0, None).await;
    let annual = issue(
        &app,
        lib_id,
        series_id,
        dir.path(),
        "Saga Annual 001",
        1.0,
        Some("Annual"),
    )
    .await;
    assert_cover(&app, &auth, series_id, &one, &[&zero, &annual]).await;
}

/// Adventures of Superman (1987): the run starts at #424, has a #0
/// prelude, and `Annuals/… Annual 001.cbz` parses to sort_number 1.
#[tokio::test]
async fn run_starting_mid_numbering_uses_its_first_regular_issue_not_the_annual() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Adventures of Superman")
        .insert(&app.state().db)
        .await;
    let zero = issue(&app, lib_id, series_id, dir.path(), "AoS 000", 0.0, None).await;
    let first = issue(&app, lib_id, series_id, dir.path(), "AoS 424", 424.0, None).await;
    let _next = issue(&app, lib_id, series_id, dir.path(), "AoS 425", 425.0, None).await;
    let annual = issue(
        &app,
        lib_id,
        series_id,
        dir.path(),
        "AoS Annual 001",
        1.0,
        Some("Annual"),
    )
    .await;
    assert_cover(&app, &auth, series_id, &first, &[&zero, &annual]).await;
}

/// Only preludes in the run: the #0 still beats the annual.
#[tokio::test]
async fn preludes_only_run_still_prefers_the_regular_issue_over_a_special() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Prelude")
        .insert(&app.state().db)
        .await;
    let zero = issue(
        &app,
        lib_id,
        series_id,
        dir.path(),
        "Prelude 000",
        0.0,
        None,
    )
    .await;
    let annual = issue(
        &app,
        lib_id,
        series_id,
        dir.path(),
        "Prelude Annual 001",
        1.0,
        Some("Annual"),
    )
    .await;
    assert_cover(&app, &auth, series_id, &zero, &[&annual]).await;
}
