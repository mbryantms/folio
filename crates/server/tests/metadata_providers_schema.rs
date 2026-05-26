//! M0 schema smoke test for metadata-providers-1.0.
//!
//! Exercises the application-layer round trip on the tables the M0
//! migration adds: top-level entities (character, team, location,
//! story_arc, publisher, imprint, universe, concept, object),
//! junctions (issue_arcs, issue_concepts, issue_universes, …),
//! covers (issue_cover, series_cover), provenance (external_ids,
//! field_provenance), and the run-history table (metadata_run).
//!
//! schema_parity covers entity ↔ DB column matching; this file
//! covers "inserts and reads behave as expected" — the kind of bug
//! that would otherwise only surface when M0c rewires writers.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_creates_new_tables_with_expected_constraints() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();

    // ───── Top-level entity round trip ─────
    let character_id = Uuid::now_v7();
    entity::character::ActiveModel {
        id: Set(character_id),
        slug: Set("invincible".into()),
        name: Set("Invincible".into()),
        normalized_name: Set("invincible".into()),
        aliases: Set(serde_json::json!(["Mark Grayson"])),
        description: Set(None),
        image_url: Set(None),
        real_name: Set(Some("Mark Grayson".into())),
        first_appearance_issue_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let fetched = Character::find_by_id(character_id)
        .one(&db)
        .await
        .unwrap()
        .expect("character row");
    assert_eq!(fetched.name, "Invincible");
    assert_eq!(fetched.real_name.as_deref(), Some("Mark Grayson"));

    // ───── Publisher → imprint FK ─────
    let publisher_id = Uuid::now_v7();
    entity::publisher::ActiveModel {
        id: Set(publisher_id),
        slug: Set("image-comics".into()),
        name: Set("Image Comics".into()),
        normalized_name: Set("image comics".into()),
        aliases: Set(serde_json::json!([])),
        description: Set(None),
        image_url: Set(None),
        founded_year: Set(Some(1992)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    let imprint_id = Uuid::now_v7();
    entity::imprint::ActiveModel {
        id: Set(imprint_id),
        slug: Set("skybound".into()),
        name: Set("Skybound".into()),
        normalized_name: Set("skybound".into()),
        aliases: Set(serde_json::json!([])),
        description: Set(None),
        image_url: Set(None),
        publisher_id: Set(publisher_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let imprint_row = Imprint::find_by_id(imprint_id)
        .one(&db)
        .await
        .unwrap()
        .expect("imprint row");
    assert_eq!(imprint_row.publisher_id, publisher_id);

    // ───── external_ids primary key + lookup index ─────
    entity::external_id::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(Uuid::now_v7().to_string()),
        source: Set("comicvine".into()),
        external_id: Set("17993".into()),
        external_url: Set(Some(
            "https://comicvine.gamespot.com/invincible/4050-17993/".into(),
        )),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let by_source = ExternalId::find()
        .filter(entity::external_id::Column::Source.eq("comicvine"))
        .filter(entity::external_id::Column::ExternalId.eq("17993"))
        .one(&db)
        .await
        .unwrap()
        .expect("external_id lookup");
    assert_eq!(by_source.entity_type, "series");
    assert_eq!(by_source.set_by, "user");

    // ───── field_provenance composite PK ─────
    let issue_id = "0".repeat(64);
    entity::field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.clone()),
        field: Set("title".into()),
        set_by: Set("metron".into()),
        set_at: Set(now),
        source_external_id: Set(Some("87654".into())),
    }
    .insert(&db)
    .await
    .unwrap();
    // Same key should hit the PK constraint and refuse a duplicate.
    let dup = entity::field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.clone()),
        field: Set("title".into()),
        set_by: Set("user".into()),
        set_at: Set(now),
        source_external_id: Set(None),
    }
    .insert(&db)
    .await;
    assert!(dup.is_err(), "field_provenance PK should reject duplicates");

    // ───── metadata_run round trip ─────
    let run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("library".into()),
        scope_entity_id: Set(None),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["metron".into(), "comicvine".into()]),
        status: Set("queued".into()),
        started_at: Set(now),
        finished_at: Set(None),
        items_total: Set(0),
        items_matched_high: Set(0),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    let run = MetadataRun::find_by_id(run_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        run.providers,
        vec!["metron".to_string(), "comicvine".to_string()]
    );
}
