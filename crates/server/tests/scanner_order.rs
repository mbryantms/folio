//! A scan's result does not depend on the order the filesystem lists a
//! folder (`read_dir` is newest-first on tmpfs, hash order on ext4).
//!
//! The two-library comparison below only proves something on a filesystem
//! whose listing follows creation order (tmpfs lists newest first, so
//! reversing the creation order reverses the listing). ext4 lists by a
//! per-filesystem name hash, the same for both libraries, so there the
//! equality assertions hold with or without the sort, and the start-year
//! assertion catches a revert only for some hash orders.
//! `walk_lists_in_archive_order_whatever_the_creation_order` does not
//! depend on the filesystem: it checks the walk's exact output.
//!
//! What this file pins:
//!  - the same folder content, its files created in opposite orders in
//!    two libraries, gives the same series, rows, findings and counters,
//!    on the first scan and on a rescan that changes a file;
//!  - the identity peek reads the folder's first file in natural order
//!    (`Saga 1`, not `Saga 10` or a `Specials/` file), so a new series
//!    takes that issue's year;
//!  - a file that changes while a byte-identical copy of its new content
//!    appears in the same scan keeps its row, and the copy is a
//!    `DuplicateContent` finding, even when the copy's name sorts first; a
//!    new file holding the changed file's *old* bytes is its own issue.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, library_health_issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner::{self, ScanStats};
use std::io::{Cursor, Write};
use std::path::Path;
use uuid::Uuid;

/// A distinct PNG per colour (distinct bytes, distinct content hash).
fn png(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(6, 4, image::Rgb(rgb));
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

/// A one-page CBZ tagged `Saga` #`number`, cover year `year`.
fn cbz(number: &str, year: i32, rgb: [u8; 3]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(&png(rgb)).unwrap();
        zw.start_file("ComicInfo.xml", opts).unwrap();
        write!(
            zw,
            r#"<?xml version="1.0"?><ComicInfo><Series>Saga</Series><Number>{number}</Number><Year>{year}</Year></ComicInfo>"#
        )
        .unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

/// The first scan's folder: issue numbers past 9, a name in another case,
/// a byte-identical copy of #10 (two new files, one content), and a
/// special whose year would be a wrong start year.
fn first_scan_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Saga 1.cbz", cbz("1", 2012, [200, 0, 0])),
        ("Saga 2.cbz", cbz("2", 2012, [0, 200, 0])),
        ("Saga 10.cbz", cbz("10", 2013, [0, 0, 200])),
        ("Saga 10 (copy).cbz", cbz("10", 2013, [0, 0, 200])),
        ("saga 11.cbz", cbz("11", 2014, [120, 0, 120])),
        ("Specials/Saga Special 1.cbz", cbz("1", 2018, [200, 200, 0])),
    ]
}

/// What the rescan changes: `Saga 2` gets new bytes; `Saga 0` (sorts
/// before it) is a byte-identical copy of those new bytes; `Saga 1 (old)`
/// (also sorts before it) holds `Saga 2`'s old bytes.
fn rescan_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Saga 2.cbz", cbz("2", 2012, [0, 120, 120])),
        ("Saga 0.cbz", cbz("2", 2012, [0, 120, 120])),
        ("Saga 1 (old).cbz", cbz("2", 2012, [0, 200, 0])),
    ]
}

/// Write `files` under `folder` in the given order, each a moment after
/// the last, so a filesystem that lists by age lists them that way.
fn write_in_order(folder: &Path, files: &[(&str, Vec<u8>)]) {
    for (name, bytes) in files {
        let path = folder.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Everything a scan decides, with the library root taken out of paths.
#[derive(Debug, PartialEq)]
struct Outcome {
    series: Vec<(String, Option<i32>, Option<i32>, String)>,
    issues: Vec<(String, String, String, Option<String>, String)>,
    findings: Vec<(String, String)>,
    counters: Vec<u64>,
}

fn counters(s: &ScanStats) -> Vec<u64> {
    vec![
        s.files_seen,
        s.files_added,
        s.files_updated,
        s.files_unchanged,
        s.files_skipped,
        s.files_duplicate,
        s.files_malformed,
        s.files_page_hashed,
        s.series_created,
    ]
}

async fn outcome(app: &TestApp, lib: Uuid, root: &Path, stats: &ScanStats) -> Outcome {
    let db = &app.state().db;
    let root_str = root.to_string_lossy().into_owned();
    let rel = |p: &str| p.replace(&root_str, "<root>");
    let mut series: Vec<_> = series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|s| (s.name, s.year, s.volume, s.slug))
        .collect();
    series.sort();
    let rows = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib))
        .all(db)
        .await
        .unwrap();
    // An issue's id is the content hash it was first inserted with; a new
    // row whose content hash is already some row's id gets a generated id
    // instead, different on every run, so it is compared as a placeholder.
    let hashes: std::collections::HashSet<String> =
        rows.iter().map(|i| i.content_hash.clone()).collect();
    let mut issues: Vec<_> = rows
        .into_iter()
        .map(|i| {
            let id = if hashes.contains(&i.id) {
                i.id
            } else {
                "<generated>".to_owned()
            };
            (rel(&i.file_path), id, i.slug, i.number_raw, i.content_hash)
        })
        .collect();
    issues.sort();
    let mut findings: Vec<_> = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|h| (format!("{:?}", h.kind), rel(&h.payload.to_string())))
        .collect();
    findings.sort();
    Outcome {
        series,
        issues,
        findings,
        counters: counters(stats),
    }
}

