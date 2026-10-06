//! `GET /series/{slug}/issues?kind=main|special[&special_type=…]`: the
//! series page lists the run and its specials (annuals / one-shots /
//! specials / collected editions) from separate server-side queries, so
//! a section never depends on which pages of the other are loaded.

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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

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
        .with_title(name)
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

fn ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn kind_and_special_type_filters_split_the_run_from_its_specials() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let d = dir.path();
    let one = issue(&app, lib_id, series_id, d, "Saga 001", 1.0, None).await;
    let two = issue(&app, lib_id, series_id, d, "Saga 002", 2.0, None).await;
    let annual = issue(
        &app,
        lib_id,
        series_id,
        d,
        "Saga Annual 001",
        1.0,
        Some("Annual"),
    )
    .await;
    let oneshot = issue(
        &app,
        lib_id,
        series_id,
        d,
        "Saga Oneshot",
        1.0,
        Some("OneShot"),
    )
    .await;

    let base = format!("/api/series/{series_id}/issues");
    // Unfiltered: everything (the pre-sections shape).
    let all = get_json(&app, &auth, &base).await;
    assert_eq!(all["total"], 4);

    let main = get_json(&app, &auth, &format!("{base}?kind=main")).await;
    assert_eq!(main["total"], 2);
    assert_eq!(ids(&main), vec![one.clone(), two.clone()]);

    let specials = get_json(&app, &auth, &format!("{base}?kind=special")).await;
    assert_eq!(specials["total"], 2);
    let mut got = ids(&specials);
    got.sort();
    let mut want = vec![annual.clone(), oneshot.clone()];
    want.sort();
    assert_eq!(got, want);

    let annuals = get_json(&app, &auth, &format!("{base}?special_type=Annual")).await;
    assert_eq!(ids(&annuals), vec![annual.clone()]);
    assert_eq!(annuals["items"][0]["special_type"], "Annual");

    // A search spans both kinds unless narrowed.
    let searched = get_json(&app, &auth, &format!("{base}?q=Saga")).await;
    assert_eq!(searched["total"], 4);
    let searched_main = get_json(&app, &auth, &format!("{base}?q=Saga&kind=main")).await;
    assert_eq!(searched_main["total"], 2);
}
