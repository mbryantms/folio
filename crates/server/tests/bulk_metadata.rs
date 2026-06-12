//! Integration coverage for `PATCH /me/issues/bulk-metadata`
//! (`manga-and-bulk-metadata-1.0` M4).
//!
//! Asserts:
//!   - 9-field patch lands across N issues in one call.
//!   - `mode=skip_if_set` leaves already-set values alone.
//!   - `mode=replace` overwrites unconditionally.
//!   - Validation rejects empty patches and unknown manga values.
//!   - Credit fields (writer, penciller, …) are NOT accepted —
//!     forbidden by design.
//!   - `user_edited` accumulates the touched field names so the
//!     scanner skips them on rescan.

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
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
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
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery-staple"}"#,
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
    let extract = |prefix: &str| {
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

async fn seed_three_issues(app: &TestApp) -> Vec<String> {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Bulk Lib".into()),
        root_path: Set(format!("/tmp/bulk-{lib_id}")),
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
    }
    .insert(&db)
    .await
    .unwrap();

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Bulk Series".into()),
        normalized_name: Set(normalize_name("Bulk Series")),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
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
        slug: Set("bulk-series".into()),
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

    let mut ids = Vec::with_capacity(3);
    for i in 0..3 {
        let issue_id = format!("{i:0>64}");
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("issue-{i}")),
            file_path: Set(format!("/tmp/bulk/{i}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some(f64::from(i))),
            number_raw: Set(Some(i.to_string())),
            volume: Set(None),
            year: Set(None),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            // Middle issue starts with `manga = "No"` so we can prove
            // skip_if_set leaves it alone and replace overwrites it.
            manga: Set(if i == 1 { Some("No".into()) } else { None }),
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
            hash_algorithm: Set(0),
            metroninfo_present: Set(None),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
            cover_page_index: Set(0),
        }
        .insert(&db)
        .await
        .unwrap();
        ids.push(issue_id);
    }
    ids
}

async fn patch_bulk(
    app: &TestApp,
    auth: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/me/issues/bulk-metadata")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", auth.csrf.clone())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_applies_language_to_all_issues() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "language_code": "ja" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], 3);
    assert_eq!(body["skipped"], 0);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = IssueEntity::find()
        .filter(entity::issue::Column::Id.is_in(ids.clone()))
        .all(&db)
        .await
        .unwrap();
    for r in &rows {
        assert_eq!(r.language_code.as_deref(), Some("ja"));
        // user_edited tracks the bulk-touched field.
        let edited: Vec<String> = serde_json::from_value(r.user_edited.clone()).unwrap();
        assert!(
            edited.contains(&"language_code".to_owned()),
            "user_edited should include language_code: {edited:?}",
        );
    }
}

/// `mode = skip_if_set` (default) leaves issues with the field
/// already set untouched. In our seed the middle issue has
/// `manga = "No"`; only the two NULL ones should flip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_skip_if_set_leaves_existing_values_alone() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "manga": "YesAndRightToLeft" },
            "mode": "skip_if_set",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], 2);
    assert_eq!(body["skipped"], 1);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = IssueEntity::find()
        .filter(entity::issue::Column::Id.is_in(ids.clone()))
        .all(&db)
        .await
        .unwrap();
    let by_id: std::collections::HashMap<_, _> = rows
        .iter()
        .map(|r| (r.id.clone(), r.manga.clone()))
        .collect();
    // First + third NULL → flipped to YesAndRightToLeft.
    assert_eq!(by_id[&ids[0]].as_deref(), Some("YesAndRightToLeft"));
    assert_eq!(by_id[&ids[2]].as_deref(), Some("YesAndRightToLeft"));
    // Middle "No" preserved.
    assert_eq!(by_id[&ids[1]].as_deref(), Some("No"));
}

/// `mode = replace` overwrites every selected row regardless of
/// existing value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_replace_overwrites_set_values() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "manga": "YesAndRightToLeft" },
            "mode": "replace",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], 3);
    assert_eq!(body["skipped"], 0);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = IssueEntity::find()
        .filter(entity::issue::Column::Id.is_in(ids.clone()))
        .all(&db)
        .await
        .unwrap();
    for r in &rows {
        assert_eq!(r.manga.as_deref(), Some("YesAndRightToLeft"));
    }
}

/// Empty patch (no fields set) is rejected — clients shouldn't
/// round-trip a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_empty_patch_is_rejected() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({ "issue_ids": ids, "patch": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.empty_patch");
}

/// Unknown `manga` value is rejected up-front; no rows touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_unknown_manga_is_rejected() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "manga": "Definitely Yes" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.manga");
}

/// Credit fields are not part of the accepted patch shape — the
/// dialog won't surface them and the server-side struct doesn't
/// deserialize them. Sending one is silently ignored, but the
/// `is_empty()` check catches the all-no-fields-touched case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_ignores_credit_fields() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "writer": "Some Author" },
        }),
    )
    .await;
    // Unknown patch keys are silently dropped by serde; the empty
    // check then fires.
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.empty_patch");
}

/// Multiple fields in one call all land.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_multifield_patch() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    let (status, body) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": {
                "language_code": "ja",
                "manga": "YesAndRightToLeft",
                "publisher": "Shueisha",
                "format": "Annual",
                "age_rating": "Teen",
                "genre": "Action",
                "tags": "shonen,manga",
                "story_arc": "Origin",
            },
            "mode": "replace",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], 3);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = IssueEntity::find()
        .filter(entity::issue::Column::Id.is_in(ids))
        .all(&db)
        .await
        .unwrap();
    for r in &rows {
        assert_eq!(r.language_code.as_deref(), Some("ja"));
        assert_eq!(r.manga.as_deref(), Some("YesAndRightToLeft"));
        assert_eq!(r.publisher.as_deref(), Some("Shueisha"));
        assert_eq!(r.format.as_deref(), Some("Annual"));
        assert_eq!(r.age_rating.as_deref(), Some("Teen"));
        assert_eq!(r.genre.as_deref(), Some("Action"));
        assert_eq!(r.tags.as_deref(), Some("shonen,manga"));
        assert_eq!(r.story_arc.as_deref(), Some("Origin"));
    }
}

/// `null` patch value clears (in replace mode).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_metadata_null_clears_in_replace_mode() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let ids = seed_three_issues(&app).await;

    // First populate then clear.
    let _ = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "language_code": "ja" },
            "mode": "replace",
        }),
    )
    .await;
    let (status, _) = patch_bulk(
        &app,
        &auth,
        serde_json::json!({
            "issue_ids": ids,
            "patch": { "language_code": null },
            "mode": "replace",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let rows = IssueEntity::find()
        .filter(entity::issue::Column::Id.is_in(ids))
        .all(&db)
        .await
        .unwrap();
    for r in &rows {
        assert!(
            r.language_code.is_none(),
            "language_code should be cleared: {r:?}"
        );
    }
}
