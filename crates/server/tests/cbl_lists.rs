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
    upload_cbl_with_content_type(app, auth, file_name, file_bytes, "application/xml").await
}

async fn upload_cbl_with_content_type(
    app: &TestApp,
    auth: &Authed,
    file_name: &str,
    file_bytes: &[u8],
    content_type: &str,
) -> (StatusCode, serde_json::Value) {
    let boundary = "----folio-test-boundary";
    let mut body: Vec<u8> = Vec::with_capacity(file_bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
             Content-Type: {content_type}\r\n\r\n",
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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
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
        declared_age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
        series_json_present: Set(None),
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
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    // Insert three Invincible issues with CV IDs from the sample.
    // `Invincible #1` = CV 105347, `#2` = 105532, `#3` = 105533.
    // CV ids live on external_ids (entity_type='issue',
    // source='comicvine') post-M0; written below after the insert.
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
            state: Set(entity::issue::IssueState::Active),
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
            deck: Set(None),
            store_date: Set(None),
            foc_date: Set(None),
            price: Set(None),
            sku: Set(None),
            staff_rating: Set(None),
            aliases: Set(serde_json::json!([])),
            last_metadata_sync_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            hash_algorithm: Set(1),
            metroninfo_present: Set(None),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
            last_sidecar_rewrite_at: Set(None),
            metron_info_raw: Set(None),
            cover_page_index: Set(0),
            metadata_review_accepted_at: Set(None),
            metadata_review_accepted_by: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
        server::metadata::writers::set_external_id(
            &db,
            "issue",
            &issue_id,
            &server::metadata::Identifier::new(server::metadata::Source::ComicVine, cv.to_string()),
            server::metadata::writers::SetBy::ComicInfo,
        )
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
async fn upload_rejects_obvious_non_xml_content_type() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bad-type@example.com").await;

    let (status, body) = upload_cbl_with_content_type(
        &app,
        &auth,
        "sample.png",
        SAMPLE_CBL.as_bytes(),
        "image/png",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "body: {body:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_rejects_oversized_file_before_parse() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "too-large@example.com").await;
    let oversized = vec![b'<'; 4 * 1024 * 1024 + 1];

    let (status, body) = upload_cbl(&app, &auth, "oversized.cbl", &oversized).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "body: {body:#?}");
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

    // Resolve the seeded Invincible #1 by its CV id via external_ids.
    let cv_row = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("issue"))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .filter(entity::external_id::Column::ExternalId.eq("105347"))
        .one(&db)
        .await
        .unwrap()
        .expect("seeded external_ids row for Invincible #1");
    let target_issue = entity::issue::Entity::find_by_id(cv_row.entity_id)
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
async fn manual_match_increments_collection_count() {
    // Regression: pre-fix, `stats.matched` strictly counted rows with
    // `match_status='matched'`. Manual matches set `match_status='manual'`
    // and ARE separately counted as `stats.manual`, but the Collection
    // pill in the UI displays `matched / total` only — so a user who
    // resolved an ambiguous / missing entry by hand saw the pill stay
    // put even though the row was now resolved. Worse, `read_count`
    // already counted `matched_issue_id IS NOT NULL` (status-agnostic),
    // so read-progress could legitimately exceed Collection.
    //
    // After the fix, `stats.matched` counts every row with a resolved
    // issue (auto + manual). The Collection pill increments on manual
    // resolution. `stats.manual` stays available as a sub-count for
    // the Resolution tab to call out user overrides.
    let app = TestApp::spawn().await;
    let auth = register(&app, "delta@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

    // Snapshot stats before any manual match: 3 auto-matched Invincibles.
    let stats_before = &view["stats"];
    assert_eq!(stats_before["matched"].as_i64(), Some(3));
    assert_eq!(stats_before["manual"].as_i64(), Some(0));

    // Manually resolve "Tech Jacket #1" (currently missing) to a
    // seeded Invincible issue. Bumps the entry from `missing` to
    // `manual`.
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
    // Resolve the seeded Invincible #1 by its CV id via external_ids.
    let cv_row = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("issue"))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .filter(entity::external_id::Column::ExternalId.eq("105347"))
        .one(&db)
        .await
        .unwrap()
        .expect("seeded external_ids row for Invincible #1");
    let target_issue = entity::issue::Entity::find_by_id(cv_row.entity_id)
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

    // Re-fetch the list and assert the Collection number went up.
    let detail_url = format!("/api/me/cbl-lists/{list_id}");
    let (status, after) = http(&app, Method::GET, &detail_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let stats_after = &after["stats"];
    assert_eq!(
        stats_after["matched"].as_i64(),
        Some(4),
        "manual match must bump the Collection count: stats={stats_after:#?}",
    );
    assert_eq!(
        stats_after["manual"].as_i64(),
        Some(1),
        "the manual sub-count tracks user overrides",
    );
    // And consistency: manual is a SUBSET of matched (documented in
    // the CblStatsView doc), so matched >= manual must hold.
    assert!(
        stats_after["matched"].as_i64().unwrap() >= stats_after["manual"].as_i64().unwrap(),
        "manual entries must be included in `matched`",
    );

    // Clearing a manual match should remove it from both counts.
    let clear_url = format!(
        "/api/me/cbl-lists/{list_id}/entries/{entry_id}/clear-match",
        entry_id = missing_entry.id
    );
    let (status, _) = http(&app, Method::POST, &clear_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, restored) = http(&app, Method::GET, &detail_url, Some(&auth), None).await;
    let stats_restored = &restored["stats"];
    assert_eq!(
        stats_restored["matched"].as_i64(),
        Some(3),
        "clearing the manual match drops Collection back",
    );
    assert_eq!(stats_restored["manual"].as_i64(), Some(0));
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
async fn deleting_cbl_saved_view_also_removes_underlying_cbl_list() {
    // Regression for the "deleted CBL still surfaces in OPDS feeds + On
    // Deck" bug (2026-05-19): the /settings/views surface deletes the
    // kind='cbl' saved_view wrapper but historically left the
    // cbl_lists row in place. OPDS `/opds/v1/lists` and the On Deck
    // rail both query cbl_lists directly, so the deleted CBL stayed
    // visible. The user-scoped delete handler now cascades through
    // the underlying list when the caller owns it.
    let app = TestApp::spawn().await;
    let auth = register(&app, "grace@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap().to_owned();

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

    // Delete the saved-view wrapper — the path used by /settings/views.
    let url = format!("/api/me/saved-views/{saved_view_id}");
    let (status, body) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete failed: {body:?}");

    let db = Database::connect(&app.db_url).await.unwrap();
    let list_uuid = Uuid::parse_str(&list_id).unwrap();
    let view_uuid = Uuid::parse_str(&saved_view_id).unwrap();
    let list = cbl_list::Entity::find_by_id(list_uuid)
        .one(&db)
        .await
        .unwrap();
    assert!(
        list.is_none(),
        "deleting the saved view must also drop the underlying cbl_list — otherwise OPDS + On Deck keep surfacing the deleted CBL",
    );
    let saved = entity::saved_view::Entity::find_by_id(view_uuid)
        .one(&db)
        .await
        .unwrap();
    assert!(saved.is_none(), "saved_view should be gone");
    let entries = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_uuid))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(entries, 0, "cbl_entries should have cascaded with the list");
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

/// E9 (audit-2026-11): the "Search this list" box matches `%` / `_` / `\`
/// literally. Unescaped, `?q=%` and `?q=_` were `ILIKE '%%%'` /
/// `'%_%'` and returned every entry; none of the sample's entries carry
/// those characters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_search_matches_like_metacharacters_literally() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "like-esc@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();

    for q in ["%25", "_", "%5C"] {
        let (status, body) = http(
            &app,
            Method::GET,
            &format!("/api/me/cbl-lists/{list_id}/entries?q={q}"),
            Some(&auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "q={q}: {body}");
        assert_eq!(body["total"].as_i64(), Some(0), "q={q}: {body}");
    }
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

    // `?status=bogus` is a semantic enum violation (audit-remediation
    // M9.3 reclassification): the field is structurally correct, the
    // *value* isn't accepted. 422.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries?status=bogus"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"].as_str(), Some("validation"));

    // `?cursor=malformed` is parse-shape (base64-decode failure) — stays 400.
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

// ───── Regression guard: list-pagination-completeness 1.0 ─────
//
// The original bug (2026-05-14) silently truncated CBL lists at 500
// entries because `GET /me/cbl-lists/{id}` defaulted `limit.unwrap_or(500)`
// and the UI passed no override. The fix moved entries to a paginated
// `/me/cbl-lists/{id}/entries` endpoint; this test directly anchors
// the invariant by synthesizing a 600-entry list (well past the old
// 500 cap) and walking the cursor end-to-end. Failure mode this
// catches: a future refactor that re-introduces ANY hidden cap below
// the entry count silently drops rows past the cap.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_endpoint_walks_past_old_500_cap() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "regression-walker@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    // Look up the just-registered user — register() doesn't return the
    // id, and we need it as the cbl_list's `owner_user_id` so the
    // /me/cbl-lists endpoint's ownership check passes.
    let user = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("regression-walker@example.com"))
        .one(&db)
        .await
        .unwrap()
        .expect("user row");

    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(Some(user.id)),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList/>".into()),
        parsed_name: Set("600-entry stress list".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(600)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();

    // Insert 600 entries directly — no need to actually parse a
    // 600-issue CBL file; we just need the row count past the old
    // cap. Match-status varies so a future filter-aware regression
    // wouldn't accidentally pass by walking only matched rows.
    const N: i32 = 600;
    for pos in 0..N {
        let status = match pos % 3 {
            0 => "matched",
            1 => "ambiguous",
            _ => "missing",
        };
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(pos),
            series_name: Set(format!("Series {pos}")),
            issue_number: Set(format!("{pos}")),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(None),
            match_status: Set(status.into()),
            match_method: Set(None),
            match_confidence: Set(None),
            ambiguous_candidates: Set(None),
            matched_at: Set(None),
            user_resolved_at: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    // Walk the cursor end-to-end. Bug-class regression: any silent
    // truncation will surface as `seen < 600`.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["total"].as_i64(),
        Some(N as i64),
        "total on first page"
    );

    let mut positions: Vec<i64> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["position"].as_i64().unwrap())
        .collect();
    let mut cursor = body["next_cursor"].as_str().map(str::to_owned);
    let mut page_count = 1;
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
        for it in body["items"].as_array().unwrap() {
            positions.push(it["position"].as_i64().unwrap());
        }
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        page_count += 1;
        // Safety net: stop after 50 pages so a runaway loop fails fast.
        assert!(page_count < 50, "infinite loop guard tripped");
    }
    assert_eq!(positions.len(), N as usize, "every position is reachable");
    let dedup: std::collections::HashSet<_> = positions.iter().collect();
    assert_eq!(dedup.len(), N as usize, "no position returned twice");

    // Sanity: positions cover the full 0..N range with no gaps.
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(sorted.first().copied(), Some(0));
    assert_eq!(sorted.last().copied(), Some((N - 1) as i64));
}

/// Provider series-boundary divergence regression: a `series_provider_range`
/// mapping must NOT perturb CBL entry resolution. CBL resolution keys off
/// stable issue ids + provider ids + the parent series'
/// `normalized_name`/`year`/`volume` — none of which the range table touches
/// (it's a separate table the local series row + CBL matcher never read).
/// This anchors that invariant so a future change can't silently couple them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_provider_range_does_not_affect_cbl_resolution() {
    use entity::series_provider_range;
    use server::cbl::matcher::{EntryToMatch, MatchStatus, match_entries};

    let app = TestApp::spawn().await;
    let lib_id = seed_matchable_issues(&app).await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let series = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&db)
        .await
        .unwrap()
        .expect("seeded series present");

    // Fallback-only entry (no CV/Metron id) → resolves via
    // normalized_name + volume(year) + number.
    let entry = EntryToMatch {
        series_name: "Invincible".into(),
        number: "1".into(),
        volume: Some("2003".into()),
        cv_issue_id: None,
        metron_issue_id: None,
    };

    let before = match_entries(&db, std::slice::from_ref(&entry))
        .await
        .unwrap();
    assert_eq!(before[0].status, MatchStatus::Matched);
    let matched_before = before[0].issue_id.clone().expect("matched an issue");

    // Add a (deliberately divergent) range mapping for this series.
    let now = Utc::now().fixed_offset();
    series_provider_range::ActiveModel {
        id: Set(Uuid::new_v4()),
        series_id: Set(series.id),
        source: Set("metron".into()),
        provider_series_id: Set("62349".into()),
        provider_series_url: Set(None),
        provider_series_name: Set(Some("Invincible (alt)".into())),
        range_low: Set(Some("1".into())),
        range_high: Set(Some("3".into())),
        declared_year: Set(Some(2099)),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    let after = match_entries(&db, std::slice::from_ref(&entry))
        .await
        .unwrap();
    assert_eq!(after[0].status, MatchStatus::Matched);
    assert_eq!(
        after[0].issue_id.as_deref(),
        Some(matched_before.as_str()),
        "CBL resolution must be unaffected by a provider-range mapping",
    );
}

