//! Integration tests for `GET /issues/{issue_id}/next-up` — the
//! single-issue resolver used by the reader's "Up Next" pill and
//! end-of-issue card.
//!
//! Covers:
//!   - CBL > series precedence when both yield a target.
//!   - CBL stale param (issue not in list) falls back to series.
//!   - CBL exhausted (every later entry finished) falls back to series.
//!   - Series exhausted with no CBL → `source = "none"`.
//!   - ACL: hidden library returns 404 for the current issue itself.
//!   - Soft-deleted next issue is skipped.
//!   - CBL `cbl_position` advances past the current entry's position.
//!   - Empty / invalid `?cbl=` param falls through to series.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    cbl_entry, cbl_list,
    issue::ActiveModel as IssueAM,
    library, library_user_access, progress_record, saved_view,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set, Unchanged};
use tower::ServiceExt;
use uuid::Uuid;

// ───── Test scaffolding (duplicated from rails.rs by intent — these
// tests cover a separate handler and the helpers are small enough not
// to be worth promoting into a shared `common` submodule) ─────

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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let session = extract("__Host-comic_session=");
    let csrf = extract("__Host-comic_csrf=");
    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::{ColumnTrait, QueryFilter};
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row by email");
    let user_id = user_row.id;
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::user::ActiveModel {
        id: Unchanged(user_id),
        role: Set("user".into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    path: &str,
    user: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    if let Some(u) = user {
        let cookie = format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            u.session, u.csrf
        );
        req = req.header(header::COOKIE, cookie);
        if method != Method::GET {
            req = req.header("X-CSRF-Token", &u.csrf);
        }
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
    let body = body_json(resp.into_body()).await;
    (status, body)
}

async fn seed_one_issue(app: &TestApp, slug_prefix: &str) -> (Uuid, Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {slug_prefix}")),
        root_path: Set(format!("/tmp/{slug_prefix}-{lib_id}")),
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
        name: Set(format!("Series {slug_prefix}")),
        normalized_name: Set(normalize_name(&format!("Series {slug_prefix}"))),
        year: Set(Some(2020)),
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
        slug: Set(format!("{slug_prefix}-series")),
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
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 1u8);
    insert_issue(&db, lib_id, series_id, &issue_id, 1.0, slug_prefix, "1").await;
    (lib_id, series_id, issue_id)
}

async fn seed_extra_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    sort_number: f64,
    slug: &str,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), sort_number as u8);
    insert_issue(
        &db,
        lib_id,
        series_id,
        &issue_id,
        sort_number,
        slug,
        &format!("{sort_number}"),
    )
    .await;
    issue_id
}

