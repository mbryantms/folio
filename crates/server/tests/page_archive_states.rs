//! Missing, malformed, unreadable and changed archives across the routes
//! that open an issue's archive (audit S-23 / R-05 / R-06a). The status
//! mapping lives in `api::page_source`; these tests pin it from outside:
//!
//! - visibility is checked before anything about the row or the file, so
//!   a caller without a grant gets the unknown-id answer, byte for byte,
//!   and no archive is opened for them;
//! - a `malformed` row whose archive opens serves its pages, one whose
//!   archive is broken answers 410 and is read once per window;
//! - an unconverted CBR answers 409, an offline library 503, an I/O error
//!   503 with `Retry-After`;
//! - the page-stream and variant ETags (and the variant bytes) follow the
//!   file, by rename-over and by in-place rewrite.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::issue::IssueState;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp, email: &str) -> Authed {
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
    let json: serde_json::Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id: Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

/// `(status, headers, body)` of one request.
async fn call(
    app: &TestApp,
    user: Option<&Authed>,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    if let Some(u) = user {
        req = req.header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                u.session, u.csrf
            ),
        );
        if method != Method::GET {
            req = req.header("X-CSRF-Token", &u.csrf);
        }
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn code(bytes: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .map(|v| v["error"]["code"].as_str().unwrap_or_default().to_owned())
        .unwrap_or_default()
}

