//! `GET /issues?sort=year` orders by release date — `(year, month, day,
//! id)`, NULLS LAST on every tier in both directions — so the grid's
//! "Release date" label is true (post-audit T2.3). Also pins the
//! cursor-helper fix that keeps the NULL bucket reachable after a
//! non-NULL boundary row (`apply_i32_cursor` / `apply_f64_cursor`).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::Value;
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

/// Walk every page of a cursor list and return the ids in order.
async fn walk(app: &TestApp, auth: &Authed, base: &str, limit: u64) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let path = match &cursor {
            Some(c) => format!("{base}&limit={limit}&cursor={c}"),
            None => format!("{base}&limit={limit}"),
        };
        let body = get_json(app, auth, &path).await;
        for item in body["items"].as_array().unwrap() {
            out.push(item["id"].as_str().unwrap().to_owned());
        }
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => return out,
        }
    }
    panic!("cursor walk did not terminate");
}

async fn dated_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    dir: &std::path::Path,
    name: &str,
    n: f64,
    date: (Option<i32>, Option<i32>, Option<i32>),
) -> String {
    let path = dir.join(format!("{name}.cbz"));
    let id = IssueSeed::new(lib_id, series_id, &path, name.as_bytes(), n)
        .with_title(name)
        .insert(&app.state().db)
        .await;
    let mut am: entity::issue::ActiveModel = entity::issue::Entity::find_by_id(id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.year = Set(date.0);
    am.month = Set(date.1);
    am.day = Set(date.2);
    am.update(&app.state().db).await.unwrap();
    id
}

#[tokio::test]
async fn release_date_sort_orders_by_year_month_day_with_nulls_last() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Dated")
        .insert(&app.state().db)
        .await;
    let d = dir.path();
    let a = dated_issue(
        &app,
        lib_id,
        series_id,
        d,
        "a",
        1.0,
        (Some(2020), Some(3), Some(5)),
    )
    .await;
    let b = dated_issue(
        &app,
        lib_id,
        series_id,
        d,
        "b",
        2.0,
        (Some(2020), Some(3), None),
    )
    .await;
    let c = dated_issue(
        &app,
        lib_id,
        series_id,
        d,
        "c",
        3.0,
        (Some(2020), Some(1), Some(20)),
    )
    .await;
    let e = dated_issue(
        &app,
        lib_id,
        series_id,
        d,
        "e",
        4.0,
        (Some(2019), Some(12), Some(31)),
    )
    .await;
    let n = dated_issue(&app, lib_id, series_id, d, "n", 5.0, (None, None, None)).await;

    // ASC: 2019-12-31, 2020-01-20, 2020-03-05, 2020-03-??, undated.
    // `limit=2` puts a page boundary on every interesting edge: a dated
    // row before the next month, a NULL-day row before the NULL-year
    // bucket (the bucket used to be unreachable past a non-NULL boundary).
    let asc = walk(&app, &auth, "/api/issues?sort=year&order=asc", 2).await;
    assert_eq!(
        asc,
        vec![e.clone(), c.clone(), a.clone(), b.clone(), n.clone()]
    );

    // DESC keeps NULLs last on every tier: month 3 (day 5, then no day),
    // month 1, 2019, undated.
    let desc = walk(&app, &auth, "/api/issues?sort=year&order=desc", 2).await;
    assert_eq!(desc, vec![a, b, c, e, n]);
}

#[tokio::test]
async fn series_year_sort_keeps_the_undated_bucket_reachable_past_a_dated_boundary() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path()).insert(db).await;
    let dated = [
        (
            SeriesSeed::new(lib_id, "S 2020").insert(db).await,
            Some(2020),
        ),
        (
            SeriesSeed::new(lib_id, "S 2021").insert(db).await,
            Some(2021),
        ),
        (SeriesSeed::new(lib_id, "S undated").insert(db).await, None),
    ];
    for (id, year) in &dated {
        let mut am: entity::series::ActiveModel = entity::series::Entity::find_by_id(*id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .into();
        am.year = Set(*year);
        am.update(db).await.unwrap();
    }
    // Page 1 = the two dated series; the boundary row is the last dated
    // one, so page 2 must still contain the undated series.
    let ids = walk(&app, &auth, "/api/series?sort=year&order=asc", 2).await;
    assert_eq!(
        ids,
        vec![
            dated[0].0.to_string(),
            dated[1].0.to_string(),
            dated[2].0.to_string()
        ]
    );
}
