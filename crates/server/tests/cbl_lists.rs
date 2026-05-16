//! Saved smart views — M4 integration coverage.
//!
//! Exercises the end-to-end CBL import path: parse the sample.cbl
//! fixture, persist the list + entries, run the matcher against seeded
//! issues with ComicVine IDs, verify the resolution outcomes,
//! manual-match overrides survive a refresh, post-scan rematch lifts
//! missing entries, and the user-scoped CRUD + RBAC boundaries hold.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_entry, cbl_list, cbl_refresh_log,
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, EntityTrait, PaginatorTrait, QueryFilter, Set,
};
use tower::ServiceExt;
use uuid::Uuid;

const SAMPLE_CBL: &str = include_str!("../../../docs/sample.cbl");

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
    #[allow(dead_code)]
    user_id: Uuid,
}

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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Construct a `multipart/form-data` POST body for the upload endpoint.
async fn upload_cbl(
    app: &TestApp,
    auth: &Authed,
    file_name: &str,
    file_bytes: &[u8],
) -> (StatusCode, serde_json::Value) {
    let boundary = "----folio-test-boundary";
    let mut body: Vec<u8> = Vec::with_capacity(file_bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
             Content-Type: application/xml\r\n\r\n",
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/me/cbl-lists/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::from(body))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Seed a library with the issues needed to match a few entries from
/// the sample CBL fixture. Each issue is given a `comicvine_id` that
/// matches the CV `<Database>` IDs in `sample.cbl`.
async fn seed_matchable_issues(app: &TestApp) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("CBL test library".into()),
        root_path: Set(format!("/tmp/cbl-test-{lib_id}")),
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
        thumbnail_format: Set("webp".into()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    // Three sample series to cover both ID-match (CV) and library
    // visibility filtering.
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Invincible".into()),
        normalized_name: Set(normalize_name("Invincible")),
        year: Set(Some(2003)),
        volume: Set(Some(1)),
        publisher: Set(Some("Image".into())),
        imprint: Set(None),
        status: Set("ended".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(Some(17993)),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set("invincible-2003".into()),
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

    // Insert three Invincible issues with CV IDs from the sample.
    // `Invincible #1` = CV 105347, `#2` = 105532, `#3` = 105533.
    let cv_ids = [(1, 105347i64), (2, 105532), (3, 105533)];
    for (num, cv) in cv_ids {
        let issue_id = format!("{:0>62}{:02x}", series_id.simple(), num as u8);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("invincible-{num}")),
            file_path: Set(format!("/tmp/cbl-test/Invincible #{num}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some(num as f64)),
            number_raw: Set(Some(num.to_string())),
            volume: Set(None),
            year: Set(Some(2003)),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            manga: Set(None),
            age_rating: Set(None),
            page_count: Set(Some(20)),
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
            comicvine_id: Set(Some(cv)),
            metron_id: Set(None),
            gtin: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            hash_algorithm: Set(1),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    lib_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_imports_sample_cbl_and_matches_seeded_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;
    let _lib = seed_matchable_issues(&app).await;

    let (status, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "view: {view:#?}");
    assert_eq!(view["parsed_name"], "[Image] Invincible Universe (WEB-KCV)");
    assert_eq!(view["source_kind"], "upload");
    let stats = &view["stats"];
    assert_eq!(stats["total"].as_i64(), Some(269), "all entries persisted");
    // Three matched (Invincible #1-3 via comicvine_id).
    assert_eq!(stats["matched"].as_i64(), Some(3));
    // The remaining 266 are missing — no other CV IDs match seeded issues.
    assert_eq!(stats["missing"].as_i64(), Some(266));
    assert_eq!(stats["manual"].as_i64(), Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_match_overrides_survive_refresh() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

    // Pick an entry currently in `missing` (Tech Jacket #1 — no seeded
    // issue with that CV ID). Manually attach it to one of our seeded
    // Invincible issues.
    let db = Database::connect(&app.db_url).await.unwrap();
    let list_uuid = Uuid::parse_str(&list_id).unwrap();
    let missing_entry = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .filter(cbl_entry::Column::MatchStatus.eq("missing"))
        .filter(cbl_entry::Column::SeriesName.eq("Tech Jacket"))
        .filter(cbl_entry::Column::IssueNumber.eq("1"))
        .one(&db)
        .await
        .unwrap()
        .expect("Tech Jacket #1 entry");

    let target_issue = entity::issue::Entity::find()
        .filter(entity::issue::Column::ComicvineId.eq(105347_i64))
        .one(&db)
        .await
        .unwrap()
        .expect("seeded Invincible #1 issue");

    let url = format!(
        "/api/me/cbl-lists/{list_id}/entries/{entry_id}/match",
        entry_id = missing_entry.id
    );
    let body = serde_json::json!({ "issue_id": target_issue.id });
    let (status, _) = http(&app, Method::POST, &url, Some(&auth), Some(body)).await;
    assert_eq!(status, StatusCode::OK);

    // Force refresh. For an upload, refresh = re-match against the same
    // raw_xml. Manual entry must not regress to `missing`. The refresh
    // path replaces entries (cheaper than per-row UPSERT); manual
    // overrides are preserved by composite-key lookup, not entry id.
    let refresh_url = format!("/api/me/cbl-lists/{list_id}/refresh");
    let (status, _summary) = http(&app, Method::POST, &refresh_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);

    let still_manual = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .filter(cbl_entry::Column::SeriesName.eq("Tech Jacket"))
        .filter(cbl_entry::Column::IssueNumber.eq("1"))
        .one(&db)
        .await
        .unwrap()
        .expect("Tech Jacket #1 still present after refresh");
    assert_eq!(still_manual.match_status, "manual");
    assert_eq!(
        still_manual.matched_issue_id.as_deref(),
        Some(target_issue.id.as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_match_drops_status_to_missing() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "carol@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let db = Database::connect(&app.db_url).await.unwrap();
    let list_uuid = Uuid::parse_str(list_id).unwrap();
    let matched = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .filter(cbl_entry::Column::MatchStatus.eq("matched"))
        .one(&db)
        .await
        .unwrap()
        .expect("at least one matched entry");

    let url = format!(
        "/api/me/cbl-lists/{list_id}/entries/{entry_id}/clear-match",
        entry_id = matched.id
    );
    let (status, _) = http(&app, Method::POST, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);

    let after = cbl_entry::Entity::find_by_id(matched.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.match_status, "missing");
    assert!(after.matched_issue_id.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_cascades_entries_and_refresh_log() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dan@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let db = Database::connect(&app.db_url).await.unwrap();
    let list_uuid = Uuid::parse_str(list_id).unwrap();
    let entry_count = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .count(&db)
        .await
        .unwrap();
    assert!(entry_count > 0);

    let url = format!("/api/me/cbl-lists/{list_id}");
    let (status, _) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let post_entries = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(post_entries, 0, "cascade dropped entries");
    let post_log = cbl_refresh_log::Entity::find()
        .filter(cbl_refresh_log::Column::CblListId.eq(list_uuid))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(post_log, 0, "cascade dropped log");
    let post_list = cbl_list::Entity::find_by_id(list_uuid)
        .one(&db)
        .await
        .unwrap();
    assert!(post_list.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_cascades_linked_cbl_saved_view() {
    // Regression for the "On Deck stacks N copies after N add/remove
    // cycles" bug (dev DB 2026-05-14): the import dialog creates a
    // kind='cbl' saved_view alongside the cbl_lists row. The FK was
    // ON DELETE SET NULL, but the saved_views_kind_chk CHECK
    // constraint (kind='cbl' ↔ cbl_list_id IS NOT NULL) rejected the
    // NULL update, aborting the whole DELETE silently. After the
    // migration changed the action to CASCADE, deleting the CBL list
    // now also drops the linked saved view.
    let app = TestApp::spawn().await;
    let auth = register(&app, "frank@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

    // Frontend's CBL import dialog calls this same endpoint right after
    // upload to register the saved view that backs the rail / sidebar
    // entry. Without that step the bug doesn't trigger, because there's
    // nothing referencing cbl_lists.id.
    let body = serde_json::json!({
        "kind": "cbl",
        "name": "Sample saved view",
        "cbl_list_id": &list_id,
    });
    let (status, view_body) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "saved view create: {view_body:?}"
    );
    let saved_view_id = view_body["id"].as_str().unwrap().to_owned();

    let url = format!("/api/me/cbl-lists/{list_id}");
    let (status, body) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "DELETE must not fail on the kind=cbl saved_view check constraint: {body:?}",
    );

    // Both the CBL row and the dependent saved view are gone.
    let db = Database::connect(&app.db_url).await.unwrap();
    let list_uuid = Uuid::parse_str(&list_id).unwrap();
    let view_uuid = Uuid::parse_str(&saved_view_id).unwrap();
    let list = entity::cbl_list::Entity::find_by_id(list_uuid)
        .one(&db)
        .await
        .unwrap();
    assert!(list.is_none(), "cbl_lists row should be gone");
    let saved = entity::saved_view::Entity::find_by_id(view_uuid)
        .one(&db)
        .await
        .unwrap();
    assert!(
        saved.is_none(),
        "linked kind=cbl saved_view should have cascaded",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_owner_cannot_access_other_users_list() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &owner, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let other = register(&app, "other@example.com").await;
    let url = format!("/api/me/cbl-lists/{list_id}");
    let (status, _) = http(&app, Method::GET, &url, Some(&other), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issues_endpoint_returns_matched_issues_in_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "eve@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let url = format!("/api/me/cbl-lists/{list_id}/issues");
    let (status, body) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    // Three Invincible issues seeded → three items.
    assert_eq!(items.len(), 3, "items: {body:#?}");
    // Position order matches CBL: Invincible #1 lands at position 3 in
    // the sample (after 3 Tech Jacket entries). The matched issues come
    // back in `cbl_entries.position` order — so #1 first among ours,
    // then #2, then #3.
    let numbers: Vec<String> = items
        .iter()
        .map(|i| i["number"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(numbers, vec!["1", "2", "3"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_endpoint_returns_user_owned_lists_with_stats() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "frank@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let _ = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;

    let (status, body) = http(&app, Method::GET, "/api/me/cbl-lists", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["stats"]["total"].as_i64(), Some(269));
    assert_eq!(items[0]["stats"]["matched"].as_i64(), Some(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_returns_raw_xml_with_filename() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "exporter@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

    // Hit the export endpoint and inspect headers + body.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/me/cbl-lists/{list_id}/export"))
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.starts_with("application/xml"), "content-type: {ct}");
    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(cd.starts_with("attachment;"), "content-disposition: {cd}");
    assert!(cd.contains(".cbl"), "filename should be .cbl: {cd}");
    let body_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let xml = std::str::from_utf8(&body_bytes).unwrap();
    // Sample.cbl starts with the standard ReadingList xml root.
    assert!(xml.contains("<ReadingList"));
    assert!(xml.contains("Invincible"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_rejected_for_other_users_lists() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "owner@example.com").await;
    let intruder = register(&app, "intruder@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &owner, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

    let (status, _body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/export"),
        Some(&intruder),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_sources_lists_seeded_dieseltech() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "grace@example.com").await;
    let (status, body) = http(&app, Method::GET, "/api/catalog/sources", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert!(items.iter().any(|s| s["github_owner"] == "DieselTech"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_refresh_schedule_null_clears_column() {
    // Regression: serde's default `Option<Option<T>>` collapses an
    // explicit JSON `null` into `None`, which the handler treats as
    // "field absent" and skips the clear branch. With the
    // `deserialize_some` helper, `null` round-trips as `Some(None)`
    // and the column is set back to NULL.
    let app = TestApp::spawn().await;
    let auth = register(&app, "harold@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();
    let url = format!("/api/me/cbl-lists/{list_id}");

    let (status, body) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({"refresh_schedule": "@daily"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refresh_schedule"].as_str(), Some("@daily"));

    let (status, body) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({"refresh_schedule": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["refresh_schedule"].is_null(),
        "expected refresh_schedule cleared, got {:?}",
        body["refresh_schedule"]
    );
}

// ───── /entries pagination (M1 of the list-pagination-completeness plan) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_endpoint_paginates_via_cursor_and_returns_total_on_first_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-walker@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    // Page 1: total reported, cursor present (269 entries > 100 default).
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"].as_i64(), Some(269), "total on first page");
    let cursor1 = body["next_cursor"].as_str().expect("cursor on page 1");
    let items1 = body["items"].as_array().unwrap();
    assert_eq!(items1.len(), 100, "default page size = 100");

    // Walk all pages, accumulate ids. Total visited should equal 269.
    let mut seen: Vec<String> = items1
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    let mut cursor = Some(cursor1.to_owned());
    while let Some(c) = cursor {
        let (status, body) = http(
            &app,
            Method::GET,
            &format!("/api/me/cbl-lists/{list_id}/entries?cursor={c}"),
            Some(&auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // total is first-page-only.
        assert!(body["total"].is_null(), "total only on first page");
        for it in body["items"].as_array().unwrap() {
            seen.push(it["id"].as_str().unwrap().to_owned());
        }
        cursor = body["next_cursor"].as_str().map(str::to_owned);
    }
    assert_eq!(
        seen.len(),
        269,
        "cursor walk covers every entry exactly once"
    );
    let dedup: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(dedup.len(), 269, "no entry returned twice across pages");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_status_filter_narrows_results() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "filterer@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    // matched only — sample has 3 matched issues.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries?status=matched"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"].as_i64(), Some(3));
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["match_status"].as_str(), Some("matched"));
        assert!(
            item["issue"].is_object(),
            "matched entries hydrate the issue"
        );
    }

    // ambiguous + missing — Resolution-tab use case.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries?status=ambiguous,missing"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Sample has 266 missing + 0 ambiguous.
    assert_eq!(body["total"].as_i64(), Some(266));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_rejects_invalid_status_and_cursor() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "rejected@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries?status=bogus"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"].as_str(), Some("validation"));

    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries?cursor=not-base64-or-malformed"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"].as_str(), Some("validation"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_endpoint_rejects_non_owner() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "owner@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let other = register(&app, "interloper@example.com").await;
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries"),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detail_endpoint_no_longer_embeds_entries() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "detail-watcher@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.get("entries").is_none(),
        "detail response must not embed entries (M1 of list-pagination-completeness): {body:#?}",
    );
    // stats carry the per-status counts the UI used to derive client-side.
    assert_eq!(body["stats"]["total"].as_i64(), Some(269));
    assert_eq!(body["stats"]["matched"].as_i64(), Some(3));
    assert_eq!(body["stats"]["missing"].as_i64(), Some(266));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_saved_view_auto_seeds_year_range_from_entries() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "year-seeder@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    // Caller omits both year bounds → server seeds from the cbl_entries
    // year column. sample.cbl spans 2002..=2026, regardless of which
    // of those issues actually matched library content.
    let body = serde_json::json!({
        "kind": "cbl",
        "name": "Invincible Universe",
        "cbl_list_id": list_id,
    });
    let (status, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "view: {view:#?}");
    assert_eq!(view["custom_year_start"].as_i64(), Some(2002));
    assert_eq!(view["custom_year_end"].as_i64(), Some(2026));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_saved_view_respects_explicit_year_overrides() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "year-override@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    // Caller supplies one explicit bound → auto-seed is skipped entirely
    // (we don't blend halves), the other side stays null.
    let body = serde_json::json!({
        "kind": "cbl",
        "name": "Manually scoped",
        "cbl_list_id": list_id,
        "custom_year_start": 2010,
    });
    let (status, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "view: {view:#?}");
    assert_eq!(view["custom_year_start"].as_i64(), Some(2010));
    assert!(
        view["custom_year_end"].is_null(),
        "explicit override skips auto-seed: {view:#?}",
    );
}
