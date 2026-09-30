//! WP-3.8 (audit OP-5 + DI-15): the `folio_thumbs_bytes` gauge and the
//! keyset-paged variant-cover backfill.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::issue_cover;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

async fn scrape_metrics(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn cover_row(
    issue_id: &str,
    kind: &str,
    ordinal: i32,
    local_path: &str,
) -> issue_cover::ActiveModel {
    issue_cover::ActiveModel {
        id: Set(Uuid::now_v7()),
        issue_id: Set(issue_id.to_owned()),
        kind: Set(kind.into()),
        ordinal: Set(ordinal),
        source_provider: Set(Some("comicvine".into())),
        source_external_id: Set(None),
        source_url: Set(Some("http://127.0.0.1:1/c.png".into())),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set(local_path.to_owned()),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(Utc::now().fixed_offset()),
        is_active: Set(true),
    }
}

async fn seed_issue(app: &TestApp, dir: &std::path::Path) -> String {
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir).insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    IssueSeed::new(lib_id, series_id, &dir.join("saga-1.cbz"), b"x", 1.0)
        .insert(db)
        .await
}

/// Done-when: the gauge is visible at `/metrics`, and the sweep counts
/// generated thumbs + downloaded provider covers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumbs_bytes_gauge_is_exported_at_metrics() {
    let app = TestApp::spawn().await;
    let data = app.state().cfg().data_path.clone();
    let thumbs = data.join("thumbs");
    std::fs::create_dir_all(thumbs.join("abc/s")).unwrap();
    std::fs::create_dir_all(thumbs.join("issues/abc/covers")).unwrap();
    std::fs::write(thumbs.join("abc.webp"), vec![0u8; 1000]).unwrap();
    std::fs::write(thumbs.join("abc/s/1.webp"), vec![0u8; 200]).unwrap();
    std::fs::write(thumbs.join("issues/abc/covers/x.jpg"), vec![0u8; 30]).unwrap();

    let out = server::jobs::orphan_sweep::run_budget_sweep(&app.state())
        .await
        .unwrap();
    assert_eq!(out.total_after, 1230);
    assert_eq!(out.protected_bytes, 30);
    assert_eq!(out.evicted_files, 0, "budget off by default → no eviction");

    let body = scrape_metrics(&app).await;
    assert!(
        body.lines()
            .any(|l| l.starts_with("folio_thumbs_bytes ") || l.starts_with("folio_thumbs_bytes{")),
        "missing folio_thumbs_bytes gauge:\n{body}"
    );
}

/// DI-15: the variant-cover backfill pages by id, so a full first page of
/// already-stored rows no longer hides the recoverable rows behind it.
#[tokio::test]
async fn variant_cover_backfill_pages_past_a_full_page_of_stored_rows() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let issue_id = seed_issue(&app, dir.path()).await;
    let data = app.state().cfg().data_path.clone();
    let stored_rel = format!("thumbs/issues/{issue_id}/covers/stored.jpg");
    std::fs::create_dir_all(data.join(format!("thumbs/issues/{issue_id}/covers"))).unwrap();
    std::fs::write(data.join(&stored_rel), b"img").unwrap();

    let cap = server::metadata::writers::VARIANT_BACKFILL_BATCH_CAP as i32;
    let db = &app.state().db;
    // A full page of rows whose bytes are already on disk (explicit
    // ascending ids so the page boundary is deterministic)…
    let rows: Vec<issue_cover::ActiveModel> = (1..=cap)
        .map(|i| {
            let mut r = cover_row(&issue_id, "variant", i, &stored_rel);
            r.id = Set(Uuid::from_u128(i as u128));
            r
        })
        .collect();
    issue_cover::Entity::insert_many(rows)
        .exec(db)
        .await
        .unwrap();
    // …then one row with no local artifact behind it.
    let mut trailing = cover_row(&issue_id, "variant", cap + 1, "");
    trailing.id = Set(Uuid::from_u128(cap as u128 + 1));
    trailing.insert(db).await.unwrap();

    let (first, next) = server::metadata::writers::run_variant_cover_backfill_page(db, &data, None)
        .await
        .unwrap();
    assert_eq!(first.considered, 0, "first page is all already-stored");
    let next = next.expect("a full page yields a next cursor");

    let (second, end) =
        server::metadata::writers::run_variant_cover_backfill_page(db, &data, Some(next))
            .await
            .unwrap();
    assert_eq!(second.considered, 1, "the trailing row is reached");
    assert_eq!(second.skipped, 1, "its SSRF-rejected URL stays a hotlink");
    assert!(end.is_none(), "short page ends the walk");
}
