//! A provider variant as the issue's primary cover (audit 2026-11 §5 #11):
//!
//!   - `POST /series/{s}/issues/{i}/cover/variant/{variant_id}` copies the
//!     variant into a new active `primary/0` `issue_cover` row (hashes
//!     computed at write time), pins `cover.primary` under the user tier,
//!     audits, and the cover thumbnail renders from it;
//!   - a later provider apply keeps the pick, DB-direct and sidecar alike
//!     (also when the sidecar apply was queued before the pick); only
//!     `override_user_edits` replaces it;
//!   - `DELETE …/cover/variant` reverts to the archive cover page and
//!     restores the provider primary the pick displaced;
//!   - admin-only; an unknown or foreign variant is a 404.
//!
//! Every test writes only to temp dirs; no real archive is touched.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{
    audit_log, field_provenance, issue, issue_cover, metadata_run, metadata_run_candidate,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::json;
use server::jobs::metadata_apply::apply_issue_inline;
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::GenericMetadata;
use server::metadata::writers::{self, CoverOverwritePolicy, CoverWrite};
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

// ───────── fixtures ─────────

/// One stored page with a PNG signature but no decodable image, so the
/// archive's own cover page cannot produce a thumbnail: a 200 from the
/// thumb route proves it rendered the chosen cover.
fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

