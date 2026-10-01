//! CB7 (7z) reader validation (WP-6.5).
//!
//! The committed fixtures come from `fixtures/make-cb7-fixture.py` (host
//! `7z`): a non-solid COPY archive, a solid LZMA2 archive, and an
//! AES-encrypted one. Hostile shapes (bombs, traversal names, misnamed
//! sidecars) are synthesized per-test with `sevenz-rust2`'s writer (the
//! `compress` feature is a dev-dependency only).
//!
//!   cargo test -p archive --test cb7_fixture

use archive::cb7::Cb7;
use archive::comic_archive::ComicArchive;
use archive::{ArchiveError, ArchiveLimits, open};
use sevenz_rust2::{
    ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod, SourceReader,
};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// Page bytes exactly as `make-cb7-fixture.py` writes them.
fn page_bytes(i: u8) -> Vec<u8> {
    let mut v = hex("FFD8FFE000104A46494600010100000100010000");
    v.extend(std::iter::repeat_n(i, 64));
    v.extend([0xFF, 0xD9]);
    v
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn assert_fixture_contents(path: &Path, expect_solid: bool) {
    let mut a = Cb7::open(path, ArchiveLimits::default()).expect("open cb7");
    assert_eq!(a.is_solid(), expect_solid);
    let pages: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(pages, ["page-001.jpg", "page-002.jpg", "page-003.jpg"]);
    assert!(a.entries_skipped().is_empty());
    assert!(a.find("comicinfo.xml").is_some(), "case-insensitive find");
    assert!(a.find("notes.txt").is_some(), "foreign entry listed");

    // Out of order: the solid case must re-decode from the block start.
    assert_eq!(a.read_entry_bytes("page-003.jpg").unwrap(), page_bytes(3));
    assert_eq!(a.read_entry_bytes("page-001.jpg").unwrap(), page_bytes(1));
    assert_eq!(a.read_entry_bytes("PAGE-002.JPG").unwrap(), page_bytes(2));
    let xml = a.read_entry_bytes("ComicInfo.xml").unwrap();
    assert!(xml.starts_with(b"<?xml"), "ComicInfo decodes");
    assert_eq!(
        a.read_entry_prefix("page-002.jpg", 3).unwrap(),
        [0xFF, 0xD8, 0xFF]
    );
    assert!(matches!(
        a.read_entry_bytes("missing.jpg"),
        Err(ArchiveError::Malformed(_))
    ));
}

#[test]
fn non_solid_fixture_lists_and_decodes() {
    assert_fixture_contents(&fixture("synthetic-3page.cb7"), false);
}

#[test]
fn solid_fixture_lists_and_decodes() {
    assert_fixture_contents(&fixture("synthetic-3page-solid.cb7"), true);
}

#[test]
fn dispatch_by_extension_opens_cb7() {
    let a = open(
        &fixture("synthetic-3page-solid.cb7"),
        ArchiveLimits::default(),
    )
    .expect("archive::open dispatches .cb7");
    assert_eq!(a.pages().len(), 3);
}

#[test]
fn preload_all_serves_every_entry_from_one_pass() {
    let mut a = Cb7::open(
        fixture("synthetic-3page-solid.cb7"),
        ArchiveLimits::default(),
    )
    .unwrap();
    a.preload_all().expect("preload");
    // A prefix read leaves the cached entry for the full read that follows.
    assert_eq!(
        a.read_entry_prefix("page-002.jpg", 3).unwrap(),
        [0xFF, 0xD8, 0xFF]
    );
    for i in 1..=3u8 {
        let name = format!("page-{i:03}.jpg");
        assert_eq!(a.read_entry_bytes(&name).unwrap(), page_bytes(i));
        // Served from (and drained out of) the cache; a re-read decodes again.
        assert_eq!(a.read_entry_bytes(&name).unwrap(), page_bytes(i));
    }
    assert!(
        a.read_entry_bytes("notes.txt")
            .unwrap()
            .starts_with(b"foreign entry")
    );
}

#[test]
fn encrypted_fixtures_are_refused_as_encrypted() {
    // Encrypted headers fail inside the header parse; encrypted data with
    // plaintext headers is caught by the AES-coder preflight.
    for name in [
        "synthetic-3page-encrypted.cb7",
        "synthetic-3page-encrypted-data.cb7",
    ] {
        let err = Cb7::open(fixture(name), ArchiveLimits::default())
            .expect_err("encrypted cb7 must not open");
        assert!(
            matches!(err, ArchiveError::Encrypted),
            "{name}: expected Encrypted, got {err:?}"
        );
    }
}

#[test]
fn garbage_bytes_are_malformed() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("junk.cb7");
    std::fs::write(&p, b"definitely not a 7z archive").unwrap();
    assert!(matches!(
        Cb7::open(&p, ArchiveLimits::default()),
        Err(ArchiveError::Malformed(_))
    ));
}

// ── synthesized archives ─────────────────────────────────────────────

