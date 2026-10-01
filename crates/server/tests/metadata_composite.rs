//! Composite (multi-provider) merge integration tests.
//!
//! Seeds a run with TWO candidates from different providers (ComicVine +
//! Metron) whose cached details populate DIFFERENT fields, then drives
//! `composite::compute_composite_diff` + `composite::apply_composite`
//! directly (DB-direct library) and asserts:
//! - the comparison surfaces each provider's proposals + the policy's
//!   chosen source per field,
//! - the apply merges fields from different providers in one operation,
//! - per-field `field_provenance.set_by` records the TRUE contributing
//!   provider,
//! - external IDs from BOTH providers land (additive union).

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{external_id, field_provenance, issue, metadata_run, metadata_run_candidate};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::json;
use server::metadata::apply::ApplyMode;
use server::metadata::cache::{self, CacheEntity};
use server::metadata::composite::{self, CompositeApplyArgs};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::{EntityCandidate, GenericMetadata};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};
use tempfile::tempdir;
use uuid::Uuid;

fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        // PNG-signed so the archive crate's open-time content sniff keeps
        // the entry as a page; the label keeps each fixture distinct.
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn entity(name: &str) -> EntityCandidate {
    EntityCandidate {
        name: name.into(),
        identifiers: vec![],
        is_first_appearance: false,
        died_in_issue: None,
        disbanded_in_issue: None,
        position_in_arc: None,
    }
}

