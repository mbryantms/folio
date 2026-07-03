//! archive-rewrite-1.0 M2 — page-byte editing integration tests.
//!
//! Drives [`server::jobs::archive_edit::edit_one_issue`] (the core
//! rewrite, which the apalis `handle` wraps with the mutex) and the
//! `handle` entry point (for the mutex-serialization check) against real
//! CBZ files on disk.

mod common;

use apalis::prelude::Data;
use archive::ArchiveLimits;
use archive::cbz::Cbz;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::library_event::{Column as EventCol, Entity as EventEntity};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::archive_rewrite::{self, mutex};
use server::jobs::archive_edit::{
    ArchiveEditJob, BulkArchiveOp, PageOp, Rot, edit_one_issue, handle, simulate_ops,
};
use server::jobs::archive_transforms::TransformStep;
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;

/// A distinct, decodable PNG of the given size + fill colour.
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

fn build_cbt(pages: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut tw = tar::Builder::new(&mut buf);
        for (name, bytes) in pages {
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tw.append_data(&mut header, *name, bytes.as_slice())
                .unwrap();
        }
        tw.into_inner().unwrap();
    }
    buf.into_inner()
}

/// Seed a writeback-enabled library + series + one issue whose archive is
/// `cbz`. Returns (issue_id, archive path).
async fn seed_issue_with_cbz(
    app: &TestApp,
    cbz: Vec<u8>,
    dir: &Path,
) -> (String, std::path::PathBuf) {
    seed_issue_with_archive(app, cbz, dir, "issue.cbz").await
}

/// Seed a writeback-enabled library + series + one issue with `file_name`.
async fn seed_issue_with_archive(
    app: &TestApp,
    bytes: Vec<u8>,
    dir: &Path,
    file_name: &str,
) -> (String, std::path::PathBuf) {
    let db = &app.state().db;
    let lib = LibrarySeed::new(dir)
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series = SeriesSeed::new(lib, "Edit Series").insert(db).await;
    let path = dir.join(file_name);
    let issue_id = IssueSeed::new(lib, series, &path, &bytes, 1.0)
        .insert(db)
        .await;
    (issue_id, path)
}

fn job(issue_id: &str, ops: Vec<PageOp>) -> ArchiveEditJob {
    ArchiveEditJob {
        issue_id: issue_id.to_owned(),
        ops,
        bulk_op: None,
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
    }
}

fn bulk_job(issue_id: &str, op: BulkArchiveOp) -> ArchiveEditJob {
    ArchiveEditJob {
        issue_id: issue_id.to_owned(),
        ops: Vec::new(),
        bulk_op: Some(op),
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
    }
}

fn page_bytes(path: &Path, name: &str) -> Vec<u8> {
    let mut c = Cbz::open(path, ArchiveLimits::default()).unwrap();
    c.read_entry_bytes_by_name(name).unwrap()
}

/// Format-agnostic page-name list (works for cbz + cbt) via the dispatch
/// reader.
fn open_page_names(path: &Path) -> Vec<String> {
    let a = archive::open(path, ArchiveLimits::default()).unwrap();
    a.pages().iter().map(|e| e.name.clone()).collect()
}

/// Format-agnostic single-page read.
fn read_any_page(path: &Path, name: &str) -> Vec<u8> {
    let mut a = archive::open(path, ArchiveLimits::default()).unwrap();
    a.read_entry_bytes(name).unwrap()
}

