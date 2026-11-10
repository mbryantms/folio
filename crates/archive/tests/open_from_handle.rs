//! Every reader opens from a file handle and keeps reading through it.
//!
//! For each format with a fixture (CBZ and CBT built here, CBR from
//! `fixtures/synthetic-3page.cbr`, CB7 from `fixtures/synthetic-3page.cb7`
//! and the solid variant), the archive's handle is opened, and then its
//! path is renamed away or replaced by a symbolic link to a different
//! archive, both before the reader is built and again before the pages are
//! read. The pages must still be the original archive's bytes: the readers
//! never look at the name again.
//!
//!   cargo test -p archive --test open_from_handle

use archive::comic_archive::ComicArchive;
use archive::{ArchiveFile, ArchiveLimits, open_file};
use std::io::Write;
use std::path::{Path, PathBuf};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

fn png(label: &str) -> Vec<u8> {
    let mut v = PNG.to_vec();
    v.extend_from_slice(label.as_bytes());
    v.extend(std::iter::repeat_n(0u8, 32));
    v
}

fn write_cbz(path: &Path, label: &str) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 1..=3 {
        zw.start_file(format!("p{i:03}.png"), opts).unwrap();
        zw.write_all(&png(&format!("{label}-{i}"))).unwrap();
    }
    zw.finish().unwrap();
}

fn write_cbt(path: &Path, label: &str) {
    let mut tw = tar::Builder::new(std::fs::File::create(path).unwrap());
    for i in 1..=3 {
        let bytes = png(&format!("{label}-{i}"));
        let mut h = tar::Header::new_ustar();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tw.append_data(&mut h, format!("p{i:03}.png"), bytes.as_slice())
            .unwrap();
    }
    tw.finish().unwrap();
}

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    assert!(p.is_file(), "fixtures/{name} is committed");
    p
}

/// Every page of `a`, in page order.
fn all_pages(a: &mut dyn ComicArchive) -> Vec<Vec<u8>> {
    let names: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
    names
        .iter()
        .map(|n| a.read_entry_bytes(n).expect("page reads"))
        .collect()
}

/// What happens to the archive's name after its handle was opened.
#[derive(Clone, Copy, Debug)]
enum Swap {
    RenamedAway,
    LinkToOtherArchive,
}

fn swap(path: &Path, other: &Path, how: Swap) {
    let away = path.with_extension("moved");
    if away.exists() {
        std::fs::remove_file(&away).unwrap();
    }
    std::fs::rename(path, &away).unwrap();
    match how {
        Swap::RenamedAway => {}
        Swap::LinkToOtherArchive => std::os::unix::fs::symlink(other, path).unwrap(),
    }
}

/// Open `path`'s handle, swap the name, build the reader from the handle,
/// and read every page; returns the pages read through the handle.
fn read_after_swap(path: &Path, other: &Path, how: Swap) -> Vec<Vec<u8>> {
    let handle = std::fs::File::open(path).unwrap();
    swap(path, other, how);
    let mut a = open_file(ArchiveFile::new(handle, path), ArchiveLimits::default())
        .unwrap_or_else(|e| panic!("{how:?}: open from handle: {e}"));
    assert_eq!(a.path(), path, "display path is kept for messages");
    all_pages(&mut *a)
}

/// Build the reader first, swap the name, then read: per-entry reads (the
/// formats that used to reopen the path for each one) stay on the handle.
fn read_after_late_swap(path: &Path, other: &Path, how: Swap) -> Vec<Vec<u8>> {
    let handle = std::fs::File::open(path).unwrap();
    let mut a = open_file(ArchiveFile::new(handle, path), ArchiveLimits::default()).unwrap();
    swap(path, other, how);
    all_pages(&mut *a)
}

fn check_format(make: impl Fn(&Path, &str), ext: &str) {
    for how in [Swap::RenamedAway, Swap::LinkToOtherArchive] {
        for late in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("issue.{ext}"));
            let other = dir.path().join(format!("other.{ext}"));
            make(&path, "orig");
            make(&other, "other");
            let pages = if late {
                read_after_late_swap(&path, &other, how)
            } else {
                read_after_swap(&path, &other, how)
            };
            let want: Vec<Vec<u8>> = (1..=3).map(|i| png(&format!("orig-{i}"))).collect();
            assert_eq!(pages, want, "{ext} {how:?} late={late}");
        }
    }
}

#[test]
fn cbz_reads_through_the_handle_after_the_path_is_swapped() {
    check_format(write_cbz, "cbz");
}

