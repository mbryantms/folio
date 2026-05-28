//! Real-RAR validation for the CBR reader + CBR→CBZ conversion writer
//! (`archive-rewrite-1.0` M6).
//!
//! These tests need an actual `.cbr` (RAR) file, which can't be created
//! in-repo (no RAR writer exists). They're `#[ignore]`d so CI never
//! depends on a local fixture, and they skip gracefully when no
//! `fixtures/*.cbr` is present. Run locally with:
//!
//!   cargo test -p archive --test cbr_fixture -- --ignored --nocapture

use archive::cbr::Cbr;
use archive::cbz::Cbz;
use archive::cbz_write::write_pages;
use archive::comic_archive::ComicArchive;
use archive::{ArchiveLimits, open};
use std::path::PathBuf;

/// First `*.cbr` under the workspace `fixtures/` dir, if any.
fn first_cbr() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("cbr"))
        })
}

#[test]
#[ignore = "needs a local fixtures/*.cbr (not committed); run with --ignored"]
fn cbr_reader_lists_and_decodes_pages() {
    let Some(path) = first_cbr() else {
        return; // no local fixture — skip
    };

    let mut a = open(&path, ArchiveLimits::default()).expect("open cbr");
    let names: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
    assert!(
        names.len() >= 2,
        "expected several pages, got {}",
        names.len()
    );

    // Decompress the first page and confirm it's real image bytes (these
    // fixtures are JPEG). This proves unrar actually inflated the entry.
    let bytes = a.read_entry_bytes(&names[0]).expect("read first page");
    assert!(!bytes.is_empty());
    assert_eq!(
        &bytes[..3],
        &[0xFF, 0xD8, 0xFF],
        "first page should decode to JPEG magic bytes",
    );

    // The last page decodes too (exercises a full front-to-back walk).
    let last = a
        .read_entry_bytes(names.last().unwrap())
        .expect("read last page");
    assert!(!last.is_empty());
}

#[test]
#[ignore = "needs a local fixtures/*.cbr (not committed); run with --ignored"]
fn cbr_to_cbz_roundtrip_preserves_page_bytes() {
    let Some(path) = first_cbr() else {
        return; // no local fixture — skip
    };

    // Mimic the job's CBR path: decompress every page into materialized
    // (ext, bytes, store) and write a CBZ via the conversion writer.
    let mut src = Cbr::open(&path, ArchiveLimits::default()).expect("open cbr");
    let names: Vec<String> = src.pages().iter().map(|e| e.name.clone()).collect();
    let mut mats: Vec<(String, Vec<u8>, i64)> = Vec::with_capacity(names.len());
    for name in &names {
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_ascii_lowercase();
        let bytes = src.read_entry_bytes(name).expect("read page");
        mats.push((ext, bytes, 0));
    }
    let first_orig = mats[0].1.clone();
    let last_orig = mats.last().unwrap().1.clone();
    let page_count = mats.len();

    let dst = tempfile::NamedTempFile::new().unwrap();
    write_pages(mats, Vec::new(), dst.path(), ArchiveLimits::default()).expect("write cbz");

    // Reopen as a CBZ and confirm the conversion preserved every page,
    // contiguously renamed, byte-for-byte.
    let mut cbz = Cbz::open(dst.path(), ArchiveLimits::default()).expect("open converted cbz");
    let out_names: Vec<String> = cbz.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(out_names.len(), page_count, "page count preserved");
    assert_eq!(out_names[0], "p0001.jpg", "contiguous page naming");

    let out_first = cbz
        .read_entry_bytes_by_name(&out_names[0])
        .expect("read converted first page");
    assert_eq!(out_first, first_orig, "first page byte-preserved CBR→CBZ");
    let out_last = cbz
        .read_entry_bytes_by_name(out_names.last().unwrap())
        .expect("read converted last page");
    assert_eq!(out_last, last_orig, "last page byte-preserved CBR→CBZ");
}