#[tokio::test]
async fn remove_and_reorder_rewrites_archive() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let p1 = png_bytes(4, 4, [10, 0, 0]);
    let p2 = png_bytes(4, 4, [0, 20, 0]);
    let p3 = png_bytes(4, 4, [0, 0, 30]);
    let p4 = png_bytes(4, 4, [40, 40, 0]);
    let cbz = build_cbz(&[
        ("p1.png", p1.clone()),
        ("p2.png", p2.clone()),
        ("p3.png", p3.clone()),
        ("p4.png", p4.clone()),
    ]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    // Remove index 1 (p2) → [p1,p3,p4]; reorder [2,0,1] → [p4,p1,p3].
    let ops = vec![
        PageOp::Remove { ordinal: 1 },
        PageOp::Reorder {
            new_order: vec![2, 0, 1],
        },
    ];
    let res = edit_one_issue(&state, &job(&issue_id, ops)).await.unwrap();
    assert_eq!(res.page_count_before, 4);
    assert_eq!(res.page_count_after, 3);

    let c = Cbz::open(&path, ArchiveLimits::default()).unwrap();
    let names: Vec<String> = c.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["p0001.png", "p0002.png", "p0003.png"]);
    drop(c);
    assert_eq!(page_bytes(&path, "p0001.png"), p4);
    assert_eq!(page_bytes(&path, "p0002.png"), p1);
    assert_eq!(page_bytes(&path, "p0003.png"), p3);

    // Bookkeeping: edit stamp + cover reset (structural).
    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_rewrite_kind.as_deref(), Some("edit"));
    assert_eq!(row.cover_page_index, 0);
    assert_eq!(row.thumbnail_version, 0);
}

#[tokio::test]
async fn edit_wipes_stale_on_disk_thumbnails() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let p1 = png_bytes(4, 4, [10, 0, 0]);
    let p2 = png_bytes(4, 4, [0, 20, 0]);
    let cbz = build_cbz(&[("p1.png", p1.clone()), ("p2.png", p2.clone())]);
    let (issue_id, _path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    // Pre-edit artifacts on disk: cover, @sm cover, and a strip page.
    // Every generator in `library::thumbnails` is skip-if-file-exists, so
    // if the edit leaves these behind the post-edit regen no-ops and the
    // page map keeps serving pre-edit pixels.
    let thumbs = app._data_dir.path().join("thumbs");
    let strip_dir = thumbs.join(&issue_id).join("s");
    std::fs::create_dir_all(&strip_dir).unwrap();
    let cover = thumbs.join(format!("{issue_id}.webp"));
    let cover_sm = thumbs.join(format!("{issue_id}@sm.webp"));
    let strip = strip_dir.join("1.webp");
    std::fs::write(&cover, b"stale").unwrap();
    std::fs::write(&cover_sm, b"stale").unwrap();
    std::fs::write(&strip, b"stale").unwrap();

    let ops = vec![PageOp::Rotate {
        ordinal: 0,
        degrees: Rot::R90,
    }];
    edit_one_issue(&state, &job(&issue_id, ops)).await.unwrap();

    assert!(
        !cover.exists(),
        "stale cover thumb must be wiped by the edit"
    );
    assert!(
        !cover_sm.exists(),
        "stale @sm cover thumb must be wiped by the edit"
    );
    assert!(
        !strip.exists(),
        "stale strip thumb must be wiped by the edit"
    );
}

#[tokio::test]
async fn bulk_remove_last_lowers_per_issue_and_rewrites() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let p1 = png_bytes(4, 4, [10, 0, 0]);
    let p2 = png_bytes(4, 4, [0, 20, 0]);
    let p3 = png_bytes(4, 4, [0, 0, 30]);
    let p4 = png_bytes(4, 4, [40, 40, 0]);
    let cbz = build_cbz(&[
        ("p1.png", p1.clone()),
        ("p2.png", p2.clone()),
        ("p3.png", p3.clone()),
        ("p4.png", p4.clone()),
    ]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    // Bulk "remove last 2" is lowered against this issue's 4 pages → drops
    // p3 + p4, leaving [p1, p2].
    let res = edit_one_issue(
        &state,
        &bulk_job(&issue_id, BulkArchiveOp::RemoveLast { count: 2 }),
    )
    .await
    .unwrap();
    assert_eq!(res.page_count_before, 4);
    assert_eq!(res.page_count_after, 2);

    let names = open_page_names(&path);
    assert_eq!(names, vec!["p0001.png", "p0002.png"]);
    assert_eq!(read_any_page(&path, "p0001.png"), p1);
    assert_eq!(read_any_page(&path, "p0002.png"), p2);

    // Removal is structural → cover index reset.
    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.cover_page_index, 0);
}