/// A real, decodable PNG; `seed` varies the pixels.
fn png(seed: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(48, 64, |x, y| {
        image::Rgb([
            (x * 5 + seed) as u8,
            (y * 3 + seed * 7) as u8,
            ((x + y) * 2) as u8,
        ])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

struct Fixture {
    app: TestApp,
    _dir: TempDir,
    issue_id: String,
    series_slug: String,
    issue_slug: String,
}

impl Fixture {
    fn data_path(&self) -> PathBuf {
        self.app.state().cfg().data_path.clone()
    }
}

/// One issue in a library (writeback on or off), with the ComicVine issue
/// detail `payload` cached so an apply never opens a socket.
async fn fixture(writeback: bool, payload: &GenericMetadata) -> Fixture {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempfile::tempdir().unwrap();
    let db = &app.state().db;
    let mut lib = LibrarySeed::new(dir.path());
    if writeback {
        lib = lib.with_sidecar_writeback();
    }
    let lib_id = lib.insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(db)
        .await;
    server::metadata::cache::put(
        db,
        Source::ComicVine,
        server::metadata::cache::CacheEntity::Issue,
        "67890",
        payload,
    )
    .await
    .unwrap();
    let row = issue::Entity::find_by_id(&issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let series = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    Fixture {
        app,
        _dir: dir,
        issue_id,
        series_slug: series.slug,
        issue_slug: row.slug,
    }
}

fn provider_payload(cover_url: &str) -> GenericMetadata {
    GenericMetadata {
        title: Some("Chapter One".into()),
        cover_image_url: Some(cover_url.into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "67890",
            "issue",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("67890".into()),
        ..Default::default()
    }
}

/// A downloaded Metron variant row (file under `data_path`).
async fn seed_variant(f: &Fixture, label: &str, bytes: &[u8]) -> issue_cover::Model {
    let id = Uuid::now_v7();
    let rel = format!("thumbs/issues/{}/covers/{id}.png", f.issue_id);
    let on_disk = f.data_path().join(&rel);
    std::fs::create_dir_all(on_disk.parent().unwrap()).unwrap();
    std::fs::write(&on_disk, bytes).unwrap();
    issue_cover::ActiveModel {
        id: Set(id),
        issue_id: Set(f.issue_id.clone()),
        kind: Set("variant".into()),
        ordinal: Set(next_variant_ordinal(f).await),
        source_provider: Set(Some("metron".into())),
        source_external_id: Set(Some("5551".into())),
        source_url: Set(Some(format!("https://static.metron.cloud/{id}.png"))),
        variant_label: Set(Some(label.into())),
        variant_artist_person_id: Set(None),
        local_path: Set(rel),
        width: Set(Some(48)),
        height: Set(Some(64)),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(Utc::now().fixed_offset()),
        is_active: Set(true),
    }
    .insert(&f.app.state().db)
    .await
    .unwrap()
}

/// One past the issue's highest variant ordinal (the active slot is unique).
async fn next_variant_ordinal(f: &Fixture) -> i32 {
    issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&f.issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .all(&f.app.state().db)
        .await
        .unwrap()
        .iter()
        .map(|c| c.ordinal)
        .max()
        .unwrap_or(0)
        + 1
}

/// A provider primary written the way an apply writes it.
async fn seed_provider_primary(f: &Fixture, url: &str) -> Uuid {
    let ident = Identifier {
        source: Source::ComicVine,
        id: "67890".into(),
        url: None,
    };
    let bytes = png(1);
    writers::apply_cover(
        &f.app.state().db,
        &f.data_path(),
        CoverWrite {
            issue_id: &f.issue_id,
            kind: "primary",
            ordinal: 0,
            identifier: Some(&ident),
            source_url: Some(url),
            variant_label: None,
            variant_artist_person_id: None,
            bytes: &bytes,
            width: None,
            height: None,
            override_user_pin: false,
        },
        CoverOverwritePolicy::Always,
    )
    .await
    .unwrap()
    .expect("provider primary written")
}

async fn active_primary(f: &Fixture) -> Option<issue_cover::Model> {
    issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&f.issue_id))
        .filter(issue_cover::Column::Kind.eq("primary"))
        .filter(issue_cover::Column::Ordinal.eq(0))
        .filter(issue_cover::Column::IsActive.eq(true))
        .one(&f.app.state().db)
        .await
        .unwrap()
}

async fn cover_pinned(f: &Fixture) -> bool {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(&f.issue_id))
        .filter(field_provenance::Column::Field.eq("cover.primary"))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .one(&f.app.state().db)
        .await
        .unwrap()
        .is_some()
}

// ───────── HTTP ─────────

struct Authed {
    session: String,
    csrf: String,
}

/// The first registered user is the admin; later ones are plain users.
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
            .unwrap()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http_raw(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method.clone()).uri(path).header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

async fn http(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = http_raw(app, method, path, user).await;
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

fn pick_path(f: &Fixture, variant_id: Uuid) -> String {
    format!(
        "/api/series/{}/issues/{}/cover/variant/{variant_id}",
        f.series_slug, f.issue_slug
    )
}

fn revert_path(f: &Fixture) -> String {
    format!(
        "/api/series/{}/issues/{}/cover/variant",
        f.series_slug, f.issue_slug
    )
}

async fn audit_actions(f: &Fixture) -> Vec<String> {
    audit_log::Entity::find()
        .filter(audit_log::Column::TargetId.eq(&f.issue_id))
        .all(&f.app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect()
}

// ───────── endpoint ─────────

/// The variant becomes the hashed, pinned, audited primary; the gallery
/// reports the pick and the cover thumbnail renders from it.
#[tokio::test]
async fn variant_becomes_the_primary_cover() {
    let f = fixture(false, &provider_payload("https://covers.example.com/x.png")).await;
    let admin = register(&f.app, "admin@example.com").await;
    let variant_bytes = png(9);
    let variant = seed_variant(&f, "Cover B (Adam Hughes)", &variant_bytes).await;

    let (status, body) = http(&f.app, Method::POST, &pick_path(&f, variant.id), &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let primary = active_primary(&f).await.expect("active primary");
    assert_eq!(body["cover_id"], json!(primary.id));
    assert_ne!(primary.id, variant.id, "a copy, not the variant row");
    assert_eq!(primary.source_url, variant.source_url);
    assert_eq!(primary.source_provider.as_deref(), Some("metron"));
    assert_eq!(primary.variant_label, variant.variant_label);
    assert!(
        primary.phash.is_some() && primary.dhash.is_some() && primary.ahash.is_some(),
        "hashes computed at write time: {primary:?}"
    );
    assert_eq!(
        std::fs::read(f.data_path().join(&primary.local_path)).unwrap(),
        variant_bytes
    );
    assert!(
        cover_pinned(&f).await,
        "cover.primary pinned under the user tier"
    );
    // The variant itself stays in the gallery.
    let still = issue_cover::Entity::find_by_id(variant.id)
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(still.is_active);
    assert!(
        audit_actions(&f)
            .await
            .contains(&"admin.issue.cover.variant.set".to_owned())
    );

    let (status, covers) = http(
        &f.app,
        Method::GET,
        &format!("/api/issues/{}/covers", f.issue_id),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(covers["variant_cover_chosen"], json!(true));

    // The archive page is not decodable, so a cover thumbnail can only
    // come from the chosen image.
    let (status, thumb) = http_raw(
        &f.app,
        Method::GET,
        &format!("/issues/{}/pages/0/thumb", f.issue_id),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        image::load_from_memory(&thumb).is_ok(),
        "an encoded thumbnail"
    );

    // Picking again replaces the earlier pick: one chosen row, its file gone.
    let second = seed_variant(&f, "Cover C", &png(4)).await;
    let (status, _) = http(&f.app, Method::POST, &pick_path(&f, second.id), &admin).await;
    assert_eq!(status, StatusCode::OK);
    let now = active_primary(&f).await.unwrap();
    assert_eq!(now.source_url, second.source_url);
    assert!(
        issue_cover::Entity::find_by_id(primary.id)
            .one(&f.app.state().db)
            .await
            .unwrap()
            .is_none(),
        "the earlier pick is deleted, not parked"
    );
    assert!(!f.data_path().join(&primary.local_path).exists());
}

#[tokio::test]
async fn non_admin_is_forbidden_and_unknown_variants_are_404() {
    let f = fixture(false, &provider_payload("https://covers.example.com/x.png")).await;
    let admin = register(&f.app, "admin@example.com").await;
    let reader = register(&f.app, "reader@example.com").await;
    let variant = seed_variant(&f, "Cover B", &png(2)).await;

    let (status, _) = http(&f.app, Method::POST, &pick_path(&f, variant.id), &reader).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = http(&f.app, Method::DELETE, &revert_path(&f), &reader).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(active_primary(&f).await.is_none());

    let (status, body) = http(&f.app, Method::POST, &pick_path(&f, Uuid::now_v7()), &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "cover.variant_not_found");

    // A primary row is not a variant.
    let primary_id = seed_provider_primary(&f, "https://covers.example.com/p.png").await;
    let (status, _) = http(&f.app, Method::POST, &pick_path(&f, primary_id), &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!cover_pinned(&f).await);
}

/// Revert: the pick and its pin go, the provider primary it displaced is
/// active again, the gallery no longer reports a pick; audited. A second
/// revert changes nothing.
#[tokio::test]
async fn revert_restores_the_displaced_primary() {
    let f = fixture(false, &provider_payload("https://covers.example.com/x.png")).await;
    let admin = register(&f.app, "admin@example.com").await;
    let provider_id = seed_provider_primary(&f, "https://covers.example.com/p.png").await;
    let variant = seed_variant(&f, "Cover B", &png(3)).await;

    let (status, _) = http(&f.app, Method::POST, &pick_path(&f, variant.id), &admin).await;
    assert_eq!(status, StatusCode::OK);
    let pick = active_primary(&f).await.unwrap();
    assert_ne!(pick.id, provider_id);

    let (status, body) = http(&f.app, Method::DELETE, &revert_path(&f), &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reverted"], json!(true));
    assert!(!cover_pinned(&f).await);
    assert_eq!(active_primary(&f).await.map(|p| p.id), Some(provider_id));
    assert!(!f.data_path().join(&pick.local_path).exists());
    let actions = audit_actions(&f).await;
    assert!(actions.contains(&"admin.issue.cover.variant.clear".to_owned()));

    let (_, covers) = http(
        &f.app,
        Method::GET,
        &format!("/api/issues/{}/covers", f.issue_id),
        &admin,
    )
    .await;
    assert_eq!(covers["variant_cover_chosen"], json!(false));

    let (status, body) = http(&f.app, Method::DELETE, &revert_path(&f), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reverted"], json!(false));
    assert_eq!(active_primary(&f).await.map(|p| p.id), Some(provider_id));
}

/// Choosing an archive page as the cover drops a chosen variant, so the
/// thumbnail shows the page the admin picked.
#[tokio::test]
async fn page_pick_replaces_a_chosen_variant() {
    let f = fixture(false, &provider_payload("https://covers.example.com/x.png")).await;
    let admin = register(&f.app, "admin@example.com").await;
    let variant = seed_variant(&f, "Cover B", &png(6)).await;
    let (status, _) = http(&f.app, Method::POST, &pick_path(&f, variant.id), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cover_pinned(&f).await);

    let resp = f
        .app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!(
                    "/api/series/{}/issues/{}/cover",
                    f.series_slug, f.issue_slug
                ))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        admin.session, admin.csrf
                    ),
                )
                .header("X-CSRF-Token", &admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"page_index":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!cover_pinned(&f).await, "the variant pick is gone");
    assert!(active_primary(&f).await.is_none());
}

// ───────── provider applies keep the pick ─────────

fn args(run_id: Uuid, override_user_edits: bool) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal: 0,
        mode: ApplyMode::ReplaceAll,
        apply_cover: true,
        cover_overwrite_policy: CoverOverwritePolicy::Always,
        override_user_edits,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

async fn seed_issue_run(f: &Fixture) -> Uuid {
    let db = &f.app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("issue".into()),
        scope_entity_id: Set(Some(f.issue_id.clone())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
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
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set("67890".into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "issue"})),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

/// Pick a variant through the writer the endpoint uses.
async fn pick(f: &Fixture) -> issue_cover::Model {
    let variant = seed_variant(f, "Cover B", &png(5)).await;
    let bytes = std::fs::read(f.data_path().join(&variant.local_path)).unwrap();
    writers::set_user_primary_cover(
        &f.app.state().db,
        &f.data_path(),
        CoverWrite {
            issue_id: &f.issue_id,
            kind: "primary",
            ordinal: 0,
            identifier: None,
            source_url: variant.source_url.as_deref(),
            variant_label: variant.variant_label.as_deref(),
            variant_artist_person_id: None,
            bytes: &bytes,
            width: None,
            height: None,
            override_user_pin: false,
        },
    )
    .await
    .unwrap();
    active_primary(f).await.unwrap()
}

fn stubbed_cover_url() -> String {
    let url = format!("https://covers.example.com/{}.png", Uuid::now_v7());
    writers::stub_cover_fetch_for_tests(&url, png(7));
    url
}

#[tokio::test]
async fn db_direct_apply_keeps_the_pick_unless_overridden() {
    let url = stubbed_cover_url();
    let f = fixture(false, &provider_payload(&url)).await;
    let chosen = pick(&f).await;

    let run_id = seed_issue_run(&f).await;
    let outcome = apply_issue_inline(&f.app.state(), &f.issue_id, args(run_id, false))
        .await
        .expect("apply");
    assert!(!outcome.enqueued_rewrite, "DB-direct path");
    assert_eq!(outcome.cover_skipped_reason.as_deref(), Some("user_pinned"));
    assert_eq!(active_primary(&f).await.map(|p| p.id), Some(chosen.id));
    assert!(cover_pinned(&f).await);

    // The admin-only override replaces it; the pick, its pin and the
    // cover thumbnail rendered from it go.
    let thumb = server::library::thumbnails::cover_path(
        &f.data_path(),
        &f.issue_id,
        server::library::thumbnails::ThumbFormat::default(),
    );
    std::fs::create_dir_all(thumb.parent().unwrap()).unwrap();
    std::fs::write(&thumb, b"rendered from the pick").unwrap();
    let run_id = seed_issue_run(&f).await;
    let outcome = apply_issue_inline(&f.app.state(), &f.issue_id, args(run_id, true))
        .await
        .expect("apply");
    assert!(outcome.cover_replaced, "{outcome:?}");
    assert!(!thumb.exists(), "the pick's cover thumbnail is wiped");
    let now = active_primary(&f).await.unwrap();
    assert_eq!(now.source_url.as_deref(), Some(url.as_str()));
    assert!(!cover_pinned(&f).await);
    assert!(
        issue_cover::Entity::find_by_id(chosen.id)
            .one(&f.app.state().db)
            .await
            .unwrap()
            .is_none()
    );
}

/// Every queued `RewriteIssueSidecarsJob`, with the queue cleared.
async fn take_rewrite_jobs(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    all.values()
        .map(|blob| serde_json::from_str(blob).expect("job args"))
        .collect()
}

async fn run_job(app: &TestApp, job: RewriteIssueSidecarsJob) {
    server::jobs::rewrite_sidecars::handle(
        job,
        apalis::prelude::Data::new(app.state()),
        apalis::prelude::Attempt::new_with_value(1),
    )
    .await
    .expect("rewrite lands");
}

async fn sidecar_apply(
    f: &Fixture,
) -> (
    server::metadata::apply::ApplyOutcome,
    RewriteIssueSidecarsJob,
) {
    let run_id = seed_issue_run(f).await;
    let outcome = apply_issue_inline(&f.app.state(), &f.issue_id, args(run_id, false))
        .await
        .expect("apply");
    assert!(outcome.enqueued_rewrite, "writeback path");
    let mut jobs = take_rewrite_jobs(&f.app).await;
    assert_eq!(jobs.len(), 1);
    (outcome, jobs.remove(0))
}

/// Writeback library: the pick is an `issue_cover` row (the archive is
/// not involved), and the sidecar apply defers no cover over it.
#[tokio::test]
async fn sidecar_apply_keeps_the_pick() {
    let url = stubbed_cover_url();
    let f = fixture(true, &provider_payload(&url)).await;
    let chosen = pick(&f).await;

    let (outcome, job) = sidecar_apply(&f).await;
    assert!(!outcome.cover_deferred, "no provider cover over a pick");
    assert_eq!(outcome.cover_skipped_reason.as_deref(), Some("user_pinned"));
    run_job(&f.app, job).await;
    assert_eq!(active_primary(&f).await.map(|p| p.id), Some(chosen.id));
    assert!(cover_pinned(&f).await);
}

/// A sidecar apply queued *before* the pick still carries the provider
/// cover; the writer keeps the pick when the job runs.
#[tokio::test]
async fn sidecar_apply_queued_before_the_pick_keeps_it() {
    let url = stubbed_cover_url();
    let f = fixture(true, &provider_payload(&url)).await;

    let (outcome, job) = sidecar_apply(&f).await;
    assert!(outcome.cover_deferred);
    let chosen = pick(&f).await;
    run_job(&f.app, job).await;
    assert_eq!(active_primary(&f).await.map(|p| p.id), Some(chosen.id));
    assert!(cover_pinned(&f).await);
}
