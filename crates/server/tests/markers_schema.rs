//! Markers + Collections M4 — `markers` table schema coverage.
//!
//! Exercises the per-kind CHECK invariants (body required for notes,
//! region required for highlights, body size cap, page-index lower
//! bound) and the cascade behavior on user / series / issue delete.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    marker::ActiveModel as MarkerAM,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::ActiveModel as UserAM,
};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, Set, Statement};
use uuid::Uuid;

async fn seed_user(db_url: &str, email: &str) -> Uuid {
    let db = Database::connect(db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    UserAM {
        id: Set(id),
        external_id: Set(format!("local:{id}")),
        display_name: Set(email.split('@').next().unwrap().to_owned()),
        email: Set(Some(email.into())),
        email_verified: Set(true),
        password_hash: Set(Some("x".into())),
        totp_secret: Set(None),
        state: Set("active".into()),
        role: Set("user".into()),
        token_version: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        last_login_at: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert user");
    id
}

async fn seed_library_series_issue(db_url: &str) -> (Uuid, Uuid, String) {
    let db = Database::connect(db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let series_id = Uuid::now_v7();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    let now = Utc::now().fixed_offset();

    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib-{lib_id}")),
        root_path: Set(format!("/tmp/{lib_id}")),
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
    .expect("insert library");

    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("S".into()),
        normalized_name: Set(normalize_name("S")),
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
    .expect("insert series");

    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("i-{series_id}")),
        file_path: Set(format!("/tmp/{series_id}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
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
    .expect("insert issue");

    (lib_id, series_id, issue_id)
}

fn marker_am(
    user_id: Uuid,
    series_id: Uuid,
    issue_id: &str,
    kind: &str,
    page_index: i32,
) -> MarkerAM {
    let now = Utc::now().fixed_offset();
    MarkerAM {
        id: Set(Uuid::now_v7()),
        user_id: Set(user_id),
        series_id: Set(series_id),
        issue_id: Set(issue_id.to_owned()),
        page_index: Set(page_index),
        kind: Set(kind.into()),
        is_favorite: Set(false),
        tags: Set(Vec::new()),
        region: Set(None),
        selection: Set(None),
        body: Set(None),
        color: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn happy_path_each_kind_accepted() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u1@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    // Bookmark: page-level (region NULL) accepted.
    marker_am(user_id, series_id, &issue_id, "bookmark", 0)
        .insert(&db)
        .await
        .expect("bookmark insert");

    // is_favorite is a flag (not a kind) — any kind can be starred.
    let mut starred = marker_am(user_id, series_id, &issue_id, "bookmark", 1);
    starred.is_favorite = Set(true);
    starred.insert(&db).await.expect("starred bookmark insert");

    // Note: body required.
    let mut note = marker_am(user_id, series_id, &issue_id, "note", 2);
    note.body = Set(Some("Great panel.".into()));
    note.insert(&db).await.expect("note insert");

    // Highlight: region required.
    let mut hl = marker_am(user_id, series_id, &issue_id, "highlight", 3);
    hl.region = Set(Some(
        serde_json::json!({ "x": 10.0, "y": 20.0, "w": 30.0, "h": 15.0, "shape": "rect" }),
    ));
    hl.insert(&db).await.expect("highlight insert");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_without_body_rejected() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u2@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    let bad = marker_am(user_id, series_id, &issue_id, "note", 0)
        .insert(&db)
        .await;
    assert!(bad.is_err(), "note must require body");

    // Empty string body is treated as missing (length > 0).
    let mut empty = marker_am(user_id, series_id, &issue_id, "note", 0);
    empty.body = Set(Some(String::new()));
    let bad2 = empty.insert(&db).await;
    assert!(bad2.is_err(), "empty body rejected for note");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn highlight_without_region_rejected() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u3@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    let bad = marker_am(user_id, series_id, &issue_id, "highlight", 0)
        .insert(&db)
        .await;
    assert!(bad.is_err(), "highlight must require region");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_kind_rejected() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u4@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    let bad = marker_am(user_id, series_id, &issue_id, "scribble", 0)
        .insert(&db)
        .await;
    assert!(bad.is_err(), "unknown kind rejected by allow-list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_size_cap_enforced() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u5@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    // 10 KB body is fine.
    let mut ten_kb = marker_am(user_id, series_id, &issue_id, "note", 0);
    ten_kb.body = Set(Some("a".repeat(10_240)));
    ten_kb.insert(&db).await.expect("10kb body accepted");

    // 10 KB + 1 is rejected.
    let mut over = marker_am(user_id, series_id, &issue_id, "note", 1);
    over.body = Set(Some("b".repeat(10_241)));
    let bad = over.insert(&db).await;
    assert!(bad.is_err(), "body over 10KB rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn negative_page_index_rejected() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u6@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;

    let bad = marker_am(user_id, series_id, &issue_id, "bookmark", -1)
        .insert(&db)
        .await;
    assert!(bad.is_err(), "negative page_index rejected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cascade_on_issue_delete_removes_markers() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u7@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;
    marker_am(user_id, series_id, &issue_id, "bookmark", 0)
        .insert(&db)
        .await
        .expect("seed marker");

    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM issues WHERE id = $1",
        [issue_id.clone().into()],
    ))
    .await
    .expect("delete issue");

    let count: i64 = db
        .query_one(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS c FROM markers WHERE issue_id = $1",
            [issue_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "c")
        .unwrap();
    assert_eq!(count, 0, "markers cascade-deleted with the issue");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cascade_on_user_delete_removes_markers() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u8@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;
    marker_am(user_id, series_id, &issue_id, "bookmark", 0)
        .insert(&db)
        .await
        .expect("seed marker");

    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM users WHERE id = $1::uuid",
        [user_id.into()],
    ))
    .await
    .expect("delete user");

    let count: i64 = db
        .query_one(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS c FROM markers WHERE user_id = $1::uuid",
            [user_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "c")
        .unwrap();
    assert_eq!(count, 0, "markers cascade-deleted with the user");
}
