//! Library Scanner v1 — Milestone 12 multi-format archive smoke tests.
//!
//! Validates the dispatch + the CBT (tar) reader. CBR is now a real
//! read-only reader (archive-rewrite-1.0 M6); a valid `.cbr` fixture
//! can't be created in-repo (no RAR writer exists), so we only smoke-test
//! that a non-RAR `.cbr` fails gracefully. CB7 is still scaffolded.

use archive::{ArchiveError, ArchiveLimits, open};
use tempfile::tempdir;

fn write_tar(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let f = std::fs::File::create(path).unwrap();
    let mut tar_writer = tar::Builder::new(f);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar_writer.append(&header, *bytes).unwrap();
    }
    let _ = tar_writer.into_inner().unwrap().sync_all();
}

#[test]
fn cbt_round_trip() {
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("test.cbt");
    write_tar(
        &p,
        &[
            ("page-001.png", b"\x89PNGfake-bytes-001"),
            ("page-002.png", b"\x89PNGfake-bytes-002"),
            ("ComicInfo.xml", b"<ComicInfo></ComicInfo>"),
        ],
    );

    let mut archive = open(&p, ArchiveLimits::default()).unwrap();
    let pages = archive.pages();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].name, "page-001.png");
    assert_eq!(pages[1].name, "page-002.png");

    assert!(archive.find("ComicInfo.xml").is_some());
    let bytes = archive.read_entry_bytes("ComicInfo.xml").unwrap();
    assert_eq!(bytes, b"<ComicInfo></ComicInfo>");

    let p1 = archive.read_entry_bytes("page-001.png").unwrap();
    assert_eq!(p1, b"\x89PNGfake-bytes-001");
}

#[test]
fn cbr_rejects_non_rar_gracefully() {
    // The CBR reader is implemented (unrar-backed), but a bogus payload
    // must surface as a typed Malformed error rather than panicking — and
    // it must NOT be the old "not yet implemented" stub message.
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("test.cbr");
    std::fs::write(&p, b"not a real rar").unwrap();
    let err = match open(&p, ArchiveLimits::default()) {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    let msg = match err {
        ArchiveError::Malformed(s) => s,
        other => panic!("expected Malformed, got {other:?}"),
    };
    assert!(
        !msg.contains("not yet implemented"),
        "stub message leaked: {msg}"
    );
}

#[test]
fn cb7_currently_returns_not_implemented() {
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("test.cb7");
    std::fs::write(&p, b"not a real 7z").unwrap();
    let err = match open(&p, ArchiveLimits::default()) {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    let msg = match err {
        ArchiveError::Malformed(s) => s,
        other => panic!("expected Malformed, got {other:?}"),
    };
    assert!(msg.contains("CB7 support not yet implemented"));
}

#[test]
fn unknown_extension_rejected() {
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("test.zip");
    std::fs::write(&p, b"junk").unwrap();
    let err = match open(&p, ArchiveLimits::default()) {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    assert!(matches!(err, ArchiveError::Malformed(_)));
}
