//! archive-rewrite-1.0 M8 / roadmap WP-1.5 — `.bak` retention sweep.
//!
//! Drives [`server::jobs::backup_prune::run`] (the daily 04:45 UTC job)
//! against seeded libraries with real files on disk and asserts that only
//! backups older than the library's `archive_backup_retain_days` go away,
//! that `0` keeps everything, and that libraries without archive
//! writeback are never walked.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::library;
use entity::library_event::{Column as EventCol, Entity as EventEntity};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::backup_prune::run;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;
use uuid::Uuid;

const DAY: Duration = Duration::from_secs(86_400);

/// Create `path` with `age` subtracted from its mtime.
fn touch_aged(path: &Path, age: Duration) {
    std::fs::write(path, b"backup bytes").unwrap();
    let t = filetime::FileTime::from_system_time(SystemTime::now() - age);
    filetime::set_file_mtime(path, t).unwrap();
}

async fn set_retain_days(app: &TestApp, lib_id: Uuid, days: i32) {
    let am = library::ActiveModel {
        id: Set(lib_id),
        archive_backup_retain_days: Set(days),
        ..Default::default()
    };
    am.update(&app.state().db).await.unwrap();
}

#[tokio::test]
async fn prunes_backups_older_than_retain_days_and_keeps_younger_ones() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    // `with_sidecar_writeback` flips `allow_archive_writeback`; the seed's
    // default `archive_backup_retain_days` is 30.
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;

    let nested = dir.path().join("Series (2020)");
    std::fs::create_dir_all(&nested).unwrap();
    let old_bak = nested.join("issue-001.cbz.bak");
    let old_slot = nested.join("issue-001.cbz.bak.1");
    let young_bak = nested.join("issue-002.cbz.bak");
    let archive = nested.join("issue-001.cbz");
    let decoy = nested.join("notes.bak.txt"); // not a backup name
    touch_aged(&old_bak, 45 * DAY);
    touch_aged(&old_slot, 31 * DAY);
    touch_aged(&young_bak, 2 * DAY);
    touch_aged(&archive, 400 * DAY);
    touch_aged(&decoy, 400 * DAY);

    let stats = run(&app.state()).await.unwrap();
    assert_eq!(stats.removed, 2, "{stats:?}");
    assert_eq!(stats.errors, 0, "{stats:?}");
    assert_eq!(stats.libraries, 1, "{stats:?}");
    assert!(stats.bytes > 0);

    assert!(!old_bak.exists(), "45-day-old .bak must be pruned");
    assert!(!old_slot.exists(), "31-day-old .bak.1 must be pruned");
    assert!(young_bak.exists(), "2-day-old .bak must survive");
    assert!(archive.exists(), "the archive itself is never touched");
    assert!(decoy.exists(), "only `.bak` / `.bak.N` names are backups");

    // One library event per swept library, so operators see the reclaim
    // on the Library stream.
    let events = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib_id))
        .filter(EventCol::Category.eq("archive"))
        .filter(EventCol::Action.eq("removed"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "expected exactly one prune event");
    let detail = events[0].detail.clone().expect("detail json");
    assert_eq!(detail["removed"], 2);
    assert_eq!(detail["retain_days"], 30);

    // Second run: nothing left to prune, no new event.
    let again = run(&app.state()).await.unwrap();
    assert_eq!(again.removed, 0);
    let events = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib_id))
        .filter(EventCol::Category.eq("archive"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn zero_retain_days_keeps_backups_forever() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    set_retain_days(&app, lib_id, 0).await;

    let ancient = dir.path().join("issue.cbz.bak");
    touch_aged(&ancient, 3_000 * DAY);

    let stats = run(&app.state()).await.unwrap();
    assert_eq!(stats.removed, 0);
    assert_eq!(stats.libraries, 0, "retain_days=0 libraries are not walked");
    assert!(ancient.exists());
}

#[tokio::test]
async fn libraries_without_archive_writeback_are_skipped() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    // Plain seed: allow_archive_writeback = false, retain_days = 30.
    let _lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;

    let ancient = dir.path().join("issue.cbz.bak");
    touch_aged(&ancient, 400 * DAY);

    let stats = run(&app.state()).await.unwrap();
    assert_eq!(stats.removed, 0);
    assert_eq!(stats.libraries, 0);
    assert!(
        ancient.exists(),
        "a .bak under a non-writeback library isn't Folio's to delete"
    );
}

/// Audit 2026-11 N-L4: the sweep is what keeps the health page's backup
/// card current. Every library gets a stored tally of the backups that
/// survive — the writeback library after its prune, and the non-writeback
/// one from a read-only walk that deletes nothing.
#[tokio::test]
async fn sweep_stores_a_backup_tally_for_every_library() {
    use entity::library_backup_stats::Entity as Stats;

    let app = TestApp::spawn().await;
    let wb_dir = tempdir().unwrap();
    let wb_lib = LibrarySeed::new(wb_dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    touch_aged(&wb_dir.path().join("old.cbz.bak"), 45 * DAY);
    touch_aged(&wb_dir.path().join("young.cbz.bak"), 2 * DAY);
    touch_aged(&wb_dir.path().join("young.cbz.bak.1"), DAY);

    let ro_dir = tempdir().unwrap();
    let ro_lib = LibrarySeed::new(ro_dir.path())
        .insert(&app.state().db)
        .await;
    touch_aged(&ro_dir.path().join("stray.cbz.bak"), 400 * DAY);

    // Pretend both cards were flagged stale by a scan; the sweep clears it.
    for lib in [wb_lib, ro_lib] {
        server::library::backup_stats::store(
            &app.state().db,
            lib,
            &server::library::backup_stats::Tally::default(),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        server::library::backup_stats::mark_stale(&app.state().db, lib).await;
    }

    let stats = run(&app.state()).await.unwrap();
    assert_eq!(stats.removed, 1, "{stats:?}");
    assert_eq!(stats.libraries, 1, "only the writeback library is pruned");

    let wb = Stats::find_by_id(wb_lib)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wb.file_count, 2, "the two young backups survive");
    assert_eq!(wb.total_bytes, 2 * "backup bytes".len() as i64);
    assert!(!wb.stale);
    assert!(wb.oldest_modified_at.unwrap() < wb.newest_modified_at.unwrap());

    let ro = Stats::find_by_id(ro_lib)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ro.file_count, 1, "counted, never deleted");
    assert!(!ro.stale);
    assert!(ro_dir.path().join("stray.cbz.bak").exists());
}
