//! A9 (2026-11 audit remediation): the per-user `pages` bucket on
//! `GET /issues/{id}/pages/{n}` and the per-user `thumbnails` bucket shared
//! by `GET /issues/{id}/pages/{n}/thumb` and `GET /issues/{id}/covers/{id}`.
//!
//! - Past the burst, the next request is a 429 in the canonical envelope
//!   with `Retry-After` (at least 1 s), answered before the handler runs.
//! - Requests without a session key by client address, so an anonymous
//!   flood is limited, and the signed-in users behind that same address
//!   keep reading: their requests are keyed by user, not by address.
//! - The busiest legitimate client pattern measured for each bucket fits.
//!
//! The buckets are the real compile-time constants, and the governor runs
//! before the handler, so a cheap request spends a token exactly as a page
//! read does. The drains are anonymous: a request with an access token
//! costs two Ed25519 verifications (the key, then the session check),
//! about 10 ms in a debug build, which cannot outpace a 250/s or 500/s
//! refill on a CI runner. That two users' budgets are independent at
//! these constants is the unit test
//! `rate_limit::tests::page_and_thumbnail_budgets_are_per_user`; that a
//! signed-in key names its user is `tests/rate_limits_per_user.rs`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use server::middleware::rate_limit::{Bucket, PAGES, THUMBNAILS};
use std::io::Write;
use std::time::{Duration, Instant};
use tower::ServiceExt;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// Requests in flight per drain wave: enough that a drain outpaces the
/// refill (250/s and 500/s) on a loaded runner.
const WAVE: usize = 64;

async fn register(app: &TestApp, email: &str) -> String {
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
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .expect("session cookie")
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_session=")
        .to_owned()
}