#[tokio::test]
async fn rotate_swaps_dimensions() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    // 10 wide × 20 tall.
    let cbz = build_cbz(&[("p1.png", png_bytes(10, 20, [120, 30, 30]))]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    edit_one_issue(
        &state,
        &job(
            &issue_id,
            vec![PageOp::Rotate {
                ordinal: 0,
                degrees: Rot::R90,
            }],
        ),
    )
    .await
    .unwrap();

    let bytes = page_bytes(&path, "p0001.png");
    let img = image::load_from_memory(&bytes).unwrap();
    // 90° rotation swaps width/height → 20 wide × 10 tall.
    assert_eq!((img.width(), img.height()), (20, 10));
}

#[tokio::test]
async fn replace_swaps_page_content() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let cbz = build_cbz(&[("p1.png", png_bytes(4, 4, [1, 2, 3]))]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    // Stage an upload (8×16) the way POST /uploads would.
    let uploads = state.cfg().data_path.join("uploads");
    std::fs::create_dir_all(&uploads).unwrap();
    let image_id = Uuid::now_v7();
    std::fs::write(
        uploads.join(image_id.to_string()),
        png_bytes(8, 16, [9, 9, 9]),
    )
    .unwrap();

    edit_one_issue(
        &state,
        &job(
            &issue_id,
            vec![PageOp::Replace {
                ordinal: 0,
                image_id,
            }],
        ),
    )
    .await
    .unwrap();

    let bytes = page_bytes(&path, "p0001.png");
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (8, 16));
}

#[tokio::test]
async fn transform_crop_changes_dimensions() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    // 20 wide × 20 tall; crop to a 6×8 box.
    let cbz = build_cbz(&[("p1.png", png_bytes(20, 20, [40, 80, 120]))]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();

    edit_one_issue(
        &state,
        &job(
            &issue_id,
            vec![PageOp::Transform {
                ordinal: 0,
                chain: vec![
                    TransformStep::BrightnessContrast {
                        brightness: 20,
                        contrast: 10,
                    },
                    TransformStep::CropBox {
                        x: 2,
                        y: 2,
                        w: 6,
                        h: 8,
                    },
                ],
            }],
        ),
    )
    .await
    .unwrap();

    // Page count is unchanged; the page is re-encoded to the cropped dims.
    let cbz = Cbz::open(&path, ArchiveLimits::default()).unwrap();
    assert_eq!(cbz.pages().len(), 1);
    let bytes = page_bytes(&path, "p0001.png");
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (6, 8));
}

#[tokio::test]
async fn restore_from_bak_reverts_archive() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let cbz = build_cbz(&[
        ("p1.png", png_bytes(4, 4, [1, 0, 0])),
        ("p2.png", png_bytes(4, 4, [0, 1, 0])),
        ("p3.png", png_bytes(4, 4, [0, 0, 1])),
    ]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();
    let original = std::fs::read(&path).unwrap();

    edit_one_issue(&state, &job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]))
        .await
        .unwrap();
    assert_ne!(
        std::fs::read(&path).unwrap(),
        original,
        "edit changed the file"
    );

    archive_rewrite::restore_latest_backup(&path).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "restore returned byte-identical original",
    );
}

