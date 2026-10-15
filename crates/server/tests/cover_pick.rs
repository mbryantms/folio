//! Cover picks (post-audit T4.2, decision D8a):
//!   - `PATCH /series/{s}/issues/{i}/cover {page_index}` sets the issue's
//!     cover page, pins it under the user tier, marks the page FrontCover
//!     in the pages JSON, and a content-changed rescan keeps the pick.
//!   - `PATCH /series/{s}/cover {issue_id}` picks the issue whose cover
//!     represents the series on the detail hero and the grid; `null`
//!     clears it; a pick whose issue is removed falls back.
//!   - both are admin-only and audited.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{audit_log, field_provenance, issue, series_cover};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

/// The first registered user is the admin; later ones are plain users.
async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .unwrap()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    let cookie = format!(
        "__Host-comic_session={}; __Host-comic_csrf={}",
        user.session, user.csrf
    );
    req = req.header(header::COOKIE, cookie);
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let body_bytes = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body_bytes).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// A three-page CBZ whose ComicInfo marks `front_cover` as the cover.
fn write_cbz(path: &Path, front_cover: i32, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for n in 1..=3u32 {
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&unique_marker.to_le_bytes());
        png.extend_from_slice(&n.to_le_bytes());
        png.extend(std::iter::repeat_n(0u8, 64));
        zw.start_file(format!("page-{n:03}.png"), opts).unwrap();
        zw.write_all(&png).unwrap();
    }
    let xml = format!(
        r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>Pick</Title>
  <Series>Cover Pick</Series>
  <Number>1</Number>
  <Pages>
    <Page Image="0"/>
    <Page Image="1"/>
    <Page Image="{front_cover}" Type="FrontCover"/>
  </Pages>
</ComicInfo>"#
    );
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(xml.as_bytes()).unwrap();
    zw.finish().unwrap();
}

async fn slugs(app: &TestApp, issue_id: &str) -> (String, String) {
    let db = &app.state().db;
    let row = issue::Entity::find_by_id(issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let s = entity::series::Entity::find_by_id(row.series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    (s.slug, row.slug)
}

async fn user_pin_exists(app: &TestApp, issue_id: &str, field: &str) -> bool {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(issue_id))
        .filter(field_provenance::Column::Field.eq(field))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .one(&app.state().db)
        .await
        .unwrap()
        .is_some()
}

// ───── issue cover page ─────

#[tokio::test]
async fn issue_cover_pick_sets_index_pins_it_and_marks_the_page() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "cover-admin@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path()).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Cover Pick").insert(db).await;
    let file = tmp.path().join("pick-1.cbz");
    write_cbz(&file, 0, 1);
    let payload = std::fs::read(&file).unwrap();
    let issue_id = IssueSeed::new(lib_id, series_id, &file, &payload, 1.0)
        .with_page_count(3)
        .insert(db)
        .await;
    let (s, i) = slugs(&app, &issue_id).await;
    let url = format!("/api/series/{s}/issues/{i}/cover");

    let (status, body) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "page_index": 2 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["cover_page_index"], 2);
    assert_eq!(body["sidecar"], "not_writeback");

    let row = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.cover_page_index, 2);
    let marked: Vec<i64> = row
        .pages
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["type"] == "FrontCover")
        .map(|p| p["image"].as_i64().unwrap())
        .collect();
    assert_eq!(
        marked,
        vec![2],
        "exactly the picked page is FrontCover: {}",
        row.pages
    );
    assert!(user_pin_exists(&app, &issue_id, "cover_page_index").await);

    let audited = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.issue.cover.set"))
        .filter(audit_log::Column::TargetId.eq(issue_id.clone()))
        .one(db)
        .await
        .unwrap();
    assert!(audited.is_some(), "the pick is audited");

    // Out of range → 422; negative → 422 (garde); a plain user → 403.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "page_index": 3 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "page_index": -1 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let user = register(&app, "cover-user@example.com").await;
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &user,
        Some(serde_json::json!({ "page_index": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rescan_keeps_the_picked_cover_page() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "cover-rescan@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Cover Pick (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Cover Pick 001.cbz");
    write_cbz(&file, 0, 1);
    let state = app.state();
    let lib_id = LibrarySeed::new(tmp.path()).insert(&state.db).await;

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    let row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.cover_page_index, 0, "ComicInfo's FrontCover");
    let issue_id = row.id.clone();
    let (s, i) = slugs(&app, &issue_id).await;

    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/api/series/{s}/issues/{i}/cover"),
        &admin,
        Some(serde_json::json!({ "page_index": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");

    // The file changes on disk and now says page 2 is the cover; the pin
    // keeps the admin's page 1.
    write_cbz(&file, 2, 2);
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_updated, 1, "{stats:?}");
    let after = issue::Entity::find_by_id(issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.cover_page_index, 1,
        "the user pick survives the rescan"
    );

    // A sibling issue with no pick still follows the file's marker.
    let other = folder.join("Cover Pick 002.cbz");
    write_cbz(&other, 2, 3);
    scanner::scan_library(&state, lib_id).await.unwrap();
    let sibling = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::Id.ne(issue_id.clone()))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sibling.cover_page_index, 2);
}

