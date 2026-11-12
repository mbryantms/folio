//! The OCR work behind `api::issue_ocr`, driven through the routes with
//! debug-build stand-ins for the detector and recognizer
//! (`ocr::pipeline::test_models`, keyed by content hash) instead of the
//! real models, which download hundreds of MB on first use.
//!
//! - Finished work reaches the cache even when the request that started
//!   it is gone (route timeout, client abort): the work owns the permit
//!   and writes the cache itself, so a retry is a cache hit.
//!
//! The stand-ins only exist in debug builds, so this binary does too.

#![cfg(debug_assertions)]

mod common;

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use image::DynamicImage;
use server::ocr::cache::{CachedBbox, CachedDetection};
use server::ocr::pipeline::{PageDetector, test_models};
use server::ocr::recognizer::{Recognition, Recognizer};
use tower::ServiceExt;

/// Tests that need OCR permits share this; a test that holds every
/// permit takes it exclusively, so it never starves the others into a 503.
static ALL_PERMITS: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

// ─── Stand-ins ───────────────────────────────────────────────────

/// A latch a stand-in waits on until the test opens it, so a test can
/// observe "still running" without depending on wall time.
#[derive(Default)]
struct Gate {
    open: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
}

impl Gate {
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }
}

/// A detector that takes `delay` (or waits for `gate`) and finds one box.
struct SlowDetector {
    delay: Duration,
    gate: Option<&'static Gate>,
    calls: AtomicUsize,
}

impl PageDetector for SlowDetector {
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.gate {
            Some(gate) => gate.wait(),
            None => std::thread::sleep(self.delay),
        }
        Ok(CachedDetection {
            page_w: page.width(),
            page_h: page.height(),
            bboxes: vec![CachedBbox {
                xmin: 2.0,
                ymin: 2.0,
                xmax: 30.0,
                ymax: 20.0,
                confidence: 0.9,
                class: 0,
            }],
        })
    }
}

/// A recognizer that takes `delay` and reads `HELLO`.
struct SlowRecognizer {
    delay: Duration,
    calls: AtomicUsize,
}

impl Recognizer for SlowRecognizer {
    fn recognize(&self, _image: &DynamicImage) -> anyhow::Result<Recognition> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(Recognition {
            text: "HELLO".to_owned(),
            confidence: 0.9,
            words: None,
        })
    }
}

fn slow_detector(ms: u64) -> &'static SlowDetector {
    Box::leak(Box::new(SlowDetector {
        delay: Duration::from_millis(ms),
        gate: None,
        calls: AtomicUsize::new(0),
    }))
}

fn gated_detector(gate: &'static Gate) -> &'static SlowDetector {
    Box::leak(Box::new(SlowDetector {
        delay: Duration::ZERO,
        gate: Some(gate),
        calls: AtomicUsize::new(0),
    }))
}

/// A recognizer that fails: with an error, or by panicking.
struct FailingRecognizer {
    panic: bool,
    calls: AtomicUsize,
}

impl Recognizer for FailingRecognizer {
    fn recognize(&self, _image: &DynamicImage) -> anyhow::Result<Recognition> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.panic {
            panic!("stand-in recognizer panics on purpose");
        }
        anyhow::bail!("stand-in recognizer fails on purpose")
    }
}

fn slow_recognizer(ms: u64) -> &'static SlowRecognizer {
    Box::leak(Box::new(SlowRecognizer {
        delay: Duration::from_millis(ms),
        calls: AtomicUsize::new(0),
    }))
}

// ─── Plumbing ────────────────────────────────────────────────────

struct Authed {
    session: String,
    csrf: String,
    user_id: uuid::Uuid,
}

async fn register(app: &TestApp, email: &str) -> Authed {
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
    let cookie = |name: &str| {
        resp.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(&format!("{name}=")))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{name}="))
            .to_owned()
    };
    let session = cookie("__Host-comic_session");
    let csrf = cookie("__Host-comic_csrf");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    Authed {
        session,
        csrf,
        user_id: json["user"]["id"].as_str().unwrap().parse().unwrap(),
    }
}

fn cookie_header(a: &Authed) -> String {
    format!(
        "__Host-comic_session={}; __Host-comic_csrf={}",
        a.session, a.csrf
    )
}

