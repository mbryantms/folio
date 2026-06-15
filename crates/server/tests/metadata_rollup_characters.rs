//! Integration coverage for the characters/teams/locations rollup that
//! lands as part of the saved-views parity work. Mirrors the existing
//! genres rollup tests by driving `replace_issue_metadata` then
//! `rollup_series_metadata` against a live DB and asserting both the
//! per-issue junction (`issue_characters`/`issue_teams`/`issue_locations`)
//! and the series-level rollup (`series_characters`/`series_teams`/
//! `series_locations`) carry the dedup'd union.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    issue_character, issue_location, issue_team, library,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_character, series_location, series_team,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set,
};
use server::library::scanner::metadata_rollup::{
    IssueMetadataInputs, replace_issue_metadata, rollup_series_metadata,
};
use uuid::Uuid;

async fn seed_library_and_series(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let lib_id = Uuid::now_v7();
    let series_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test".into()),
        root_path: Set(format!("/tmp/rollup-{lib_id}")),
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
    }
    .insert(db)
    .await
    .unwrap();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Rollup Test".into()),
        normalized_name: Set(normalize_name("Rollup Test")),
        year: Set(Some(2020)),
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
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
    (lib_id, series_id)
}

async fn seed_issue(db: &DatabaseConnection, lib_id: Uuid, series_id: Uuid, suffix: u8) -> String {
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), suffix);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("issue-{suffix}")),
        file_path: Set(format!("/tmp/rollup/{suffix}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(suffix as f64)),
        number_raw: Set(Some(suffix.to_string())),
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
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    issue_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_unions_characters_teams_locations_across_issues() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let (lib_id, series_id) = seed_library_and_series(&db).await;
    let issue_a = seed_issue(&db, lib_id, series_id, 0).await;
    let issue_b = seed_issue(&db, lib_id, series_id, 1).await;

    // Issue A: Spider-Man + Mary Jane / Avengers / New York
    let inputs_a = IssueMetadataInputs {
        characters: Some("Spider-Man, Mary Jane"),
        teams: Some("Avengers"),
        locations: Some("New York"),
        ..Default::default()
    };
    replace_issue_metadata(&db, &issue_a, &inputs_a)
        .await
        .unwrap();
    // Issue B: Spider-Man (overlap with A) + Black Cat / X-Men / Brooklyn
    let inputs_b = IssueMetadataInputs {
        characters: Some("Spider-Man, Black Cat"),
        teams: Some("X-Men"),
        locations: Some("Brooklyn"),
        ..Default::default()
    };
    replace_issue_metadata(&db, &issue_b, &inputs_b)
        .await
        .unwrap();

    rollup_series_metadata(&db, series_id).await.unwrap();

    // Per-issue tables hold each issue's own CSV split, no cross-pollination.
    let a_chars: Vec<String> = issue_character::Entity::find()
        .filter(issue_character::Column::IssueId.eq(&issue_a))
        .order_by_asc(issue_character::Column::Character)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.character)
        .collect();
    assert_eq!(a_chars, vec!["Mary Jane".to_owned(), "Spider-Man".into()]);
    let b_chars: Vec<String> = issue_character::Entity::find()
        .filter(issue_character::Column::IssueId.eq(&issue_b))
        .order_by_asc(issue_character::Column::Character)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.character)
        .collect();
    assert_eq!(b_chars, vec!["Black Cat".to_owned(), "Spider-Man".into()]);

    // Series-level tables hold the distinct union.
    let series_chars: Vec<String> = series_character::Entity::find()
        .filter(series_character::Column::SeriesId.eq(series_id))
        .order_by_asc(series_character::Column::Character)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.character)
        .collect();
    assert_eq!(
        series_chars,
        vec![
            "Black Cat".to_owned(),
            "Mary Jane".into(),
            "Spider-Man".into()
        ],
        "series_characters dedup'd union of both issues"
    );
    let series_teams: Vec<String> = series_team::Entity::find()
        .filter(series_team::Column::SeriesId.eq(series_id))
        .order_by_asc(series_team::Column::Team)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.team)
        .collect();
    assert_eq!(series_teams, vec!["Avengers".to_owned(), "X-Men".into()]);
    let series_locs: Vec<String> = series_location::Entity::find()
        .filter(series_location::Column::SeriesId.eq(series_id))
        .order_by_asc(series_location::Column::Location)
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.location)
        .collect();
    assert_eq!(series_locs, vec!["Brooklyn".to_owned(), "New York".into()]);

    // Re-running both writes is idempotent — counts stay identical.
    replace_issue_metadata(&db, &issue_a, &inputs_a)
        .await
        .unwrap();
    replace_issue_metadata(&db, &issue_b, &inputs_b)
        .await
        .unwrap();
    rollup_series_metadata(&db, series_id).await.unwrap();

    let series_chars_count = series_character::Entity::find()
        .filter(series_character::Column::SeriesId.eq(series_id))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(series_chars_count, 3, "idempotent: still 3 characters");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replace_issue_metadata_clears_when_csv_becomes_empty() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let (lib_id, series_id) = seed_library_and_series(&db).await;
    let issue_a = seed_issue(&db, lib_id, series_id, 0).await;

    // Initial write.
    replace_issue_metadata(
        &db,
        &issue_a,
        &IssueMetadataInputs {
            characters: Some("Spider-Man"),
            teams: Some("Avengers"),
            locations: Some("Queens"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        issue_character::Entity::find()
            .filter(issue_character::Column::IssueId.eq(&issue_a))
            .all(&db)
            .await
            .unwrap()
            .len(),
        1
    );

    // Re-write with empty inputs — junctions should empty out, matching
    // the same drop semantics as genres/tags.
    replace_issue_metadata(
        &db,
        &issue_a,
        &IssueMetadataInputs {
            characters: None,
            teams: None,
            locations: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        issue_character::Entity::find()
            .filter(issue_character::Column::IssueId.eq(&issue_a))
            .all(&db)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        issue_team::Entity::find()
            .filter(issue_team::Column::IssueId.eq(&issue_a))
            .all(&db)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        issue_location::Entity::find()
            .filter(issue_location::Column::IssueId.eq(&issue_a))
            .all(&db)
            .await
            .unwrap()
            .len(),
        0
    );
}
