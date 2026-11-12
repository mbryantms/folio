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
    ArchiveEditJob, BulkArchiveOp, EditError, EditFaults, PageOp, Rot, edit_one_issue, handle,
    handle_with_faults, simulate_ops,
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
        attempt: 0,
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
        attempt: 0,
    }
}

fn page_bytes(path: &Path, name: &str) -> Vec<u8> {
    let mut c = Cbz::open_path_unchecked(path, ArchiveLimits::default()).unwrap();
    c.read_entry_bytes_by_name(name).unwrap()
}

/// Format-agnostic page-name list (works for cbz + cbt) via the dispatch
/// reader.
fn open_page_names(path: &Path) -> Vec<String> {
    let a = archive::open_path_unchecked(path, ArchiveLimits::default()).unwrap();
    a.pages().iter().map(|e| e.name.clone()).collect()
}

/// Format-agnostic single-page read.
fn read_any_page(path: &Path, name: &str) -> Vec<u8> {
    let mut a = archive::open_path_unchecked(path, ArchiveLimits::default()).unwrap();
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

    let c = Cbz::open_path_unchecked(&path, ArchiveLimits::default()).unwrap();
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
    let cbz = Cbz::open_path_unchecked(&path, ArchiveLimits::default()).unwrap();
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

    archive_rewrite::restore_latest_backup(
        &server::library::no_follow::LibraryRoot::resolve(dir.path()),
        &path,
    )
    .unwrap();
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
    let c = Cbz::open_path_unchecked(&path, ArchiveLimits::default()).unwrap();
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

        let c = Cbz::open_path_unchecked(&path, ArchiveLimits::default()).unwrap();
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

/// The committed synthetic `fixtures/synthetic-3page.cbr` (RAR5, stored, three
/// JFIF-stub pages — see `fixtures/make-cbr-fixture.py`), falling back to any
/// other `*.cbr` a developer dropped under `fixtures/`. The CBR
/// path can only be exercised against a real RAR (none can be created
/// in-repo), so the e2e test below is `#[ignore]`d + fixture-gated.
fn first_cbr_fixture() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let synthetic = dir.join("synthetic-3page.cbr");
    if synthetic.is_file() {
        return Some(synthetic);
    }
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
async fn cbr_edit_converts_to_cbz_and_repoints_issue() {
    let fixture = first_cbr_fixture()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");
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

    let pre = archive::open_path_unchecked(&cbr_path, ArchiveLimits::default())
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
    let cbz = Cbz::open_path_unchecked(&cbz_path, ArchiveLimits::default()).unwrap();
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

/// WP-2.6 (b) / audit DI-11: a page edit carries every foreign non-page
/// entry (`CoMet.xml`, `notes.txt`, an embedded `.json`) and the existing
/// root `ComicInfo.xml` through byte-for-byte; junk is dropped. Same policy
/// as the sidecar rewrite (`archive::rewrite_policy`), for both CBZ and CBT.
#[tokio::test]
async fn page_edit_preserves_foreign_sidecars_cbz_and_cbt() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let comet = b"<comet><title>Kept</title></comet>".to_vec();
    let notes = b"scanner notes".to_vec();
    let meta = b"{\"kept\":true}".to_vec();
    let comicinfo = b"<ComicInfo><Title>Old</Title></ComicInfo>".to_vec();
    let entries: Vec<(&str, Vec<u8>)> = vec![
        ("p1.png", png_bytes(4, 4, [1, 0, 0])),
        ("p2.png", png_bytes(4, 4, [0, 1, 0])),
        ("CoMet.xml", comet.clone()),
        ("notes.txt", notes.clone()),
        ("meta.json", meta.clone()),
        ("ComicInfo.xml", comicinfo.clone()),
        ("Thumbs.db", b"junk".to_vec()),
    ];

    for (file_name, bytes) in [
        ("issue.cbz", build_cbz(&entries)),
        ("issue.cbt", build_cbt(&entries)),
    ] {
        let dir = tempdir().unwrap();
        let (issue_id, path) = seed_issue_with_archive(&app, bytes, dir.path(), file_name).await;
        edit_one_issue(&state, &job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]))
            .await
            .unwrap();

        let mut a = archive::open_path_unchecked(&path, ArchiveLimits::default()).unwrap();
        assert_eq!(open_page_names(&path), vec!["p0001.png"], "{file_name}");
        assert_eq!(
            a.read_entry_bytes("CoMet.xml").unwrap(),
            comet,
            "{file_name}"
        );
        assert_eq!(
            a.read_entry_bytes("notes.txt").unwrap(),
            notes,
            "{file_name}"
        );
        assert_eq!(
            a.read_entry_bytes("meta.json").unwrap(),
            meta,
            "{file_name}"
        );
        assert_eq!(
            a.read_entry_bytes("ComicInfo.xml").unwrap(),
            comicinfo,
            "{file_name}"
        );
        assert!(a.find("Thumbs.db").is_none(), "{file_name}: junk dropped");
    }
}

