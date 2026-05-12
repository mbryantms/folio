//! Markers + Collections M1 — schema-level coverage.
//!
//! Exercises the new `kind = 'collection'` discriminator on
//! `saved_views`, the per-user `system_key` partial unique, the
//! `collection_entries` XOR + uniqueness invariants, and the M9
//! "Want to Read" filter-template rename. Fixture-seeded with the
//! same ActiveModel shapes the saved-views integration test uses so
//! schema drift surfaces in one place.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    collection_entry::ActiveModel as CollectionEntryAM,
    issue::ActiveModel as IssueAM,
    library,
    saved_view::ActiveModel as SavedViewAM,
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

async fn make_collection(
    db_url: &str,
    user_id: Uuid,
    name: &str,
    system_key: Option<&str>,
) -> Uuid {
    let db = Database::connect(db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(user_id)),
        kind: Set("collection".into()),
        system_key: Set(system_key.map(str::to_owned)),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .expect("insert collection saved_view");
    id
}

async fn insert_entry(
    db_url: &str,
    view_id: Uuid,
    position: i32,
    entry_kind: &str,
    series_id: Option<Uuid>,
    issue_id: Option<String>,
) -> Result<(), sea_orm::DbErr> {
    let db = Database::connect(db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    CollectionEntryAM {
        id: Set(Uuid::now_v7()),
        saved_view_id: Set(view_id),
        position: Set(position),
        entry_kind: Set(entry_kind.into()),
        series_id: Set(series_id),
        issue_id: Set(issue_id),
        added_at: Set(now),
    }
    .insert(&db)
    .await
    .map(|_| ())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_kind_admitted_and_validated() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "u1@example.com").await;

    // Happy path.
    let view_id = make_collection(&app.db_url, user_id, "My Capes", None).await;
    assert!(!view_id.is_nil());

    // user_id NOT NULL is required.
    let bad = db
        .execute(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            r"INSERT INTO saved_views (id, user_id, kind, name, custom_tags,
                                       auto_pin, created_at, updated_at)
              VALUES (gen_random_uuid(), NULL, 'collection', 'Orphan',
                      ARRAY[]::text[], FALSE, NOW(), NOW())",
        ))
        .await;
    assert!(bad.is_err(), "expected CHECK violation for NULL user_id");

    // Filter columns must stay NULL on collection rows.
    let bad = db
        .execute(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            r"INSERT INTO saved_views (id, user_id, kind, name, custom_tags,
                                       match_mode, conditions, sort_field,
                                       sort_order, result_limit,
                                       auto_pin, created_at, updated_at)
              VALUES (gen_random_uuid(), $1::uuid, 'collection', 'Hybrid',
                      ARRAY[]::text[], 'all', '[]'::jsonb, 'name', 'asc', 10,
                      FALSE, NOW(), NOW())",
            [user_id.into()],
        ))
        .await;
    assert!(bad.is_err(), "expected CHECK violation for filter columns");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_entry_xor_enforced() {
    let app = TestApp::spawn().await;
    let user_id = seed_user(&app.db_url, "u2@example.com").await;
    let (_lib, series_id, issue_id) = seed_library_series_issue(&app.db_url).await;
    let view_id = make_collection(&app.db_url, user_id, "Mixed", None).await;

    // Both refs populated → reject.
    assert!(
        insert_entry(
            &app.db_url,
            view_id,
            0,
            "series",
            Some(series_id),
            Some(issue_id.clone())
        )
        .await
        .is_err(),
        "expected XOR violation when both refs set"
    );

    // entry_kind = 'series' but issue_id populated → reject.
    assert!(
        insert_entry(
            &app.db_url,
            view_id,
            0,
            "series",
            None,
            Some(issue_id.clone())
        )
        .await
        .is_err(),
        "expected XOR violation for kind/ref mismatch"
    );

    // Neither populated → reject.
    assert!(
        insert_entry(&app.db_url, view_id, 0, "series", None, None)
            .await
            .is_err(),
        "expected XOR violation for empty refs"
    );

    // Mixed series + issue entries coexist in one collection.
    insert_entry(&app.db_url, view_id, 0, "series", Some(series_id), None)
        .await
        .expect("series entry");
    insert_entry(&app.db_url, view_id, 1, "issue", None, Some(issue_id))
        .await
        .expect("issue entry — mixed refs in one collection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_entry_idempotent_add() {
    let app = TestApp::spawn().await;
    let user_id = seed_user(&app.db_url, "u3@example.com").await;
    let (_lib, series_id, _issue) = seed_library_series_issue(&app.db_url).await;
    let view_a = make_collection(&app.db_url, user_id, "A", None).await;
    let view_b = make_collection(&app.db_url, user_id, "B", None).await;

    insert_entry(&app.db_url, view_a, 0, "series", Some(series_id), None)
        .await
        .expect("first add");

    assert!(
        insert_entry(&app.db_url, view_a, 1, "series", Some(series_id), None)
            .await
            .is_err(),
        "expected unique violation on duplicate series in same view"
    );

    insert_entry(&app.db_url, view_b, 0, "series", Some(series_id), None)
        .await
        .expect("same series in a different collection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_user_and_global_system_key_uniqueness() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_a = seed_user(&app.db_url, "ua@example.com").await;
    let user_b = seed_user(&app.db_url, "ub@example.com").await;

    // Two different users can each own a `want_to_read` row.
    make_collection(&app.db_url, user_a, "Want to Read", Some("want_to_read")).await;
    make_collection(&app.db_url, user_b, "Want to Read", Some("want_to_read")).await;

    // Same user can't hold two `want_to_read` rows.
    let dup = SavedViewAM {
        id: Set(Uuid::now_v7()),
        user_id: Set(Some(user_a)),
        kind: Set("collection".into()),
        system_key: Set(Some("want_to_read".into())),
        name: Set("Want to Read dupe".into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(Utc::now().fixed_offset()),
        updated_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await;
    assert!(
        dup.is_err(),
        "expected per-user system_key unique violation"
    );

    // Global system rows still can't share a key — seeded
    // `continue_reading` is already present from M9; attempt a dupe.
    let bad = db
        .execute(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            r"INSERT INTO saved_views (id, user_id, kind, system_key, name,
                                       custom_tags, auto_pin, created_at, updated_at)
              VALUES (gen_random_uuid(), NULL, 'system', 'continue_reading',
                      'Continue reading 2', ARRAY[]::text[], FALSE, NOW(), NOW())",
        ))
        .await;
    assert!(bad.is_err(), "expected global system_key unique violation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn want_to_read_template_renamed_to_unstarted() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = db
        .query_one(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT name FROM saved_views \
             WHERE id = '00000000-0000-0000-0000-000000000004'::uuid",
        ))
        .await
        .unwrap()
        .expect("M9 template row present");
    let name: String = row.try_get("", "name").unwrap();
    assert_eq!(name, "Unstarted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_entries_cascade_on_view_delete() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_id = seed_user(&app.db_url, "cascade@example.com").await;
    let (_lib, series_id, _issue) = seed_library_series_issue(&app.db_url).await;
    let view_id = make_collection(&app.db_url, user_id, "ToDelete", None).await;
    insert_entry(&app.db_url, view_id, 0, "series", Some(series_id), None)
        .await
        .expect("seed entry");

    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM saved_views WHERE id = $1::uuid",
        [view_id.into()],
    ))
    .await
    .expect("delete view");

    let count: i64 = db
        .query_one(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT COUNT(*)::bigint AS c FROM collection_entries WHERE saved_view_id = $1::uuid",
            [view_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "c")
        .unwrap();
    assert_eq!(count, 0, "entries cascade-deleted with the view");
}
