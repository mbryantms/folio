//! archive-rewrite-1.0 M8 / roadmap WP-1.5 — `.bak` retention sweep.
//!
//! Drives [`server::jobs::backup_prune::run_at`] (the daily 04:45 UTC job,
//! with its clock moved forward) against seeded libraries with real files
//! on disk and asserts that only backups made more than the library's
//! `archive_backup_retain_days` ago go away, that `0` keeps everything,
//! and that libraries without archive writeback are never walked.
//!
//! A backup's age counts from when it was made (its inode change time,
//! `archive_rewrite::backup_made_at`), which no test can set back, so the
//! tests move the sweep's clock forward instead of back-dating files.
//! Back-dating the mtime still matters: it is what a hard-linked backup
//! of an old archive looks like, and it must not make the backup "old".

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::library;
use entity::library_event::{Column as EventCol, Entity as EventEntity};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::backup_prune::{run, run_at};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;
use uuid::Uuid;

const DAY: Duration = Duration::from_secs(86_400);

/// Create `path` with `age` subtracted from its mtime (its ctime, the
/// backup's age, stays "now").
fn touch_aged(path: &Path, age: Duration) {
    std::fs::write(path, b"backup bytes").unwrap();
    let t = filetime::FileTime::from_system_time(SystemTime::now() - age);
    filetime::set_file_mtime(path, t).unwrap();
}

/// A moment strictly between the files made before and after it, far
/// enough apart that the filesystem's (coarse) ctime tells them apart.
fn checkpoint() -> SystemTime {
    std::thread::sleep(Duration::from_millis(60));
    let t = SystemTime::now();
    std::thread::sleep(Duration::from_millis(60));
    t
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
    touch_aged(&archive, 400 * DAY);
    touch_aged(&decoy, 400 * DAY);
    let made = checkpoint();
    // Made after the checkpoint; its old mtime does not make it old.
    touch_aged(&young_bak, 400 * DAY);

    // Not yet: 29 days after the old backups were made.
    let early = run_at(&app.state(), made + 29 * DAY).await.unwrap();
    assert_eq!(early.removed, 0, "{early:?}");

    let stats = run_at(&app.state(), made + 30 * DAY).await.unwrap();
    assert_eq!(stats.removed, 2, "{stats:?}");
    assert_eq!(stats.errors, 0, "{stats:?}");
    assert_eq!(stats.libraries, 1, "{stats:?}");
    assert!(stats.bytes > 0);

    assert!(!old_bak.exists(), "a .bak made 30 days ago must be pruned");
    assert!(
        !old_slot.exists(),
        "a .bak.1 made 30 days ago must be pruned"
    );
    assert!(young_bak.exists(), "a .bak made later must survive");
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
    let again = run_at(&app.state(), made + 30 * DAY).await.unwrap();
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

    let stats = run_at(&app.state(), SystemTime::now() + 3_000 * DAY)
        .await
        .unwrap();
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

    let stats = run_at(&app.state(), SystemTime::now() + 400 * DAY)
        .await
        .unwrap();
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
    let ro_dir = tempdir().unwrap();
    let ro_lib = LibrarySeed::new(ro_dir.path())
        .insert(&app.state().db)
        .await;
    touch_aged(&wb_dir.path().join("old.cbz.bak"), 45 * DAY);
    touch_aged(&ro_dir.path().join("stray.cbz.bak"), 400 * DAY);
    let made = checkpoint();
    touch_aged(&wb_dir.path().join("young.cbz.bak"), 2 * DAY);
    std::thread::sleep(Duration::from_millis(60));
    touch_aged(&wb_dir.path().join("young.cbz.bak.1"), DAY);

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

    let stats = run_at(&app.state(), made + 30 * DAY).await.unwrap();
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
    // The card's dates are when the backups were made, not their mtimes.
    let (oldest, newest) = (
        wb.oldest_modified_at.unwrap(),
        wb.newest_modified_at.unwrap(),
    );
    assert!(oldest < newest);
    assert!(oldest.with_timezone(&chrono::Utc) > chrono::DateTime::<chrono::Utc>::from(made));

    let ro = Stats::find_by_id(ro_lib)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ro.file_count, 1, "counted, never deleted");
    assert!(!ro.stale);
    assert!(ro_dir.path().join("stray.cbz.bak").exists());
}

/// The daily job end to end with the real clock: a backup a rewrite made
/// a moment ago of an archive last modified 400 days ago is still there
/// after today's sweep (the bug pruned it: the hard link carries the
/// archive's mtime). A user's `notes.bak` / `photo.jpg.bak` beside it are
/// never deleted, however old.
#[tokio::test]
async fn todays_sweep_keeps_a_fresh_backup_of_an_old_archive_and_foreign_bak_files() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series = dir.path().join("Saga (2012)");
    std::fs::create_dir_all(&series).unwrap();
    let archive = series.join("Saga 001.cbz");
    touch_aged(&archive, 400 * DAY);
    let root = server::library::no_follow::LibraryRoot::resolve(dir.path());
    let outcome = server::archive_rewrite::rewrite_atomic(&root, &archive, 1, |io| {
        use std::io::Write;
        io.staging_file()?.write_all(b"rewritten")?;
        Ok(())
    })
    .unwrap();
    let bak = outcome.backup.expect("backup made");
    assert!(
        std::fs::metadata(&bak).unwrap().modified().unwrap() < SystemTime::now() - 399 * DAY,
        "the hard-linked backup carries the archive's old mtime"
    );
    let notes = series.join("notes.bak");
    let photo = series.join("photo.jpg.bak");
    touch_aged(&notes, 4_000 * DAY);
    touch_aged(&photo, 4_000 * DAY);

    let stats = run(&app.state()).await.unwrap();
    assert_eq!(stats.removed, 0, "{stats:?}");
    assert!(
        bak.exists(),
        "a backup made just now survives today's sweep"
    );

    // Control: a month later the backup goes, the user's files never do.
    let later = run_at(&app.state(), SystemTime::now() + 31 * DAY)
        .await
        .unwrap();
    assert_eq!(later.removed, 1, "{later:?}");
    assert!(!bak.exists());
    let far = run_at(&app.state(), SystemTime::now() + 10_000 * DAY)
        .await
        .unwrap();
    assert_eq!(far.removed, 0, "{far:?}");
    assert!(notes.exists() && photo.exists());
    assert_eq!(std::fs::read(&archive).unwrap(), b"rewritten");
}