/// Build the folder in one creation order, scan, apply the rescan's
/// changes in the same order, scan again.
async fn run(reverse: bool) -> (Outcome, Outcome, TestApp, tempfile::TempDir, Uuid) {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let folder = root.join("Saga");
    std::fs::create_dir_all(&folder).unwrap();
    let mut first = first_scan_files();
    let mut second = rescan_files();
    if reverse {
        first.reverse();
        second.reverse();
    }
    write_in_order(&folder, &first);
    let state = app.state();
    let lib = LibrarySeed::new(&root).insert(&state.db).await;
    let stats = scanner::scan_library(&state, lib).await.unwrap();
    let a = outcome(&app, lib, &root, &stats).await;
    write_in_order(&folder, &second);
    let stats = scanner::scan_library(&state, lib).await.unwrap();
    let b = outcome(&app, lib, &root, &stats).await;
    (a, b, app, dir, lib)
}

/// The walk the scanner builds every folder's list with hands back the
/// exact expected sequence, for names whose natural order, byte order and
/// creation order all differ, whatever the filesystem.
#[test]
fn walk_lists_in_archive_order_whatever_the_creation_order() {
    use server::library::ignore::IgnoreRules;
    use server::library::scanner::enumerate;
    // Created in this order; byte order would be
    // `A 3`, `B 10`, `Specials/…`, `a 1`, `b 2`.
    let created = [
        "b 2.cbz",
        "Specials/s 1.cbz",
        "B 10.cbz",
        "a 1.cbz",
        "A 3.cbz",
    ];
    // Natural order ignoring case; the subfolder last.
    let expected = [
        "a 1.cbz",
        "A 3.cbz",
        "b 2.cbz",
        "B 10.cbz",
        "Specials/s 1.cbz",
    ];
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Mixed");
    for name in created {
        let path = folder.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
    }
    let rel = |w: enumerate::ArchiveWalk| -> Vec<String> {
        w.archives
            .iter()
            .map(|p| {
                p.strip_prefix(&folder)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    };
    let ignore = IgnoreRules::default();
    assert_eq!(
        rel(enumerate::walk_archives_with(&folder, &ignore)),
        expected
    );
    assert_eq!(
        rel(enumerate::list_archives_changed_since(
            &folder,
            &ignore,
            chrono::DateTime::<chrono::Utc>::MIN_UTC,
        )),
        expected
    );
}

#[tokio::test]
async fn same_folder_content_gives_the_same_scan_whatever_the_creation_order() {
    let (first_a, rescan_a, _app_a, _dir_a, _) = run(false).await;
    let (first_b, rescan_b, _app_b, _dir_b, _) = run(true).await;
    assert_eq!(first_a, first_b, "first scan");
    assert_eq!(rescan_a, rescan_b, "rescan");

    // The peek read `Saga 1` (2012): not `Saga 10`, `saga 11` or the
    // special, whichever the filesystem listed first.
    assert_eq!(
        first_a.series.iter().map(|s| s.1).collect::<Vec<_>>(),
        vec![Some(2012)],
        "{:?}",
        first_a.series
    );
    // One of the two new identical files is a duplicate, the same one in
    // both libraries (checked by the equality above).
    assert_eq!(first_a.counters[5], 1, "{:?}", first_a.counters);
}

/// The duplicate case: the changed original keeps its row and the copy of
/// its new bytes is the duplicate, although the copy's name (`Saga 0`)
/// sorts first. The file holding the original's old bytes is a new issue.
#[tokio::test]
async fn changed_file_keeps_its_row_and_the_copy_of_its_new_bytes_is_the_duplicate() {
    let (first, rescan, app, dir, lib) = run(false).await;
    let root = dir.path();
    let db = &app.state().db;
    let row = |name: &'static str| {
        let path = root.join("Saga").join(name).to_string_lossy().into_owned();
        async move {
            issue::Entity::find()
                .filter(issue::Column::FilePath.eq(path))
                .one(db)
                .await
                .unwrap()
        }
    };
    let saga2_before = first
        .issues
        .iter()
        .find(|i| i.0.ends_with("/Saga 2.cbz"))
        .unwrap()
        .1
        .clone();
    let saga2 = row("Saga 2.cbz").await.expect("the original keeps its row");
    assert_eq!(saga2.id, saga2_before, "same issue id");
    assert_eq!(
        saga2.content_hash,
        blake3::hash(&rescan_files()[0].1).to_hex().to_string(),
        "carrying the new bytes"
    );
    assert!(
        row("Saga 0.cbz").await.is_none(),
        "the copy is not an issue"
    );
    assert!(
        row("Saga 1 (old).cbz").await.is_some(),
        "the original's old bytes belong to no other row now: a new issue"
    );
    // added: `Saga 1 (old)`; updated: `Saga 2`; duplicates: `Saga 0`, and
    // the first scan's copy of #10 again (a duplicate has no row, so it is
    // checked on every scan).
    assert_eq!(
        (rescan.counters[1], rescan.counters[2], rescan.counters[5]),
        (1, 1, 2),
        "{:?}",
        rescan.counters
    );
    let dups: Vec<_> = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .filter(|h| format!("{:?}", h.kind) == "DuplicateContent")
        .map(|h| h.payload.to_string())
        .filter(|p| p.contains("Saga 0.cbz"))
        .collect();
    assert_eq!(dups.len(), 1, "{dups:?}");
    assert!(
        dups[0].contains("Saga 2.cbz"),
        "kept: the original: {dups:?}"
    );
}