#[test]
fn cbz_pread_index_describes_the_handle_not_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("issue.cbz");
    let other = dir.path().join("other.cbz");
    write_cbz(&path, "orig");
    write_cbz(&other, "a-much-longer-label-so-offsets-differ");
    let handle = std::fs::File::open(&path).unwrap();
    swap(&path, &other, Swap::LinkToOtherArchive);
    let src = ArchiveFile::new(handle, &path);
    let cbz = archive::cbz::Cbz::open_file(src.clone(), ArchiveLimits::default()).unwrap();
    let index = cbz.build_pread_index();
    for (i, e) in cbz.pages().iter().enumerate() {
        let ext = index.extent(e.index).expect("stored page has an extent");
        let mut buf = vec![0u8; ext.length as usize];
        src.read_exact_at(&mut buf, ext.data_start).unwrap();
        assert_eq!(buf, png(&format!("orig-{}", i + 1)));
    }
}

#[test]
fn cbt_reads_through_the_handle_after_the_path_is_swapped() {
    check_format(write_cbt, "cbt");
}

#[test]
fn cbr_reads_through_the_handle_after_the_path_is_swapped() {
    let fixture = fixture("synthetic-3page.cbr");
    let mut expected = archive::open_path_unchecked(&fixture, ArchiveLimits::default()).unwrap();
    let want = all_pages(&mut *expected);
    assert_eq!(want.len(), 3);
    for how in [Swap::RenamedAway, Swap::LinkToOtherArchive] {
        for late in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("issue.cbr");
            std::fs::copy(&fixture, &path).unwrap();
            // The link points at a different archive (a CBZ under a .cbr
            // name): reading the name would sniff ZIP, not RAR.
            let other = dir.path().join("other.cbr");
            write_cbz(&other, "other");
            let pages = if late {
                read_after_late_swap(&path, &other, how)
            } else {
                read_after_swap(&path, &other, how)
            };
            assert_eq!(pages, want, "cbr {how:?} late={late}");
        }
    }
}

#[test]
fn cb7_reads_through_the_handle_after_the_path_is_swapped() {
    for name in ["synthetic-3page.cb7", "synthetic-3page-solid.cb7"] {
        let fixture = fixture(name);
        let mut expected =
            archive::open_path_unchecked(&fixture, ArchiveLimits::default()).unwrap();
        let want = all_pages(&mut *expected);
        assert_eq!(want.len(), 3);
        for how in [Swap::RenamedAway, Swap::LinkToOtherArchive] {
            for late in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("issue.cb7");
                std::fs::copy(&fixture, &path).unwrap();
                let other = dir.path().join("other.cb7");
                write_cbz(&other, "other");
                let pages = if late {
                    read_after_late_swap(&path, &other, how)
                } else {
                    read_after_swap(&path, &other, how)
                };
                assert_eq!(pages, want, "{name} {how:?} late={late}");
            }
        }
    }
}

#[test]
fn a_multi_volume_rar_is_refused() {
    // RAR5 main archive header flag 0x0001 marks a volume; the committed
    // fixture is a single volume, so flip that flag in a copy.
    let bytes = std::fs::read(fixture("synthetic-3page.cbr")).unwrap();
    let Some(patched) = mark_rar5_volume(&bytes) else {
        panic!("fixture is not the expected RAR5 layout");
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("part1.cbr");
    std::fs::write(&path, patched).unwrap();
    let err = archive::open_path_unchecked(&path, ArchiveLimits::default())
        .err()
        .expect("a volume archive is refused");
    assert!(err.to_string().contains("multi-volume"), "{err}");
}

/// Set the "volume" flag in a RAR5 main archive header and fix its CRC.
/// Layout after the 8-byte signature: CRC32 (4), header size (vint),
/// header type (vint, 1 = main), header flags (vint), [extra size],
/// archive flags (vint, bit 0 = volume).
fn mark_rar5_volume(bytes: &[u8]) -> Option<Vec<u8>> {
    fn vint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = *b.get(i)?;
            v |= u64::from(byte & 0x7f) << shift;
            i += 1;
            if byte & 0x80 == 0 {
                return Some((v, i));
            }
            shift += 7;
        }
    }
    let mut out = bytes.to_vec();
    let crc_at = 8;
    let (size, body) = vint(&out, crc_at + 4)?;
    let (kind, after_kind) = vint(&out, body)?;
    if kind != 1 {
        return None;
    }
    let (hflags, mut i) = vint(&out, after_kind)?;
    if hflags & 0x01 != 0 {
        i = vint(&out, i)?.1;
    }
    if hflags & 0x02 != 0 {
        i = vint(&out, i)?.1;
    }
    // Archive flags: a one-byte vint in the fixture.
    if out.get(i)? & 0x80 != 0 {
        return None;
    }
    out[i] |= 0x01;
    let end = body + usize::try_from(size).ok()?;
    let crc = crc32(&out[crc_at + 4..end]);
    out[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
    Some(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