async fn insert_issue(
    db: &sea_orm::DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    issue_id: &str,
    sort_number: f64,
    slug: &str,
    number_raw: &str,
) {
    let now = Utc::now().fixed_offset();
    IssueAM {
        id: Set(issue_id.into()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(slug.into()),
        file_path: Set(format!("/tmp/{slug}/{sort_number}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.into()),
        title: Set(Some(format!("Issue {sort_number}"))),
        sort_number: Set(Some(sort_number)),
        number_raw: Set(Some(number_raw.into())),
        volume: Set(None),
        year: Set(Some(2020)),
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
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        superseded_by: Set(None),
        special_type: Set(None),
        hash_algorithm: Set(0),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(Some(0)),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn soft_delete_issue(app: &TestApp, issue_id: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::issue::ActiveModel {
        id: Unchanged(issue_id.into()),
        state: Set("removed".into()),
        removed_at: Set(Some(now)),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
}

async fn seed_cbl_list(app: &TestApp, name: &str, entries: &[(i32, &str)]) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set(name.into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(entries.len() as i32)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    for (pos, issue_id) in entries {
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(*pos),
            series_name: Set("Series".into()),
            issue_number: Set(format!("{pos}")),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(Some((*issue_id).into())),
            match_status: Set("matched".into()),
            match_method: Set(Some("test".into())),
            match_confidence: Set(Some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(Some(now)),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    list_id
}

/// Wrap a CBL list in a kind='cbl' saved view so the next-up resolver
/// can find it via the `?cbl=<saved_view_id>` query param.
async fn seed_cbl_saved_view(
    app: &TestApp,
    owner_user_id: Option<Uuid>,
    cbl_list_id: Uuid,
    name: &str,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(owner_user_id),
        kind: Set("cbl".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(vec![]),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(Some(cbl_list_id)),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn grant_access(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn finish_issue(app: &TestApp, user_id: Uuid, issue_id: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    progress_record::ActiveModel {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(19),
        percent: Set(1.0),
        finished: Set(true),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

// ───── Tests ─────

#[tokio::test]
async fn next_up_series_returns_next_issue_in_sort_order() {
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-series@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-series").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-series-2").await;
    let _issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "nu-series-3").await;
    grant_access(&app, user.user_id, lib_id).await;
    finish_issue(&app, user.user_id, &issue1_id).await;

    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "series");
    assert_eq!(body["target"]["id"], issue2_id);
    assert!(body["cbl_list_id"].is_null());
    assert!(body["cbl_position"].is_null());
}

#[tokio::test]
async fn next_up_series_skips_finished_and_soft_deleted_issues() {
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-skips@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-skip").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-skip-2").await;
    let issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "nu-skip-3").await;
    let issue4_id = seed_extra_issue(&app, lib_id, series_id, 4.0, "nu-skip-4").await;
    grant_access(&app, user.user_id, lib_id).await;

    finish_issue(&app, user.user_id, &issue1_id).await;
    finish_issue(&app, user.user_id, &issue2_id).await; // already-finished after current
    soft_delete_issue(&app, &issue3_id).await; // soft-deleted, must be skipped

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "series");
    assert_eq!(
        body["target"]["id"], issue4_id,
        "must skip past finished {issue2_id} and soft-deleted {issue3_id}"
    );
}

#[tokio::test]
async fn next_up_series_returns_none_when_caught_up() {
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-done@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-done").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-done-2").await;
    grant_access(&app, user.user_id, lib_id).await;
    finish_issue(&app, user.user_id, &issue1_id).await;
    finish_issue(&app, user.user_id, &issue2_id).await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue2_id}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "none");
    assert!(body["target"].is_null());
    // No other series / CBLs exist for this user → top_on_deck_card
    // returns None → fallback_suggestion stays null. The D-6 fix only
    // populates the field when there's a real suggestion to render.
    assert!(
        body["fallback_suggestion"].is_null(),
        "no on-deck candidates → fallback_suggestion must stay null"
    );
}

#[tokio::test]
async fn next_up_caught_up_populates_fallback_suggestion_when_user_has_on_deck() {
    // D-6 fix: when the user is caught up on the issue they just
    // finished BUT has an unrelated series with unread issues, the
    // resolver surfaces the top On Deck card as a fallback suggestion.
    // The end-of-issue card's caught-up body renders it as a "try this
    // next" tile so the user isn't stranded.
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-fallback@example.com").await;
    demote_to_user(&app, user.user_id).await;

    // Series A: the user just finished both issues. The resolver
    // queried for issue2 will return source=none.
    let (lib_id, series_a, issue_a1) = seed_one_issue(&app, "nu-fb-a").await;
    let issue_a2 = seed_extra_issue(&app, lib_id, series_a, 2.0, "nu-fb-a-2").await;
    grant_access(&app, user.user_id, lib_id).await;
    finish_issue(&app, user.user_id, &issue_a1).await;
    finish_issue(&app, user.user_id, &issue_a2).await;

    // Series B (separate series in the same library so the ACL grant
    // covers it): user finished issue 1, hasn't started issue 2 →
    // top On Deck card will pick this series as series_next.
    let series_b = Uuid::now_v7();
    {
        use entity::series::{ActiveModel as SeriesAM, normalize_name};
        use sea_orm::Set;
        let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
        let now = chrono::Utc::now().fixed_offset();
        SeriesAM {
            id: Set(series_b),
            library_id: Set(lib_id),
            name: Set("Series nu-fb-b".into()),
            normalized_name: Set(normalize_name("Series nu-fb-b")),
            year: Set(Some(2020)),
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
            slug: Set("nu-fb-b-series".into()),
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
    }
    let issue_b1 = seed_extra_issue(&app, lib_id, series_b, 1.0, "nu-fb-b-1").await;
    let issue_b2 = seed_extra_issue(&app, lib_id, series_b, 2.0, "nu-fb-b-2").await;
    finish_issue(&app, user.user_id, &issue_b1).await;
    let _ = issue_b2;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue_a2}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "none");
    assert!(body["target"].is_null());
    let fallback = &body["fallback_suggestion"];
    assert!(
        fallback.is_object(),
        "expected fallback_suggestion to populate; got {fallback:?}"
    );
    assert_eq!(
        fallback["kind"], "series_next",
        "series B should surface as the on-deck candidate"
    );
    assert_eq!(
        fallback["issue"]["id"], issue_b2,
        "fallback should target issue_b2 (the next unread in series B)"
    );
}

#[tokio::test]
async fn next_up_caught_up_excludes_current_issue_from_fallback() {
    // Guard against suggesting the very issue the user just finished.
    // If the only on-deck candidate would target `current.id`, the
    // resolver must skip it and return None rather than suggesting
    // what the reader is already on.
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-fb-exclude@example.com").await;
    demote_to_user(&app, user.user_id).await;

    // Only one issue in the only series; user finished it. On Deck
    // composition produces nothing (no unread issues), so the exclude
    // logic is moot here — but we exercise the path for documentation.
    let (lib_id, _series_id, issue1_id) = seed_one_issue(&app, "nu-fb-excl").await;
    grant_access(&app, user.user_id, lib_id).await;
    finish_issue(&app, user.user_id, &issue1_id).await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "none");
    assert!(
        body["fallback_suggestion"].is_null(),
        "no separate unread series → fallback stays null"
    );
}

#[tokio::test]
async fn next_up_cbl_takes_precedence_over_series() {
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-cbl-win@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-cbl-win").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-cbl-win-2").await;
    let issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "nu-cbl-win-3").await;
    grant_access(&app, user.user_id, lib_id).await;

    // CBL puts issue3 right after issue1, skipping series-sort issue2.
    // If CBL precedence works, next-up after issue1 must be issue3, not
    // issue2 (which would be series-next).
    let list_id = seed_cbl_list(
        &app,
        "Skip-2 List",
        &[(0, &issue1_id), (1, &issue3_id), (2, &issue2_id)],
    )
    .await;
    let sv_id = seed_cbl_saved_view(&app, None, list_id, "Skip-2 saved view").await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up?cbl={sv_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "cbl");
    assert_eq!(body["target"]["id"], issue3_id);
    assert_eq!(body["cbl_list_id"], list_id.to_string());
    assert_eq!(body["cbl_list_name"], "Skip-2 List");
    assert_eq!(
        body["cbl_position"], 2,
        "1-based: position-1 entry → cbl_position 2"
    );
    assert_eq!(body["cbl_total"], 3);
}

