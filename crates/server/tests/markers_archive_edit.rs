//! Per-user page anchors (markers + reading progress) must follow their
//! pages through the archive page editor (roadmap WP-1.2, audit DI-19).
//!
//! Before this, `jobs::archive_edit` rewrote the page list and enqueued
//! a rescan without touching `markers.page_index` or
//! `progress_records.last_page`, so every anchor after an edited ordinal
//! silently pointed at different pixels.

mod common;

use archive::ArchiveLimits;
use archive::cbz::Cbz;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed, seed_progress};
use entity::{marker, progress_record};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::archive_edit::{ArchiveEditJob, PageOp, Rot, edit_one_issue};
use server::reading::page_remap::{PAGE_REMOVED_TAG, PageMap, remap_issue_anchors};
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

fn png_bytes(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let dynimg = image::DynamicImage::ImageRgb8(img);
    let mut buf = Cursor::new(Vec::new());
    dynimg.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

fn build_cbz(pages: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in pages {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(bytes).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn four_page_cbz() -> Vec<u8> {
    build_cbz(&[
        ("p1.png", png_bytes(4, 4, [10, 0, 0])),
        ("p2.png", png_bytes(4, 4, [0, 20, 0])),
        ("p3.png", png_bytes(4, 4, [0, 0, 30])),
        ("p4.png", png_bytes(4, 4, [40, 40, 0])),
    ])
}

/// Register a user over HTTP (markers need a real `users` row).
async fn register_user(app: &TestApp) -> Uuid {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"reader@example.com","password":"correctly-horse-battery-staple"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap()
}

/// Writeback-enabled library + series + one 4-page CBZ issue, one user,
/// one marker per page (bookmark, note, highlight, favorite) and a
/// progress row parked on page 2.
async fn seed(app: &TestApp, dir: &Path) -> (String, Uuid, Uuid, std::path::PathBuf) {
    let db = &app.state().db;
    let lib = LibrarySeed::new(dir)
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series = SeriesSeed::new(lib, "Anchored").insert(db).await;
    let path = dir.join("issue.cbz");
    let issue_id = IssueSeed::new(lib, series, &path, &four_page_cbz(), 1.0)
        .with_page_count(4)
        .insert(db)
        .await;
    let user_id = register_user(app).await;

    let now = Utc::now().fixed_offset();
    let kinds: [(&str, i32); 4] = [
        ("bookmark", 0),
        ("note", 1),
        ("highlight", 2),
        ("favorite", 3),
    ];
    for (kind, page) in kinds {
        marker::ActiveModel {
            id: Set(Uuid::now_v7()),
            user_id: Set(user_id),
            series_id: Set(series),
            issue_id: Set(issue_id.clone()),
            page_index: Set(page),
            kind: Set(kind.into()),
            is_favorite: Set(false),
            tags: Set(vec![]),
            region: Set((kind == "highlight").then(
                || serde_json::json!({"x": 10.0, "y": 10.0, "w": 20.0, "h": 20.0, "shape": "rect"}),
            )),
            selection: Set(None),
            body: Set((kind == "note").then(|| "a note".to_owned())),
            color: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            hidden_from_log: Set(false),
            page_hash: Set(None),
        }
        .insert(db)
        .await
        .unwrap();
    }
    seed_progress(db, user_id, &issue_id, 2, 0.5, false).await;
    (issue_id, user_id, series, path)
}

fn job(issue_id: &str, ops: Vec<PageOp>) -> ArchiveEditJob {
    ArchiveEditJob {
        issue_id: issue_id.to_owned(),
        ops,
        bulk_op: None,
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
        attempt: 0,
    }
}

/// `kind → (page_index, tags)` for the issue's markers.
async fn marker_state(app: &TestApp, issue_id: &str) -> Vec<(String, i32, Vec<String>)> {
    let mut rows: Vec<_> = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.kind, m.page_index, m.tags))
        .collect();
    rows.sort();
    rows
}

async fn progress_state(app: &TestApp, user_id: Uuid, issue_id: &str) -> (i32, f64) {
    let p = progress_record::Entity::find_by_id((user_id, issue_id.to_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    (p.last_page, p.percent)
}

#[tokio::test]
async fn remove_and_reorder_moves_markers_and_progress_with_their_pages() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (issue_id, user_id, _series, path) = seed(&app, dir.path()).await;

    // Remove ordinal 1 (the note's page) → [p1,p3,p4]; reorder [2,0,1]
    // → [p4,p1,p3]. Old→new: 0→1, 1→removed, 2→2, 3→0.
    let ops = vec![
        PageOp::Remove { ordinal: 1 },
        PageOp::Reorder {
            new_order: vec![2, 0, 1],
        },
    ];
    let res = edit_one_issue(&app.state(), &job(&issue_id, ops))
        .await
        .unwrap();
    assert_eq!(res.page_count_after, 3);
    // bookmark 0→1 and favorite 3→0 move; the highlight stays at 2; the
    // note's page was removed and it lands on new ordinal 1 — the same
    // number it had, so it is orphaned (tagged) but not counted as moved.
    assert_eq!(res.anchors.markers_moved, 2, "{:?}", res.anchors);
    assert_eq!(res.anchors.markers_orphaned, 1);
    assert_eq!(res.anchors.progress_moved, 1);

    // Sanity: the archive really is [p4, p1, p3] now.
    let c = Cbz::open(&path, ArchiveLimits::default()).unwrap();
    assert_eq!(c.pages().len(), 3);
    drop(c);

    let removed = vec![PAGE_REMOVED_TAG.to_owned()];
    assert_eq!(
        marker_state(&app, &issue_id).await,
        vec![
            ("bookmark".to_owned(), 1, vec![]),
            ("favorite".to_owned(), 0, vec![]),
            ("highlight".to_owned(), 2, vec![]),
            // The note's page is gone: it lands on the nearest surviving
            // page below (old 0 → new 1) and is tagged.
            ("note".to_owned(), 1, removed),
        ]
    );
    // Progress was on old page 2 (p3), which is new page 2; percent is
    // re-based on the new page count.
    let (last, pct) = progress_state(&app, user_id, &issue_id).await;
    assert_eq!(last, 2);
    assert!((pct - 2.0 / 3.0).abs() < 1e-9, "percent {pct}");
}

#[tokio::test]
async fn rotate_only_edit_leaves_anchors_untouched() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (issue_id, user_id, _series, _path) = seed(&app, dir.path()).await;
    let before_markers = marker_state(&app, &issue_id).await;
    let before_progress = progress_state(&app, user_id, &issue_id).await;

    let res = edit_one_issue(
        &app.state(),
        &job(
            &issue_id,
            vec![PageOp::Rotate {
                ordinal: 1,
                degrees: Rot::R90,
            }],
        ),
    )
    .await
    .unwrap();
    assert_eq!(res.anchors, Default::default());
    assert_eq!(marker_state(&app, &issue_id).await, before_markers);
    assert_eq!(
        progress_state(&app, user_id, &issue_id).await,
        before_progress
    );
}

/// The scanner applies a truncation map when a rescan finds fewer pages
/// than before (an external replacement). Exercise the same helper the
/// scanner calls: anchors past the new end pull onto the last page.
#[tokio::test]
async fn truncation_pulls_trailing_anchors_onto_the_last_page() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (issue_id, user_id, _series, _path) = seed(&app, dir.path()).await;

    let map = PageMap::truncation(4, 2);
    let o = remap_issue_anchors(&app.state().db, &issue_id, &map)
        .await
        .unwrap();
    assert_eq!(o.markers_moved, 2, "{o:?}");
    assert_eq!(o.markers_orphaned, 2);
    assert_eq!(o.progress_moved, 1);

    let removed = vec![PAGE_REMOVED_TAG.to_owned()];
    assert_eq!(
        marker_state(&app, &issue_id).await,
        vec![
            ("bookmark".to_owned(), 0, vec![]),
            ("favorite".to_owned(), 1, removed.clone()),
            ("highlight".to_owned(), 1, removed),
            ("note".to_owned(), 1, vec![]),
        ]
    );
    let (last, pct) = progress_state(&app, user_id, &issue_id).await;
    assert_eq!(last, 1);
    assert!((pct - 0.5).abs() < 1e-9, "percent {pct}");

    // Idempotent: a second pass changes nothing and adds no second tag.
    let o2 = remap_issue_anchors(&app.state().db, &issue_id, &map)
        .await
        .unwrap();
    assert_eq!(o2, Default::default());
}
