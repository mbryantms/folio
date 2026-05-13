//! Admin thumbnail status + regenerate endpoints (M3).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::{ActiveModel as IssueAM, Entity as IssueEntity},
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::library::thumbnails::{self, THUMBNAIL_VERSION, ThumbFormat};
use std::{fs, path::Path};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"thumbs-admin@example.com","password":"correctly-horse-battery"}"#,
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

async fn seed_library_with_issues(
    app: &TestApp,
    states: &[(bool, bool)], // (generated, errored)
) -> (Uuid, Vec<String>) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Status Lib".into()),
        root_path: Set(format!("/tmp/status-{lib_id}")),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(lib_id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("S".into()),
        normalized_name: Set(normalize_name("S")),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(series_id.to_string()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let mut ids = Vec::with_capacity(states.len());
    for (i, (generated, errored)) in states.iter().enumerate() {
        let id = format!("{:0>64}", format!("{:x}", i + 1));
        IssueAM {
            id: Set(id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            file_path: Set(format!("/tmp/status/{i}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(id.clone()),
            title: Set(None),
            sort_number: Set(Some(i as f64)),
            number_raw: Set(None),
            volume: Set(None),
            year: Set(None),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            manga: Set(None),
            age_rating: Set(None),
            page_count: Set(Some(1)),
            pages: Set(serde_json::json!([])),
            comic_info_raw: Set(serde_json::json!({})),
            alternate_series: Set(None),
            story_arc: Set(None),
            story_arc_number: Set(None),
            characters: Set(None),
            teams: Set(None),
            locations: Set(None),
            tags: Set(None),
            genre: Set(None),
            writer: Set(None),
            penciller: Set(None),
            inker: Set(None),
            colorist: Set(None),
            letterer: Set(None),
            cover_artist: Set(None),
            editor: Set(None),
            translator: Set(None),
            publisher: Set(None),
            imprint: Set(None),
            scan_information: Set(None),
            community_rating: Set(None),
            review: Set(None),
            web_url: Set(None),
            comicvine_id: Set(None),
            metron_id: Set(None),
            gtin: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            slug: Set(id.clone()),
            hash_algorithm: Set(1),
            thumbnails_generated_at: Set(if *generated { Some(now) } else { None }),
            thumbnail_version: Set(if *generated { THUMBNAIL_VERSION } else { 0 }),
            thumbnails_error: Set(if *errored { Some("oops".into()) } else { None }),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
        ids.push(id);
    }
    (lib_id, ids)
}

async fn get(app: &TestApp, auth: &Authed, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn post(app: &TestApp, auth: &Authed, path: &str) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

#[tokio::test]
async fn status_counts_match_seeded_state() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, ids) = seed_library_with_issues(
        &app,
        &[
            (true, false),  // generated
            (true, false),  // generated
            (false, false), // missing
            (false, true),  // missing + errored
        ],
    )
    .await;
    let data_dir = app.state().cfg().data_path.clone();
    for id in ids.iter().take(2) {
        let path = thumbnails::strip_path(&data_dir, id, 0, ThumbFormat::Webp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"thumb").unwrap();
    }

    let (status, body) = get(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-status"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 4);
    assert_eq!(body["generated"], 2);
    assert_eq!(body["missing"], 2);
    assert_eq!(body["errored"], 1);
    assert_eq!(body["page_total"], 4);
    assert_eq!(body["page_generated"], 2);
    assert_eq!(body["page_missing"], 2);
    assert_eq!(body["current_version"], THUMBNAIL_VERSION);
}

#[tokio::test]
async fn status_requires_admin() {
    // Spawn a fresh app and register a NON-admin user (second-registered
    // is `user`, not `admin` — the first user always becomes admin).
    let app = TestApp::spawn().await;
    let _admin = register_admin(&app).await;

    // Register a regular user and use their session.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"reader@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let session = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_session=")
        .to_owned();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/admin/libraries/{}/thumbnails-status",
                    Uuid::nil()
                ))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn force_recreate_clears_state_and_reports_count() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, ids) =
        seed_library_with_issues(&app, &[(true, false), (true, false), (false, false)]).await;

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails/force-recreate"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // After clearing, all three are pending.
    assert_eq!(body["enqueued"], 3);

    // Verify DB rows cleared.
    let state = app.state();
    for id in ids {
        let row = IssueEntity::find_by_id(id)
            .one(&state.db)
            .await
            .unwrap()
            .unwrap();
        assert!(row.thumbnails_generated_at.is_none());
        assert_eq!(row.thumbnail_version, 0);
        assert!(row.thumbnails_error.is_none());
    }
}

#[tokio::test]
async fn generate_missing_reenqueues_outdated_versions() {
    // Version bumps should be visible to the catchup path; the worker is now
    // cover-first, so this regenerates the cheap cover artifact instead of
    // flooding the queue with full page-strip work.
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, ids) = seed_library_with_issues(&app, &[(true, false), (true, false)]).await;

    // Backdate one issue's thumbnail_version to simulate an encoder bump.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let row = IssueEntity::find_by_id(ids[0].clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.thumbnail_version = Set(0);
    am.update(&db).await.unwrap();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails/generate-missing"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 1);
}

#[tokio::test]
async fn generate_missing_skips_disabled_library() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(false, false), (false, false)]).await;

    // Disable thumbnails on the library.
    let resp = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(resp.0, StatusCode::OK);
    assert_eq!(resp.1["enabled"], false);

    // generate-missing should now 409 because thumbnails are disabled.
    let (status, _) = post_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails/generate-missing"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn generate_page_map_enqueues_strip_jobs_for_active_issues() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(true, false), (true, false)]).await;

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails/generate-page-map"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 2);
}

