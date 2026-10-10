//! The series page's Metadata tab: `GET /series/{slug}/metadata/overview`,
//! `GET /series/{slug}/metadata/issues` (filters server-side) and the bulk
//! acknowledgement `POST /series/{slug}/metadata/issues/accept`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::{TestApp, seed};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn register(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": "admin@example.com", "password": "correctly-horse-battery"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .filter(|c| c.starts_with("__Host-comic_session=") || c.starts_with("__Host-comic_csrf="))
        .collect::<Vec<_>>()
        .join("; ")
}

fn csrf(cookie: &str) -> String {
    cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned()
}

async fn call(
    app: &TestApp,
    cookie: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf(cookie));
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => {
            req = req.header(header::CONTENT_LENGTH, "0");
            req.body(Body::empty()).unwrap()
        }
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn external_id(db: &sea_orm::DatabaseConnection, entity_id: &str, source: &str) {
    let now = chrono::Utc::now().fixed_offset();
    entity::external_id::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(entity_id.to_owned()),
        source: Set(source.into()),
        external_id: Set(format!("{entity_id}-{source}")),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn fill(db: &sea_orm::DatabaseConnection, id: &str, summary: bool, notes: Option<&str>) {
    entity::issue::ActiveModel {
        id: Set(id.to_owned()),
        year: Set(Some(1990)),
        month: Set(Some(7)),
        summary: Set(summary.then(|| "A summary.".to_owned())),
        writer: Set(Some("Someone".into())),
        comic_info_raw: Set(json!({ "notes": notes.unwrap_or(""), "genre": "", "web": "" })),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

struct Fixture {
    slug: String,
    complete: String,
    bare: String,
    annual: String,
    _dir: tempfile::TempDir,
}

async fn fixture(app: &TestApp) -> Fixture {
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    let series_id = seed::seed_series(db, lib, "Wonder Woman").await;
    // The folder holds a Mylar cvinfo and one rewrite backup.
    std::fs::write(
        dir.path().join("cvinfo"),
        "https://comicvine.gamespot.com/x/4050-1/",
    )
    .unwrap();
    std::fs::write(dir.path().join("Wonder Woman 001.cbz.bak"), b"old").unwrap();
    entity::series::ActiveModel {
        id: Set(series_id),
        folder_path: Set(Some(dir.path().to_string_lossy().into_owned())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;

    // #1: every id and core field → complete.
    let complete = seed::seed_issue(
        db,
        lib,
        series_id,
        &dir.path().join("Wonder Woman 001.cbz"),
        b"a",
        1.0,
    )
    .await;
    for src in ["comicvine", "metron", "gcd"] {
        external_id(db, &complete, src).await;
    }
    fill(
        db,
        &complete,
        true,
        Some("Tagged with Folio on 2026-10-10 — sources Metron"),
    )
    .await;
    // #2: no id at all, tagged by ComicTagger from Metron → needs metadata.
    let bare = seed::seed_issue(
        db,
        lib,
        series_id,
        &dir.path().join("Wonder Woman 002.cbz"),
        b"b",
        2.0,
    )
    .await;
    fill(
        db,
        &bare,
        true,
        Some("Tagged with ComicTagger 1.6.0 using info from Metron on 2025-05-02. [Issue ID 7]"),
    )
    .await;
    // Annual 1: a Metron id, no summary → partial.
    let annual = seed::seed_issue(
        db,
        lib,
        series_id,
        &dir.path().join("Wonder Woman Annual 001.cbz"),
        b"c",
        1.0,
    )
    .await;
    entity::issue::ActiveModel {
        id: Set(annual.clone()),
        special_type: Set(Some("Annual".into())),
        number_raw: Set(Some("1".into())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
    external_id(db, &annual, "metron").await;
    fill(db, &annual, false, None).await;
    Fixture {
        slug,
        complete,
        bare,
        annual,
        _dir: dir,
    }
}

#[tokio::test]
async fn overview_reports_ids_tiers_sync_files_and_provenance() {
    let app = TestApp::spawn().await;
    let cookie = register(&app).await;
    let f = fixture(&app).await;

    let (st, body) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{}/metadata/overview", f.slug),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["series_ids"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["issue_ids"]["total"], 3);
    assert_eq!(body["issue_ids"]["comicvine"], 1);
    assert_eq!(body["issue_ids"]["metron"], 2);
    assert_eq!(body["issue_ids"]["gcd"], 1);
    assert_eq!(body["issue_ids"]["all_sources"], 1);
    assert_eq!(body["tiers"]["complete"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["partial"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["needs_metadata"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["accepted"], 0);
    assert_eq!(body["sync"]["issues_synced"], 0);
    assert_eq!(body["sync"]["sidecars_rewritten"], 0);
    assert_eq!(body["sync"]["metadata_sync_paused"], false);
    assert_eq!(body["files"]["archives"], 3);
    assert_eq!(body["files"]["by_extension"][0]["extension"], "cbz");
    assert_eq!(body["files"]["by_extension"][0]["count"], 3);
    assert_eq!(body["files"]["cvinfo_present"], true);
    assert_eq!(body["files"]["backups"], 1);
    assert_eq!(body["runs_total"], 0);
    assert!(body["last_run"].is_null());
    assert_eq!(body["series_provenance"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["issue_provenance"]["file"], 0);
}

#[tokio::test]
async fn issues_list_rows_with_tier_ids_tagger_and_server_side_filters() {
    let app = TestApp::spawn().await;
    let cookie = register(&app).await;
    let f = fixture(&app).await;
    let base = format!("/api/series/{}/metadata/issues", f.slug);

    let (st, body) = call(&app, &cookie, Method::GET, &base, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 3);
    assert_eq!(body["series_total"], 3);
    let items = body["items"].as_array().unwrap();
    let row = |id: &str| {
        items
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id}: {body}"))
    };
    let complete = row(&f.complete);
    assert_eq!(complete["label"], "#1");
    assert_eq!(complete["tier"], "complete");
    assert_eq!(complete["cover_date"], "1990-07");
    assert_eq!(
        complete["ids"]["comicvine"],
        format!("{}-comicvine", f.complete)
    );
    assert_eq!(complete["ids"]["gcd"], format!("{}-gcd", f.complete));
    assert_eq!(complete["tagged_by"], "folio");
    assert!(complete["missing_core"].as_array().unwrap().is_empty());
    assert!(complete["last_run"].is_null());
    let bare = row(&f.bare);
    assert_eq!(bare["label"], "#2");
    assert_eq!(bare["tier"], "needs_metadata");
    assert_eq!(bare["tagged_by"], "comictagger_metron");
    assert!(bare["ids"]["metron"].is_null());
    assert!(
        bare["missing_core"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "external_id")
    );
    let annual = row(&f.annual);
    assert_eq!(annual["label"], "Annual 1");
    assert_eq!(annual["special_type"], "Annual");
    assert_eq!(annual["tier"], "partial");
    assert_eq!(annual["missing_core"], json!(["summary"]));
    assert_eq!(annual["tagged_by"], "untagged");

    let ids = |body: &Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, b) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{base}?tier=incomplete"),
        None,
    )
    .await;
    assert_eq!(b["total"], 2);
    assert!(!ids(&b).contains(&f.complete));
    let (_, b) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{base}?without=metron"),
        None,
    )
    .await;
    assert_eq!(ids(&b), vec![f.bare.clone()]);
    let (_, b) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{base}?kind=specials"),
        None,
    )
    .await;
    assert_eq!(ids(&b), vec![f.annual.clone()]);
    let (_, b) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{base}?kind=main&tier=needs_metadata"),
        None,
    )
    .await;
    assert_eq!(ids(&b), vec![f.bare.clone()]);
    let (st, b) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{base}?tier=bogus"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
    let (st, _) = call(
        &app,
        &cookie,
        Method::GET,
        "/api/series/no-such-series/metadata/issues",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bulk_accept_marks_only_this_series_issues_and_reverts() {
    let app = TestApp::spawn().await;
    let cookie = register(&app).await;
    let f = fixture(&app).await;
    let path = format!("/api/series/{}/metadata/issues/accept", f.slug);

    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &path,
        Some(json!({"issue_ids": [], "accepted": true})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &path,
        Some(json!({"issue_ids": [f.bare, f.annual, "not-an-issue"], "accepted": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["updated"], 2);

    let (_, overview) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{}/metadata/overview", f.slug),
        None,
    )
    .await;
    assert_eq!(overview["tiers"]["accepted"], 2, "{}", overview["tiers"]);
    assert_eq!(overview["tiers"]["needs_metadata"], 0);
    let (_, accepted) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{}/metadata/issues?tier=accepted", f.slug),
        None,
    )
    .await;
    assert_eq!(accepted["total"], 2);
    assert!(
        accepted["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["accepted"] == true)
    );

    // Already accepted: nothing to change.
    let (_, again) = call(
        &app,
        &cookie,
        Method::POST,
        &path,
        Some(json!({"issue_ids": [f.bare], "accepted": true})),
    )
    .await;
    assert_eq!(again["updated"], 0);
    // Revert one.
    let (_, revert) = call(
        &app,
        &cookie,
        Method::POST,
        &path,
        Some(json!({"issue_ids": [f.bare], "accepted": false})),
    )
    .await;
    assert_eq!(revert["updated"], 1);
    let (_, overview) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{}/metadata/overview", f.slug),
        None,
    )
    .await;
    assert_eq!(overview["tiers"]["accepted"], 1);
    assert_eq!(overview["tiers"]["needs_metadata"], 1);
}
