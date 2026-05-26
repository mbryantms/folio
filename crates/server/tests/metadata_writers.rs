//! Tests for `crate::metadata::writers` (M0b).
//!
//! Covers the behaviors the plan calls out:
//! - Identifier-first dedup (`upsert_*` collapses two calls sharing
//!   an external id into one entity, even with different names).
//! - Name fallback (no identifiers → normalized_name lookup).
//! - `set_external_id` user-precedence (`set_by='user'` rows aren't
//!   overwritten by provider writes; user writes always pass).
//! - Cross-source ID propagation (one upsert with N identifiers
//!   writes N external_ids rows).
//! - Junction reconcile (`set_issue_characters` replaces the full
//!   set + writes provenance + queues a CSV-cache rebuild).
//! - CSV rebuild produces deterministic output from junction state.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use server::metadata::writers::{self, CharacterSpec, CsvRebuildBatch, SetBy};
use server::metadata::{Identifier, Source};
use uuid::Uuid;

async fn seed_minimal_issue(app: &TestApp) -> (sea_orm::DatabaseConnection, String, Uuid, Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    entity::library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test".into()),
        root_path: Set(format!("/tmp/lib-{lib_id}")),
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
        thumbnail_cover_quality: Set(80),
        thumbnail_page_quality: Set(75),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    let series_id = Uuid::now_v7();
    entity::series::ActiveModel {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Invincible".into()),
        normalized_name: Set("invincible".into()),
        slug: Set(format!("invincible-{series_id}")),
        year: Set(Some(2003)),
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
        series_group: Set(None),
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
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    let issue_id = "0".repeat(64);
    entity::issue::ActiveModel {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("invincible-1-{}", &issue_id[..8])),
        file_path: Set(format!("/tmp/{issue_id}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(Some("Family Matters".into())),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
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
        page_count: Set(Some(22)),
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
    (db, issue_id, series_id, lib_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_person_dedups_by_identifier_even_when_names_differ() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    // First call: "Brian Bendis" with CV id 100.
    let cv_id = Identifier::new(Source::ComicVine, "100");
    let first = writers::upsert_person(
        &db,
        "Brian Bendis",
        std::slice::from_ref(&cv_id),
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();

    // Second call: "Brian Michael Bendis" with the SAME CV id, from
    // Metron. Identifier match should collapse the two.
    let second = writers::upsert_person(
        &db,
        "Brian Michael Bendis",
        &[cv_id.clone(), Identifier::new(Source::Metron, "44")],
        SetBy::Provider(Source::Metron),
    )
    .await
    .unwrap();
    assert_eq!(first, second, "shared CV id must collapse to one person");

    // Both identifiers should now exist on that single row.
    let cv_row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("person"))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .filter(entity::external_id::Column::ExternalId.eq("100"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let metron_row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("person"))
        .filter(entity::external_id::Column::Source.eq("metron"))
        .filter(entity::external_id::Column::ExternalId.eq("44"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cv_row.entity_id, first.to_string());
    assert_eq!(metron_row.entity_id, first.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_person_falls_back_to_normalized_name_when_no_identifier_match() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();

    // First call: name only, no identifiers.
    let first = writers::upsert_person(&db, "  Robert Kirkman ", &[], SetBy::ComicInfo)
        .await
        .unwrap();

    // Second call: same name with different whitespace + casing, +
    // an identifier. Name fallback hits, identifier gets attached.
    let metron_id = Identifier::new(Source::Metron, "77");
    let second = writers::upsert_person(
        &db,
        "robert kirkman",
        std::slice::from_ref(&metron_id),
        SetBy::Provider(Source::Metron),
    )
    .await
    .unwrap();
    assert_eq!(first, second);

    let row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("person"))
        .filter(entity::external_id::Column::Source.eq("metron"))
        .filter(entity::external_id::Column::ExternalId.eq("77"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.entity_id, first.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_external_id_user_precedence_blocks_provider_overwrite() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let series_id = Uuid::now_v7().to_string();

    // User sets CV id 1234.
    writers::set_external_id(
        &db,
        "series",
        &series_id,
        &Identifier::new(Source::ComicVine, "1234"),
        SetBy::User,
    )
    .await
    .unwrap();

    // Provider tries to overwrite with a different id.
    writers::set_external_id(
        &db,
        "series",
        &series_id,
        &Identifier::new(Source::ComicVine, "9999"),
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();

    // User value wins.
    let row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(&series_id))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.external_id, "1234");
    assert_eq!(row.set_by, "user");

    // But the user can replace their own value.
    writers::set_external_id(
        &db,
        "series",
        &series_id,
        &Identifier::new(Source::ComicVine, "5555"),
        SetBy::User,
    )
    .await
    .unwrap();
    let row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(&series_id))
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.external_id, "5555");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_external_id_writes_canonical_url_when_template_known() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let series_id = Uuid::now_v7().to_string();

    writers::set_external_id(
        &db,
        "series",
        &series_id,
        &Identifier::new(Source::Metron, "42"),
        SetBy::User,
    )
    .await
    .unwrap();

    let row = ExternalId::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::Source.eq("metron"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.external_url.as_deref(),
        Some("https://metron.cloud/series/42/")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_issue_characters_reconciles_and_rebuilds_csv() {
    let app = TestApp::spawn().await;
    let (db, issue_id, _series_id, _lib_id) = seed_minimal_issue(&app).await;

    // Upsert two character entities.
    let mark = writers::upsert_character(
        &db,
        "Invincible",
        &[Identifier::new(Source::ComicVine, "200")],
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();
    let atom_eve = writers::upsert_character(&db, "Atom Eve", &[], SetBy::ComicInfo)
        .await
        .unwrap();

    let batch = CsvRebuildBatch::new();
    writers::set_issue_characters(
        &db,
        &issue_id,
        vec![
            CharacterSpec::from((mark, true, false)),
            CharacterSpec::from((atom_eve, false, false)),
        ],
        SetBy::Provider(Source::ComicVine),
        Some("105347".into()),
        &batch,
    )
    .await
    .unwrap();

    // Junction rows exist, FK populated.
    let chars = entity::issue_character::Entity::find()
        .filter(entity::issue_character::Column::IssueId.eq(&issue_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(chars.len(), 2);
    assert!(chars.iter().all(|c| c.character_id.is_some()));
    let mark_row = chars.iter().find(|c| c.character_id == Some(mark)).unwrap();
    assert!(mark_row.is_first_appearance);

    // field_provenance row was written.
    let prov = FieldProvenance::find()
        .filter(entity::field_provenance::Column::EntityType.eq("issue"))
        .filter(entity::field_provenance::Column::EntityId.eq(&issue_id))
        .filter(entity::field_provenance::Column::Field.eq("characters"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prov.set_by, "comicvine");
    assert_eq!(prov.source_external_id.as_deref(), Some("105347"));

    // CSV cache hasn't been rebuilt yet (batch deferred); flush.
    let before = Issue::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(before.characters.is_none(), "no rebuild before flush");

    batch.flush(&db).await.unwrap();
    let after = Issue::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    // Alphabetical aggregation: "Atom Eve, Invincible".
    assert_eq!(after.characters.as_deref(), Some("Atom Eve, Invincible"));

    // Subsequent call with a smaller set REPLACES (reconcile semantics).
    let batch2 = CsvRebuildBatch::new();
    writers::set_issue_characters(
        &db,
        &issue_id,
        vec![CharacterSpec::from((mark, true, false))],
        SetBy::Provider(Source::ComicVine),
        Some("105347".into()),
        &batch2,
    )
    .await
    .unwrap();
    batch2.flush(&db).await.unwrap();
    let after2 = Issue::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after2.characters.as_deref(), Some("Invincible"));
    let chars_after = entity::issue_character::Entity::find()
        .filter(entity::issue_character::Column::IssueId.eq(&issue_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(chars_after.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csv_rebuild_batch_dedupes_queued_issues() {
    let batch = CsvRebuildBatch::new();
    batch.queue("issue-a");
    batch.queue("issue-a");
    batch.queue("issue-b");
    batch.queue("issue-a");
    let drained = batch.drain();
    assert_eq!(drained.len(), 2);
    assert!(batch.is_empty());
}