#[tokio::test]
async fn generate_page_map_skips_disabled_library() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(true, false)]).await;

    let resp = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(resp.0, StatusCode::OK);

    let (status, _) = post_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails/generate-page-map"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn update_settings_rejects_unknown_format() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(false, false)]).await;

    let (status, _) = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "format": "tiff" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn update_settings_accepts_separate_quality_values() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(false, false)]).await;

    let (status, body) = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "cover_quality": 92, "page_quality": 35 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cover_quality"], 92);
    assert_eq!(body["page_quality"], 35);

    let (status, body) = get(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cover_quality"], 92);
    assert_eq!(body["page_quality"], 35);
}

#[tokio::test]
async fn update_settings_rejects_quality_outside_slider_range() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, _) = seed_library_with_issues(&app, &[(false, false)]).await;

    let (status, _) = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "cover_quality": 101 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_all_clears_state() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, ids) =
        seed_library_with_issues(&app, &[(true, false), (true, false), (true, true)]).await;

    let (status, body) = delete_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 3);

    let state = app.state();
    for id in ids {
        let row = IssueEntity::find_by_id(id)
            .one(&state.db)
            .await
            .unwrap()
            .unwrap();
        assert!(row.thumbnails_generated_at.is_none());
        assert_eq!(row.thumbnail_version, 0);
        assert!(row.thumbnails_error.is_none());
    }
}

#[tokio::test]
async fn regenerate_issue_cover_returns_200_and_clears_state() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false)]).await;

    // `seed_library_with_issues` uses the series id's text as the slug.
    let issue = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let series_slug = issue.series_id.to_string();
    let issue_slug = issue.slug.clone();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/regenerate-cover"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 1);

    let row = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.thumbnails_generated_at.is_none());
}

/// Cover regen must wipe only the cover file — strip thumbs the user paid
/// to encode should survive. Regression for the pre-rename behavior, which
/// nuked the entire `<id>/` subtree.
#[tokio::test]
async fn regenerate_issue_cover_preserves_strip_dir() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false)]).await;
    let issue = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let data_dir = app.state().cfg().data_path.clone();

    // Plant a cover file + a strip file so we can prove the wipe scope.
    let cover_path = thumbnails::cover_path(&data_dir, &ids[0], ThumbFormat::Webp);
    fs::create_dir_all(cover_path.parent().unwrap()).unwrap();
    fs::write(&cover_path, b"cover").unwrap();
    let strip_path = thumbnails::strip_path(&data_dir, &ids[0], 0, ThumbFormat::Webp);
    fs::create_dir_all(strip_path.parent().unwrap()).unwrap();
    fs::write(&strip_path, b"strip").unwrap();

    let (status, _) = post_json(
        &app,
        &auth,
        &format!(
            "/admin/series/{}/issues/{}/thumbnails/regenerate-cover",
            issue.series_id, issue.slug
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(!cover_path.exists(), "cover should be wiped");
    assert!(strip_path.exists(), "strips must be preserved");
}

#[tokio::test]
async fn regenerate_issue_cover_404_for_unknown() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let s = post(
        &app,
        &auth,
        "/admin/series/no-such-series/issues/no-such-issue/thumbnails/regenerate-cover",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn generate_issue_page_map_enqueues_strip_job() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false)]).await;
    let issue = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!(
            "/admin/series/{}/issues/{}/thumbnails/generate-page-map",
            issue.series_id, issue.slug
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 1);

    // Dedupe: a second call with the prior job still queued returns 0
    // because `try_mark_thumb_job_queued` rejects duplicates.
    let (status, body) = post_json(
        &app,
        &auth,
        &format!(
            "/admin/series/{}/issues/{}/thumbnails/generate-page-map",
            issue.series_id, issue.slug
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 0);
}

