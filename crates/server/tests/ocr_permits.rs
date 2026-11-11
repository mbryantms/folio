//! The OCR semaphore at route level (`api::issue_ocr`). Its own test
//! binary: the semaphore is process-wide, and these tests hold every
//! permit, which would stall OCR tests running beside them.
//!
//! With every permit held:
//! - a result-cache hit answers at once (it takes no permit);
//! - the cheap answers (a missing file, a page past the end) come back at
//!   once, never queued behind OCR work;
//! - a request that needs the work waits for a permit and then answers
//!   503 `service_unavailable` with `Retry-After` (when this host can load
//!   the recognizer; without the models it fails at model preparation,
//!   before any permit, which is also what the order promises);
//! - no page bytes are read by any of these: the page is read only inside
//!   a permit (`ocr_page_reads` stays put while every permit is held).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use std::io::Write;
use std::time::{Duration, Instant};
use tower::ServiceExt;

struct Authed {
    session: String,
    csrf: String,
}

async fn register(app: &TestApp) -> Authed {
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

async fn post_ocr(
    app: &TestApp,
    a: &Authed,
    id: &str,
    page: u32,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let body = serde_json::json!({
        "page": page, "region": { "x": 0, "y": 0, "w": 20, "h": 20 }, "lang": "western"
    });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/me/issues/{id}/ocr"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        a.session, a.csrf
                    ),
                )
                .header("X-CSRF-Token", &a.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([250, 250, 250, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_work_takes_an_ocr_permit() {
    use redis::AsyncCommands;
    let app = TestApp::spawn().await;
    let admin = register(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.txt"), b"x").unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = LibrarySeed::new(&root).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Series").insert(&db).await;
    let good_path = root.join("good.cbz");
    let good = IssueSeed::new(lib_id, series_id, &good_path, &cbz(&png(64, 64)), 1.0)
        .insert(&db)
        .await;
    let gone_path = root.join("gone.cbz");
    let gone = IssueSeed::new(lib_id, series_id, &gone_path, &cbz(&png(65, 64)), 2.0)
        .insert(&db)
        .await;
    std::fs::remove_file(&gone_path).unwrap();

    // Hold every permit.
    let sem = server::api::issue_ocr::ocr_semaphore();
    let held = sem
        .acquire_many(server::api::issue_ocr::OCR_MAX_CONCURRENT as u32)
        .await
        .unwrap();
    assert_eq!(sem.available_permits(), 0);
    // Nothing else in this binary runs OCR, so the count only moves if a
    // request below reads a page.
    let reads_before = server::api::issue_ocr::ocr_page_reads();
    let quick = server::api::issue_ocr::OCR_PERMIT_WAIT / 2;

    // A cache hit takes none.
    let region_hash = server::ocr::cache::region_hash(0, 0, 20, 20);
    let key = server::ocr::cache::cache_key(&good, 0, "western", false, &region_hash);
    let mut redis = app.state().jobs.redis.clone();
    let _: () = redis
        .set_ex(
            &key,
            serde_json::json!({ "text": "cached", "confidence": 1.0_f64 }).to_string(),
            60,
        )
        .await
        .unwrap();
    let t = Instant::now();
    let (status, _, body) = post_ocr(&app, &admin, &good, 0).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "cached");
    assert!(t.elapsed() < quick, "a cache hit never waits for a permit");
    let _: () = redis.del(&key).await.unwrap();

    // The cheap answers never wait either.
    let t = Instant::now();
    let (status, _, body) = post_ocr(&app, &admin, &gone, 0).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let (status, _, _) = post_ocr(&app, &admin, &good, 9).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(t.elapsed() < quick, "404 / 410 are not queued behind OCR");

    // The work needs a permit: with none free it waits, then answers 503.
    let t = Instant::now();
    let (status, headers, body) = post_ocr(&app, &admin, &good, 0).await;
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        // This host cannot load the recognizer; model preparation runs
        // before the permit and failed there.
        let code = body["error"]["code"].as_str().unwrap_or_default();
        assert!(
            code == "ocr_failed" || code == "ocr_model_unverified",
            "{body}"
        );
    } else {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"]["code"], "service_unavailable");
        assert_eq!(headers[header::RETRY_AFTER], "2");
        assert!(
            t.elapsed() >= server::api::issue_ocr::OCR_PERMIT_WAIT - Duration::from_millis(50),
            "waited for a permit"
        );
    }
    assert_eq!(
        server::api::issue_ocr::ocr_page_reads(),
        reads_before,
        "no page bytes are read without an OCR permit"
    );
    drop(held);
    assert_eq!(
        sem.available_permits(),
        server::api::issue_ocr::OCR_MAX_CONCURRENT
    );
}