// ───── series cover ─────

async fn seed_series_with_issues(
    app: &TestApp,
    root: &Path,
    n: usize,
) -> (Uuid, Vec<String>, String) {
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(root).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Series Cover").insert(db).await;
    let mut ids = Vec::new();
    for k in 1..=n {
        let file = root.join(format!("sc-{k}.cbz"));
        write_cbz(&file, 0, k as u32);
        let payload = std::fs::read(&file).unwrap();
        ids.push(
            IssueSeed::new(lib_id, series_id, &file, &payload, k as f64)
                .with_page_count(3)
                .insert(db)
                .await,
        );
    }
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    (series_id, ids, slug)
}

async fn series_cover_url(app: &TestApp, auth: &Authed, slug: &str) -> String {
    let (status, body) = http(app, Method::GET, &format!("/api/series/{slug}"), auth, None).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body["cover_url"].as_str().unwrap_or_default().to_owned()
}

async fn grid_cover_url(app: &TestApp, auth: &Authed, series_id: Uuid) -> String {
    let (status, body) = http(app, Method::GET, "/api/series?limit=50", auth, None).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == series_id.to_string())
        .and_then(|s| s["cover_url"].as_str())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn series_cover_pick_overrides_the_heuristic_and_clears() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "series-cover@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_id, ids, slug) = seed_series_with_issues(&app, tmp.path(), 3).await;
    let cover_of = |id: &str| format!("/issues/{id}/pages/0/thumb");

    assert_eq!(
        series_cover_url(&app, &admin, &slug).await,
        cover_of(&ids[0])
    );
    assert_eq!(
        grid_cover_url(&app, &admin, series_id).await,
        cover_of(&ids[0])
    );

    let url = format!("/api/series/{slug}/cover");
    let (status, body) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "issue_id": ids[2] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["issue_id"], ids[2]);
    assert_eq!(body["cover_url"], cover_of(&ids[2]));
    assert_eq!(
        series_cover_url(&app, &admin, &slug).await,
        cover_of(&ids[2])
    );
    assert_eq!(
        grid_cover_url(&app, &admin, series_id).await,
        cover_of(&ids[2])
    );

    // Re-picking replaces the row (one active pick per series).
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "issue_id": ids[1] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = series_cover::Entity::find()
        .filter(series_cover::Column::SeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_external_id.as_deref(), Some(ids[1].as_str()));
    assert_eq!(
        series_cover_url(&app, &admin, &slug).await,
        cover_of(&ids[1])
    );

    // Clearing restores the heuristic.
    let (status, body) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "issue_id": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert!(body.get("issue_id").is_none());
    assert_eq!(
        series_cover_url(&app, &admin, &slug).await,
        cover_of(&ids[0])
    );

    let audited = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.series.cover.set"))
        .filter(audit_log::Column::TargetId.eq(series_id.to_string()))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(audited.len(), 3, "every pick and the clear are audited");
}

#[tokio::test]
async fn series_cover_pick_rejects_foreign_or_missing_issues_and_falls_back_when_removed() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "series-cover-2@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_id, ids, slug) = seed_series_with_issues(&app, tmp.path(), 2).await;
    let db = &app.state().db;
    let other_series = SeriesSeed::new(
        issue::Entity::find_by_id(&ids[0])
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .library_id,
        "Other",
    )
    .insert(db)
    .await;
    let other_file = tmp.path().join("other.cbz");
    write_cbz(&other_file, 0, 9);
    let other_payload = std::fs::read(&other_file).unwrap();
    let other_issue = IssueSeed::new(
        issue::Entity::find_by_id(&ids[0])
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .library_id,
        other_series,
        &other_file,
        &other_payload,
        1.0,
    )
    .insert(db)
    .await;
    let url = format!("/api/series/{slug}/cover");

    for bad in [other_issue.as_str(), "does-not-exist"] {
        let (status, body) = http(
            &app,
            Method::PATCH,
            &url,
            &admin,
            Some(serde_json::json!({ "issue_id": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body:#?}");
    }
    let user = register(&app, "series-cover-user@example.com").await;
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &user,
        Some(serde_json::json!({ "issue_id": ids[1] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Pick #2, then soft-remove it: the hero falls back to #1 without
    // anyone touching the pick row.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        &admin,
        Some(serde_json::json!({ "issue_id": ids[1] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut am: issue::ActiveModel = issue::Entity::find_by_id(&ids[1])
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.removed_at = Set(Some(chrono::Utc::now().fixed_offset()));
    am.update(db).await.unwrap();
    assert_eq!(
        series_cover_url(&app, &admin, &slug).await,
        format!("/issues/{}/pages/0/thumb", ids[0])
    );
    let _ = series_id;
}