fn cv_detail() -> GenericMetadata {
    GenericMetadata {
        title: Some("Saga".into()),
        description: Some("A sweeping space opera.".into()),
        age_rating: Some("Teen".into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "cv1",
            "issue",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("cv1".into()),
        ..Default::default()
    }
}

fn metron_detail() -> GenericMetadata {
    GenericMetadata {
        title: Some("Saga".into()),
        sku: Some("75960620001".into()),
        characters: vec![entity("Hazel"), entity("Marko"), entity("Alana")],
        identifiers: vec![
            Identifier::with_canonical_url(Source::Metron, "m1", "issue"),
            Identifier::with_canonical_url(Source::ComicVine, "cv1", "issue"),
        ],
        source_provider: Some(Source::Metron),
        source_external_id: Some("m1".into()),
        ..Default::default()
    }
}

fn issue_candidate_json(source: &str, external_id: &str) -> serde_json::Value {
    json!({
        "source": source,
        "external_id": external_id,
        "external_url": null,
        "issue_number": "1",
        "name": "Saga #1",
        "cover_date": null,
        "series_name": "Saga",
        "series_year": 2012,
        "series_external_id": null,
        "cover_image_url": null,
    })
}

/// Seed a completed issue-scope run with two candidates (CV ord 0,
/// Metron ord 1) + cache their details. Returns the run id.
async fn seed_two_provider_run(app: &TestApp, issue_id: &str) -> Uuid {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(issue_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into(), "metron".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();

    for (ordinal, source, ext) in [(0, "comicvine", "cv1"), (1, "metron", "m1")] {
        metadata_run_candidate::ActiveModel {
            run_id: Set(run_id),
            ordinal: Set(ordinal),
            source: Set(source.into()),
            external_id: Set(ext.into()),
            bucket: Set("high".into()),
            score: Set(90.0),
            score_breakdown: Set(json!({})),
            candidate: Set(issue_candidate_json(source, ext)),
            applied_at: Set(None),
        }
        .insert(db)
        .await
        .unwrap();
    }

    cache::put(
        db,
        Source::ComicVine,
        CacheEntity::Issue,
        "cv1",
        &cv_detail(),
    )
    .await
    .unwrap();
    cache::put(
        db,
        Source::Metron,
        CacheEntity::Issue,
        "m1",
        &metron_detail(),
    )
    .await
    .unwrap();
    run_id
}

async fn setup() -> (TestApp, String, Uuid) {
    let app = TestApp::spawn_with_providers("cv-key", "metron-user", "metron-pass").await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = build_cbz_bytes("saga-1");
    let issue_id = IssueSeed::new(lib_id, series_id, &dir.path().join("saga-1.cbz"), &cbz, 1.0)
        .insert(&app.state().db)
        .await;
    let run_id = seed_two_provider_run(&app, &issue_id).await;
    (app, issue_id, run_id)
}

#[tokio::test]
async fn composite_diff_surfaces_both_providers_and_policy_choice() {
    let (app, _issue_id, run_id) = setup().await;
    // Empty include → default best (lowest-ordinal) candidate per
    // provider: ComicVine (ordinal 0) + Metron (ordinal 1).
    let resp =
        composite::compute_composite_diff(&app.state(), run_id, ApplyMode::FillMissing, false, &[])
            .await
            .expect("composite diff");

    assert_eq!(resp.providers.len(), 2, "both candidates shown as columns");

    let row = |key: &str| resp.rows.iter().find(|r| r.field == key).unwrap();

    // description: only ComicVine has it → chosen ComicVine (ordinal 0).
    let desc = row("description");
    assert_eq!(desc.chosen_ordinal, Some(0));
    assert!(
        desc.proposals
            .iter()
            .any(|p| p.source == "comicvine" && p.value.is_some())
    );
    assert!(
        desc.proposals
            .iter()
            .any(|p| p.source == "metron" && p.value.is_none())
    );

    // characters: only Metron has them (richer) → chosen Metron (ordinal 1).
    assert_eq!(row("characters").chosen_ordinal, Some(1));

    // title: both have it → default preference (Metron first) wins → ordinal 1.
    assert_eq!(row("title").chosen_ordinal, Some(1));

    // Both providers' external IDs are additive new rows (issue had none).
    let new_sources: HashSet<&str> = resp
        .external_ids_new
        .iter()
        .map(|n| n.source.as_str())
        .collect();
    assert!(new_sources.contains("comicvine"));
    assert!(new_sources.contains("metron"));
}

#[tokio::test]
async fn composite_apply_merges_fields_with_true_per_provider_provenance() {
    let (app, issue_id, run_id) = setup().await;
    let db = &app.state().db;

    // Pick description+age_rating from ComicVine (ordinal 0),
    // characters+sku+title from Metron (ordinal 1).
    let field_sources: HashMap<String, i32> = [
        ("description".to_string(), 0),
        ("age_rating".to_string(), 0),
        ("characters".to_string(), 1),
        ("sku".to_string(), 1),
        ("title".to_string(), 1),
    ]
    .into_iter()
    .collect();

    let outcome = composite::apply_composite(
        &app.state(),
        CompositeApplyArgs {
            run_id,
            field_sources,
            included: vec![0, 1],
            // ReplaceAll so the merge overwrites the seeded title with
            // Metron's value (FillMissing would correctly leave the
            // existing "Issue 1" untouched).
            mode: ApplyMode::ReplaceAll,
            apply_cover: false,
            cover_overwrite_policy: server::metadata::writers::CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            override_external_id_sources: HashSet::new(),
            actor_id: None,
        },
    )
    .await
    .expect("composite apply");
    assert!(!outcome.applied_fields.is_empty());

    // Merged values landed.
    let row = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.summary.as_deref(), Some("A sweeping space opera.")); // from CV
    assert_eq!(row.age_rating.as_deref(), Some("Teen")); // from CV
    assert_eq!(row.sku.as_deref(), Some("75960620001")); // from Metron
    assert_eq!(row.title.as_deref(), Some("Saga")); // from Metron
    let chars = row.characters.as_deref().unwrap_or("");
    assert!(
        chars.contains("Hazel") && chars.contains("Marko"),
        "characters from Metron: {chars}"
    );

    // Per-field provenance reflects the TRUE provider per field.
    let prov: HashMap<String, String> = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(&issue_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|p| (p.field, p.set_by))
        .collect();
    assert_eq!(
        prov.get("description").map(String::as_str),
        Some("comicvine")
    );
    assert_eq!(
        prov.get("age_rating").map(String::as_str),
        Some("comicvine")
    );
    assert_eq!(prov.get("characters").map(String::as_str), Some("metron"));

    // Both providers' external IDs were written (additive union).
    let ext: HashSet<String> = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("issue"))
        .filter(external_id::Column::EntityId.eq(&issue_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.source)
        .collect();
    assert!(ext.contains("comicvine"), "CV id present: {ext:?}");
    assert!(ext.contains("metron"), "Metron id present: {ext:?}");
}

#[tokio::test]
async fn composite_apply_excludes_dropped_provider() {
    let (app, issue_id, run_id) = setup().await;
    let db = &app.state().db;

    // Only Metron (ordinal 1) included; pick characters from Metron.
    // ComicVine's description must NOT land.
    let field_sources: HashMap<String, i32> = [("characters".to_string(), 1)].into_iter().collect();
    composite::apply_composite(
        &app.state(),
        CompositeApplyArgs {
            run_id,
            field_sources,
            included: vec![1],
            mode: ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: server::metadata::writers::CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            override_external_id_sources: HashSet::new(),
            actor_id: None,
        },
    )
    .await
    .expect("composite apply");

    let row = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.summary.is_none(), "CV description excluded");
    assert!(
        row.characters.as_deref().unwrap_or("").contains("Hazel"),
        "Metron characters applied",
    );
}