/// One GET, as `session` or anonymous: `(status, headers, body)`.
async fn get(app: &TestApp, session: Option<&str>, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(Method::GET).uri(path);
    if let Some(s) = session {
        req = req.header(header::COOKIE, format!("__Host-comic_session={s}"));
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

/// `n` requests at once (in waves of [`WAVE`]), cycling through `paths`.
/// Returns how many answered 429.
async fn burst(app: &TestApp, session: Option<&str>, paths: &[String], n: usize) -> usize {
    let mut limited = 0;
    let mut sent = 0;
    while sent < n {
        let wave = (sent..n.min(sent + WAVE)).map(|i| get(app, session, &paths[i % paths.len()]));
        sent = n.min(sent + WAVE);
        limited += futures::future::join_all(wave)
            .await
            .into_iter()
            .filter(|(s, _, _)| *s == StatusCode::TOO_MANY_REQUESTS)
            .count();
    }
    limited
}

/// Send waves through `paths` until one answers 429; returns how many got
/// through before it and that 429. Panics if nothing trips within 60 s.
async fn drain(
    app: &TestApp,
    session: Option<&str>,
    paths: &[String],
) -> (usize, HeaderMap, serde_json::Value) {
    let started = Instant::now();
    let mut allowed = 0;
    let mut i = 0;
    while started.elapsed() < Duration::from_secs(60) {
        let start = i;
        i += WAVE;
        let wave = (0..WAVE).map(|k| get(app, session, &paths[(start + k) % paths.len()]));
        let mut hit = None;
        for (status, headers, body) in futures::future::join_all(wave).await {
            if status == StatusCode::TOO_MANY_REQUESTS {
                hit.get_or_insert((headers, body));
            } else {
                allowed += 1;
            }
        }
        if let Some((headers, body)) = hit {
            return (allowed, headers, serde_json::from_slice(&body).unwrap());
        }
    }
    panic!("no 429 after {allowed} requests in 60 s");
}

fn assert_rate_limited(
    bucket: &Bucket,
    allowed: usize,
    headers: &HeaderMap,
    body: &serde_json::Value,
) {
    assert!(
        allowed >= bucket.burst as usize,
        "{} tripped after only {allowed} requests (burst {})",
        bucket.name,
        bucket.burst
    );
    let retry_after: u64 = headers
        .get(header::RETRY_AFTER)
        .expect("429 carries Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry_after >= 1);
    assert_eq!(body["error"]["code"], "rate_limited");
    assert_eq!(body["error"]["retry_after_seconds"], retry_after);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains(bucket.name),
        "message names the bucket: {body}"
    );
}

fn cbz(pages: usize) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..pages {
        zw.start_file(format!("page-{i:03}.png"), opts).unwrap();
        let mut page = PNG.to_vec();
        page.extend(std::iter::repeat_n(i as u8, 256));
        zw.write_all(&page).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

/// A library holding one readable 3-page issue; returns `(tempdir, file,
/// issue id)`. The first registered user (an admin) sees it.
async fn seeded_issue(app: &TestApp) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.txt"), b"x").unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = LibrarySeed::new(&root).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Series").insert(&db).await;
    let path = root.join("issue.cbz");
    let id = IssueSeed::new(lib_id, series_id, &path, &cbz(3), 1.0)
        .with_page_count(3)
        .insert(&db)
        .await;
    (tmp, path, id)
}

/// The busiest measured patterns, sent all at once, which is stricter than
/// their real pace: holding the next-page key through a 500-page issue
/// (about 2 page URLs per turn, about 1,000 requests over 17 s), and on
/// the thumbnail side two page-editor opens of a 500-page issue (500
/// thumbnails each), the reader strip's 25-thumbnail warm and a fast
/// fling through ten 60-card grid pages of covers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_busiest_reader_and_grid_patterns_fit() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;

    let pages: Vec<String> = (0..500)
        .map(|n| format!("/issues/no-such-issue/pages/{n}"))
        .collect();
    assert_eq!(burst(&app, Some(&alice), &pages, 1_000).await, 0);

    let thumbs: Vec<String> = (0..500)
        .map(|n| format!("/issues/no-such-issue/pages/{n}/thumb?variant=strip"))
        .collect();
    let covers: Vec<String> = (0..600)
        .map(|n| format!("/issues/cover-{n}/pages/0/thumb"))
        .collect();
    assert_eq!(burst(&app, Some(&alice), &thumbs, 1_025).await, 0);
    assert_eq!(burst(&app, Some(&alice), &covers, 600).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_anonymous_page_flood_is_limited_before_the_handler() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let (_tmp, _file, id) = seeded_issue(&app).await;
    let page = format!("/issues/{id}/pages/0");

    // Below the limit an anonymous request reaches the session check and
    // is refused there (401), never served.
    assert_eq!(get(&app, None, &page).await.0, StatusCode::UNAUTHORIZED);
    let (allowed, headers, body) = drain(&app, None, std::slice::from_ref(&page)).await;
    assert_rate_limited(&PAGES, allowed, &headers, &body);
    // Past it the answer is the 429, not the handler's 401: the request
    // never reaches the handler, let alone an archive.

    // The signed-in user behind the same address reads normally, and the
    // thumbnail store is untouched.
    assert_eq!(get(&app, Some(&alice), &page).await.0, StatusCode::OK);
    assert_eq!(
        get(&app, None, &format!("/issues/{id}/pages/0/thumb"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thumbnails_and_stored_covers_share_one_store() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let (_tmp, _file, id) = seeded_issue(&app).await;

    let thumbs: Vec<String> = (0..8)
        .map(|n| format!("/issues/{id}/pages/{n}/thumb?variant=strip"))
        .collect();
    let (allowed, headers, body) = drain(&app, None, &thumbs).await;
    assert_rate_limited(&THUMBNAILS, allowed, &headers, &body);

    // A stored cover draws on the same store. Drain it with cover
    // requests alone: had covers a store of their own, a full burst (4,000)
    // would get through first; sharing the drained store, only what refilled
    // meanwhile does. The bound does not depend on request latency, only on
    // a drain wave finishing well inside the two seconds half a burst takes
    // to refill at 500/s.
    let cover = format!("/issues/{id}/covers/no-such-cover");
    let (allowed, headers, body) = drain(&app, None, std::slice::from_ref(&cover)).await;
    assert!(
        allowed < THUMBNAILS.burst as usize / 2,
        "{allowed} cover requests got through a drained thumbnail store"
    );
    assert!(headers.contains_key(header::RETRY_AFTER));
    assert_eq!(body["error"]["code"], "rate_limited");

    // Page bytes are a separate store; the signed-in user is unaffected.
    assert_eq!(
        get(&app, None, &format!("/issues/{id}/pages/0")).await.0,
        StatusCode::UNAUTHORIZED
    );
    // (The fixture pages are not decodable images, so the thumbnail
    // itself fails to generate; what matters is that it is not a 429.)
    let (status, _, _) = get(
        &app,
        Some(&alice),
        &format!("/issues/{id}/pages/0/thumb?variant=strip"),
    )
    .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}