// ───────── boot-time staging sweep ─────────

/// Write `name` under `dir` with an mtime `age` in the past.
fn aged_file(dir: &Path, name: &str, age: std::time::Duration) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, b"leftover").unwrap();
    let t = filetime::FileTime::from_system_time(std::time::SystemTime::now() - age);
    filetime::set_file_mtime(&p, t).unwrap();
    p
}

/// The boot sweep removes only Folio's own staging files
/// (`<archive>.<32 hex>.tmp`) older than the floor, and only in libraries
/// with archive writeback on. A library with writeback off is never
/// walked; a foreign `.tmp` is never touched anywhere.
#[tokio::test]
async fn startup_cleanup_removes_only_own_staging_in_writeback_libraries() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let old = std::time::Duration::from_secs(60 * 60);
    let fresh = std::time::Duration::from_secs(10);
    let staging = |n: u8| format!("issue{n}.cbz.{}.tmp", Uuid::new_v4().simple());
    assert!(archive_rewrite::is_staging_name(&staging(0)));

    // Writeback OFF: nothing is touched, not even a correctly named old
    // staging file.
    let off = tempdir().unwrap();
    LibrarySeed::new(off.path()).insert(db).await;
    let off_files = [
        aged_file(off.path(), "notes.tmp", old),
        aged_file(off.path(), "x.cbz.tmp", old),
        aged_file(off.path(), &staging(1), old),
    ];

    // Writeback ON: only the old, correctly named staging file goes.
    let on = tempdir().unwrap();
    LibrarySeed::new(on.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let nested = on.path().join("Series (2020)");
    std::fs::create_dir(&nested).unwrap();
    let on_old_staging = aged_file(&nested, &staging(2), old);
    let on_kept = [
        aged_file(on.path(), "notes.tmp", old),
        aged_file(on.path(), "x.cbz.tmp", old),
        aged_file(on.path(), &staging(3), fresh),
    ];

    let removed = archive_rewrite::spawn_startup_cleanup(db.clone())
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert!(!on_old_staging.exists(), "old staging file removed");
    for p in off_files.iter().chain(on_kept.iter()) {
        assert!(p.exists(), "{} must survive", p.display());
    }
}

// ───────── ARC-2 (4): an unclear commit that did land ─────────