// ───────── SE-3 / WP-6.3: override gate + `_force` audit on composite ─────────

/// Register `email`; returns `(cookie header, csrf token)`.
async fn register_cookie(app: &TestApp, email: &str) -> (String, String) {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use tower::ServiceExt;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
                )))
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
        .map(|c| c.split(';').next().unwrap_or("").to_owned())
        .collect();
    let csrf = cookies
        .iter()
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .expect("csrf cookie")
        .to_owned();
    (cookies.join("; "), csrf)
}

async fn composite_apply_http(
    app: &TestApp,
    (cookie, csrf): &(String, String),
    issue_id: &str,
    run_id: Uuid,
    override_user_edits: bool,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;
    use axum::http::{Method, Request, header};
    use tower::ServiceExt;
    let row = issue::Entity::find_by_id(issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let body = json!({
        "run_id": run_id,
        "field_sources": [{"field": "description", "ordinal": 0}],
        "included": [0],
        "apply_cover": false,
        "override_user_edits": override_user_edits,
    });
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/series/{}/issues/{}/metadata/composite-apply",
                    row.series_id, row.slug
                ))
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn composite_apply_override_is_admin_gated_structurally() {
    use axum::http::StatusCode;
    let (app, issue_id, run_id) = setup().await;
    let _admin = register_cookie(&app, "admin@example.com").await;
    let user = register_cookie(&app, "user@example.com").await;
    // Give the user library access so only the override flag is in play.
    let db = &app.state().db;
    let row = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let user_row = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("user@example.com"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let now = Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
        user_id: Set(user_row.id),
        library_id: Set(row.library_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    let resp = composite_apply_http(&app, &user, &issue_id, run_id, true).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "auth.permission_denied");
    // Nothing was applied.
    let after = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.summary, row.summary);
}

#[tokio::test]
async fn composite_force_apply_audits_as_force_and_counts_on_the_dashboard() {
    use axum::http::StatusCode;
    let (app, issue_id, run_id) = setup().await;
    let admin = register_cookie(&app, "admin@example.com").await;

    let resp = composite_apply_http(&app, &admin, &issue_id, run_id, true).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let actions: Vec<String> = entity::audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert!(
        actions
            .iter()
            .any(|a| a == "admin.issue.metadata_composite_apply_force"),
        "override must audit under the `_force` action: {actions:?}"
    );
    assert!(
        !actions
            .iter()
            .any(|a| a == "admin.issue.metadata_composite_apply"),
        "{actions:?}"
    );
    assert!(
        server::api::admin_metadata::MANUAL_APPLY_ACTIONS
            .contains(&"admin.issue.metadata_composite_apply_force")
    );

    // The dashboard's 7-day counter includes it.
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/admin/metadata/dashboard")
                .header(header::COOKIE, &admin.0)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["applies_last_7_days"], 1, "{body}");
}