#[tokio::test]
async fn next_up_cbl_stale_param_falls_back_to_series() {
    // User opens an issue with `?cbl=<id>` set, but that issue isn't
    // actually in the referenced CBL (deleted entry, wrong saved view,
    // shared link from a different context). Resolver must NOT 404 —
    // it falls back to series-next so the reader stays useful, AND
    // surfaces `cbl_param_was_stale: true` so the web can scrub the
    // dead `?cbl=` from the URL.
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-cbl-stale@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-stale").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-stale-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    // CBL contains only issue2 — issue1 is not in this list.
    let list_id = seed_cbl_list(&app, "Other", &[(0, &issue2_id)]).await;
    let sv_id = seed_cbl_saved_view(&app, None, list_id, "Other view").await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up?cbl={sv_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(
        body["source"], "series",
        "stale ?cbl= must not block series fallback"
    );
    assert_eq!(body["target"]["id"], issue2_id);
    assert_eq!(
        body["cbl_param_was_stale"], true,
        "stale-param flag must surface so the web can scrub ?cbl="
    );
}

#[tokio::test]
async fn next_up_cbl_no_match_does_not_set_stale_flag() {
    // Distinguish "no match" (deleted view / wrong kind / not owned)
    // from genuine staleness — only the latter sets the URL-scrub flag.
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-cbl-nomatch@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-nomatch").await;
    let _issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-nomatch-2").await;
    grant_access(&app, user.user_id, lib_id).await;

    // Use a saved-view UUID that doesn't exist → resolver returns NoMatch.
    let phantom_sv = Uuid::now_v7();

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up?cbl={phantom_sv}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["source"], "series");
    // The flag should be absent (serde-skipped when false) — checking
    // for `is_null()` covers both "field missing" and "field=false".
    assert!(
        body.get("cbl_param_was_stale").is_none() || body["cbl_param_was_stale"] == false,
        "non-existent saved view is not stale-param; flag must be absent / false (got {:?})",
        body.get("cbl_param_was_stale")
    );
}

