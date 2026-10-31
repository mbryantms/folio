//! Series rollup resilience (audit 2026-11, N-L2).
//!
//! Two guards on `rollup_series_metadata`:
//!
//! 1. **Person-slug race.** Concurrent rollups of series that share a
//!    brand-new creator used to collide on the `person_slug_key` unique
//!    index: the insert's `ON CONFLICT (normalized_name)` arbiter did not
//!    cover the slug index, so the loser got a 23505 and the `?` aborted
//!    the rest of that series' rollup for the pass (18 occurrences in the
//!    50k load run). Eight rollups now race on the same creators and every
//!    one must succeed, leave exactly one `person` row per creator, and
//!    link every credit to it.
//! 2. **Step isolation.** A failure in one sub-step (forced here with a
//!    trigger that rejects every `series_genres` insert — the test's own
//!    clone DB is the seam) must still let the credits, the person links
//!    and the CSV read-cache land for that series in the same pass. The
//!    error is still reported to the caller.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    issue_credit, library, person,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_credit,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, Statement,
};
use server::library::scanner::metadata_rollup::{
    IssueMetadataInputs, replace_issue_metadata, rollup_series_metadata,
};
use uuid::Uuid;

async fn seed_library(db: &DatabaseConnection) -> Uuid {
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Persons".into()),
        root_path: Set(format!("/tmp/rollup-persons-{lib_id}")),
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
    }
    .insert(db)
    .await
    .unwrap();
    lib_id
}

async fn seed_series(db: &DatabaseConnection, lib_id: Uuid, n: usize) -> Uuid {
    let series_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let name = format!("Race Series {n}");
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.clone()),
        normalized_name: Set(normalize_name(&name)),
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
    series_id
}

async fn seed_issue(db: &DatabaseConnection, lib_id: Uuid, series_id: Uuid, suffix: u8) -> String {
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), suffix);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("issue-{suffix}")),
        file_path: Set(format!("/tmp/rollup-persons/{series_id}/{suffix}.cbz")),
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
        comicinfo_count: Set(None),
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        last_sidecar_rewrite_at: Set(None),
        metron_info_raw: Set(None),
        cover_page_index: Set(0),
        metadata_review_accepted_at: Set(None),
        metadata_review_accepted_by: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    issue_id
}

const SERIES_COUNT: usize = 8;
const CREATOR_COUNT: usize = 48;
/// The collision window is narrow (a rollup's slug `COUNT(*)` has to run
/// before a sibling's `INSERT` commits, and its own `INSERT` after), so one
/// batch of eight rollups does not always hit it. Several rounds over fresh
/// creator names make the pre-fix failure dependable without a hook.
const ROUNDS: usize = 4;

fn creator(round: usize, n: usize) -> String {
    format!("Race Creator {round}-{n:03}")
}

