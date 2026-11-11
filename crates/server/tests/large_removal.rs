//! Large-removal warning (audit 2026-11 R-01 step 4, D-5).
//!
//! A library scan that soft-removes more than the threshold of the library
//! raises one `LargeRemoval` finding and one `health` / `removed` library
//! event; the removal itself proceeds unchanged. The finding stays open
//! across later scans while the removed content stays away, sticks when
//! dismissed, and resolves when the content returns.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::library_health_issue::{self, HealthIssueKind};
use entity::{issue, library, library_event, scan_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use server::library::scanner;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tower::ServiceExt;
use uuid::Uuid;

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

fn write_series(folder: &Path, prefix: &str, count: u32, marker: u32) {
    std::fs::create_dir_all(folder).unwrap();
    for n in 1..=count {
        write_cbz(&folder.join(format!("{prefix} {n:03}.cbz")), marker + n);
    }
}

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(id),
        name: Set(format!("Removal Lib {id}")),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(false),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(false),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

/// A publisher-layout library: `Marvel/` holds two 30-issue series,
/// `Image/` one 20-issue series (80 issues, 3 series). `Marvel/` can be
/// moved out of the root (`park`) and back (`unpark`), as a share that was
/// not mounted during a scan.
struct Fixture {
    app: TestApp,
    _tmp: tempfile::TempDir,
    root: PathBuf,
    parked: PathBuf,
    lib: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let app = TestApp::spawn().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("library");
        write_series(&root.join("Marvel/Big A (2020)"), "Big A", 30, 1_000);
        write_series(&root.join("Marvel/Big B (2021)"), "Big B", 30, 2_000);
        write_series(&root.join("Image/Keep (2019)"), "Keep", 20, 3_000);
        let parked = tmp.path().join("parked-marvel");
        let lib = create_library(&app, &root).await;
        let s = scanner::scan_library(&app.state(), lib).await.unwrap();
        assert_eq!(s.files_added, 80, "{s:?}");
        Self {
            app,
            _tmp: tmp,
            root,
            parked,
            lib,
        }
    }

    fn park(&self) {
        std::fs::rename(self.root.join("Marvel"), &self.parked).unwrap();
    }

    fn unpark(&self) {
        std::fs::rename(&self.parked, self.root.join("Marvel")).unwrap();
    }

    async fn scan(&self) -> scanner::stats::ScanStats {
        scanner::scan_library(&self.app.state(), self.lib)
            .await
            .unwrap()
    }

    async fn findings(&self) -> Vec<library_health_issue::Model> {
        library_health_issue::Entity::find()
            .filter(library_health_issue::Column::LibraryId.eq(self.lib))
            .filter(library_health_issue::Column::Kind.eq(HealthIssueKind::LargeRemoval))
            .all(&self.app.state().db)
            .await
            .unwrap()
    }

    async fn only_finding(&self) -> library_health_issue::Model {
        let rows = self.findings().await;
        assert_eq!(rows.len(), 1, "one LargeRemoval row per library: {rows:?}");
        rows.into_iter().next().unwrap()
    }

    async fn events(&self) -> Vec<library_event::Model> {
        library_event::Entity::find()
            .filter(library_event::Column::LibraryId.eq(self.lib))
            .filter(library_event::Column::Category.eq("health"))
            .filter(library_event::Column::Action.eq("removed"))
            .all(&self.app.state().db)
            .await
            .unwrap()
    }

    async fn latest_run(&self) -> Uuid {
        scan_run::Entity::find()
            .filter(scan_run::Column::LibraryId.eq(self.lib))
            .order_by_desc(scan_run::Column::StartedAt)
            .one(&self.app.state().db)
            .await
            .unwrap()
            .unwrap()
            .id
    }

    async fn removed_issue_count(&self) -> usize {
        issue::Entity::find()
            .filter(issue::Column::LibraryId.eq(self.lib))
            .filter(issue::Column::RemovedAt.is_not_null())
            .all(&self.app.state().db)
            .await
            .unwrap()
            .len()
    }
}

