//! Cover colours for library covers (roadmap WP-9.7).
//!
//! `issues.cover_tone` holds a cover's average and most vibrant colours
//! (`metadata::cover_tone`). These tests pin the two halves around it:
//!
//! - every card payload carries the pair (issue lists, the series list's
//!   cover, the series and issue detail views), and a stored value that is
//!   not twelve hex digits never reaches a client;
//! - the `cover_tone` backfill reads it from the cover thumbnail already
//!   on disk, skips issues that have none without stopping short of the
//!   rest, and leaves a colour that is already there alone.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use entity::{issue, series};
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use serde_json::Value;
use tower::ServiceExt;

async fn register(app: &TestApp, email: &str) -> String {
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
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .map(|c| c.split(';').next().unwrap().to_owned())
        .expect("session cookie")
}

async fn get(app: &TestApp, session: &str, path: &str) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::COOKIE, session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn set_tone(app: &TestApp, issue_id: &str, tone: Option<&str>) {
    let mut row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    row.cover_tone = Set(tone.map(str::to_owned));
    row.update(&app.state().db).await.unwrap();
}

async fn tone_of(app: &TestApp, issue_id: &str) -> Option<String> {
    issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .cover_tone
}

fn item<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("item {id} in {body}"))
}

#[tokio::test]
async fn cards_carry_the_covers_colours() {
    let app = TestApp::spawn().await;
    let session = register(&app, "tone@example.com").await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed_library(db, dir.path()).await;
    let sid = seed_series(db, lib, "Saga").await;
    let first = seed_issue(db, lib, sid, &dir.path().join("saga-1.cbz"), b"one", 1.0).await;
    let second = seed_issue(db, lib, sid, &dir.path().join("saga-2.cbz"), b"two", 2.0).await;
    let slug = series::Entity::find_by_id(sid)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;

    // Not read yet: the field is absent everywhere.
    let issues = get(&app, &session, &format!("/api/series/{slug}/issues")).await;
    assert!(item(&issues, &first).get("cover_tone").is_none());
    let list = get(&app, &session, "/api/series").await;
    assert!(item(&list, &sid.to_string()).get("cover_tone").is_none());

    set_tone(&app, &first, Some("c8281ef0a020")).await;
    set_tone(&app, &second, Some("101010e0e0e0")).await;

    // Issue cards.
    let issues = get(&app, &session, &format!("/api/series/{slug}/issues")).await;
    assert_eq!(item(&issues, &first)["cover_tone"], "c8281ef0a020");
    assert_eq!(item(&issues, &second)["cover_tone"], "101010e0e0e0");

    // The series card and the series page take their cover issue's pair
    // (issue #1 here), from the cover query they already run.
    let list = get(&app, &session, "/api/series").await;
    let card = item(&list, &sid.to_string());
    assert_eq!(card["cover_url"], format!("/issues/{first}/pages/0/thumb"));
    assert_eq!(card["cover_tone"], "c8281ef0a020");
    let detail = get(&app, &session, &format!("/api/series/{slug}")).await;
    assert_eq!(detail["cover_tone"], "c8281ef0a020");

    // The issue page's header cover.
    let issue_slug = item(&issues, &second)["slug"].as_str().unwrap().to_owned();
    let issue_detail = get(
        &app,
        &session,
        &format!("/api/series/{slug}/issues/{issue_slug}"),
    )
    .await;
    assert_eq!(issue_detail["cover_tone"], "101010e0e0e0");

    // The web app writes the value into inline styles: anything that is
    // not twelve lowercase hex digits is dropped, not passed on.
    set_tone(&app, &first, Some("red; background: url(x)")).await;
    let issues = get(&app, &session, &format!("/api/series/{slug}/issues")).await;
    assert!(item(&issues, &first).get("cover_tone").is_none());
    let list = get(&app, &session, "/api/series").await;
    assert!(item(&list, &sid.to_string()).get("cover_tone").is_none());
}

/// A solid-colour cover thumbnail where the thumbnail job writes one.
fn write_thumb(app: &TestApp, issue_id: &str, rgb: [u8; 3]) {
    let dir = app.state().cfg().data_path.join("thumbs");
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbImage::from_pixel(60, 90, image::Rgb(rgb))
        .save(dir.join(format!("{issue_id}.png")))
        .unwrap();
}

#[tokio::test]
async fn the_backfill_reads_colours_from_the_thumbnails_on_disk() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed_library(db, dir.path()).await;
    let sid = seed_series(db, lib, "Saga").await;
    let mut ids = Vec::new();
    for n in 1..=4u8 {
        let path = dir.path().join(format!("saga-{n}.cbz"));
        ids.push(seed_issue(db, lib, sid, &path, &[n], f64::from(n)).await);
    }
    // In id order, as the drain walks them: a cover, no thumbnail at all,
    // a file that is not an image, and another cover after both.
    ids.sort();
    write_thumb(&app, &ids[0], [200, 40, 30]);
    let thumbs = app.state().cfg().data_path.join("thumbs");
    std::fs::write(thumbs.join(format!("{}.png", ids[2])), b"not an image").unwrap();
    write_thumb(&app, &ids[3], [10, 20, 240]);
    // An issue that already has its colours is left alone.
    let done = seed_issue(db, lib, sid, &dir.path().join("saga-9.cbz"), b"nine", 9.0).await;
    set_tone(&app, &done, Some("000000ffffff")).await;
    write_thumb(&app, &done, [1, 2, 3]);

    let (read, skipped) = server::jobs::backfill::drain_cover_tones(&app.state()).await;
    assert_eq!((read, skipped), (2, 2));
    assert_eq!(
        tone_of(&app, &ids[0]).await.as_deref(),
        Some("c8281ec8281e")
    );
    assert_eq!(tone_of(&app, &ids[1]).await, None);
    assert_eq!(tone_of(&app, &ids[2]).await, None);
    assert_eq!(
        tone_of(&app, &ids[3]).await.as_deref(),
        Some("0a14f00a14f0"),
        "the skipped issues before it did not stop the drain"
    );
    assert_eq!(tone_of(&app, &done).await.as_deref(), Some("000000ffffff"));

    // Re-running finds only the two it cannot read.
    let (read, skipped) = server::jobs::backfill::drain_cover_tones(&app.state()).await;
    assert_eq!((read, skipped), (0, 2));
}