/// Every series credits the same `CREATOR_COUNT` brand-new writers and
/// pencillers, so eight concurrent rollups all try to mint the same
/// `person` rows at once.
async fn seed_racing_series(db: &DatabaseConnection, lib_id: Uuid, round: usize) -> Vec<Uuid> {
    let writers = (0..CREATOR_COUNT / 2)
        .map(|n| creator(round, n))
        .collect::<Vec<_>>()
        .join(", ");
    let pencillers = (CREATOR_COUNT / 2..CREATOR_COUNT)
        .map(|n| creator(round, n))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = Vec::with_capacity(SERIES_COUNT);
    for n in 0..SERIES_COUNT {
        let series_id = seed_series(db, lib_id, round * SERIES_COUNT + n).await;
        for suffix in 0..2u8 {
            let issue_id = seed_issue(db, lib_id, series_id, suffix).await;
            replace_issue_metadata(
                db,
                &issue_id,
                &IssueMetadataInputs {
                    writer: Some(&writers),
                    penciller: Some(&pencillers),
                    genre: Some("Action"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        out.push(series_id);
    }
    out
}

async fn count_scalar(db: &DatabaseConnection, sql: &str, series_id: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        [series_id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

async fn assert_creators_linked(db: &DatabaseConnection, round: usize, series_ids: &[Uuid]) {
    // Exactly one person row per creator.
    for n in 0..CREATOR_COUNT {
        let normalized = creator(round, n).to_lowercase();
        let rows = person::Entity::find()
            .filter(person::Column::NormalizedName.eq(&normalized))
            .count(db)
            .await
            .unwrap();
        assert_eq!(
            rows, 1,
            "expected one person row for {normalized:?}, got {rows}"
        );
    }
    // Every credit in every racing series links to the person that owns
    // its name, and the series-level rollup carries the id along.
    for series_id in series_ids.iter().copied() {
        let unlinked = count_scalar(
            db,
            "SELECT COUNT(*)::bigint AS n FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             WHERE i.series_id = $1 AND ic.person_id IS NULL",
            series_id,
        )
        .await;
        assert_eq!(
            unlinked, 0,
            "series {series_id} still has {unlinked} unlinked credits"
        );
        let mismatched = count_scalar(
            db,
            "SELECT COUNT(*)::bigint AS n FROM issue_credits ic \
             JOIN issues i ON i.id = ic.issue_id \
             JOIN person p ON p.id = ic.person_id \
             WHERE i.series_id = $1 AND p.normalized_name <> btrim(lower(ic.person))",
            series_id,
        )
        .await;
        assert_eq!(
            mismatched, 0,
            "series {series_id} has credits linked to the wrong person"
        );
        let series_credits = series_credit::Entity::find()
            .filter(series_credit::Column::SeriesId.eq(series_id))
            .count(db)
            .await
            .unwrap();
        assert_eq!(
            series_credits as usize, CREATOR_COUNT,
            "series {series_id} credits rolled up"
        );
        let without_person = series_credit::Entity::find()
            .filter(series_credit::Column::SeriesId.eq(series_id))
            .filter(series_credit::Column::PersonId.is_null())
            .count(db)
            .await
            .unwrap();
        assert_eq!(
            without_person, 0,
            "series {series_id} rolled up credits without person ids"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_rollups_sharing_new_creators_all_succeed() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = seed_library(&db).await;

    // No person rows yet: every rollup below has to mint all of them.
    let before = person::Entity::find()
        .filter(person::Column::NormalizedName.like("race creator %"))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(before, 0);

    let mut failures = Vec::new();
    for round in 0..ROUNDS {
        let series_ids = seed_racing_series(&db, lib_id, round).await;
        let mut handles = Vec::with_capacity(SERIES_COUNT);
        for series_id in series_ids.iter().copied() {
            let db = db.clone();
            handles.push(tokio::spawn(async move {
                rollup_series_metadata(&db, series_id).await
            }));
        }
        for (series_id, handle) in series_ids.iter().zip(handles) {
            if let Err(e) = handle.await.expect("rollup task panicked") {
                failures.push(format!("round {round} series {series_id}: {e}"));
            }
        }
        if failures.is_empty() {
            assert_creators_linked(&db, round, &series_ids).await;
        }
    }
    assert!(
        failures.is_empty(),
        "concurrent rollups failed: {failures:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollup_finishes_the_remaining_steps_when_one_step_fails() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = seed_library(&db).await;
    let series_id = seed_series(&db, lib_id, 0).await;
    let issue_id = seed_issue(&db, lib_id, series_id, 0).await;
    replace_issue_metadata(
        &db,
        &issue_id,
        &IssueMetadataInputs {
            writer: Some("Resilient Writer"),
            genre: Some("Action"),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Seam: the test owns its clone DB, so a trigger that rejects every
    // `series_genres` insert makes the *first* rollup step fail
    // deterministically without touching application code.
    db.execute_unprepared(
        "CREATE FUNCTION rollup_test_reject() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'forced series_genres failure'; END $$; \
         CREATE TRIGGER rollup_test_reject BEFORE INSERT ON series_genres \
         FOR EACH ROW EXECUTE FUNCTION rollup_test_reject();",
    )
    .await
    .unwrap();

    let err = rollup_series_metadata(&db, series_id)
        .await
        .expect_err("the forced step failure is still reported");
    assert!(
        err.to_string().contains("forced series_genres failure"),
        "unexpected error: {err}"
    );

    // The later steps ran anyway: person minted + linked, credits rolled
    // up, CSV read-cache rebuilt from the junction.
    let p = person::Entity::find()
        .filter(person::Column::NormalizedName.eq("resilient writer"))
        .one(&db)
        .await
        .unwrap()
        .expect("person row minted despite the failing genres step");
    let credit = issue_credit::Entity::find()
        .filter(issue_credit::Column::IssueId.eq(&issue_id))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credit.person_id, Some(p.id), "credit linked to the person");
    let rolled = series_credit::Entity::find()
        .filter(series_credit::Column::SeriesId.eq(series_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rolled.len(), 1, "series_credits rebuilt");
    assert_eq!(rolled[0].person_id, Some(p.id));
    let issue = entity::issue::Entity::find_by_id(&issue_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        issue.writer.as_deref(),
        Some("Resilient Writer"),
        "CSV read-cache rebuilt from the junction"
    );
}