#[tokio::test]
async fn next_up_cbl_exhausted_falls_back_to_series() {
    // Caller is reading through a CBL but everything after their current
    // entry is already finished. The CBL branch has nothing to return —
    // resolver falls through to series-next so the user keeps moving.
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-cbl-exhausted@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-cbl-ex").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-cbl-ex-2").await;
    let issue3_id = seed_extra_issue(&app, lib_id, series_id, 3.0, "nu-cbl-ex-3").await;
    grant_access(&app, user.user_id, lib_id).await;

    // CBL covers issue1 + issue2 only. issue2 is already finished.
    let list_id = seed_cbl_list(&app, "Done CBL", &[(0, &issue1_id), (1, &issue2_id)]).await;
    let sv_id = seed_cbl_saved_view(&app, None, list_id, "Done view").await;
    finish_issue(&app, user.user_id, &issue2_id).await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up?cbl={sv_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(
        body["source"], "series",
        "no more CBL entries → fall back to series"
    );
    assert_eq!(
        body["target"]["id"], issue3_id,
        "series-next walks past finished issue2"
    );
}

#[tokio::test]
async fn next_up_acl_returns_404_for_invisible_current_issue() {
    let app = TestApp::spawn().await;
    let user = register(&app, "next-up-acl@example.com").await;
    demote_to_user(&app, user.user_id).await;

    let (_lib_id, _series_id, issue1_id) = seed_one_issue(&app, "nu-acl").await;
    // Note: no grant_access call. User can't see the issue's library.

    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn next_up_cbl_belonging_to_other_user_is_silently_ignored() {
    // A CBL saved view that's owned by a different user must fall
    // through to series-next, not return that user's CBL data.
    let app = TestApp::spawn().await;
    let alice = register(&app, "next-up-alice@example.com").await;
    demote_to_user(&app, alice.user_id).await;
    let bob = register(&app, "next-up-bob@example.com").await;
    demote_to_user(&app, bob.user_id).await;

    let (lib_id, series_id, issue1_id) = seed_one_issue(&app, "nu-other").await;
    let issue2_id = seed_extra_issue(&app, lib_id, series_id, 2.0, "nu-other-2").await;
    grant_access(&app, alice.user_id, lib_id).await;

    // Bob owns the saved view, alice tries to use it.
    let list_id = seed_cbl_list(&app, "Bob List", &[(0, &issue1_id), (1, &issue2_id)]).await;
    let sv_id = seed_cbl_saved_view(&app, Some(bob.user_id), list_id, "Bob view").await;

    let (_, body) = http(
        &app,
        Method::GET,
        &format!("/issues/{issue1_id}/next-up?cbl={sv_id}"),
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(
        body["source"], "series",
        "alice can't pivot through bob's CBL view"
    );
    assert_eq!(body["target"]["id"], issue2_id);
}
