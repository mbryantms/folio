//! Roadmap WP-2.10: hand edits reach the archive when a library is in
//! writeback mode. `PATCH` on an issue enqueues one sidecar rewrite; a
//! series identity edit fans out one rewrite per active issue plus a single
//! series-scoped rescan; non-writeback libraries and refused formats
//! enqueue nothing. External-id adds and unlinks (audit 2026-11 N-M5) take
//! the same path: an issue id is one rewrite, a series id is the fan-out.

mod common;

use archive::ArchiveLimits;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{field_provenance, issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn read_entry(path: &Path, name: &str) -> String {
    let mut a = archive::open(path, ArchiveLimits::default()).unwrap();
    String::from_utf8(
        a.read_entry_bytes(name)
            .unwrap_or_else(|e| panic!("{name} present: {e}")),
    )
    .unwrap()
}

fn read_comicinfo(path: &Path) -> String {
    read_entry(path, "ComicInfo.xml")
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery-staple"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", auth.csrf.clone());
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
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
    (status, body_json(resp.into_body()).await)
}

async fn patch(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(app, auth, Method::PATCH, uri, Some(body)).await
}

/// Every queued sidecar rewrite job (decoded from the apalis data hash).
async fn queued_rewrite_jobs(app: &TestApp) -> Vec<RewriteIssueSidecarsJob> {
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let mut jobs: Vec<RewriteIssueSidecarsJob> = all
        .values()
        .map(|blob| serde_json::from_str(blob).expect("job args"))
        .collect();
    jobs.sort_by(|a, b| a.issue_id.cmp(&b.issue_id));
    jobs
}

async fn run_queued_rewrite_jobs(app: &TestApp) -> usize {
    let jobs = queued_rewrite_jobs(app).await;
    let n = jobs.len();
    for job in jobs {
        server::jobs::rewrite_sidecars::handle(
            job,
            apalis::prelude::Data::new(app.state()),
            apalis::prelude::Attempt::new_with_value(1),
        )
        .await
        .expect("rewrite job handle");
    }
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    n
}

/// Number of queued series-scan jobs (the job-level rescans the rewrite
/// worker and the series fan-out enqueue).
async fn queued_scan_jobs(app: &TestApp) -> i64 {
    let storage = app.state().jobs.scan_series_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    redis::cmd("HLEN")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap()
}

