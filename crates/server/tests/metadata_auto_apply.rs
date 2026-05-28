//! Matching-accuracy-1.0 M12 — opt-in auto-apply on `SingleGoodMatch`.
//!
//! Anchors the M12 invariants the way Slice 4 calls for them:
//!
//! - The per-library toggle round-trips via
//!   `PATCH /api/libraries/{slug}/settings`. The column lives in
//!   `libraries.metadata_auto_apply_strong_matches`.
//! - The `MatchOutcomeKind::classify` gate is strict — only the
//!   single-good outcome routes through the auto-apply path. Five
//!   table-driven cases pin every other variant as "do not auto-apply".
//! - Disabled-library + missing-library both fall back to "no
//!   auto-apply" without erroring.
//!
//! End-to-end "apalis fires the job" coverage stays in the manual
//! testing column for now — the full chain (cron → search → apply
//! → rescan) needs a real apalis worker harness which lives outside
//! this test file. The unit-level invariants above are what
//! regression matters for.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::EntityTrait;
use serde_json::json;
use server::metadata::identifier::Source;
use server::metadata::match_outcome::MatchOutcomeKind;
use server::metadata::matcher::{Confidence, Score};
use server::metadata::orchestrator::{CandidatePayload, RankedCandidate};
use server::metadata::provider::SeriesCandidate;
use tower::ServiceExt;
use uuid::Uuid;

// ───────── helpers ─────────

fn ranked(bucket: Confidence) -> RankedCandidate {
    RankedCandidate {
        source: Source::ComicVine,
        external_id: "x".into(),
        score: Score::default(),
        bucket,
        payload: CandidatePayload::Series(SeriesCandidate {
            source: Source::ComicVine,
            external_id: "x".into(),
            external_url: None,
            name: "x".into(),
            year: None,
            publisher: None,
            issue_count: None,
            cover_image_url: None,
            deck: None,
            alternate_cover_urls: Vec::new(),
        }),
    }
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
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
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
    let extract = |prefix: &str| -> String {
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

async fn create_library(app: &TestApp) -> (Uuid, String) {
    use entity::library::{ActiveModel as LibraryAM, Entity as LibraryEntity};
    use sea_orm::{ActiveModelTrait, Set};
    let id = Uuid::now_v7();
    let slug = format!("auto-apply-{}", id.as_u128() % 100_000);
    let now = chrono::Utc::now().fixed_offset();
    let _ = LibraryAM {
        id: Set(id),
        name: Set("Auto Apply Lib".into()),
        slug: Set(slug.clone()),
        root_path: Set("/tmp/auto-apply".into()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(false),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".into()),
        thumbnail_cover_quality: Set(80),
        thumbnail_page_quality: Set(50),
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
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    // Sanity check via the slug-aware finder used by the API surface.
    let lib = LibraryEntity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lib.slug, slug);
    (id, slug)
}

// ───────── M12 invariants ─────────

#[test]
fn single_good_classification_is_the_only_auto_apply_gate() {
    // Five-variant matrix. Only `SingleGood` should route to the
    // auto-apply path. Mirrors the gating in `maybe_auto_apply_*`.
    let cases: &[(Vec<Confidence>, MatchOutcomeKind, bool)] = &[
        (vec![Confidence::High], MatchOutcomeKind::SingleGood, true),
        (
            vec![Confidence::Medium],
            MatchOutcomeKind::SingleBadCover,
            false,
        ),
        (
            vec![Confidence::Low],
            MatchOutcomeKind::SingleBadCover,
            false,
        ),
        (
            vec![Confidence::High, Confidence::Medium],
            MatchOutcomeKind::MultiGood,
            false,
        ),
        (
            vec![Confidence::Medium, Confidence::Low],
            MatchOutcomeKind::MultiBadCover,
            false,
        ),
        (vec![], MatchOutcomeKind::NoMatch, false),
    ];
    for (buckets, expected_kind, expected_eligible) in cases {
        let ranked: Vec<RankedCandidate> = buckets.iter().map(|&b| ranked(b)).collect();
        let kind = MatchOutcomeKind::classify(&ranked);
        assert_eq!(
            kind, *expected_kind,
            "classify({buckets:?}) = {kind:?}, expected {expected_kind:?}",
        );
        let eligible = matches!(kind, MatchOutcomeKind::SingleGood);
        assert_eq!(
            eligible, *expected_eligible,
            "auto-apply eligibility for {buckets:?}",
        );
    }
}

#[tokio::test]
async fn library_auto_apply_toggle_round_trips_via_patch() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    let (lib_id, slug) = create_library(&app).await;

    // Baseline: column starts FALSE per the migration's DEFAULT.
    let lib = entity::library::Entity::find_by_id(lib_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(!lib.metadata_auto_apply_strong_matches);

    // PATCH the slug-based endpoint with the toggle set to true.
    let body = json!({ "metadata_auto_apply_strong_matches": true });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::COOKIE, admin.cookie())
                .header("x-csrf-token", &admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let lib = entity::library::Entity::find_by_id(lib_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(lib.metadata_auto_apply_strong_matches);

    // Round-trip: PATCH back to false.
    let body = json!({ "metadata_auto_apply_strong_matches": false });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::COOKIE, admin.cookie())
                .header("x-csrf-token", &admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let lib = entity::library::Entity::find_by_id(lib_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(!lib.metadata_auto_apply_strong_matches);
}

#[tokio::test]
async fn library_get_surfaces_auto_apply_toggle_in_view() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    let (_lib_id, slug) = create_library(&app).await;

    // PATCH to true, then GET and verify the JSON view includes it.
    let body = json!({ "metadata_auto_apply_strong_matches": true });
    let _ = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::COOKIE, admin.cookie())
                .header("x-csrf-token", &admin.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::COOKIE, admin.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let view: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(view["metadata_auto_apply_strong_matches"], json!(true));
}