async fn issue_events(app: &TestApp, issue_id: &str) -> Vec<entity::library_event::Model> {
    EventEntity::find()
        .filter(EventCol::EntityId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

/// Seed the CBR fixture as an issue; returns the issue id, its path and
/// the page count.
async fn seed_cbr(app: &TestApp, dir: &Path) -> (String, std::path::PathBuf, usize) {
    let fixture = first_cbr_fixture().expect("fixtures/synthetic-3page.cbr is committed");
    let bytes = std::fs::read(&fixture).unwrap();
    let (issue_id, cbr_path) = seed_issue_with_archive(app, bytes, dir, "Thanos 001.cbr").await;
    let pages = archive::open_path_unchecked(&cbr_path, ArchiveLimits::default())
        .unwrap()
        .pages()
        .len();
    (issue_id, cbr_path, pages)
}

/// The bookkeeping transaction of a CBR edit commits, but the client sees
/// an error (injected right after the commit). The row names the new
/// `.cbz`, so the conversion is kept and the edit is reported **done**,
/// once, with a warning saying what was unclear; no failure is reported,
/// so the admin is not invited to repeat the remove on the edited archive.
/// Pre-fix the edit was reported as failed.
#[tokio::test]
async fn a_cbr_edit_whose_commit_landed_despite_an_error_is_reported_done() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (issue_id, cbr_path, pre) = seed_cbr(&app, dir.path()).await;
    let cbz_path = cbr_path.with_extension("cbz");

    let faults = EditFaults {
        after_commit: Some(Box::new(|| {
            Err(EditError::Db(sea_orm::DbErr::Custom(
                "connection reset while committing".into(),
            )))
        })),
        ..Default::default()
    };
    handle_with_faults(
        job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]),
        app.state(),
        faults,
    )
    .await
    .unwrap();

    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Path::new(&row.file_path), cbz_path.as_path());
    assert!(!cbr_path.exists(), "the conversion was finished");
    assert!(archive_rewrite::backup_slot(&cbr_path, 0).exists());
    let edited = Cbz::open_path_unchecked(&cbz_path, ArchiveLimits::default()).unwrap();
    assert_eq!(edited.pages().len(), pre - 1, "the remove was applied once");

    let events = issue_events(&app, &issue_id).await;
    assert_eq!(events.len(), 1, "reported once: {events:?}");
    assert_eq!(events[0].action, "updated", "reported done: {events:?}");
    assert_eq!(events[0].severity, "warning");
    assert!(
        events[0].summary.contains("Do not repeat"),
        "{}",
        events[0].summary
    );
}

/// Control: when the transaction really fails (a trigger rejects the row
/// update), the row still names the `.cbr`, the conversion is undone and
/// the edit is reported failed, with the archive as it was.
#[tokio::test]
async fn a_cbr_edit_whose_commit_failed_is_undone_and_reported_failed() {
    use sea_orm::ConnectionTrait;
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (issue_id, cbr_path, _) = seed_cbr(&app, dir.path()).await;
    let original = std::fs::read(&cbr_path).unwrap();
    app.state()
        .db
        .execute_unprepared(
            "CREATE FUNCTION test_fail_edit_stamp() RETURNS trigger AS $$ \
             BEGIN RAISE EXCEPTION 'injected'; END $$ LANGUAGE plpgsql; \
             CREATE TRIGGER test_fail_edit_stamp BEFORE UPDATE ON issues FOR EACH ROW \
             WHEN (NEW.last_rewrite_kind = 'edit') EXECUTE FUNCTION test_fail_edit_stamp();",
        )
        .await
        .unwrap();

    handle_with_faults(
        job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]),
        app.state(),
        EditFaults::default(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(&cbr_path).unwrap(),
        original,
        "original in place"
    );
    assert!(
        !cbr_path.with_extension("cbz").exists(),
        "conversion undone"
    );
    let events = issue_events(&app, &issue_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].action, "errored");
}

/// An in-place edit whose bookkeeping fails after the swap is still a
/// failure, but its message says the archive was edited and must not be
/// edited again.
#[tokio::test]
async fn an_in_place_edit_whose_bookkeeping_failed_says_the_archive_was_edited() {
    use sea_orm::ConnectionTrait;
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let cbz = build_cbz(&[
        ("p1.png", png_bytes(4, 4, [1, 0, 0])),
        ("p2.png", png_bytes(4, 4, [0, 1, 0])),
    ]);
    let (issue_id, _) = seed_issue_with_cbz(&app, cbz, dir.path()).await;
    app.state()
        .db
        .execute_unprepared(
            "CREATE FUNCTION test_fail_edit_stamp() RETURNS trigger AS $$ \
             BEGIN RAISE EXCEPTION 'injected'; END $$ LANGUAGE plpgsql; \
             CREATE TRIGGER test_fail_edit_stamp BEFORE UPDATE ON issues FOR EACH ROW \
             WHEN (NEW.last_rewrite_kind = 'edit') EXECUTE FUNCTION test_fail_edit_stamp();",
        )
        .await
        .unwrap();
    let err = edit_one_issue(
        &app.state(),
        &job(&issue_id, vec![PageOp::Remove { ordinal: 0 }]),
    )
    .await
    .err()
    .expect("the bookkeeping failed");
    assert!(matches!(err, EditError::NotRecorded(_)), "{err:?}");
    assert!(err.to_string().contains("Do not repeat the edit"), "{err}");
}