/// Library (writeback per `writeback`) + series + `n` CBZ issues. Returns
/// `(lib_id, series_id, series_slug, [(issue_id, issue_slug, path)])`.
async fn seed(
    app: &TestApp,
    dir: &Path,
    writeback: bool,
    n: usize,
    ext: &str,
) -> (
    Uuid,
    Uuid,
    String,
    Vec<(String, String, std::path::PathBuf)>,
) {
    let db = &app.state().db;
    let mut lib = LibrarySeed::new(dir);
    if writeback {
        lib = lib.with_sidecar_writeback();
    }
    let lib_id = lib.insert(db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    let series_slug = series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let mut issues = Vec::new();
    for i in 1..=n {
        let path = dir.join(format!("saga-{i}.{ext}"));
        let id = IssueSeed::new(
            lib_id,
            series_id,
            &path,
            &build_cbz_bytes(&format!("saga-{i}")),
            i as f64,
        )
        .insert(db)
        .await;
        let slug = issue::Entity::find_by_id(id.clone())
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .slug;
        issues.push((id, slug, path));
    }
    (lib_id, series_id, series_slug, issues)
}

async fn provenance(
    app: &TestApp,
    entity_type: &str,
    entity_id: &str,
    field: &str,
) -> Option<String> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .filter(field_provenance::Column::Field.eq(field))
        .one(&app.state().db)
        .await
        .unwrap()
        .map(|r| r.set_by)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_patch_in_writeback_library_lands_in_the_archive() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 1, "cbz").await;
    let (issue_id, issue_slug, path) = issues.into_iter().next().unwrap();

    let (status, body) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
        serde_json::json!({ "title": "Hand Title", "summary": "Hand summary" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // One job, composed from the database, attributed to the editor, with
    // nothing deferred (the PATCH already wrote the user pins).
    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert_eq!(job.issue_id, issue_id);
    assert!(!job.skip_rescan);
    assert!(job.post_apply.is_none());
    assert!(job.actor_id.is_some(), "rewrite audit names the editor");
    assert!(
        job.comic_info_xml.contains("<Title>Hand Title</Title>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.comic_info_xml
            .contains("<Summary>Hand summary</Summary>")
    );

    // Run it: the archive now carries the edit, and the worker queued the
    // scoped rescan that re-ingests it (the WP-2.5 tier gate keeps the
    // user's value through that rescan).
    let scans_before = queued_scan_jobs(&app).await;
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);
    let xml = read_comicinfo(&path);
    assert!(xml.contains("<Title>Hand Title</Title>"), "{xml}");
    assert!(xml.contains("<Summary>Hand summary</Summary>"), "{xml}");
    assert!(
        queued_scan_jobs(&app).await > scans_before,
        "rewrite worker enqueues the scoped rescan"
    );
    assert_eq!(
        provenance(&app, "issue", &issue_id, "title")
            .await
            .as_deref(),
        Some("user")
    );
}

/// A hand edit of a list column writes the junction rows it rolls up into
/// through the audited writers, in the same transaction as the row and its
/// pins, so the composer (which reads the junctions, audit 2026-11 N-M2)
/// writes the edit into the file — spelled the way the scanner normalises
/// it: `"Mike Deodato, Jr."` is one person, not `Mike Deodato` + `Jr.`.
/// Roles the edit did not touch keep their rows, including one ComicInfo
/// has no column for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_patch_of_a_credit_column_rederives_the_junction_and_the_file_carries_it() {
    use entity::{issue_arc, issue_credit};
    use server::metadata::identifier::Source;
    use server::metadata::writers::{self, CsvRebuildBatch, SetBy};

    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 1, "cbz").await;
    let (issue_id, issue_slug, path) = issues.into_iter().next().unwrap();
    let db = &app.state().db;

    // A provider apply left an editor and a journalist (no ComicInfo column).
    let set_by = SetBy::Provider(Source::Metron);
    let editor = writers::upsert_person(db, "Eddie Editor", &[], set_by)
        .await
        .unwrap();
    let journalist = writers::upsert_person(db, "Jo Journalist", &[], set_by)
        .await
        .unwrap();
    let batch = CsvRebuildBatch::new();
    writers::set_issue_credits(
        db,
        &issue_id,
        vec![
            (editor, "editor".into(), 0),
            (journalist, "journalist".into(), 0),
        ],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    batch.flush(db).await.unwrap();

    let (status, body) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
        serde_json::json!({
            "writer": "Hand Writer",
            "penciller": "Mike Deodato, Jr.",
            "characters": "Alana, Marko",
            "story_arc": "Origins",
            "story_arc_number": "2"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The response already carries the rebuilt (normalised) columns.
    assert_eq!(body["penciller"], "Mike Deodato Jr.");
    assert_eq!(body["editor"], "Eddie Editor");

    // The edited roles hold the edit; the untouched ones keep their rows.
    let rows = issue_credit::Entity::find()
        .filter(issue_credit::Column::IssueId.eq(issue_id.as_str()))
        .all(db)
        .await
        .unwrap();
    let mut credits: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.role.clone(), r.person.clone()))
        .collect();
    credits.sort();
    assert_eq!(
        credits,
        vec![
            ("editor".to_string(), "Eddie Editor".to_string()),
            ("journalist".to_string(), "Jo Journalist".to_string()),
            ("penciller".to_string(), "Mike Deodato Jr.".to_string()),
            ("writer".to_string(), "Hand Writer".to_string()),
        ]
    );
    assert!(
        rows.iter().all(|r| r.person_id.is_some()),
        "every credit is linked to its person row: {rows:?}"
    );
    let arcs = issue_arc::Entity::find()
        .filter(issue_arc::Column::IssueId.eq(issue_id.as_str()))
        .all(db)
        .await
        .unwrap();
    assert_eq!(arcs.len(), 1, "{arcs:?}");
    assert_eq!(arcs[0].position_in_arc, Some(2));

    // …and the composed sidecars carry it as one creator.
    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert!(
        job.comic_info_xml.contains("<Writer>Hand Writer</Writer>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.comic_info_xml
            .contains("<Penciller>Mike Deodato Jr.</Penciller>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.comic_info_xml
            .contains("<Characters>Alana, Marko</Characters>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.comic_info_xml.contains("<StoryArc>Origins</StoryArc>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.comic_info_xml.contains("<Editor>Eddie Editor</Editor>"),
        "{}",
        job.comic_info_xml
    );
    assert_eq!(
        job.metron_info_xml.matches("<Credit>").count(),
        4,
        "editor, journalist, penciller, writer\n{}",
        job.metron_info_xml
    );
    assert!(
        job.metron_info_xml
            .contains("<Creator>Mike Deodato Jr.</Creator>"),
        "{}",
        job.metron_info_xml
    );
    assert!(
        job.metron_info_xml
            .contains("<Creator>Jo Journalist</Creator>"),
        "{}",
        job.metron_info_xml
    );
    assert!(
        !job.metron_info_xml.contains("<Creator>Jr.</Creator>"),
        "{}",
        job.metron_info_xml
    );

    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);
    let xml = read_comicinfo(&path);
    assert!(
        xml.contains("<Penciller>Mike Deodato Jr.</Penciller>"),
        "{xml}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_patch_in_non_writeback_library_enqueues_nothing() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), false, 1, "cbz").await;
    let (_issue_id, issue_slug, path) = issues.into_iter().next().unwrap();
    let before = std::fs::read(&path).unwrap();

    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
        serde_json::json!({ "title": "Hand Title" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(queued_rewrite_jobs(&app).await.is_empty(), "DB-only edit");
    assert_eq!(std::fs::read(&path).unwrap(), before, "file untouched");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_identity_patch_fans_out_and_schedules_one_series_rescan() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 2, "cbz").await;

    let (status, body) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({ "name": "Saga Deluxe", "publisher": "Image Comics" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 2, "one rewrite per active issue: {jobs:?}");
    assert!(
        jobs.iter().all(|j| j.skip_rescan),
        "fan-out defers to one series-scoped rescan"
    );
    assert!(
        jobs.iter()
            .all(|j| j.comic_info_xml.contains("<Series>Saga Deluxe</Series>"))
    );
    assert!(
        queued_scan_jobs(&app).await >= 1,
        "series edit schedules the series-scoped rescan"
    );

    assert_eq!(run_queued_rewrite_jobs(&app).await, 2);
    for (_, _, path) in &issues {
        let xml = read_comicinfo(path);
        assert!(xml.contains("<Series>Saga Deluxe</Series>"), "{xml}");
        assert!(xml.contains("<Publisher>Image Comics</Publisher>"), "{xml}");
    }

    // A non-identity series edit (status only) does not rewrite anything.
    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({ "status": "ended" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(queued_rewrite_jobs(&app).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbr_without_conversion_is_refused_and_the_edit_stays_in_the_database() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    // Zip bytes under a .cbr name: the sidecar path refuses on extension
    // when the library does not auto-convert RAR archives.
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 1, "cbr").await;
    let (issue_id, issue_slug, _path) = issues.into_iter().next().unwrap();

    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
        serde_json::json!({ "title": "Hand Title" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        queued_rewrite_jobs(&app).await.is_empty(),
        "refused: no job"
    );
    let row = issue::Entity::find_by_id(issue_id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.title.as_deref(),
        Some("Hand Title"),
        "the database still holds the edit"
    );
    assert_eq!(
        provenance(&app, "issue", &issue_id, "title")
            .await
            .as_deref(),
        Some("user")
    );
}

// ───────── external ids (audit 2026-11 N-M5) ─────────

/// Adding and unlinking an issue identifier enqueues one rewrite each,
/// composed from the database and attributed to the editor, and the
/// archive's ComicInfo / MetronInfo carry (then drop) the id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_external_id_add_and_unlink_land_in_the_archive() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 1, "cbz").await;
    let (issue_id, issue_slug, path) = issues.into_iter().next().unwrap();
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/external-ids");

    let (status, body) = send(
        &app,
        &auth,
        Method::POST,
        &uri,
        Some(serde_json::json!({ "source": "comicvine", "external_id": "12345" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["set_by"], "user");

    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert_eq!(job.issue_id, issue_id);
    assert!(!job.skip_rescan);
    assert!(job.post_apply.is_none());
    assert!(job.actor_id.is_some(), "rewrite audit names the editor");
    assert!(
        job.comic_info_xml
            .contains("<ComicVineID>12345</ComicVineID>"),
        "{}",
        job.comic_info_xml
    );
    assert!(
        job.metron_info_xml
            .contains(r#"<ID source="Comic Vine" primary="true">12345</ID>"#),
        "{}",
        job.metron_info_xml
    );
    let scans_before = queued_scan_jobs(&app).await;
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);
    assert!(
        read_comicinfo(&path).contains("<ComicVineID>12345</ComicVineID>"),
        "{}",
        read_comicinfo(&path)
    );
    assert!(
        read_entry(&path, "MetronInfo.xml")
            .contains(r#"<ID source="Comic Vine" primary="true">12345</ID>"#),
        "{}",
        read_entry(&path, "MetronInfo.xml")
    );
    assert!(
        queued_scan_jobs(&app).await > scans_before,
        "rewrite worker enqueues the scoped rescan"
    );

    // Unlinking is an edit too: the archive must stop claiming the id.
    let (status, body) = send(
        &app,
        &auth,
        Method::DELETE,
        &format!("{uri}/comicvine"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(
        !jobs[0].comic_info_xml.contains("<ComicVineID>"),
        "{}",
        jobs[0].comic_info_xml
    );
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);
    let xml = read_comicinfo(&path);
    assert!(!xml.contains("<ComicVineID>"), "{xml}");
    let metron = read_entry(&path, "MetronInfo.xml");
    assert!(!metron.contains(r#"source="Comic Vine""#), "{metron}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_external_id_edits_in_non_writeback_library_touch_nothing() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), false, 1, "cbz").await;
    let (_issue_id, issue_slug, path) = issues.into_iter().next().unwrap();
    let before = std::fs::read(&path).unwrap();
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/external-ids");

    let (status, body) = send(
        &app,
        &auth,
        Method::POST,
        &uri,
        Some(serde_json::json!({ "source": "comicvine", "external_id": "12345" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(queued_rewrite_jobs(&app).await.is_empty(), "DB-only edit");

    let (status, _) = send(
        &app,
        &auth,
        Method::DELETE,
        &format!("{uri}/comicvine"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(queued_rewrite_jobs(&app).await.is_empty(), "DB-only edit");
    assert_eq!(std::fs::read(&path).unwrap(), before, "file untouched");
}

/// A series-level id rides in every issue's sidecars, so it fans out like
/// a series identity edit: one rewrite per active issue with the rescan
/// deferred to a single series-scoped scan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_external_id_add_fans_out_and_schedules_one_series_rescan() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let (_lib, _series, series_slug, issues) = seed(&app, dir.path(), true, 2, "cbz").await;

    let (status, body) = send(
        &app,
        &auth,
        Method::POST,
        &format!("/api/series/{series_slug}/external-ids"),
        Some(serde_json::json!({ "source": "metron", "external_id": "987" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let jobs = queued_rewrite_jobs(&app).await;
    assert_eq!(jobs.len(), 2, "one rewrite per active issue: {jobs:?}");
    assert!(
        jobs.iter().all(|j| j.skip_rescan),
        "fan-out defers to one series-scoped rescan"
    );
    assert!(
        jobs.iter().all(|j| j
            .comic_info_xml
            .contains("<MetronSeriesID>987</MetronSeriesID>")),
        "{jobs:?}"
    );
    assert!(
        queued_scan_jobs(&app).await >= 1,
        "series id edit schedules the series-scoped rescan"
    );
    assert_eq!(run_queued_rewrite_jobs(&app).await, 2);
    for (_, _, path) in &issues {
        let xml = read_comicinfo(path);
        assert!(
            xml.contains("<MetronSeriesID>987</MetronSeriesID>"),
            "{xml}"
        );
    }
}