/// A CBZ (Stored) of PNG-signed pages, distinct per `seed`.
fn cbz_bytes(pages: &[Vec<u8>]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (i, p) in pages.iter().enumerate() {
        zw.start_file(format!("page-{i:03}.png"), opts).unwrap();
        zw.write_all(p).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn sniff_page(len: usize, seed: u8) -> Vec<u8> {
    let mut v = PNG.to_vec();
    let mut i = 0usize;
    while v.len() < len {
        v.push(((i.wrapping_mul(31) + seed as usize) % 251) as u8);
        i += 1;
    }
    v
}

/// A decodable PNG, `w` wide, coloured by `shade`.
fn real_png(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([shade, 40, 180, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

struct Lib {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    lib_id: Uuid,
    series_id: Uuid,
}

async fn library(app: &TestApp) -> Lib {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    // Other content in the library, so a missing file is a missing file
    // and not an unmounted (empty) root.
    std::fs::write(root.join("README.txt"), b"x").unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let lib_id = LibrarySeed::new(&root).insert(&db).await;
    let series_id = SeriesSeed::new(lib_id, "Series").insert(&db).await;
    Lib {
        _tmp: tmp,
        root,
        lib_id,
        series_id,
    }
}

async fn issue(app: &TestApp, lib: &Lib, name: &str, bytes: &[u8], state: IssueState) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let path = lib.root.join(name);
    IssueSeed::new(lib.lib_id, lib.series_id, &path, bytes, 1.0)
        .with_state(state)
        .with_page_count(3)
        .insert(&db)
        .await
}

fn truncate_half(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    std::fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
}

/// Requests one caller makes against one issue id on every route that
/// opens the archive. The page stream carries a URL signed for `user`.
fn route_requests(
    app: &TestApp,
    id: &str,
    user: &Authed,
) -> Vec<(&'static str, Method, String, Option<serde_json::Value>)> {
    let pse = server::auth::url_signing::issue_query(
        id,
        user.user_id,
        app.state().secrets.url_signing_key.as_ref(),
    );
    vec![
        ("page", Method::GET, format!("/issues/{id}/pages/0"), None),
        (
            "variant",
            Method::GET,
            format!("/issues/{id}/pages/0?w=480"),
            None,
        ),
        (
            "thumb",
            Method::GET,
            format!("/issues/{id}/pages/1/thumb?variant=strip"),
            None,
        ),
        ("pse", Method::GET, format!("/opds/pse/{id}/0?{pse}"), None),
        (
            "ocr",
            Method::POST,
            format!("/api/me/issues/{id}/ocr"),
            Some(serde_json::json!({
                "page": 0, "region": { "x": 0, "y": 0, "w": 4, "h": 4 }, "lang": "western"
            })),
        ),
        (
            "regions",
            Method::GET,
            format!("/api/me/issues/{id}/pages/0/text-regions"),
            None,
        ),
    ]
}

/// Visibility first: to a caller without a grant, a missing file, a
/// malformed row and a broken archive all answer exactly like an unknown
/// id, on every route, and nothing is opened for them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_issues_answer_like_unknown_whatever_their_file() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let nogrant = register(&app, "nogrant@example.com").await;
    let lib = library(&app).await;

    let missing = issue(
        &app,
        &lib,
        "missing.cbz",
        &cbz_bytes(&[sniff_page(300, 1)]),
        IssueState::Active,
    )
    .await;
    std::fs::remove_file(lib.root.join("missing.cbz")).unwrap();
    let malformed = issue(
        &app,
        &lib,
        "malformed.cbz",
        &cbz_bytes(&[sniff_page(300, 2)]),
        IssueState::Malformed,
    )
    .await;
    truncate_half(&lib.root.join("malformed.cbz"));
    let encrypted = issue(
        &app,
        &lib,
        "encrypted.cbz",
        &cbz_bytes(&[sniff_page(300, 3)]),
        IssueState::Encrypted,
    )
    .await;
    let unknown = blake3::hash(Uuid::now_v7().as_bytes()).to_hex().to_string();

    let before = app.state().zip_lru.open_attempts();
    for hidden in [&missing, &malformed, &encrypted] {
        let theirs = route_requests(&app, hidden, &nogrant);
        let unknowns = route_requests(&app, &unknown, &nogrant);
        for ((route, m, path, body), (_, um, upath, ubody)) in theirs.into_iter().zip(unknowns) {
            let (status, headers, bytes) = call(&app, Some(&nogrant), m, &path, body).await;
            let (ustatus, _, ubytes) = call(&app, Some(&nogrant), um, &upath, ubody).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{route} {hidden}");
            assert_eq!(status, ustatus, "{route}: same status as an unknown id");
            assert_eq!(bytes, ubytes, "{route}: same body as an unknown id");
            assert_eq!(
                headers
                    .get(header::CACHE_CONTROL)
                    .map(|v| v.to_str().unwrap()),
                Some("no-store"),
                "{route}: a 404 is not cached"
            );
        }
    }
    assert_eq!(
        app.state().zip_lru.open_attempts(),
        before,
        "no archive opened for a caller who may not see the issue"
    );
}

/// A row the scanner marked `malformed` (a bad ComicInfo.xml, say) whose
/// archive opens serves its pages; one whose archive is broken answers
/// 410 and is read once per failed-open window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_rows_go_through_the_real_open() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;

    let page = sniff_page(400, 7);
    let fine = issue(
        &app,
        &lib,
        "fine.cbz",
        &cbz_bytes(std::slice::from_ref(&page)),
        IssueState::Malformed,
    )
    .await;
    let (status, _, bytes) = call(
        &app,
        Some(&admin),
        Method::GET,
        &format!("/issues/{fine}/pages/0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, page);

    let pages: Vec<Vec<u8>> = (0..6).map(|i| sniff_page(32 * 1024, 20 + i)).collect();
    let broken = issue(
        &app,
        &lib,
        "broken.cbz",
        &cbz_bytes(&pages),
        IssueState::Malformed,
    )
    .await;
    truncate_half(&lib.root.join("broken.cbz"));
    let before = app.state().zip_lru.open_attempts();
    for _ in 0..8 {
        let (status, headers, bytes) = call(
            &app,
            Some(&admin),
            Method::GET,
            &format!("/issues/{broken}/pages/0"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(code(&bytes), "archive_unreadable");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(app.state().zip_lru.open_attempts() - before, 1);
}

/// A CBR the library has not converted: the file is fine, the page reader
/// cannot read it. 409, its own code, not the 410 of a damaged file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unconverted_cbr_answers_409() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;
    let cbr = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic-3page.cbr"),
    )
    .unwrap();
    let id = issue(&app, &lib, "issue.cbr", &cbr, IssueState::Active).await;
    for path in [
        format!("/issues/{id}/pages/0"),
        format!("/issues/{id}/pages/1/thumb?variant=strip"),
    ] {
        let (status, headers, bytes) = call(&app, Some(&admin), Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}");
        assert_eq!(code(&bytes), "archive_needs_conversion");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    }
}

/// A missing file under a library root that is itself gone, or empty (an
/// unmounted share or bind mount), is the library being offline: 503 with
/// `Retry-After`, not a final 410.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offline_library_answers_503_not_410() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;
    let id = issue(
        &app,
        &lib,
        "a.cbz",
        &cbz_bytes(&[sniff_page(300, 9)]),
        IssueState::Active,
    )
    .await;
    let page = format!("/issues/{id}/pages/0");
    let offline = lib.root.with_extension("away");

    // Root gone.
    std::fs::rename(&lib.root, &offline).unwrap();
    let (status, headers, bytes) = call(&app, Some(&admin), Method::GET, &page, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&bytes), "library_offline");
    assert_eq!(headers[header::RETRY_AFTER], "30");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");

    // Root there but empty (what an unmounted mount point looks like).
    std::fs::create_dir(&lib.root).unwrap();
    let (status, _, bytes) = call(&app, Some(&admin), Method::GET, &page, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&bytes), "library_offline");

    // Mounted again: serves.
    std::fs::remove_dir(&lib.root).unwrap();
    std::fs::rename(&offline, &lib.root).unwrap();
    let (status, _, _) = call(&app, Some(&admin), Method::GET, &page, None).await;
    assert_eq!(status, StatusCode::OK);

    // The file alone gone, the library still there: final 410.
    std::fs::remove_file(lib.root.join("a.cbz")).unwrap();
    let (status, _, bytes) = call(&app, Some(&admin), Method::GET, &page, None).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(code(&bytes), "archive_missing");
}

/// An I/O error opening the file (here: no read permission, the test's way
/// of making the open fail with something other than not-found) answers
/// 503 with `Retry-After`, and is not remembered: once the file is
/// readable again the next request serves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_io_error_answers_503_with_retry_after() {
    use std::os::unix::fs::PermissionsExt;
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;
    let page = sniff_page(300, 11);
    let id = issue(
        &app,
        &lib,
        "locked.cbz",
        &cbz_bytes(std::slice::from_ref(&page)),
        IssueState::Active,
    )
    .await;
    let file = lib.root.join("locked.cbz");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&file).is_ok() {
        // Running as root: permissions do not stop the open.
        return;
    }
    let path = format!("/issues/{id}/pages/0");
    let (status, headers, bytes) = call(&app, Some(&admin), Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&bytes), "service_unavailable");
    assert_eq!(headers[header::RETRY_AFTER], "5");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (status, _, bytes) = call(&app, Some(&admin), Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, page);
}