/// Force-recreate must wipe the `<id>/s/` subtree but leave `<id>.<ext>`
/// (the cover file) alone — opposite scope of the cover regen test.
#[tokio::test]
async fn force_recreate_issue_page_map_wipes_strips_only() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false)]).await;
    let issue = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let data_dir = app.state().cfg().data_path.clone();

    let cover_path = thumbnails::cover_path(&data_dir, &ids[0], ThumbFormat::Webp);
    fs::create_dir_all(cover_path.parent().unwrap()).unwrap();
    fs::write(&cover_path, b"cover").unwrap();
    let strip_path = thumbnails::strip_path(&data_dir, &ids[0], 0, ThumbFormat::Webp);
    fs::create_dir_all(strip_path.parent().unwrap()).unwrap();
    fs::write(&strip_path, b"strip").unwrap();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!(
            "/admin/series/{}/issues/{}/thumbnails/force-recreate-page-map",
            issue.series_id, issue.slug
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 1);

    assert!(cover_path.exists(), "cover must be preserved");
    assert!(!strip_path.exists(), "strips should be wiped");
}

#[tokio::test]
async fn regenerate_series_cover_clears_state_for_all_issues() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) =
        seed_library_with_issues(&app, &[(true, false), (true, false), (true, true)]).await;
    let series_slug = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .series_id
        .to_string();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/series/{series_slug}/thumbnails/regenerate-cover"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 3);

    for id in &ids {
        let row = IssueEntity::find_by_id(id.clone())
            .one(&app.state().db)
            .await
            .unwrap()
            .unwrap();
        assert!(row.thumbnails_generated_at.is_none());
        assert_eq!(row.thumbnail_version, 0);
        assert!(row.thumbnails_error.is_none());
    }
}

#[tokio::test]
async fn generate_series_page_map_enqueues_strip_jobs() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false), (true, false)]).await;
    let series_slug = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .series_id
        .to_string();

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/series/{series_slug}/thumbnails/generate-page-map"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 2);
}

/// Series-scope force-recreate wipes every issue's `<id>/s/` subtree but
/// preserves every cover. Verifies the parallel-wipe path scopes correctly.
#[tokio::test]
async fn force_recreate_series_page_map_wipes_strips_only() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib_id, ids) = seed_library_with_issues(&app, &[(true, false), (true, false)]).await;
    let series_slug = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .series_id
        .to_string();
    let data_dir = app.state().cfg().data_path.clone();

    for id in &ids {
        let cover_path = thumbnails::cover_path(&data_dir, id, ThumbFormat::Webp);
        fs::create_dir_all(cover_path.parent().unwrap()).unwrap();
        fs::write(&cover_path, b"cover").unwrap();
        let strip_path = thumbnails::strip_path(&data_dir, id, 0, ThumbFormat::Webp);
        fs::create_dir_all(strip_path.parent().unwrap()).unwrap();
        fs::write(&strip_path, b"strip").unwrap();
    }

    let (status, body) = post_json(
        &app,
        &auth,
        &format!("/admin/series/{series_slug}/thumbnails/force-recreate-page-map"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enqueued"], 2);

    for id in &ids {
        let cover_path = thumbnails::cover_path(&data_dir, id, ThumbFormat::Webp);
        let strip_path = thumbnails::strip_path(&data_dir, id, 0, ThumbFormat::Webp);
        assert!(cover_path.exists(), "cover must be preserved for {id}");
        assert!(!strip_path.exists(), "strips should be wiped for {id}");
    }
}

/// One consolidated 409 case that exercises every targeted route at once —
/// each must short-circuit when the parent library has thumbnails disabled.
#[tokio::test]
async fn targeted_thumb_endpoints_skip_disabled_library() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (lib_id, ids) = seed_library_with_issues(&app, &[(true, false)]).await;
    let issue = IssueEntity::find_by_id(ids[0].clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let series_slug = issue.series_id.to_string();
    let issue_slug = issue.slug.clone();

    let resp = patch_json(
        &app,
        &auth,
        &format!("/admin/libraries/{lib_id}/thumbnails-settings"),
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(resp.0, StatusCode::OK);

    let routes = [
        format!("/admin/series/{series_slug}/thumbnails/regenerate-cover"),
        format!("/admin/series/{series_slug}/thumbnails/generate-page-map"),
        format!("/admin/series/{series_slug}/thumbnails/force-recreate-page-map"),
        format!("/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/regenerate-cover"),
        format!("/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/generate-page-map"),
        format!(
            "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/force-recreate-page-map"
        ),
    ];
    for route in routes {
        let (status, _) = post_json(&app, &auth, &route).await;
        assert_eq!(status, StatusCode::CONFLICT, "{route} should 409");
    }
}

async fn post_json(app: &TestApp, auth: &Authed, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn patch_json(
    app: &TestApp,
    auth: &Authed,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn delete_json(app: &TestApp, auth: &Authed, path: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

// Suppress unused-import warnings for helpers used only by some tests.
#[allow(dead_code)]
fn _unused(_: &Path) {}