// ───────── WP-6.3 / security-audit M-5: per-user CBL import quota ─────────

const TINY_CBL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<ReadingList><Name>Quota probe</Name><Books>
<Book Series="Tech Jacket" Number="1" Volume="2002" Year="2002" />
</Books></ReadingList>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_quota_caps_non_admin_imports_per_hour() {
    let app = TestApp::spawn().await;
    // First registered user is the (exempt) admin.
    let admin = register(&app, "quota-admin@example.com").await;
    let user = register(&app, "quota-user@example.com").await;

    for i in 0..server::cbl::quota::IMPORTS_PER_HOUR {
        let (status, body) = upload_cbl(&app, &user, "probe.cbl", TINY_CBL.as_bytes()).await;
        assert_eq!(status, StatusCode::CREATED, "import {i}: {body:#?}");
    }
    // Over quota → 429 before any parse/fetch work (Retry-After is pinned
    // in `import_quota_sets_retry_after_header`).
    let (status, body) = upload_cbl(&app, &user, "probe.cbl", TINY_CBL.as_bytes()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body:#?}");
    assert_eq!(body["error"]["code"], "rate_limited");
    // Refresh / check count against the same window.
    let (status, _) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists/check-all",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, _) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists",
        Some(&user),
        Some(serde_json::json!({"kind": "url", "url": "https://example.invalid/x.cbl"})),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Admins are exempt.
    for i in 0..=server::cbl::quota::IMPORTS_PER_HOUR {
        let (status, body) = upload_cbl(&app, &admin, "probe.cbl", TINY_CBL.as_bytes()).await;
        assert_eq!(status, StatusCode::CREATED, "admin import {i}: {body:#?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_quota_sets_retry_after_header() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "ra-admin@example.com").await;
    let user = register(&app, "ra-user@example.com").await;
    let send = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/me/cbl-lists/check-all")
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    user.session, user.csrf
                ),
            )
            .header("X-CSRF-Token", &user.csrf)
            .body(Body::empty())
            .unwrap()
    };
    for _ in 0..server::cbl::quota::IMPORTS_PER_HOUR {
        let resp = app.router.clone().oneshot(send()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let resp = app.router.clone().oneshot(send()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: i64 = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .expect("Retry-After header");
    assert!(
        retry > 0 && retry <= server::cbl::quota::WINDOW_SECS,
        "{retry}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_list_cap_rejects_new_url_import_without_fetching() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "cap-admin@example.com").await;
    let user = register(&app, "cap-user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = chrono::Utc::now().fixed_offset();
    for i in 0..server::cbl::quota::MAX_REMOTE_LISTS_PER_USER {
        entity::cbl_list::ActiveModel {
            id: Set(Uuid::now_v7()),
            owner_user_id: Set(Some(user.user_id)),
            source_kind: Set("url".into()),
            source_url: Set(Some(format!("https://example.invalid/{i}.cbl"))),
            catalog_source_id: Set(None),
            catalog_path: Set(None),
            github_blob_sha: Set(None),
            source_etag: Set(None),
            source_last_modified: Set(None),
            raw_sha256: Set(vec![0u8; 32]),
            raw_xml: Set(TINY_CBL.into()),
            parsed_name: Set(format!("list {i}")),
            parsed_matchers_present: Set(false),
            num_issues_declared: Set(None),
            description: Set(None),
            imported_at: Set(now),
            last_refreshed_at: Set(None),
            last_match_run_at: Set(None),
            refresh_schedule: Set(Some("@hourly".into())),
            created_at: Set(now),
            updated_at: Set(now),
            preserve_canonical_order: Set(false),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    // `.invalid` never resolves, so a 422 (not a 400 invalid_url / 502) proves
    // the cap fires before any outbound fetch is attempted.
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists",
        Some(&user),
        Some(serde_json::json!({"kind": "url", "url": "https://example.invalid/new.cbl"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
    assert_eq!(body["error"]["code"], "cbl.remote_list_limit");
    // Uploads aren't server-fetched, so they don't count against the cap.
    let (status, body) = upload_cbl(&app, &user, "probe.cbl", TINY_CBL.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
}

/// N-O2 + E7: a matched issue that was soft-removed reads as **missing**
/// everywhere: no card on the entries page, `match_status = "missing"` in the
/// entry view, out of `?status=matched` and into `?status=missing` (so the
/// Resolution tab lists it), and counted as missing rather than matched in
/// the list stats. The row keeps its `matched_issue_id`, so the match comes
/// back with the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entries_hide_card_of_removed_matched_issue() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "removed-match@example.com").await;
    let _lib = seed_matchable_issues(&app).await;
    let (_, view) = upload_cbl(&app, &auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    let list_id = view["id"].as_str().unwrap();
    let matched_url = format!("/api/me/cbl-lists/{list_id}/entries?status=matched");
    let missing_url = format!("/api/me/cbl-lists/{list_id}/entries?status=ambiguous,missing");
    let detail_url = format!("/api/me/cbl-lists/{list_id}");

    let (status, body) = http(&app, Method::GET, &matched_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i["issue"].is_object()));
    let gone_entry_id = items[0]["id"].as_str().unwrap().to_owned();
    let gone_issue_id = items[0]["matched_issue_id"].as_str().unwrap().to_owned();
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&auth), None).await;
    let stats_before = detail["stats"].clone();
    assert_eq!(stats_before["matched"].as_i64(), Some(3));
    let (_, body) = http(&app, Method::GET, &missing_url, Some(&auth), None).await;
    let unresolved_before = body["total"].as_i64().unwrap();

    let db = Database::connect(&app.db_url).await.unwrap();
    let set_removed = |removed: bool| {
        let db = db.clone();
        let id = gone_issue_id.clone();
        async move {
            let row = entity::issue::Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap();
            let mut am: entity::issue::ActiveModel = row.into();
            am.removed_at = Set(removed.then(|| Utc::now().fixed_offset()));
            am.update(&db).await.unwrap();
        }
    };

    set_removed(true).await;
    // Out of the matched filter: two live matches remain, both with cards.
    let (status, body) = http(&app, Method::GET, &matched_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{body:#?}");
    assert_eq!(body["total"].as_i64(), Some(2));
    assert!(items.iter().all(|i| i["issue"].is_object()));
    assert!(
        items
            .iter()
            .all(|i| i["id"].as_str() != Some(gone_entry_id.as_str()))
    );

    // Into the Resolution tab's filter, reading as missing, match kept.
    let (status, body) = http(&app, Method::GET, &missing_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"].as_i64(), Some(unresolved_before + 1));
    let gone = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"].as_str() == Some(gone_entry_id.as_str()))
        .unwrap_or_else(|| panic!("removed match listed for resolution: {body:#?}"));
    assert!(
        gone["issue"].is_null(),
        "removed match has no card: {gone:#?}"
    );
    assert_eq!(gone["match_status"], "missing");
    assert_eq!(
        gone["matched_issue_id"].as_str(),
        Some(gone_issue_id.as_str()),
        "the match itself is kept"
    );

    // Stats agree with the filters.
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&auth), None).await;
    assert_eq!(detail["stats"]["matched"].as_i64(), Some(2), "{detail:#?}");
    assert_eq!(
        detail["stats"]["missing"].as_i64(),
        Some(stats_before["missing"].as_i64().unwrap() + 1)
    );
    assert_eq!(detail["stats"]["total"], stats_before["total"]);

    // A manual match cannot point at the removed issue either: it answers
    // like an unknown id instead of writing a row that reads as missing.
    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list_id}/entries/{gone_entry_id}/match"),
        Some(&auth),
        Some(serde_json::json!({ "issue_id": gone_issue_id })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
    assert_eq!(body["error"]["message"], "issue not found");

    // The issues feed behind Continue / offline downloads drops it too.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/issues"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "{body:#?}");
    assert!(!ids.contains(&gone_issue_id.as_str()));

    // File back, match back.
    set_removed(false).await;
    let (_, body) = http(&app, Method::GET, &matched_url, Some(&auth), None).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i["issue"].is_object()));
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&auth), None).await;
    assert_eq!(detail["stats"], stats_before);
}

/// Audit 2026-11 N-A4: `POST /me/cbl-lists` validates through
/// `Validated<T>` — a URL list without a `url` and a catalog list without
/// a `catalog_path` are 422s whose `error.details` names the field, and
/// they fail before the remote-list cap or the import quota is touched.
/// Malformed JSON stays a plain 400.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_from_json_validation_names_the_field() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl-garde@example.com").await;

    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists",
        Some(&auth),
        Some(serde_json::json!({ "kind": "url", "url": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
    assert_eq!(body["error"]["code"], "validation");
    assert_eq!(body["error"]["message"], "url: url required");
    assert_eq!(body["error"]["details"][0]["field"], "url");
    assert_eq!(body["error"]["details"][0]["message"], "url required");

    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists",
        Some(&auth),
        Some(serde_json::json!({
            "kind": "catalog",
            "catalog_source_id": Uuid::now_v7(),
            "catalog_path": "",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
    assert_eq!(body["error"]["details"][0]["field"], "catalog_path");
    assert_eq!(
        body["error"]["details"][0]["message"],
        "catalog_path required"
    );

    // Parse-shape failure stays 400 (no field to bind).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/cbl-lists")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation");
    assert!(body["error"]["details"].is_null(), "{body:#?}");
}

// ───── library ACL on list entries ─────

/// Grant `user_id` a library, optionally under an age-rating cap.
async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid, cap: Option<&str>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        hide_unrated: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

/// One issue (`number` of a 2020 series) titled `title`. Returns its id.
async fn acl_issue(
    db: &sea_orm::DatabaseConnection,
    dir: &std::path::Path,
    lib: Uuid,
    series: Uuid,
    number: f64,
    title: &str,
) -> String {
    let path = dir.join(format!("{title}.cbz"));
    common::seed::IssueSeed::new(lib, series, &path, title.as_bytes(), number)
        .with_title(title)
        .insert(db)
        .await
}

/// Fixture for the reading-list ACL tests: library A (granted to the
/// readers) and libraries B and C (hidden from them), a CBL naming
///
///   0. Aurora Patrol #1    → A, unrated, "Dawn Light"
///   1. Midnight Cartel #1  → B, "Secret Moon"
///   2. Twin Tale #1        → ambiguous: one candidate in A, one in B
///   3. Ghost Pair #1       → ambiguous: candidates in B and C only
///   4. Nowhere #1          → no such book
///   5. Aurora Patrol #2    → A, rated Mature 17+, "Red Night"
struct AclFixture {
    dir: tempfile::TempDir,
    lib_a: Uuid,
    lib_b: Uuid,
    visible_id: String,
    hidden_id: String,
    twin_a_id: String,
    mature_id: String,
    cbl: String,
}

async fn seed_acl_fixture(app: &TestApp) -> AclFixture {
    let db = Database::connect(&app.db_url).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lib_a_root = dir.path().join("lib_a");
    std::fs::create_dir_all(&lib_a_root).unwrap();
    let lib_a = common::seed::LibrarySeed::new(&lib_a_root)
        .insert(&db)
        .await;
    let lib_b_root = dir.path().join("lib_b");
    std::fs::create_dir_all(&lib_b_root).unwrap();
    let lib_b = common::seed::LibrarySeed::new(&lib_b_root)
        .insert(&db)
        .await;
    let lib_c_root = dir.path().join("lib_c");
    std::fs::create_dir_all(&lib_c_root).unwrap();
    let lib_c = common::seed::LibrarySeed::new(&lib_c_root)
        .insert(&db)
        .await;
    let aurora = common::seed::seed_series(&db, lib_a, "Aurora Patrol").await;
    let cartel = common::seed::seed_series(&db, lib_b, "Midnight Cartel").await;
    let twin_a = common::seed::seed_series(&db, lib_a, "Twin Tale").await;
    let twin_b = common::seed::seed_series(&db, lib_b, "Twin Tale").await;
    let ghost_b = common::seed::seed_series(&db, lib_b, "Ghost Pair").await;
    let ghost_c = common::seed::seed_series(&db, lib_c, "Ghost Pair").await;
    let visible_id = acl_issue(&db, dir.path(), lib_a, aurora, 1.0, "Dawn Light").await;
    let hidden_id = acl_issue(&db, dir.path(), lib_b, cartel, 1.0, "Secret Moon").await;
    let twin_a_id = acl_issue(&db, dir.path(), lib_a, twin_a, 1.0, "Twin A").await;
    acl_issue(&db, dir.path(), lib_b, twin_b, 1.0, "Twin B").await;
    acl_issue(&db, dir.path(), lib_b, ghost_b, 1.0, "Ghost B").await;
    acl_issue(&db, dir.path(), lib_c, ghost_c, 1.0, "Ghost C").await;
    let mature_id = acl_issue(&db, dir.path(), lib_a, aurora, 2.0, "Red Night").await;
    sea_orm::ConnectionTrait::execute_unprepared(
        &db,
        &format!("UPDATE issues SET age_rating = 'Mature 17+' WHERE id = '{mature_id}'"),
    )
    .await
    .unwrap();
    let book = |series: &str, number: u32| {
        format!(r#"<Book Series="{series}" Number="{number}" Volume="2020" Year="2020" />"#)
    };
    let cbl = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<ReadingList><Name>ACL list</Name><Books>{}{}{}{}{}{}</Books></ReadingList>"#,
        book("Aurora Patrol", 1),
        book("Midnight Cartel", 1),
        book("Twin Tale", 1),
        book("Ghost Pair", 1),
        book("Nowhere", 1),
        book("Aurora Patrol", 2),
    );
    AclFixture {
        dir,
        lib_a,
        lib_b,
        visible_id,
        hidden_id,
        twin_a_id,
        mature_id,
        cbl,
    }
}

async fn entries_of(app: &TestApp, auth: &Authed, list_id: &str, query: &str) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/entries{query}"),
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body
}

fn entry_at(body: &serde_json::Value, position: i64) -> &serde_json::Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["position"].as_i64() == Some(position))
        .unwrap_or_else(|| panic!("entry {position}: {body:#?}"))
}

/// The match facts of an entry — what must be identical between a hidden
/// match and a genuinely missing entry.
fn match_facts(e: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "match_status": e["match_status"],
        "matched_issue_id": e["matched_issue_id"],
        "match_method": e["match_method"],
        "match_confidence": e["match_confidence"],
        "ambiguous_candidates": e["ambiguous_candidates"],
        "matched_at": e["matched_at"],
        "issue": e["issue"],
    })
}

fn stat_counts(stats: &serde_json::Value) -> [i64; 5] {
    [
        stats["total"].as_i64().unwrap(),
        stats["matched"].as_i64().unwrap(),
        stats["ambiguous"].as_i64().unwrap(),
        stats["missing"].as_i64().unwrap(),
        stats["manual"].as_i64().unwrap(),
    ]
}

/// Total of `?status=` / `?q=` on the entries endpoint (first page).
async fn entries_total(app: &TestApp, auth: &Authed, list_id: &str, query: &str) -> i64 {
    entries_of(app, auth, list_id, query).await["total"]
        .as_i64()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_entries_respect_the_readers_library_access() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "acl-admin@example.com").await;
    let reader = register(&app, "acl-reader@example.com").await;
    let capped = register(&app, "acl-capped@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    grant_library(&app, reader.user_id, fx.lib_a, None).await;
    grant_library(&app, capped.user_id, fx.lib_a, Some("Teen")).await;

    // ── Admin: the full view.
    let (status, view) = upload_cbl(&app, &admin, "acl.cbl", fx.cbl.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view:#?}");
    assert_eq!(stat_counts(&view["stats"]), [6, 3, 2, 1, 0]);
    let admin_list = view["id"].as_str().unwrap().to_owned();
    let body = entries_of(&app, &admin, &admin_list, "").await;
    assert_eq!(entry_at(&body, 1)["match_status"], "matched");
    assert_eq!(
        entry_at(&body, 1)["matched_issue_id"],
        fx.hidden_id.as_str()
    );
    assert_eq!(
        entry_at(&body, 3)["ambiguous_candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        entries_total(&app, &admin, &admin_list, "?q=Secret").await,
        1
    );

    // ── Reader granted library A only.
    let (status, view) = upload_cbl(&app, &reader, "acl.cbl", fx.cbl.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view:#?}");
    assert_eq!(
        stat_counts(&view["stats"]),
        [6, 2, 0, 4, 0],
        "import summary counts only what the reader can open: {view:#?}"
    );
    let list_id = view["id"].as_str().unwrap().to_owned();
    let (_, detail) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(detail["stats"], view["stats"], "{detail:#?}");

    let body = entries_of(&app, &reader, &list_id, "").await;
    assert_eq!(body["total"].as_i64(), Some(6));
    let missing = match_facts(entry_at(&body, 4));
    assert_eq!(missing["match_status"], "missing");
    // Hidden match and all-hidden ambiguous read exactly as missing.
    assert_eq!(match_facts(entry_at(&body, 1)), missing, "{body:#?}");
    assert_eq!(match_facts(entry_at(&body, 3)), missing, "{body:#?}");
    // Visible match keeps its card.
    assert_eq!(
        entry_at(&body, 0)["matched_issue_id"],
        fx.visible_id.as_str()
    );
    assert!(entry_at(&body, 0)["issue"].is_object());
    assert_eq!(
        entry_at(&body, 5)["matched_issue_id"],
        fx.mature_id.as_str()
    );
    // Mixed ambiguous with one visible candidate of two reads as missing
    // too: a lone visible candidate would reveal the hidden one.
    assert_eq!(match_facts(entry_at(&body, 2)), missing, "{body:#?}");

    // Status filters and search are the reader's view.
    assert_eq!(
        entries_total(&app, &reader, &list_id, "?status=matched").await,
        2
    );
    assert_eq!(
        entries_total(&app, &reader, &list_id, "?status=ambiguous").await,
        0
    );
    assert_eq!(
        entries_total(&app, &reader, &list_id, "?status=missing").await,
        4
    );
    let matched = entries_of(&app, &reader, &list_id, "?status=matched").await;
    assert!(
        matched["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["matched_issue_id"] != fx.hidden_id.as_str())
    );
    let found = entries_of(&app, &reader, &list_id, "?q=Secret").await;
    assert_eq!(found["total"].as_i64(), Some(0), "{found:#?}");
    assert!(found["items"].as_array().unwrap().is_empty());
    assert_eq!(entries_total(&app, &reader, &list_id, "?q=Dawn").await, 1);

    // Window + issues feed count only visible matches.
    let (_, window) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/window"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(window["total_matched"].as_i64(), Some(2), "{window:#?}");
    assert_eq!(window["total_entries"].as_i64(), Some(6));
    let (_, page) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/window-paginated"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(page["total_matched"].as_i64(), Some(2), "{page:#?}");
    let (_, issues) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/issues"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(issues["total"].as_i64(), Some(2), "{issues:#?}");

    // A refresh summary is the reader's view too: a book that newly
    // matches in a hidden library is neither matched nor rematched.
    let db = Database::connect(&app.db_url).await.unwrap();
    let nowhere = common::seed::seed_series(&db, fx.lib_b, "Nowhere").await;
    acl_issue(&db, fx.dir.path(), fx.lib_b, nowhere, 1.0, "Nowhere B").await;
    let (status, summary) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list_id}/refresh"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary:#?}");
    assert_eq!(summary["matched"].as_i64(), Some(2), "{summary:#?}");
    assert_eq!(summary["ambiguous"].as_i64(), Some(0));
    assert_eq!(summary["missing"].as_i64(), Some(4));
    assert_eq!(summary["rematched"].as_i64(), Some(0));

    // ── Age-capped reader: the Mature issue in library A is hidden too.
    let (status, view) = upload_cbl(&app, &capped, "acl.cbl", fx.cbl.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view:#?}");
    assert_eq!(stat_counts(&view["stats"]), [6, 1, 0, 5, 0], "{view:#?}");
    let capped_list = view["id"].as_str().unwrap().to_owned();
    let body = entries_of(&app, &capped, &capped_list, "").await;
    assert_eq!(
        match_facts(entry_at(&body, 5)),
        match_facts(entry_at(&body, 4)),
        "{body:#?}"
    );
    assert_eq!(
        entries_total(&app, &capped, &capped_list, "?q=Red").await,
        0
    );
    assert_eq!(
        entries_total(&app, &capped, &capped_list, "?status=matched").await,
        1
    );
    let (_, window) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{capped_list}/window"),
        Some(&capped),
        None,
    )
    .await;
    assert_eq!(window["total_matched"].as_i64(), Some(1), "{window:#?}");

    // ── Granting library B reveals the stored matches, no re-import.
    grant_library(&app, reader.user_id, fx.lib_b, None).await;
    let body = entries_of(&app, &reader, &list_id, "").await;
    assert_eq!(entry_at(&body, 1)["match_status"], "matched");
    assert_eq!(
        entry_at(&body, 1)["matched_issue_id"],
        fx.hidden_id.as_str()
    );
    assert!(entry_at(&body, 1)["issue"].is_object());
    // Twin Tale now has both candidates visible; Ghost Pair still has one
    // (its library C candidate stays hidden), so it still reads missing.
    assert_eq!(entry_at(&body, 2)["match_status"], "ambiguous");
    assert_eq!(entry_at(&body, 3)["match_status"], "missing");
    assert_eq!(entries_total(&app, &reader, &list_id, "?q=Secret").await, 1);
    let (_, detail) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&reader),
        None,
    )
    .await;
    // Entry 4 now matches "Nowhere" in B (refreshed above), visible too.
    assert_eq!(
        stat_counts(&detail["stats"]),
        [6, 4, 1, 1, 0],
        "{detail:#?}"
    );
}

/// Grant `user_id` every library that exists, uncapped: a non-admin who
/// sees the whole library.
async fn grant_every_library(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    for lib in library::Entity::find().all(&db).await.unwrap() {
        grant_library(app, user_id, lib.id, None).await;
    }
}

async fn refresh_log_of(app: &TestApp, auth: &Authed, list_id: &str) -> Vec<serde_json::Value> {
    let (status, body) = http(
        app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}/refresh-log"),
        Some(auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    body["items"].as_array().unwrap().clone()
}

/// The refresh log stores global match facts. A caller who cannot see the
/// whole library gets `rematched_count` as 0 and no row without a
/// structural change (every post-scan row); a non-admin with every library
/// granted and no cap, and an admin, get the stored rows unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_log_follows_the_callers_view() {
    use server::cbl::import::{RefreshTrigger, rematch_existing};
    let app = TestApp::spawn().await;
    let admin = register(&app, "log-admin@example.com").await;
    let reader = register(&app, "log-reader@example.com").await;
    let full = register(&app, "log-full@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    grant_library(&app, reader.user_id, fx.lib_a, None).await;
    grant_every_library(&app, full.user_id).await;

    // One book in hidden library B, one not in the library yet.
    let cbl = r#"<?xml version="1.0" encoding="utf-8"?>
<ReadingList><Name>Log list</Name><Books><Book Series="Midnight Cartel" Number="1" Volume="2020" Year="2020" /><Book Series="Late Arrival" Number="1" Volume="2020" Year="2020" /></Books></ReadingList>"#;
    let mut lists = Vec::new();
    for auth in [&admin, &reader, &full] {
        let (status, view) = upload_cbl(&app, auth, "log.cbl", cbl.as_bytes()).await;
        assert_eq!(status, StatusCode::CREATED, "{view:#?}");
        lists.push(view["id"].as_str().unwrap().to_owned());
    }
    let (_, detail) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{}", lists[1]),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(detail["stats"]["matched"].as_i64(), Some(0), "{detail:#?}");

    // The real post-scan rematch path: a new issue in hidden library B.
    let db = Database::connect(&app.db_url).await.unwrap();
    let late = common::seed::seed_series(&db, fx.lib_b, "Late Arrival").await;
    acl_issue(&db, fx.dir.path(), fx.lib_b, late, 1.0, "Late Arrival One").await;
    for list in &lists {
        let n = rematch_existing(
            &db,
            Uuid::parse_str(list).unwrap(),
            RefreshTrigger::PostScan,
        )
        .await
        .unwrap();
        assert_eq!(n, 1);
    }

    // Restricted reader: only the import row, its rematch count zeroed.
    let rows = refresh_log_of(&app, &reader, &lists[1]).await;
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["added_count"].as_i64(), Some(2));
    assert_eq!(rows[0]["rematched_count"].as_i64(), Some(0), "{rows:#?}");
    assert!(rows.iter().all(|r| r["trigger"] != "post_scan"));

    // Every library granted, no cap / admin: the stored rows.
    for (auth, list) in [(&full, &lists[2]), (&admin, &lists[0])] {
        let rows = refresh_log_of(&app, auth, list).await;
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(rows[0]["trigger"], "post_scan");
        assert_eq!(rows[0]["rematched_count"].as_i64(), Some(1));
        assert_eq!(rows[1]["added_count"].as_i64(), Some(2));
        assert_eq!(rows[1]["rematched_count"].as_i64(), Some(1), "{rows:#?}");
    }

    // An age cap on any grant is no longer the whole library.
    sea_orm::ConnectionTrait::execute_unprepared(
        &db,
        &format!(
            "UPDATE library_user_access SET age_rating_max = 'Teen' \
             WHERE user_id = '{}' AND library_id = '{}'",
            full.user_id, fx.lib_a
        ),
    )
    .await
    .unwrap();
    let rows = refresh_log_of(&app, &full, &lists[2]).await;
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["rematched_count"].as_i64(), Some(0));
}

/// `entry_id` of the entry at `position` (entries endpoint).
async fn entry_id_at(app: &TestApp, auth: &Authed, list_id: &str, position: i64) -> String {
    let body = entries_of(app, auth, list_id, "").await;
    entry_at(&body, position)["id"].as_str().unwrap().to_owned()
}

fn bulk_item<'a>(body: &'a serde_json::Value, list_id: &str) -> &'a serde_json::Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"].as_str() == Some(list_id))
        .unwrap_or_else(|| panic!("item {list_id}: {body:#?}"))
}

/// Check / check-all / refresh-all report `rematched` and `changed` in the
/// caller's view: an entry that would resolve to an issue the caller
/// cannot open is no change to them. Admins keep the real counts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_and_refresh_all_counts_follow_the_callers_view() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "check-admin@example.com").await;
    let reader = register(&app, "check-reader@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    grant_library(&app, reader.user_id, fx.lib_a, None).await;

    let mut lists = Vec::new();
    for auth in [&admin, &reader] {
        let (status, view) = upload_cbl(&app, auth, "acl.cbl", fx.cbl.as_bytes()).await;
        assert_eq!(status, StatusCode::CREATED, "{view:#?}");
        let list_id = view["id"].as_str().unwrap().to_owned();
        // Clear the match on entry 1 (the issue in library B): the reader
        // sees that entry as missing already.
        let entry_id = entry_id_at(&app, auth, &list_id, 1).await;
        let (status, body) = http(
            &app,
            Method::POST,
            &format!("/api/me/cbl-lists/{list_id}/entries/{entry_id}/clear-match"),
            Some(auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body:#?}");
        lists.push(list_id);
    }
    let (admin_list, reader_list) = (&lists[0], &lists[1]);

    // Check one.
    let (status, item) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{reader_list}/check"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{item:#?}");
    assert_eq!(item["rematched"].as_i64(), Some(0), "{item:#?}");
    assert_eq!(item["changed"], false, "{item:#?}");
    let (_, item) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{admin_list}/check"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(item["rematched"].as_i64(), Some(1), "{item:#?}");
    assert_eq!(item["changed"], true, "{item:#?}");

    // Check all.
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists/check-all",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    let item = bulk_item(&body, reader_list);
    assert_eq!(item["rematched"].as_i64(), Some(0), "{body:#?}");
    assert_eq!(item["changed"], false);
    assert_eq!(body["changed_count"].as_i64(), Some(0));
    let (_, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists/check-all",
        Some(&admin),
        None,
    )
    .await;
    let item = bulk_item(&body, admin_list);
    assert_eq!(item["rematched"].as_i64(), Some(1), "{body:#?}");
    assert_eq!(item["changed"], true);

    // Refresh all, after a book also appears in hidden library B.
    let db = Database::connect(&app.db_url).await.unwrap();
    let nowhere = common::seed::seed_series(&db, fx.lib_b, "Nowhere").await;
    acl_issue(&db, fx.dir.path(), fx.lib_b, nowhere, 1.0, "Nowhere B").await;
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists/refresh-all",
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    let item = bulk_item(&body, reader_list);
    assert_eq!(item["rematched"].as_i64(), Some(0), "{body:#?}");
    assert_eq!(item["changed"], false);
    let (_, body) = http(
        &app,
        Method::POST,
        "/api/me/cbl-lists/refresh-all",
        Some(&admin),
        None,
    )
    .await;
    let item = bulk_item(&body, admin_list);
    assert_eq!(item["rematched"].as_i64(), Some(2), "{body:#?}");
    assert_eq!(item["changed"], true);
}

/// An ambiguous entry reads as ambiguous to a non-admin only with two or
/// more candidates they can open; with fewer it is a missing entry
/// everywhere. A manual match into a hidden library is missing too, and
/// malformed candidate JSON never fails the request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_entries_need_two_visible_candidates() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "amb-admin@example.com").await;
    let reader = register(&app, "amb-reader@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    grant_library(&app, reader.user_id, fx.lib_a, None).await;
    let (status, view) = upload_cbl(&app, &reader, "acl.cbl", fx.cbl.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view:#?}");
    let list_id = view["id"].as_str().unwrap().to_owned();

    let db = Database::connect(&app.db_url).await.unwrap();
    let set = |position: i32, status: &'static str, cands: Option<serde_json::Value>| {
        let db = db.clone();
        let list = Uuid::parse_str(&list_id).unwrap();
        async move {
            let row = cbl_entry::Entity::find()
                .filter(cbl_entry::Column::CblListId.eq(list))
                .filter(cbl_entry::Column::Position.eq(position))
                .one(&db)
                .await
                .unwrap()
                .unwrap();
            let mut am: cbl_entry::ActiveModel = row.into();
            am.match_status = Set(status.into());
            am.ambiguous_candidates = Set(cands);
            if status == "manual" {
                am.match_method = Set(Some("manual".into()));
            } else {
                am.matched_issue_id = Set(None);
            }
            am.update(&db).await.unwrap();
        }
    };
    let cand =
        |id: &str| serde_json::json!({ "issue_id": id, "series_name": "S", "similarity": 0.9 });
    // 1: manual match to the issue in hidden library B.
    set(1, "manual", None).await;
    // 2: Twin Tale stays ambiguous [A, B] — one visible of two.
    // 3: two visible of three.
    set(
        3,
        "ambiguous",
        Some(serde_json::json!([
            cand(&fx.twin_a_id),
            cand(&fx.hidden_id),
            cand(&fx.visible_id),
        ])),
    )
    .await;
    // 4: not an array.
    set(
        4,
        "ambiguous",
        Some(serde_json::json!({ "not": "an array" })),
    )
    .await;
    // 5: malformed elements and one visible id named twice.
    set(
        5,
        "ambiguous",
        Some(serde_json::json!([
            1,
            "x",
            { "issue_id": 5 },
            cand(&fx.visible_id),
            cand(&fx.visible_id),
        ])),
    )
    .await;

    let missing = serde_json::json!({
        "match_status": "missing",
        "matched_issue_id": null,
        "match_method": null,
        "match_confidence": null,
        "ambiguous_candidates": null,
        "matched_at": null,
        "issue": null,
    });
    let body = entries_of(&app, &reader, &list_id, "").await;
    for p in [1, 2, 4, 5] {
        assert_eq!(
            match_facts(entry_at(&body, p)),
            missing,
            "entry {p}: {body:#?}"
        );
    }
    let amb = entry_at(&body, 3);
    assert_eq!(amb["match_status"], "ambiguous");
    let mut ids: Vec<&str> = amb["ambiguous_candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["issue_id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    let mut want = vec![fx.twin_a_id.as_str(), fx.visible_id.as_str()];
    want.sort_unstable();
    assert_eq!(ids, want, "{amb:#?}");

    for (status, total) in [
        ("ambiguous", 1),
        ("missing", 4),
        ("manual", 0),
        ("matched", 1),
    ] {
        assert_eq!(
            entries_total(&app, &reader, &list_id, &format!("?status={status}")).await,
            total,
            "?status={status}"
        );
    }
    let (_, detail) = http(
        &app,
        Method::GET,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(
        stat_counts(&detail["stats"]),
        [6, 1, 1, 4, 0],
        "{detail:#?}"
    );
}

/// The reader's "Issue N of M" pair and the series appearances count use
/// one scale: matched entries the reader can open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_up_and_appearances_count_what_the_reader_can_open() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "nu-admin@example.com").await;
    let reader = register(&app, "nu-reader@example.com").await;
    let capped = register(&app, "nu-capped@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    grant_library(&app, reader.user_id, fx.lib_a, None).await;
    grant_library(&app, capped.user_id, fx.lib_a, Some("Teen")).await;

    let mut views = Vec::new();
    for auth in [&reader, &capped] {
        let (status, view) = upload_cbl(&app, auth, "acl.cbl", fx.cbl.as_bytes()).await;
        assert_eq!(status, StatusCode::CREATED, "{view:#?}");
        let (status, sv) = http(
            &app,
            Method::POST,
            "/api/me/saved-views",
            Some(auth),
            Some(serde_json::json!({
                "kind": "cbl",
                "name": "ACL view",
                "cbl_list_id": view["id"],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{sv:#?}");
        views.push(sv["id"].as_str().unwrap().to_owned());
    }

    // Entries 0 (Dawn Light) and 5 (Red Night) are the reader's matches;
    // the raw entry position of Red Night is 6.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/issues/{}/next-up?cbl={}", fx.visible_id, views[0]),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["source"], "cbl", "{body:#?}");
    assert_eq!(body["target"]["id"], fx.mature_id.as_str());
    assert_eq!(body["cbl_position"], 2, "{body:#?}");
    assert_eq!(body["cbl_total"], 2, "{body:#?}");
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/issues/{}/prev-up?cbl={}", fx.mature_id, views[0]),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["source"], "cbl", "{body:#?}");
    assert_eq!(body["target"]["id"], fx.visible_id.as_str());
    assert_eq!(body["cbl_position"], 1, "{body:#?}");
    assert_eq!(body["cbl_total"], 2, "{body:#?}");

    // Series appearances: Aurora Patrol holds both of the reader's
    // matches; the capped reader can open only the unrated one.
    let db = Database::connect(&app.db_url).await.unwrap();
    let aurora = entity::series::Entity::find()
        .filter(entity::series::Column::Name.eq("Aurora Patrol"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    for (auth, count) in [(&reader, 2), (&capped, 1)] {
        let (status, body) = http(
            &app,
            Method::GET,
            &format!("/api/series/{}/appearances", aurora.slug),
            Some(auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body:#?}");
        let lists = body["reading_lists"].as_array().unwrap();
        assert_eq!(lists.len(), 1, "{body:#?}");
        assert_eq!(lists[0]["issue_count"].as_i64(), Some(count), "{body:#?}");
    }
}

/// The window endpoints drop a soft-removed match for admins too, like
/// `/issues` and next-up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn window_total_matched_excludes_a_removed_match_for_admins() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "win-admin@example.com").await;
    let fx = seed_acl_fixture(&app).await;
    let (status, view) = upload_cbl(&app, &admin, "acl.cbl", fx.cbl.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view:#?}");
    let list_id = view["id"].as_str().unwrap().to_owned();
    let totals = || async {
        let mut out = Vec::new();
        for path in ["window", "window-paginated"] {
            let (status, body) = http(
                &app,
                Method::GET,
                &format!("/api/me/cbl-lists/{list_id}/{path}"),
                Some(&admin),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body:#?}");
            out.push(body["total_matched"].as_i64().unwrap());
        }
        out
    };
    assert_eq!(totals().await, [3, 3]);
    let db = Database::connect(&app.db_url).await.unwrap();
    sea_orm::ConnectionTrait::execute_unprepared(
        &db,
        &format!(
            "UPDATE issues SET removed_at = now() WHERE id = '{}'",
            fx.hidden_id
        ),
    )
    .await
    .unwrap();
    assert_eq!(totals().await, [2, 2]);
}

// ───── refresh: entry cap, atomic writes, request budgets ─────
//
// URL sources go through `cbl::fetch::stub_source_for_tests` (debug builds
// only): a test cannot serve https from a public address, which the vetted
// fetch requires.

use server::cbl::fetch::{TestSource, stub_source_for_tests};

fn over_cap_xml() -> String {
    let over = server::cbl::MAX_BOOKS_PER_LIST + 1;
    let mut xml = String::from("<?xml version=\"1.0\"?>\n<ReadingList><Name>Grown</Name><Books>\n");
    for i in 0..over {
        xml.push_str(&format!(
            "<Book Series=\"Grown\" Number=\"{i}\" Volume=\"2020\" Year=\"2020\" />\n"
        ));
    }
    xml.push_str("</Books></ReadingList>\n");
    xml
}

/// Upload the sample list and return its id.
async fn upload_sample(app: &TestApp, auth: &Authed) -> Uuid {
    let (status, view) = upload_cbl(app, auth, "sample.cbl", SAMPLE_CBL.as_bytes()).await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    Uuid::parse_str(view["id"].as_str().unwrap()).unwrap()
}

/// Turn a list into a URL list at `url`, optionally replacing its stored
/// XML (and digest).
async fn make_url_list(db: &sea_orm::DatabaseConnection, id: Uuid, url: &str, raw: Option<&str>) {
    let row = cbl_list::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: cbl_list::ActiveModel = row.into();
    am.source_kind = Set("url".into());
    am.source_url = Set(Some(url.to_owned()));
    if let Some(raw) = raw {
        am.raw_xml = Set(raw.to_owned());
        am.raw_sha256 = Set(server::cbl::import::sha256_of(raw.as_bytes()));
    }
    am.update(db).await.unwrap();
}

/// (sorted entries, list row, refresh-log row count) — everything a failed
/// refresh must leave alone.
async fn stored_state(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> (Vec<(i32, String, String)>, cbl_list::Model, u64) {
    let mut entries: Vec<(i32, String, String)> = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.position, e.series_name, e.issue_number))
        .collect();
    entries.sort();
    let row = cbl_list::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let logs = cbl_refresh_log::Entity::find()
        .filter(cbl_refresh_log::Column::CblListId.eq(id))
        .count(db)
        .await
        .unwrap();
    (entries, row, logs)
}

/// A source whose content changed and is over the import cap is refused on
/// refresh and check with the import's error; the stored list is left as
/// it was (no truncation, no refresh-log row).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_and_check_refuse_new_content_over_the_entry_cap() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cap-grower@example.com").await;
    let list_id = upload_sample(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let url = format!("https://lists.example.test/{list_id}/grown.cbl");
    make_url_list(&db, list_id, &url, None).await;
    stub_source_for_tests(&url, TestSource::Body(over_cap_xml().into_bytes()));
    let before = stored_state(&db, list_id).await;
    assert_eq!(before.0.len(), 269);

    let expected = format!("exceeds {} entries", server::cbl::MAX_BOOKS_PER_LIST);
    for path in ["refresh", "check"] {
        let (status, body) = http(
            &app,
            Method::POST,
            &format!("/api/me/cbl-lists/{list_id}/{path}"),
            Some(&auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{path}: {body}");
        assert_eq!(body["error"]["code"], "too_many_entries", "{path}");
        assert_eq!(body["error"]["message"], expected.as_str(), "{path}");
    }
    for path in ["check-all", "refresh-all"] {
        let (status, body) = http(
            &app,
            Method::POST,
            &format!("/api/me/cbl-lists/{path}"),
            Some(&auth),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_eq!(body["failed_count"], 1, "{path}: {body}");
        assert_eq!(body["items"][0]["error"], expected.as_str());
    }
    assert_eq!(
        stored_state(&db, list_id).await,
        before,
        "stored list untouched"
    );
}

/// Stored content that has not changed is not re-checked against the cap:
/// a list already over it keeps working on refresh and check, both for an
/// upload and for a URL source that serves the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_content_over_the_cap_keeps_refreshing() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cap-unchanged@example.com").await;
    let list_id = upload_sample(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let big = over_cap_xml();
    let row = cbl_list::Entity::find_by_id(list_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: cbl_list::ActiveModel = row.into();
    am.raw_xml = Set(big.clone());
    am.raw_sha256 = Set(server::cbl::import::sha256_of(big.as_bytes()));
    am.update(&db).await.unwrap();

    let run = |path: &'static str| {
        let app = &app;
        let auth = &auth;
        async move {
            http(
                app,
                Method::POST,
                &format!("/api/me/cbl-lists/{list_id}/{path}"),
                Some(auth),
                None,
            )
            .await
        }
    };
    for path in ["check", "refresh"] {
        let (status, body) = run(path).await;
        assert_eq!(status, StatusCode::OK, "upload {path}: {body}");
    }
    let url = format!("https://lists.example.test/{list_id}/same.cbl");
    make_url_list(&db, list_id, &url, None).await;
    stub_source_for_tests(&url, TestSource::Body(big.into_bytes()));
    for path in ["check", "refresh"] {
        let (status, body) = run(path).await;
        assert_eq!(status, StatusCode::OK, "url {path}: {body}");
    }
}

/// A failure after the entries were deleted rolls the whole refresh back:
/// entries, list row and refresh log are exactly as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_that_fails_after_the_delete_leaves_the_list_unchanged() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "atomic@example.com").await;
    let list_id = upload_sample(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let before = stored_state(&db, list_id).await;

    server::cbl::import::fail_after_delete_for_tests(list_id);
    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list_id}/refresh"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(stored_state(&db, list_id).await, before);
}

/// A host that never answers ends in the handler's own timeout error on
/// the request budget, well before the 60 s route timeout, and the list is
/// untouched. (Takes the 40 s request budget to run.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_host_hits_the_request_budget_before_the_route_timeout() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "stall@example.com").await;
    let list_id = upload_sample(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let url = format!("https://lists.example.test/{list_id}/stall.cbl");
    make_url_list(&db, list_id, &url, None).await;
    stub_source_for_tests(&url, TestSource::Stall);
    let before = stored_state(&db, list_id).await;

    let started = std::time::Instant::now();
    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list_id}/refresh"),
        Some(&auth),
        None,
    )
    .await;
    let took = started.elapsed();
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["code"], "refresh_failed");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out"),
        "{body}"
    );
    assert!(
        took >= server::cbl::fetch::REQUEST_BUDGET.fetch_deadline
            && took < server::app::JSON_ROUTE_TIMEOUT,
        "{took:?}"
    );
    assert_eq!(stored_state(&db, list_id).await, before);
}