#[tokio::test]
async fn handle_serializes_on_held_mutex() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let cbz = build_cbz(&[
        ("p1.png", png_bytes(4, 4, [1, 0, 0])),
        ("p2.png", png_bytes(4, 4, [0, 1, 0])),
    ]);
    let (issue_id, path) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    let state = app.state();
    let before = std::fs::read(&path).unwrap();

    // Simulate an in-flight rewrite by pre-claiming the mutex.
    let mut redis = state.jobs.redis.clone();
    let token = mutex::try_claim(&mut redis, &issue_id, mutex::EDIT_TTL_SECS)
        .await
        .unwrap()
        .expect("mutex claim should succeed");

    // handle must observe the busy mutex and skip without touching bytes.
    handle(
        job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]),
        Data::new(state.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "busy mutex must block the edit",
    );

    // Release and retry — now it should rewrite.
    mutex::release(&mut redis, &issue_id, &token).await;
    handle(
        job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]),
        Data::new(state.clone()),
    )
    .await
    .unwrap();
    let c = Cbz::open(&path, ArchiveLimits::default()).unwrap();
    assert_eq!(c.pages().len(), 1, "second handle rewrote the archive");

    // observability-split M3b: the successful rewrite wrote a durable
    // `archive/updated` manifest row (the mutex-blocked first attempt did
    // not). One row total.
    let archive_events = EventEntity::find()
        .filter(EventCol::EntityId.eq(issue_id.clone()))
        .filter(EventCol::Category.eq("archive"))
        .filter(EventCol::Action.eq("updated"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(
        archive_events.len(),
        1,
        "expected one archive/updated manifest row, got {archive_events:?}",
    );
    let detail = archive_events[0].detail.as_ref().unwrap();
    assert_eq!(
        detail.get("page_count_before").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        detail.get("page_count_after").and_then(|v| v.as_u64()),
        Some(1)
    );
}

/// Light-weight property check: a handful of op sequences applied to a
/// 6-page archive land on disk with the page count `simulate_ops`
/// predicts, and the result reopens cleanly.
#[tokio::test]
async fn page_op_sequences_match_simulation() {
    let sequences: Vec<Vec<PageOp>> = vec![
        vec![PageOp::Remove { ordinal: 5 }],
        vec![PageOp::Remove { ordinal: 0 }, PageOp::Remove { ordinal: 0 }],
        vec![PageOp::Reorder {
            new_order: vec![5, 4, 3, 2, 1, 0],
        }],
        vec![
            PageOp::Rotate {
                ordinal: 2,
                degrees: Rot::R180,
            },
            PageOp::Remove { ordinal: 4 },
        ],
        vec![
            PageOp::Remove { ordinal: 1 },
            PageOp::Reorder {
                new_order: vec![4, 0, 3, 2, 1],
            },
            PageOp::Rotate {
                ordinal: 0,
                degrees: Rot::R270,
            },
        ],
    ];

    let app = TestApp::spawn().await;
    let state = app.state();
    let db = &state.db;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series = SeriesSeed::new(lib, "Prop Series").insert(db).await;
    let names: Vec<String> = (0..6).map(|n| format!("p{n}.png")).collect();

    for (i, ops) in sequences.into_iter().enumerate() {
        // Distinct bytes per iteration → distinct content hash → distinct
        // issue id (the seed keys the row on the archive's hash).
        let pages: Vec<(&str, Vec<u8>)> = names
            .iter()
            .enumerate()
            .map(|(n, name)| (name.as_str(), png_bytes(6, 6, [(i * 6 + n) as u8, 0, 0])))
            .collect();
        let cbz = build_cbz(&pages);
        let path = dir.path().join(format!("issue{i}.cbz"));
        let issue_id = IssueSeed::new(lib, series, &path, &cbz, (i + 1) as f64)
            .insert(db)
            .await;

        let predicted = simulate_ops(6, &ops).unwrap();
        let res = edit_one_issue(&state, &job(&issue_id, ops)).await.unwrap();
        assert_eq!(res.page_count_after, predicted, "seq {i}: count mismatch");

        let c = Cbz::open(&path, ArchiveLimits::default()).unwrap();
        assert_eq!(
            c.pages().len(),
            predicted,
            "seq {i}: on-disk count mismatch"
        );
    }
}

#[tokio::test]
async fn cbt_remove_and_reorder_rewrites_in_place() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let p1 = png_bytes(4, 4, [11, 0, 0]);
    let p2 = png_bytes(4, 4, [0, 22, 0]);
    let p3 = png_bytes(4, 4, [0, 0, 33]);
    let cbt = build_cbt(&[
        ("p1.png", p1.clone()),
        ("p2.png", p2.clone()),
        ("p3.png", p3.clone()),
    ]);
    let (issue_id, path) = seed_issue_with_archive(&app, cbt, dir.path(), "issue.cbt").await;
    let state = app.state();

    // Remove index 0 (p1) → [p2,p3]; reorder [1,0] → [p3,p2].
    let ops = vec![
        PageOp::Remove { ordinal: 0 },
        PageOp::Reorder {
            new_order: vec![1, 0],
        },
    ];
    let res = edit_one_issue(&state, &job(&issue_id, ops)).await.unwrap();
    assert_eq!(res.page_count_before, 3);
    assert_eq!(res.page_count_after, 2);
    // CBT stays a .cbt (no conversion).
    assert!(res.moved_to.is_none());
    assert_eq!(res.archive_path, path);

    // The output is a valid tar with the new contiguous order.
    assert_eq!(open_page_names(&path), vec!["p0001.png", "p0002.png"]);
    assert_eq!(read_any_page(&path, "p0001.png"), p3);
    assert_eq!(read_any_page(&path, "p0002.png"), p2);

    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_rewrite_kind.as_deref(), Some("edit"));
}