async fn admin_session(app: &TestApp) -> String {
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
        .find_map(|c| {
            c.split(';')
                .next()?
                .strip_prefix("__Host-comic_session=")
                .map(str::to_owned)
        })
        .expect("session cookie")
}

async fn health_issue_count(app: &TestApp, session: &str, lib: Uuid, run: Uuid) -> u64 {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/libraries/{lib}/scan-runs"))
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == run.to_string())
        .expect("run row")["health_issue_count"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn lost_share_warns_once_stays_open_and_resolves_when_it_returns() {
    let f = Fixture::new().await;
    let session = admin_session(&f.app).await;
    assert!(
        f.findings().await.is_empty(),
        "the first scan removes nothing"
    );

    // The share is missing during a scan: the removal proceeds as before.
    f.park();
    let s = f.scan().await;
    assert_eq!(s.issues_removed, 60, "{s:?}");
    assert_eq!(f.removed_issue_count().await, 60);
    let run = f.latest_run().await;

    let row = f.only_finding().await;
    assert_eq!(row.severity, "warning");
    assert_eq!(row.scan_id, Some(run));
    assert!(row.resolved_at.is_none() && row.dismissed_at.is_none());
    let data = &row.payload["data"];
    assert_eq!(row.payload["kind"], "large_removal");
    assert_eq!(data["scan_id"], run.to_string());
    assert_eq!(data["issues_removed"], 60);
    assert_eq!(data["issues_total"], 80);
    assert_eq!(data["series_removed"], 2);
    assert_eq!(data["series_total"], 3);
    assert_eq!(
        data["top_folders"],
        serde_json::json!([{"folder": "Marvel", "issues": 60}])
    );
    assert_eq!(data["folder_count"], 1);
    let text = row.payload.to_string();
    let root = f.root.to_string_lossy();
    assert!(!text.contains(root.as_ref()), "no absolute path: {text}");
    assert!(data.get("path").is_none() && data.get("folder").is_none());

    let events = f.events().await;
    assert_eq!(events.len(), 1, "one event: {events:?}");
    let ev = &events[0];
    assert_eq!(ev.severity, "warning");
    assert_eq!(ev.scan_run_id, Some(run));
    assert!(ev.summary.contains("60 of 80 issues"), "{}", ev.summary);
    let detail = ev.detail.as_ref().unwrap();
    assert_eq!(detail["issues_removed"], 60);
    assert_eq!(detail["series_removed"], 2);
    assert!(!detail.to_string().contains(root.as_ref()));

    assert_eq!(health_issue_count(&f.app, &session, f.lib, run).await, 1);

    // The next ordinary scan removes nothing new: the warning stays, still
    // attributed to the run that removed the content, with no new event.
    let s = f.scan().await;
    assert_eq!(s.issues_removed, 0, "{s:?}");
    let kept = f.only_finding().await;
    assert!(kept.resolved_at.is_none(), "kept open across a quiet scan");
    assert_eq!(kept.scan_id, Some(run));
    assert!(kept.last_seen_at > row.last_seen_at, "re-sighted");
    assert_eq!(f.events().await.len(), 1);
    assert_eq!(health_issue_count(&f.app, &session, f.lib, run).await, 1);

    // Dismissed, it stays dismissed (and open) on the next scan.
    let mut am: library_health_issue::ActiveModel = kept.into();
    am.dismissed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&f.app.state().db).await.unwrap();
    f.scan().await;
    let dismissed = f.only_finding().await;
    assert!(dismissed.dismissed_at.is_some(), "dismissal sticks");
    assert!(dismissed.resolved_at.is_none());

    // The share comes back: the content is restored and the row resolves.
    f.unpark();
    let s = f.scan().await;
    assert_eq!(s.issues_restored, 60, "{s:?}");
    assert_eq!(f.removed_issue_count().await, 0);
    let resolved = f.only_finding().await;
    assert!(
        resolved.resolved_at.is_some(),
        "resolves once content returns"
    );
    assert_eq!(f.events().await.len(), 1);
}