/// Write `entries` into a fresh 7z at `path` — one block per entry
/// (non-solid) or all in one block (solid) — with the given method.
fn write_7z(path: &Path, entries: &[(&str, Vec<u8>)], solid: bool, method: EncoderMethod) {
    let mut w = ArchiveWriter::create(path).unwrap();
    w.set_content_methods(vec![EncoderConfiguration::new(method)]);
    if solid {
        let metas = entries
            .iter()
            .map(|(n, _)| ArchiveEntry::new_file(n))
            .collect();
        let readers = entries
            .iter()
            .map(|(_, b)| SourceReader::new(std::io::Cursor::new(b.clone())))
            .collect();
        w.push_archive_entries(metas, readers).unwrap();
    } else {
        for (name, bytes) in entries {
            w.push_archive_entry(
                ArchiveEntry::new_file(name),
                Some(std::io::Cursor::new(bytes.clone())),
            )
            .unwrap();
        }
    }
    w.finish().unwrap();
}

#[test]
fn decompression_bomb_is_rejected_by_ratio_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("bomb.cb7");
    // 4 MiB of zeros LZMA2-packs to well under 1 KiB — far past the 200:1 cap.
    let mut page = page_bytes(1);
    page.resize(4 << 20, 0);
    write_7z(&p, &[("page-001.jpg", page)], true, EncoderMethod::LZMA2);
    let err = Cb7::open(&p, ArchiveLimits::default()).expect_err("bomb must not open");
    assert!(
        matches!(err, ArchiveError::CapExceeded("compression ratio")),
        "got {err:?}"
    );
}

#[test]
fn traversal_and_backslash_names_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    for (i, bad) in [
        "../evil.jpg",
        "a/../../evil.jpg",
        "dir\\evil.jpg",
        "/abs.jpg",
    ]
    .iter()
    .enumerate()
    {
        let p = tmp.path().join(format!("bad{i}.cb7"));
        write_7z(
            &p,
            &[("page-001.jpg", page_bytes(1)), (bad, page_bytes(2))],
            false,
            EncoderMethod::COPY,
        );
        let err = Cb7::open(&p, ArchiveLimits::default()).expect_err(bad);
        assert!(
            matches!(err, ArchiveError::UnsafeEntry(_)),
            "{bad}: expected UnsafeEntry, got {err:?}"
        );
    }
}

#[test]
fn misnamed_sidecar_is_skipped_and_stream_stays_aligned() {
    for solid in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sniff.cb7");
        write_7z(
            &p,
            &[
                (
                    "page-000.jpg",
                    b"<?xml version=\"1.0\"?><ComicInfo/>".to_vec(),
                ),
                ("page-001.jpg", page_bytes(1)),
                ("page-002.png", b"not a png either".to_vec()),
                ("page-003.jpg", page_bytes(3)),
                ("Thumbs.db", b"junk".to_vec()),
            ],
            solid,
            EncoderMethod::LZMA2,
        );
        let mut a = Cb7::open(&p, ArchiveLimits::default()).unwrap();
        let pages: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, ["page-001.jpg", "page-003.jpg"], "solid={solid}");
        let skipped: Vec<&str> = a
            .entries_skipped()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(skipped, ["page-000.jpg", "page-002.png"], "solid={solid}");
        assert!(a.find("Thumbs.db").is_none(), "junk ignored");
        // Indices stay dense after the drop.
        for (i, e) in a.entries().iter().enumerate() {
            assert_eq!(e.index, i);
        }
        // Reading past drained/skipped entries still lines up.
        assert_eq!(a.read_entry_bytes("page-003.jpg").unwrap(), page_bytes(3));
        assert_eq!(a.read_entry_bytes("page-001.jpg").unwrap(), page_bytes(1));
    }
}

#[test]
fn caps_reject_when_tightened() {
    let path = fixture("synthetic-3page.cb7");
    let base = Cb7::open(&path, ArchiveLimits::default()).unwrap();
    let n = base.entries().len() as u64;
    let biggest = base
        .entries()
        .iter()
        .map(|e| e.uncompressed_size)
        .max()
        .unwrap();
    let total: u64 = base.entries().iter().map(|e| e.uncompressed_size).sum();

    let cases = [
        (
            ArchiveLimits {
                max_entries: n - 1,
                ..ArchiveLimits::default()
            },
            "entry count",
        ),
        (
            ArchiveLimits {
                max_entry_bytes: biggest - 1,
                ..ArchiveLimits::default()
            },
            "entry size",
        ),
        (
            ArchiveLimits {
                max_total_bytes: total - 1,
                ..ArchiveLimits::default()
            },
            "total bytes",
        ),
    ];
    for (limits, want) in cases {
        match Cb7::open(&path, limits) {
            Err(ArchiveError::CapExceeded(got)) => assert_eq!(got, want),
            other => panic!("{want}: expected CapExceeded, got {other:?}"),
        }
    }
}
