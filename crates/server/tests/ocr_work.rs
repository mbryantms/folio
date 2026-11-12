//! The OCR work behind `api::issue_ocr`, driven through the routes with
//! debug-build stand-ins for the detector and recognizer
//! (`ocr::pipeline::test_models`, keyed by content hash) instead of the
//! real models, which download hundreds of MB on first use.
//!
//! - Finished work reaches the cache even when the request that started
//!   it is gone (route timeout, client abort): the work owns the permit
//!   and writes the cache itself, so a retry is a cache hit.

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

// ─── Stand-ins ───────────────────────────────────────────────────

/// A detector that takes `delay` and finds one box.
struct SlowDetector {
    delay: Duration,
    calls: AtomicUsize,
}

impl PageDetector for SlowDetector {
    fn detect(&self, page: &DynamicImage) -> anyhow::Result<CachedDetection> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
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
        calls: AtomicUsize::new(0),
    }))
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
    Authed {
        session: cookie("__Host-comic_session"),
        csrf: cookie("__Host-comic_csrf"),
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

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([250, 250, 250]));
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

/// One issue whose only page is a `w × h` PNG. Every test picks its own
/// size, so its content hash (= the issue id) is its own and the
/// stand-ins it installs reach only its requests.
async fn seed(app: &TestApp, root: &std::path::Path, w: u32, h: u32) -> String {
    std::fs::create_dir_all(root).unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = LibrarySeed::new(root).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Series").insert(&db).await;
    IssueSeed::new(
        lib_id,
        series_id,
        &root.join("issue.cbz"),
        &cbz(&png(w, h)),
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
    let app = TestApp::spawn().await;
    let admin = register(&app, "ocr-work-detect@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let id = seed(&app, &tmp.path().join("lib"), 101, 64).await;
    let detector = slow_detector(1_500);
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