#[tokio::test]
async fn a_new_large_removal_reopens_the_same_row_even_when_dismissed() {
    let f = Fixture::new().await;
    f.park();
    f.scan().await;
    let first = f.only_finding().await;
    let mut am: library_health_issue::ActiveModel = first.clone().into();
    am.dismissed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&f.app.state().db).await.unwrap();
    f.unpark();
    f.scan().await;
    assert!(f.only_finding().await.resolved_at.is_some());

    // Missing again later: a new removal, so the same row is open again.
    f.park();
    f.scan().await;
    let again = f.only_finding().await;
    assert_eq!(again.id, first.id, "same row, not a second one");
    assert!(again.resolved_at.is_none() && again.dismissed_at.is_none());
    assert_ne!(again.scan_id, first.scan_id);
    assert_eq!(f.events().await.len(), 2, "one event per crossing");
}

#[tokio::test]
async fn a_watcher_scoped_scan_raises_it_too() {
    let f = Fixture::new().await;
    f.park();
    // The watcher reports the root (an entry disappeared).
    let s =
        scanner::scan_library_scoped(&f.app.state(), f.lib, std::slice::from_ref(&f.root), None)
            .await
            .unwrap();
    assert_eq!(s.issues_removed, 60, "{s:?}");
    assert_eq!(f.only_finding().await.payload["data"]["issues_removed"], 60);
    assert_eq!(f.events().await.len(), 1);
}

#[tokio::test]
async fn an_ordinary_cleanup_does_not_warn() {
    let f = Fixture::new().await;
    // Delete one 20-issue series: 25% of the library but under the floor.
    std::fs::remove_dir_all(f.root.join("Image")).unwrap();
    let s = f.scan().await;
    assert_eq!(s.issues_removed, 20, "{s:?}");
    assert!(f.findings().await.is_empty());
    assert!(f.events().await.is_empty());
}

#[tokio::test]
async fn a_small_library_below_the_floor_does_not_warn() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    write_series(&root.join("Alpha (2020)"), "Alpha", 8, 100);
    write_series(&root.join("Beta (2020)"), "Beta", 2, 200);
    let lib = create_library(&app, &root).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    // 80% of a 10-issue library.
    std::fs::remove_dir_all(root.join("Alpha (2020)")).unwrap();
    let s = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(s.issues_removed, 8, "{s:?}");
    let rows = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib))
        .filter(library_health_issue::Column::Kind.eq(HealthIssueKind::LargeRemoval))
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn a_series_scan_never_warns() {
    let f = Fixture::new().await;
    let folder = f.root.join("Marvel/Big A (2020)");
    // Most of one series goes, in one per-series scan: 29 of its 30.
    for n in 2..=30 {
        std::fs::remove_file(folder.join(format!("Big A {n:03}.cbz"))).unwrap();
    }
    let series_id = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(f.lib))
        .filter(entity::series::Column::Name.contains("Big A"))
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap()
        .id;
    let s = scanner::scan_series_folder(
        &f.app.state(),
        f.lib,
        series_id,
        &folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.issues_removed, 29, "{s:?}");
    assert!(f.findings().await.is_empty());
    assert!(f.events().await.is_empty());
}

#[tokio::test]
async fn a_cancelled_scan_raises_nothing() {
    let f = Fixture::new().await;
    f.park();
    let lost = Arc::new(AtomicBool::new(true));
    let s = scanner::scan_library_leased(&f.app.state(), f.lib, false, None, Some(lost))
        .await
        .unwrap();
    assert_eq!(s.issues_removed, 0, "{s:?}");
    let row = scan_run::Entity::find_by_id(f.latest_run().await)
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, scan_run::ScanRunState::Cancelled);
    assert!(f.findings().await.is_empty());
    assert!(f.events().await.is_empty());
    assert_eq!(f.removed_issue_count().await, 0);
}

#[tokio::test]
async fn a_cancelled_scan_does_not_resolve_an_open_warning() {
    let f = Fixture::new().await;
    f.park();
    f.scan().await;
    assert!(f.only_finding().await.resolved_at.is_none());
    let lost = Arc::new(AtomicBool::new(true));
    scanner::scan_library_leased(&f.app.state(), f.lib, false, None, Some(lost))
        .await
        .unwrap();
    assert!(
        f.only_finding().await.resolved_at.is_none(),
        "still open: the content is still away"
    );
}