fn ocr_request(a: &Authed, id: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/me/issues/{id}/ocr"))
        .header(header::COOKIE, cookie_header(a))
        .header("X-CSRF-Token", &a.csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn regions_request(a: &Authed, id: &str, page: u32) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/me/issues/{id}/pages/{page}/text-regions"))
        .header(header::COOKIE, cookie_header(a))
        .body(Body::empty())
        .unwrap()
}

async fn send(app: &TestApp, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Send `req` and drop it once `started()` says its work is running (plus
/// a moment, so it is dropped mid-work), as the route timeout or a client
/// abort would. Waiting for the work rather than a fixed time keeps this
/// right when other tests hold the OCR permits for a while.
async fn send_and_abandon(app: &TestApp, req: Request<Body>, started: impl Fn() -> bool) {
    let fut = app.router.clone().oneshot(req);
    tokio::pin!(fut);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !started() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the work never started"
        );
        tokio::select! {
            _ = &mut fut => panic!("the request finished before it was dropped"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    let mid_work = tokio::time::timeout(Duration::from_millis(100), &mut fut).await;
    assert!(
        mid_work.is_err(),
        "the request was meant to still be running"
    );
}

/// A `w × h` near-white PNG whose first pixel is set from `salt`, so two
/// fixtures with different salts never have the same bytes.
fn png(w: u32, h: u32, salt: &[u8]) -> Vec<u8> {
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([250, 250, 250]));
    let s = blake3::hash(salt);
    let s = s.as_bytes();
    img.put_pixel(0, 0, image::Rgb([s[0], s[1], s[2]]));
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn cbz(page: &[u8]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-000.png", opts).unwrap();
    zw.write_all(page).unwrap();
    zw.finish().unwrap().into_inner()
}

/// One issue whose only page is a `w × h` PNG. The stand-ins are keyed by
/// content hash (= the issue id here) and never removed, so every fixture
/// is salted with its library folder (a fresh temporary directory per
/// test): no two tests share bytes, whatever sizes they pick.
async fn seed(app: &TestApp, root: &std::path::Path, w: u32, h: u32) -> String {
    std::fs::create_dir_all(root).unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = LibrarySeed::new(root).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Series").insert(&db).await;
    IssueSeed::new(
        lib_id,
        series_id,
        &root.join("issue.cbz"),
        &cbz(&png(w, h, root.as_os_str().as_encoded_bytes())),
        1.0,
    )
    .insert(&db)
    .await
}

async fn redis_has(app: &TestApp, key: &str) -> bool {
    use redis::AsyncCommands;
    let mut redis = app.state().jobs.redis.clone();
    redis.exists::<_, bool>(key).await.unwrap()
}

/// Wait up to 10 s for `key` to appear.
async fn wait_for_key(app: &TestApp, key: &str) -> bool {
    for _ in 0..200 {
        if redis_has(app, key).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

// ─── S1: finished work is cached when the request is gone ────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_detection_that_outlives_its_request_is_cached() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-detect@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 101, 64).await;
    let gate: &'static Gate = Box::leak(Box::default());
    let detector = gated_detector(gate);
    test_models::install(
        &id,
        test_models::Stub {
            detector: Some(detector),
            recognizer: None,
        },
    );
    let key = server::ocr::cache::detect_cache_key(&id, 0);

    send_and_abandon(&app, regions_request(&admin, &id, 0), || {
        detector.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        !redis_has(&app, &key).await,
        "still detecting when the request was dropped"
    );

    // The detection finishes on its own and is cached.
    gate.open();
    assert!(
        wait_for_key(&app, &key).await,
        "a finished detection reaches the cache"
    );

    // The retry is a cache hit: no second detection.
    let (status, body) = send(&app, regions_request(&admin, &id, 0)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["page_w"], 101);
    assert_eq!(body["regions"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(
        detector.calls.load(Ordering::SeqCst),
        1,
        "the retry did not detect again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_snap_detection_that_outlives_its_request_is_cached() {
    // `detect: true` on the OCR POST: the request is dropped mid-detection.
    // The detection is cached; the recognition is skipped (nobody waits
    // for it), so the result cache stays empty.
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-snap@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 102, 64).await;
    let detector = slow_detector(1_500);
    let recognizer = slow_recognizer(0);
    test_models::install(
        &id,
        test_models::Stub {
            detector: Some(detector),
            recognizer: Some(recognizer),
        },
    );
    let body = serde_json::json!({
        "page": 0, "region": { "x": 0, "y": 0, "w": 40, "h": 30 },
        "lang": "western", "detect": true,
    });
    let detect_key = server::ocr::cache::detect_cache_key(&id, 0);

    send_and_abandon(&app, ocr_request(&admin, &id, body.clone()), || {
        detector.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        wait_for_key(&app, &detect_key).await,
        "a finished detection reaches the cache"
    );
    // Give the (skipped) recognition time to have happened if it were run.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        recognizer.calls.load(Ordering::SeqCst),
        0,
        "nobody waits for it"
    );

    // The retry detects nothing again and returns the snapped result.
    let (status, resp) = send(&app, ocr_request(&admin, &id, body)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["text"], "HELLO");
    assert_eq!(resp["refined_bbox"]["w"], 28, "{resp}");
    assert_eq!(detector.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recognition_that_outlives_its_request_is_cached() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-recognize@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 103, 64).await;
    let recognizer = slow_recognizer(1_500);
    test_models::install(
        &id,
        test_models::Stub {
            detector: None,
            recognizer: Some(recognizer),
        },
    );
    let body = serde_json::json!({
        "page": 0, "region": { "x": 4, "y": 4, "w": 40, "h": 30 }, "lang": "western",
    });
    server::ocr::cache::warm_fingerprints().await;
    let region_hash = server::ocr::cache::region_hash(4, 4, 40, 30);
    let key = server::ocr::cache::cache_key(&id, 0, "western", false, &region_hash);

    send_and_abandon(&app, ocr_request(&admin, &id, body.clone()), || {
        recognizer.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        wait_for_key(&app, &key).await,
        "a finished recognition reaches the cache"
    );

    let (status, resp) = send(&app, ocr_request(&admin, &id, body)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["text"], "HELLO");
    assert_eq!(
        recognizer.calls.load(Ordering::SeqCst),
        1,
        "the retry is a cache hit"
    );
}

// ─── S2: the decoded-page cache ──────────────────────────────────

fn region_body(x: u32) -> serde_json::Value {
    serde_json::json!({
        "page": 0, "region": { "x": x, "y": 2, "w": 10, "h": 10 }, "lang": "western",
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn region_requests_on_one_page_decode_it_once() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pages@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 104, 64).await;
    let recognizer = slow_recognizer(0);
    test_models::install(
        &id,
        test_models::Stub {
            detector: None,
            recognizer: Some(recognizer),
        },
    );
    // Five balloons at once on a cold page (one decodes, the others wait
    // for it), then five more one after the other.
    let together: Vec<_> = (0..5)
        .map(|x| send(&app, ocr_request(&admin, &id, region_body(x * 9))))
        .collect();
    for (status, body) in futures::future::join_all(together).await {
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    for x in 5..10 {
        let (status, body) = send(&app, ocr_request(&admin, &id, region_body(x * 9))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(recognizer.calls.load(Ordering::SeqCst), 10);
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads_for(&id),
        1,
        "ten regions, one decode"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hidden_issue_is_404_with_a_warm_page_cache() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-hidden-admin@example.com").await;
    let reader = register(&app, "ocr-work-hidden-reader@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 105, 64).await;
    let recognizer = slow_recognizer(0);
    test_models::install(
        &id,
        test_models::Stub {
            detector: Some(slow_detector(0)),
            recognizer: Some(recognizer),
        },
    );
    // The admin warms the page cache.
    let (status, body) = send(&app, ocr_request(&admin, &id, region_body(0))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(server::api::issue_ocr::ocr_page_reads_for(&id), 1);

    // A reader without access to the library: the unknown-issue 404, for
    // a new region and for the warm one, on both routes.
    for region in [region_body(30), region_body(0)] {
        let (status, body) = send(&app, ocr_request(&reader, &id, region)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"]["code"], "not_found");
    }
    let (status, body) = send(&app, regions_request(&reader, &id, 0)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        recognizer.calls.load(Ordering::SeqCst),
        1,
        "nothing ran for them"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_content_hash_is_a_page_cache_miss() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-rehash@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 106, 64).await;
    let new_hash = format!("{id}-rescanned");
    for hash in [&id, &new_hash] {
        test_models::install(
            hash,
            test_models::Stub {
                detector: None,
                recognizer: Some(slow_recognizer(0)),
            },
        );
    }
    let (status, body) = send(&app, ocr_request(&admin, &id, region_body(0))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(server::api::issue_ocr::ocr_page_reads_for(&id), 1);

    // A rescan retags the row with a new content hash.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let row = entity::issue::Entity::find_by_id(id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.content_hash = Set(new_hash.clone());
    am.update(&db).await.unwrap();

    let (status, body) = send(&app, ocr_request(&admin, &id, region_body(20))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads_for(&new_hash),
        1,
        "the new hash decodes the page again"
    );
    assert_eq!(server::api::issue_ocr::ocr_page_reads_for(&id), 1);
}

// ─── S7: regions in percent of the decoded page ──────────────────

/// A recognizer that reads the crop's size, so a test can tell which
/// rectangle it was handed.
struct EchoSize {
    calls: AtomicUsize,
}

impl Recognizer for EchoSize {
    fn recognize(&self, image: &DynamicImage) -> anyhow::Result<Recognition> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Recognition {
            text: format!("{}x{}", image.width(), image.height()),
            confidence: 0.9,
            words: None,
        })
    }
}

async fn echo_issue(
    app: &TestApp,
    root: &std::path::Path,
    w: u32,
    h: u32,
) -> (String, &'static EchoSize) {
    let id = seed(app, root, w, h).await;
    let echo: &'static EchoSize = Box::leak(Box::new(EchoSize {
        calls: AtomicUsize::new(0),
    }));
    test_models::install(
        &id,
        test_models::Stub {
            detector: None,
            recognizer: Some(echo),
        },
    );
    (id, echo)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_percent_region_reads_the_pixel_crop_and_shares_its_cache_entry() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, echo) = echo_issue(&app, &tmp.path().join("lib"), 200, 100).await;

    let (status, px) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western",
                "region": { "x": 20, "y": 10, "w": 50, "h": 30 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{px}");
    assert_eq!(px["text"], "50x30");
    assert_eq!(
        (px["page_w"].as_u64(), px["page_h"].as_u64()),
        (Some(200), Some(100))
    );

    // 10 % / 10 % / 25 % / 30 % of 200 × 100 is the same rectangle: the
    // cached entry answers, nothing is recognized again.
    let (status, pct) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western",
                "region_pct": { "x": 10.0, "y": 10.0, "w": 25.0, "h": 30.0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pct}");
    assert_eq!(pct["text"], "50x30");
    assert_eq!(pct["page_w"], 200);
    assert_eq!(echo.calls.load(Ordering::SeqCst), 1, "same cache entry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_percent_region_is_clamped_and_wins_over_a_pixel_region() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-clamp@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, _echo) = echo_issue(&app, &tmp.path().join("lib"), 201, 100).await;

    // Overshoots the right and bottom edges: cut to the page.
    let (status, body) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western",
                "region_pct": { "x": 90.0, "y": 80.0, "w": 20.0, "h": 30.0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // x: round(180.9)=181 .. 201, y: 80 .. 100.
    assert_eq!(body["text"], "20x20");

    // Overshoot at the left and top edges too.
    let (status, body) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western",
                "region_pct": { "x": -5.0, "y": -10.0, "w": 10.0, "h": 20.0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "10x10");

    // Both sent: the percent region is used (a pixel region that would
    // be out of bounds is not even looked at).
    let (status, body) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western",
                "region": { "x": 500, "y": 500, "w": 7, "h": 7 },
                "region_pct": { "x": 0.0, "y": 0.0, "w": 50.0, "h": 50.0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "101x50");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_percent_regions_are_422_with_field_details() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-bad@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, echo) = echo_issue(&app, &tmp.path().join("lib"), 202, 100).await;

    for (pct, field) in [
        (
            serde_json::json!({ "x": 10.0, "y": 10.0, "w": 0.0, "h": 5.0 }),
            "region_pct.w",
        ),
        (
            serde_json::json!({ "x": 10.0, "y": 10.0, "w": 5.0, "h": -3.0 }),
            "region_pct.h",
        ),
        (
            serde_json::json!({ "x": 150.0, "y": 10.0, "w": 5.0, "h": 5.0 }),
            "region_pct",
        ),
        (
            serde_json::json!({ "x": -50.0, "y": 10.0, "w": 20.0, "h": 5.0 }),
            "region_pct",
        ),
        (
            serde_json::json!({ "x": 10.0, "y": 100.0, "w": 5.0, "h": 5.0 }),
            "region_pct",
        ),
        // On the page, but less than half a pixel wide once converted
        // (checked after the decode).
        (
            serde_json::json!({ "x": 50.0, "y": 50.0, "w": 0.1, "h": 0.1 }),
            "region_pct",
        ),
    ] {
        let (status, body) = send(
            &app,
            ocr_request(
                &admin,
                &id,
                serde_json::json!({ "page": 0, "lang": "western", "region_pct": pct }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{pct}: {body}");
        assert_eq!(body["error"]["code"], "validation", "{body}");
        let fields: Vec<&str> = body["error"]["details"]
            .as_array()
            .unwrap_or_else(|| panic!("details for {pct}: {body}"))
            .iter()
            .filter_map(|d| d["field"].as_str())
            .collect();
        assert!(fields.contains(&field), "{pct}: {body}");
    }

    // JSON has no NaN or infinity; a number too large for f64 is
    // refused as a malformed body (garde's finiteness rule is unit-tested).
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/me/issues/{id}/ocr"))
        .header(header::COOKIE, cookie_header(&admin))
        .header("X-CSRF-Token", &admin.csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"page":0,"region_pct":{"x":1e999,"y":0,"w":5,"h":5}}"#,
        ))
        .unwrap();
    let (status, _) = send(&app, req).await;
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "{status}"
    );

    // Neither region.
    let (status, body) = send(
        &app,
        ocr_request(
            &admin,
            &id,
            serde_json::json!({ "page": 0, "lang": "western" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["field"], "region", "{body}");
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
}

// ─── S8: a cached percent request needs no permit ────────────────

fn pct_body() -> serde_json::Value {
    serde_json::json!({ "page": 0, "lang": "western",
        "region_pct": { "x": 10.0, "y": 10.0, "w": 25.0, "h": 30.0 } })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cached_percent_request_needs_no_permit_and_reads_no_page() {
    let _alone = ALL_PERMITS.write().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-fast@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, echo) = echo_issue(&app, &tmp.path().join("lib"), 210, 100).await;

    let (status, first) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(
        wait_for_key(&app, &server::ocr::cache::page_size_key(&id, 0)).await,
        "the decode stored the page size"
    );
    let reads = server::api::issue_ocr::ocr_page_reads_for(&id);
    assert_eq!(reads, 1);

    // Every permit held by other work.
    let sem = server::api::issue_ocr::ocr_semaphore();
    let held = sem
        .acquire_many(server::api::issue_ocr::OCR_MAX_CONCURRENT as u32)
        .await
        .unwrap();
    let t = std::time::Instant::now();
    let (status, body) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    let took = t.elapsed();
    drop(held);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        took < server::api::issue_ocr::OCR_PERMIT_WAIT / 2,
        "answered without waiting for a permit ({took:?})"
    );
    assert_eq!(body["text"], "53x30");
    assert_eq!(
        (body["page_w"].as_u64(), body["page_h"].as_u64()),
        (Some(210), Some(100))
    );
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads_for(&id),
        reads,
        "no page read"
    );
    assert_eq!(
        echo.calls.load(Ordering::SeqCst),
        1,
        "nothing recognized again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_size_outlives_the_decoded_page() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-size@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, _echo) = echo_issue(&app, &tmp.path().join("lib"), 211, 100).await;

    let (status, body) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads_for(&id),
        1,
        "decoded once"
    );
    assert!(wait_for_key(&app, &server::ocr::cache::page_size_key(&id, 0)).await);

    // As if the decoded page had expired.
    server::ocr::page_cache::decoded_pages().forget_for_tests(&id);
    let (status, again) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["text"], body["text"]);
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads_for(&id),
        1,
        "no second read"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hidden_issue_is_404_with_the_size_and_result_cached() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-hidden-admin@example.com").await;
    let reader = register(&app, "ocr-work-pct-hidden-reader@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, echo) = echo_issue(&app, &tmp.path().join("lib"), 212, 100).await;

    let (status, body) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(wait_for_key(&app, &server::ocr::cache::page_size_key(&id, 0)).await);

    let (status, body) = send(&app, ocr_request(&reader, &id, pct_body())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");
    assert!(body.get("text").is_none());
    assert_eq!(echo.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stored_size_that_disagrees_with_the_decode_is_replaced() {
    use redis::AsyncCommands;
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-stale@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, _echo) = echo_issue(&app, &tmp.path().join("lib"), 213, 100).await;
    let size_key = server::ocr::cache::page_size_key(&id, 0);
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis.set_ex(&size_key, "400x300", 600).await.unwrap();

    let (status, body) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Converted with the decoded 213 x 100 (x 21..75, y 10..40), not the
    // stored 400 x 300.
    assert_eq!(
        (body["page_w"].as_u64(), body["page_h"].as_u64()),
        (Some(213), Some(100))
    );
    assert_eq!(body["text"], "54x30");
    let stored: Option<String> = redis.get(&size_key).await.unwrap();
    assert_eq!(stored.as_deref(), Some("213x100"), "the decode replaced it");
}

/// A result payload computed on a page of another size (a percent rectangle
/// that maps onto the same pixels of a different page) is not served after
/// the decode: the recognizer runs and the entry is overwritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_payload_of_another_page_size_is_not_served() {
    use redis::AsyncCommands;
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-pct-other-size@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (id, echo) = echo_issue(&app, &tmp.path().join("lib"), 216, 100).await;

    // No page size stored yet, so the lookup happens after the decode, under
    // the key the percent request converts to on 216 x 100.
    let r = server::api::issue_ocr::OcrRegionPct {
        x: 10.0,
        y: 10.0,
        w: 25.0,
        h: 30.0,
    }
    .to_pixels(216, 100)
    .unwrap();
    server::ocr::cache::warm_fingerprints().await;
    let key = server::ocr::cache::cache_key(
        &id,
        0,
        "western",
        false,
        &server::ocr::cache::region_hash(r.x, r.y, r.w, r.h),
    );
    let mut redis = app.state().jobs.redis.clone();
    let seeded = serde_json::json!({ "text": "stale", "confidence": 0.9_f64,
        "page_w": 999_u32, "page_h": 999_u32 })
    .to_string();
    let _: () = redis.set_ex(&key, seeded, 600).await.unwrap();

    let (status, body) = send(&app, ocr_request(&admin, &id, pct_body())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(body["text"], "stale", "the other page's payload was served");
    assert_eq!(body["text"], "54x30");
    assert_eq!(echo.calls.load(Ordering::SeqCst), 1, "the recognizer ran");

    // The detached task overwrites the entry with this page's size.
    let mut stored = None;
    for _ in 0..200 {
        let raw: Option<String> = redis.get(&key).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw.unwrap()).unwrap();
        if value["page_w"] == 216 {
            stored = Some(value);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stored = stored.expect("the entry was overwritten");
    assert_eq!(
        (stored["page_w"].as_u64(), stored["page_h"].as_u64()),
        (Some(216), Some(100))
    );
    assert_eq!(stored["text"], "54x30");
}

// ─── Review follow-ups ───────────────────────────────────────────

/// A recognition that fails (an error or a panic from the engine) still
/// leaves the detection it followed in the cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_recognition_keeps_its_detection() {
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-fail-recognize@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    for (n, panic) in [(0, false), (1, true)] {
        let id = seed(&app, &tmp.path().join(format!("lib{n}")), 120, 64).await;
        let detector = slow_detector(0);
        let recognizer: &'static FailingRecognizer = Box::leak(Box::new(FailingRecognizer {
            panic,
            calls: AtomicUsize::new(0),
        }));
        test_models::install(
            &id,
            test_models::Stub {
                detector: Some(detector),
                recognizer: Some(recognizer),
            },
        );
        let body = serde_json::json!({
            "page": 0, "region": { "x": 0, "y": 0, "w": 40, "h": 30 },
            "lang": "western", "detect": true,
        });
        let (status, resp) = send(&app, ocr_request(&admin, &id, body)).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "panic={panic}: {resp}"
        );
        assert_eq!(resp["error"]["code"], "ocr_failed");
        assert_eq!(recognizer.calls.load(Ordering::SeqCst), 1);
        assert!(
            wait_for_key(&app, &server::ocr::cache::detect_cache_key(&id, 0)).await,
            "panic={panic}: the detection is cached"
        );
        // The text-regions route now answers from it.
        let (status, regions) = send(&app, regions_request(&admin, &id, 0)).await;
        assert_eq!(status, StatusCode::OK, "{regions}");
        assert_eq!(
            detector.calls.load(Ordering::SeqCst),
            1,
            "no second detection"
        );
    }
}

/// Two issues with identical bytes (one content hash) in two libraries, a
/// user who can see only one: with the decoded page, the page size, the
/// result and the detection all cached from the visible one, the hidden
/// one is still the unknown-issue 404 on both routes, and nothing runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_bytes_in_a_hidden_library_stay_hidden() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let _shared = ALL_PERMITS.read().await;
    let app = TestApp::spawn().await;
    let _admin = register(&app, "ocr-work-twin-admin@example.com").await;
    let reader = register(&app, "ocr-work-twin-reader@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let (root_a, root_b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&root_b).unwrap();
    // Library A, visible to the reader.
    let hash = seed(&app, &root_a, 215, 100).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let visible = entity::issue::Entity::find_by_id(hash.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    entity::library_user_access::ActiveModel {
        library_id: Set(visible.library_id),
        user_id: Set(reader.user_id),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    // Library B holds the same bytes under another issue id.
    let lib_b = LibrarySeed::new(&root_b).insert(&db).await;
    let series_b = SeriesSeed::new(lib_b, "Twin").insert(&db).await;
    let twin_path = root_b.join("issue.cbz");
    std::fs::copy(root_a.join("issue.cbz"), &twin_path).unwrap();
    let hidden_id = uuid::Uuid::now_v7().to_string();
    let mut twin: entity::issue::ActiveModel = visible.clone().into();
    twin.id = Set(hidden_id.clone());
    twin.slug = Set(uuid::Uuid::now_v7().to_string());
    twin.library_id = Set(lib_b);
    twin.series_id = Set(series_b);
    twin.file_path = Set(twin_path.to_string_lossy().into_owned());
    twin.insert(&db).await.unwrap();

    let detector = slow_detector(0);
    let echo: &'static EchoSize = Box::leak(Box::new(EchoSize {
        calls: AtomicUsize::new(0),
    }));
    test_models::install(
        &hash,
        test_models::Stub {
            detector: Some(detector),
            recognizer: Some(echo),
        },
    );
    let pixel = serde_json::json!({ "page": 0, "lang": "western",
        "region": { "x": 4, "y": 4, "w": 20, "h": 20 } });

    // Warm everything through the visible issue.
    for req in [
        ocr_request(&reader, &hash, pct_body()),
        ocr_request(&reader, &hash, pixel.clone()),
        regions_request(&reader, &hash, 0),
    ] {
        let (status, body) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert!(wait_for_key(&app, &server::ocr::cache::page_size_key(&hash, 0)).await);
    let reads = server::api::issue_ocr::ocr_page_reads_for(&hash);
    let (recognized, detected) = (
        echo.calls.load(Ordering::SeqCst),
        detector.calls.load(Ordering::SeqCst),
    );

    for req in [
        ocr_request(&reader, &hidden_id, pct_body()),
        ocr_request(&reader, &hidden_id, pixel.clone()),
        regions_request(&reader, &hidden_id, 0),
    ] {
        let (status, body) = send(&app, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"]["code"], "not_found");
    }
    assert_eq!(server::api::issue_ocr::ocr_page_reads_for(&hash), reads);
    assert_eq!(echo.calls.load(Ordering::SeqCst), recognized);
    assert_eq!(detector.calls.load(Ordering::SeqCst), detected);
}

// ─── The rollback switch ─────────────────────────────────────────

/// A detector that finds three boxes side by side, one of each class
/// index (0, 1 and 2).
struct ThreeClassDetector;

impl PageDetector for ThreeClassDetector {
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection> {
        Ok(CachedDetection {
            page_w: page.width(),
            page_h: page.height(),
            bboxes: [(0, 4.0), (1, 34.0), (2, 64.0)]
                .into_iter()
                .map(|(class, x)| CachedBbox {
                    xmin: x,
                    ymin: 4.0,
                    xmax: x + 24.0,
                    ymax: 40.0,
                    confidence: 0.9,
                    class,
                })
                .collect(),
        })
    }
}

/// A recognizer that reads a word between two bars, the way a balloon's
/// borders come back from Tesseract.
struct BarsRecognizer;

impl Recognizer for BarsRecognizer {
    fn recognize(&self, _image: &DynamicImage) -> anyhow::Result<Recognition> {
        Ok(Recognition {
            text: "| HELLO |".to_owned(),
            confidence: 0.9,
            words: None,
        })
    }
}

/// What the two routes answer for one issue with the stand-ins above,
/// and which of the four cache keys (legacy / default, detection /
/// result) then exist.
struct SwitchOutcome {
    region_classes: Vec<u64>,
    text: String,
    legacy_keys: (bool, bool),
    default_keys: (bool, bool),
}

async fn outcome_with_the_three_class_stand_ins(app: &TestApp, email: &str) -> SwitchOutcome {
    use server::ocr::{EngineSet, cache};
    let admin = register(app, email).await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(app, &tmp.path().join("lib"), 100, 64).await;
    test_models::install(
        &id,
        test_models::Stub {
            detector: Some(Box::leak(Box::new(ThreeClassDetector))),
            recognizer: Some(Box::leak(Box::new(BarsRecognizer))),
        },
    );

    let (status, regions) = send(app, regions_request(&admin, &id, 0)).await;
    assert_eq!(status, StatusCode::OK, "{regions}");
    let region_classes = regions["regions"]
        .as_array()
        .expect("regions")
        .iter()
        .map(|r| r["class"].as_u64().expect("class"))
        .collect();

    let body = serde_json::json!({
        "page": 0, "region": { "x": 4, "y": 4, "w": 24, "h": 36 }, "lang": "western",
    });
    let (status, read) = send(app, ocr_request(&admin, &id, body)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        read["raw_text"].as_str().unwrap_or("| HELLO |"),
        "| HELLO |"
    );

    cache::warm_fingerprints_for(EngineSet::Legacy).await;
    cache::warm_fingerprints_for(EngineSet::Current).await;
    let region_hash = cache::region_hash(4, 4, 24, 36);
    let legacy = (
        cache::detect_cache_key_for(EngineSet::Legacy, &id, 0),
        cache::cache_key_for(EngineSet::Legacy, &id, 0, "western", false, &region_hash),
    );
    let default = (
        cache::detect_cache_key(&id, 0),
        cache::cache_key(&id, 0, "western", false, &region_hash),
    );
    assert_ne!(legacy.0, default.0);
    assert_ne!(legacy.1, default.1);
    // Both routes answered, so both writes were started; give the
    // detached tasks a moment to land them before looking for absences.
    for _ in 0..200 {
        let written = (redis_has(app, &legacy.0).await || redis_has(app, &default.0).await)
            && (redis_has(app, &legacy.1).await || redis_has(app, &default.1).await);
        if written {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    SwitchOutcome {
        region_classes,
        text: read["text"].as_str().expect("text").to_owned(),
        legacy_keys: (
            redis_has(app, &legacy.0).await,
            redis_has(app, &legacy.1).await,
        ),
        default_keys: (
            redis_has(app, &default.0).await,
            redis_has(app, &default.1).await,
        ),
    }
}

/// `COMIC_OCR_LEGACY_ENGINES` through the routes, with stand-ins for the
/// models: the legacy set offers every class of its detector as a region,
/// cleans the text with the Tesseract rules, and keeps its detections and
/// results under its own cache keys. The same stand-ins on a server
/// without the switch show the other side of each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_legacy_switch_selects_regions_cleanup_and_cache_keys() {
    let _shared = ALL_PERMITS.read().await;

    let legacy_app = TestApp::spawn_with_legacy_ocr_engines().await;
    let legacy =
        outcome_with_the_three_class_stand_ins(&legacy_app, "ocr-work-legacy@example.com").await;
    // Blocks and lines alike are regions of the legacy detector.
    assert_eq!(legacy.region_classes, [0, 1, 2]);
    // The Tesseract chain drops the bars at the line's edges.
    assert_eq!(legacy.text, "HELLO");
    assert_eq!(
        legacy.legacy_keys,
        (true, true),
        "written under the legacy keys"
    );
    assert_eq!(
        legacy.default_keys,
        (false, false),
        "nothing under the default keys"
    );

    let default_app = TestApp::spawn().await;
    let default =
        outcome_with_the_three_class_stand_ins(&default_app, "ocr-work-default@example.com").await;
    // Balloons and free text; the text inside a balloon is not offered.
    assert_eq!(default.region_classes, [0, 2]);
    // The line recogniser's cleanup keeps what was read.
    assert_eq!(default.text, "| HELLO |");
    assert_eq!(default.default_keys, (true, true));
    assert_eq!(default.legacy_keys, (false, false));
}
