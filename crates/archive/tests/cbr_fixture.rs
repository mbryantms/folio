//! Real-RAR validation for the CBR reader + CBR→CBZ conversion writer
//! (`archive-rewrite-1.0` M6).
//!
//! These tests run against `fixtures/synthetic-3page.cbr`, a RAR5 archive
//! emitted by `fixtures/make-cbr-fixture.py` (stored entries only — no free
//! RAR compressor exists, but the container format for method 0 is small
//! enough to write by hand). Any other local `fixtures/*.cbr` also works:
//!
//!   cargo test -p archive --test cbr_fixture

use archive::cbr::Cbr;
use archive::cbz::Cbz;
use archive::cbz_write::write_pages;
use archive::comic_archive::ComicArchive;
use archive::{ArchiveLimits, open_path_unchecked as open};
use std::path::PathBuf;

/// The committed synthetic `fixtures/synthetic-3page.cbr` (RAR5, stored, three
/// JFIF-stub pages — see `fixtures/make-cbr-fixture.py`), falling back to any
/// other `*.cbr` a developer dropped under `fixtures/`.
fn first_cbr() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let synthetic = dir.join("synthetic-3page.cbr");
    if synthetic.is_file() {
        return Some(synthetic);
    }
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
fn cbr_reader_lists_and_decodes_pages() {
    let path = first_cbr()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");

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
fn cbr_to_cbz_roundtrip_preserves_page_bytes() {
    let path = first_cbr()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");

    // Mimic the job's CBR path: decompress every page into materialized
    // (ext, bytes, store) and write a CBZ via the conversion writer.
    let mut src = Cbr::open_path_unchecked(&path, ArchiveLimits::default()).expect("open cbr");
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
    write_pages(
        mats,
        Vec::new(),
        std::fs::File::create(dst.path()).unwrap(),
        ArchiveLimits::default(),
    )
    .expect("write cbz");

    // Reopen as a CBZ and confirm the conversion preserved every page,
    // contiguously renamed, byte-for-byte.
    let mut cbz =
        Cbz::open_path_unchecked(dst.path(), ArchiveLimits::default()).expect("open converted cbz");
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

/// CQ-TEST-2 (audit 2026-07): CBR cap enforcement, opt-in like the other
/// fixture tests (no RAR writer exists to commit a fixture). Uses a real
/// local `fixtures/*.cbr` and tightens the caps below its own shape so
/// each guard must fire.
#[test]
fn cbr_caps_reject_when_tightened() {
    let path = first_cbr()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");

    // Baseline: opens under default limits; learn its shape.
    let a = open(&path, ArchiveLimits::default()).expect("open cbr");
    let n = a.pages().len() as u64;
    let max_entry = a
        .pages()
        .iter()
        .map(|e| e.uncompressed_size)
        .max()
        .unwrap_or(0);
    assert!(n > 0, "fixture has pages");

    // Entry-count cap below the real count must reject.
    let r = open(
        &path,
        ArchiveLimits {
            max_entries: n - 1,
            ..ArchiveLimits::default()
        },
    );
    assert!(r.is_err(), "entry-count cap must fire (n={n})");

    // Per-entry cap below the largest page must reject.
    let r = open(
        &path,
        ArchiveLimits {
            max_entry_bytes: max_entry.saturating_sub(1),
            ..ArchiveLimits::default()
        },
    );
    assert!(r.is_err(), "entry-size cap must fire (max={max_entry})");
}

/// `fixtures/synthetic-3page-damaged-tail.cbr` (`make-cbr-fixture.py
/// --damaged-tail`): three good pages, then a header with a bad CRC and no
/// end-of-archive block — the shape of old Mac-made RAR 1.5 archives
/// (Fantastic Four (1961) #309 and friends) that UnRAR reads in full but
/// answers "File header damaged" for after the last file. The reader opens
/// it with every page instead of refusing the whole file.
#[test]
fn cbr_with_a_damaged_trailing_header_reads_every_page_before_it() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page-damaged-tail.cbr");
    let mut cbr = Cbr::open_path_unchecked(&path, ArchiveLimits::default())
        .expect("a damaged trailing header is the end of the archive, not an error");
    assert_eq!(
        cbr.recovery_used(),
        Some(archive::recovery::RAR_DAMAGED_TRAILING_HEADER)
    );
    let names: Vec<String> = cbr.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["page-001.jpg", "page-002.jpg", "page-003.jpg"]);
    for name in &names {
        let bytes = cbr.read_entry_bytes(name).expect("page reads");
        assert!(
            bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
            "{name} is the JPEG stub"
        );
    }
    assert!(
        cbr.entries_skipped().is_empty(),
        "the sniff pass kept every page"
    );
}

/// The intact fixture reports no recovery.
#[test]
fn intact_cbr_reports_no_recovery() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic-3page.cbr");
    let cbr = Cbr::open_path_unchecked(&path, ArchiveLimits::default()).expect("opens");
    assert_eq!(cbr.recovery_used(), None);
}

/// `fixtures/synthetic-3page-damaged-page.cbr` (`make-cbr-fixture.py
/// --damaged-page`): page 2's data has one flipped byte after its CRC was
/// taken. A plain read still refuses it; the salvaging read returns the
/// bytes UnRAR decoded, marked damaged, and the clean pages around it read
/// as intact.
#[test]
fn salvaging_read_keeps_a_page_that_fails_its_checksum() {
    use archive::comic_archive::EntryDamage;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page-damaged-page.cbr");
    let mut cbr = Cbr::open_path_unchecked(&path, ArchiveLimits::default()).expect("opens");
    let names: Vec<String> = cbr.pages().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, ["page-001.jpg", "page-002.jpg", "page-003.jpg"]);

    assert!(
        cbr.read_entry_bytes("page-002.jpg").is_err(),
        "a plain read still refuses the damaged page"
    );

    let jfif: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0];
    let damaged = cbr.read_entry_bytes_salvaging("page-002.jpg").unwrap();
    assert_eq!(
        damaged.damage,
        Some(EntryDamage {
            reason: archive::damage::CHECKSUM_FAILED,
            declared_size: 86,
        })
    );
    assert_eq!(damaged.bytes.len(), 86, "every decoded byte is kept");
    assert!(damaged.bytes.starts_with(jfif));
    assert_eq!(damaged.bytes[40], 2 ^ 0xFF, "the flipped byte is as stored");

    for clean in ["page-001.jpg", "page-003.jpg"] {
        let r = cbr.read_entry_bytes_salvaging(clean).unwrap();
        assert_eq!(r.damage, None, "{clean}");
        assert_eq!(r.bytes, cbr.read_entry_bytes(clean).unwrap(), "{clean}");
    }
}

/// The salvaging read never lifts the entry-size limit: a damaged entry
/// over `max_entry_bytes` is refused before anything is decoded.
#[test]
fn salvaging_read_keeps_the_entry_size_limit() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page-damaged-page.cbr");
    let limits = ArchiveLimits {
        max_entry_bytes: 80,
        ..ArchiveLimits::default()
    };
    // The listing already refuses an entry over the limit.
    assert!(matches!(
        Cbr::open_path_unchecked(&path, limits),
        Err(archive::ArchiveError::CapExceeded(_))
    ));
}
