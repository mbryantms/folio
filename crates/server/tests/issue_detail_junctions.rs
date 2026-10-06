//! The junction tables feed the issue page. After the series rollup the
//! flat CSV columns are rebuilt from the junctions (normalized names,
//! `; ` when a name contains a comma), and `GET /series/{s}/issues/{i}`
//! carries structured `credits` / `cast` / `genres` / `tag_list` lists
//! with landing-page slugs — the web never splits a CSV column again.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::issue;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::Value;
use server::library::scanner::metadata_rollup::{
    replace_issue_metadata_from_row, rollup_series_metadata,
};
use tempfile::tempdir;

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
    use tower::ServiceExt;
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

use tower::ServiceExt;

#[tokio::test]
async fn rollup_rebuilds_csv_cache_from_junctions_and_detail_exposes_lists() {
    let app = TestApp::spawn().await;
    let auth = register_authed(&app).await;
    let dir = tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path()).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Venom").insert(db).await;
    let path = dir.path().join("venom-1.cbz");
    let id = IssueSeed::new(lib_id, series_id, &path, b"venom-1", 1.0)
        .insert(db)
        .await;
    // What the scanner writes from the file before deriving junctions.
    let mut am: issue::ActiveModel = issue::Entity::find_by_id(id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.writer = Set(Some("Donny Cates".into()));
    am.inker = Set(Some(
        "J. P. Mayer, Mike Deodato, Jr., Andrew Hennessy".into(),
    ));
    am.characters = Set(Some("Eddie Brock, J. Jonah Jameson, Sr".into()));
    am.teams = Set(Some("Capes, Inc.; Avengers".into()));
    am.locations = Set(Some("Fixture City, The Archive".into()));
    am.genre = Set(Some("Superhero, Horror".into()));
    am.tags = Set(Some("symbiote".into()));
    am.update(db).await.unwrap();

    replace_issue_metadata_from_row(db, &id).await.unwrap();
    rollup_series_metadata(db, series_id).await.unwrap();

    // The columns are now derived from the junctions: normalized names,
    // every list in the file's order, `; ` where a name carries a comma.
    let row = issue::Entity::find_by_id(id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.inker.as_deref(),
        Some("J. P. Mayer, Mike Deodato Jr., Andrew Hennessy")
    );
    assert_eq!(
        row.characters.as_deref(),
        Some("Eddie Brock, J. Jonah Jameson Sr.")
    );
    assert_eq!(row.teams.as_deref(), Some("Capes, Inc.; Avengers"));
    // Locations get their entity ids linked only by the rollup — a row
    // without one must still land in the column (LEFT JOIN + COALESCE).
    assert_eq!(row.locations.as_deref(), Some("Fixture City, The Archive"));
    assert_eq!(row.genre.as_deref(), Some("Superhero, Horror"));
    assert_eq!(row.tags.as_deref(), Some("symbiote"));

    // The detail view carries the junction content directly.
    let series_slug = series_id.to_string();
    let v = get_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{}", row.slug),
    )
    .await;
    let credits = v["credits"].as_array().expect("credits");
    let inkers: Vec<&str> = credits
        .iter()
        .filter(|c| c["role"] == "inker")
        .map(|c| c["person"].as_str().unwrap())
        .collect();
    assert_eq!(
        inkers,
        vec!["J. P. Mayer", "Mike Deodato Jr.", "Andrew Hennessy"],
        "stored (file) order"
    );
    let deodato = credits
        .iter()
        .find(|c| c["person"] == "Mike Deodato Jr.")
        .unwrap();
    assert!(deodato["slug"].as_str().is_some_and(|s| !s.is_empty()));
    let characters: Vec<&str> = v["cast"]["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(characters, vec!["Eddie Brock", "J. Jonah Jameson Sr."]);
    let teams: Vec<&str> = v["cast"]["teams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(teams, vec!["Capes, Inc.", "Avengers"]);
    assert_eq!(v["genres"], serde_json::json!(["Superhero", "Horror"]));
    assert_eq!(v["tag_list"], serde_json::json!(["symbiote"]));
}
