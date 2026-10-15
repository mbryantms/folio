//! Post-audit T1.2 — scan transactions vs the DB pool.
//!
//! Two things this guards:
//!  - a scan used to `BEGIN` before hashing / parsing each batch's
//!    archives, so `scan_worker_count × per-scan folder fan-out`
//!    transactions sat open through the slow archive work and could
//!    exhaust the pool; now every file of a batch is prepared (no DB)
//!    before the transaction opens, and the fan-out has its own knob;
//!  - the phase-3 `select!` loop awaited its progress-snapshot write and
//!    cancel poll inline, which stopped polling the folder futures that
//!    held the pool's connections — a self-deadlock until the acquire
//!    timeout on a small pool (this test found it); those side effects
//!    are spawned tasks now.
//!
//! Acceptance: with a 2-connection pool and 8 folders processed 8-wide, a
//! full scan completes and concurrent `GET /api/series` calls keep
//! succeeding throughout. On failure the pool's size/idle counters and the
//! slow call's latency are printed.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::LibrarySeed;
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;

fn write_cbz(path: &Path, series: &str, number: u32, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    // A few pages so the parse + probe phase is not instantaneous.
    for page in 0..6u32 {
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&marker.to_le_bytes());
        png.extend_from_slice(&page.to_le_bytes());
        png.extend(std::iter::repeat_n(0u8, 4096));
        zw.start_file(format!("page-{page:03}.png"), opts).unwrap();
        zw.write_all(&png).unwrap();
    }
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(
        format!(
            r#"<?xml version="1.0"?><ComicInfo><Series>{series}</Series><Number>{number}</Number><Year>2012</Year></ComicInfo>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zw.finish().unwrap();
}

async fn register_admin(app: &TestApp) -> String {
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
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .map(|c| c.split(';').next().unwrap().to_owned())
        .expect("session cookie")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::print_stderr)]
async fn scan_with_a_two_connection_pool_never_starves_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    const FOLDERS: u32 = 8;
    const FILES: u32 = 4;
    for f in 0..FOLDERS {
        let folder = root.join(format!("Series {f:02}"));
        std::fs::create_dir_all(&folder).unwrap();
        for n in 1..=FILES {
            write_cbz(
                &folder.join(format!("Series {f:02} {n:03}.cbz")),
                &format!("Series {f:02}"),
                n,
                f * 100 + n,
            );
        }
    }
    // Pool of 2, every folder in flight at once: the old `BEGIN`-before-hash
    // shape pinned a connection per folder for the whole batch, and the old
    // loop stopped driving the folders while it wrote progress.
    let app = TestApp::spawn_with_pool(root.clone(), 2, FOLDERS as usize).await;
    let session = register_admin(&app).await;
    let lib_id = LibrarySeed::new(&root).insert(&app.state().db).await;

    let state = app.state();
    let scan = tokio::spawn(async move { scanner::scan_library(&state, lib_id).await });

    // Hammer an unrelated read endpoint while the scan runs. Every call must
    // come back 200 — a pool starved by scan transactions surfaces here as a
    // 5xx (acquire timeout) or a hang.
    let pool = app.state().db.get_postgres_connection_pool().clone();
    let mut calls = 0u32;
    let mut failures = 0u32;
    let started = std::time::Instant::now();
    while !scan.is_finished() {
        let t = std::time::Instant::now();
        let (size, idle) = (pool.size(), pool.num_idle());
        let resp = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/series?limit=5")
                    .header(header::COOKIE, &session)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let dt = t.elapsed();
        if status != StatusCode::OK || dt.as_millis() > 1000 {
            eprintln!(
                "DIAG t={:?} call={calls} status={status} took={dt:?} pool size={size} idle={idle} now_size={} now_idle={}",
                started.elapsed(),
                pool.size(),
                pool.num_idle()
            );
        }
        if status != StatusCode::OK {
            failures += 1;
        }
        calls += 1;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    eprintln!(
        "DIAG scan finished after {:?}; calls={calls} failures={failures}",
        started.elapsed()
    );
    let stats = scan.await.unwrap().expect("scan completes");
    assert_eq!(stats.files_added, (FOLDERS * FILES) as u64, "{stats:?}");
    assert_eq!(stats.series_created, FOLDERS as u64, "{stats:?}");
    assert!(calls > 0, "the probe loop never ran");
    assert_eq!(failures, 0, "{failures} starved requests");
}