/// First `*.cbr` under the workspace `fixtures/` dir, if any. The CBR
/// path can only be exercised against a real RAR (none can be created
/// in-repo), so the e2e test below is `#[ignore]`d + fixture-gated.
fn first_cbr_fixture() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("cbr"))
        })
}

#[tokio::test]
#[ignore = "needs a local fixtures/*.cbr (not committed); run with --ignored"]
async fn cbr_edit_converts_to_cbz_and_repoints_issue() {
    let Some(fixture) = first_cbr_fixture() else {
        return; // no local fixture — skip
    };
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let bytes = std::fs::read(&fixture).unwrap();

    let db = &app.state().db;
    let lib = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series = SeriesSeed::new(lib, "Thanos").insert(db).await;
    let cbr_path = dir.path().join("Thanos 001.cbr");
    let issue_id = IssueSeed::new(lib, series, &cbr_path, &bytes, 1.0)
        .insert(db)
        .await;
    let state = app.state();

    let pre = archive::open(&cbr_path, ArchiveLimits::default())
        .unwrap()
        .pages()
        .len();
    assert!(pre >= 2);

    // Remove the trailing page (a `zlollipop.jpg` ad in these fixtures).
    let res = edit_one_issue(
        &state,
        &job(
            &issue_id,
            vec![PageOp::Remove {
                ordinal: (pre - 1) as u32,
            }],
        ),
    )
    .await
    .unwrap();

    let cbz_path = cbr_path.with_extension("cbz");
    assert_eq!(res.page_count_before, pre);
    assert_eq!(res.page_count_after, pre - 1);
    assert_eq!(res.moved_to.as_deref(), Some(cbz_path.as_path()));
    assert!(cbz_path.exists(), "converted .cbz written");
    assert!(
        res.backup_path.as_ref().is_some_and(|p| p.exists()),
        "original .cbr kept as .bak",
    );

    // The converted CBZ is a valid zip with the surviving pages.
    let cbz = Cbz::open(&cbz_path, ArchiveLimits::default()).unwrap();
    assert_eq!(cbz.pages().len(), pre - 1);

    // The issue row repoints to the .cbz and the library remembers the
    // CBR-conversion acknowledgement so the UI stops prompting.
    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.file_path.ends_with(".cbz"), "file_path repointed");
    let libr = entity::library::Entity::find_by_id(lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(libr.cbr_convert_confirmed_at.is_some());
}

#[tokio::test]
async fn cbt_rotate_swaps_dimensions() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let cbt = build_cbt(&[("p1.png", png_bytes(10, 20, [80, 40, 40]))]);
    let (issue_id, path) = seed_issue_with_archive(&app, cbt, dir.path(), "issue.cbt").await;
    let state = app.state();

    edit_one_issue(
        &state,
        &job(
            &issue_id,
            vec![PageOp::Rotate {
                ordinal: 0,
                degrees: Rot::R90,
            }],
        ),
    )
    .await
    .unwrap();

    let bytes = read_any_page(&path, "p0001.png");
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (20, 10));
}