/// Unknown issue, out-of-range page and unsatisfiable range: none of them
/// is cacheable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_found_and_range_errors_are_not_cached() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;
    let id = issue(
        &app,
        &lib,
        "a.cbz",
        &cbz_bytes(&[sniff_page(300, 13)]),
        IssueState::Active,
    )
    .await;
    for path in [
        "/issues/0000/pages/0".to_owned(),
        "/issues/0000/pages/0/thumb".to_owned(),
        format!("/issues/{id}/pages/7"),
    ] {
        let (status, headers, _) = call(&app, Some(&admin), Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store", "{path}");
    }
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/issues/{id}/pages/0"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", admin.session),
                )
                .header(header::RANGE, "bytes=5000-6000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
}

async fn etag_and_body(app: &TestApp, user: Option<&Authed>, path: &str) -> (String, Vec<u8>) {
    let (status, headers, bytes) = call(app, user, Method::GET, path, None).await;
    assert_eq!(status, StatusCode::OK, "{path}");
    (headers[header::ETAG].to_str().unwrap().to_owned(), bytes)
}

/// The page stream's ETag and a width variant's ETag, bytes and cache
/// entry follow the file: after a rename-over and after an in-place
/// rewrite of the same inode, before any rescan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_and_variant_follow_the_file() {
    use std::os::unix::fs::MetadataExt;
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let lib = library(&app).await;
    let v1 = cbz_bytes(&[real_png(900, 600, 10)]);
    let id = issue(&app, &lib, "c.cbz", &v1, IssueState::Active).await;
    let file = lib.root.join("c.cbz");
    let pse = format!(
        "/opds/pse/{id}/0?{}",
        server::auth::url_signing::issue_query(
            &id,
            admin.user_id,
            app.state().secrets.url_signing_key.as_ref()
        )
    );
    let variant = format!("/issues/{id}/pages/0?w=480");

    let (pse1, _) = etag_and_body(&app, None, &pse).await;
    let (var1, img1) = etag_and_body(&app, Some(&admin), &variant).await;
    // A disk-cache hit for the same version: same ETag, same bytes.
    let (var1b, img1b) = etag_and_body(&app, Some(&admin), &variant).await;
    assert_eq!((&var1, &img1), (&var1b, &img1b));

    // Rename-over: a new inode.
    let staged = lib.root.join("staged.tmp");
    std::fs::write(&staged, cbz_bytes(&[real_png(900, 600, 200)])).unwrap();
    std::fs::rename(&staged, &file).unwrap();
    let (pse2, _) = etag_and_body(&app, None, &pse).await;
    let (var2, img2) = etag_and_body(&app, Some(&admin), &variant).await;
    assert_ne!(pse1, pse2, "stream ETag follows a rename-over");
    assert_ne!(var1, var2, "variant ETag follows a rename-over");
    assert_ne!(img1, img2, "variant bytes are the new file's");

    // In place: same inode, new content.
    let ino = std::fs::metadata(&file).unwrap().ino();
    std::fs::write(&file, cbz_bytes(&[real_png(900, 600, 90)])).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().ino(), ino);
    let (pse3, _) = etag_and_body(&app, None, &pse).await;
    let (var3, img3) = etag_and_body(&app, Some(&admin), &variant).await;
    assert_ne!(pse2, pse3, "stream ETag follows an in-place rewrite");
    assert_ne!(var2, var3, "variant ETag follows an in-place rewrite");
    assert_ne!(img2, img3);

    // Restoring the first file's bytes is a new version again: never the
    // render stored for another version.
    std::fs::write(&file, &v1).unwrap();
    let (var4, img4) = etag_and_body(&app, Some(&admin), &variant).await;
    assert_ne!(var4, var3);
    let decoded = image::load_from_memory(&img4).unwrap().to_rgba8();
    let red = i32::from(decoded.get_pixel(0, 0)[0]);
    assert!(
        (red - 10).abs() <= 8,
        "the restored file's pixels, got red {red}"
    );
}
