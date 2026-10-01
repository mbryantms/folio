//! Scan-time CBR/CB7→CBZ conversion (per-library `auto_convert_cbr_on_scan`
//! / `auto_convert_cb7_on_scan`).
//!
//! Neither RAR nor 7z can be written, so Folio ingests both by converting
//! them to CBZ in place. An extension is not a reliable signal of the real
//! container — a large fraction of "CBR" (and some "CB7") files in the wild
//! are actually ZIP archives that were renamed or mislabeled — so the
//! converter sniffs magic bytes rather than trusting the extension:
//!
//!   - **ZIP** (`PK\x03\x04`, …) — already a valid CBZ. We just move it into
//!     place byte-for-byte via an atomic rename to the `.cbz` sibling. No
//!     decompression, no `.bak` (the original bytes survive unchanged at the
//!     new path).
//!   - **RAR** (`Rar!\x1a\x07`) / **7z** (`7z\xBC\xAF\x27\x1C`) — decompress
//!     every page with the `unrar`- / `sevenz-rust2`-backed reader, store
//!     them verbatim (deflate level 0; already-compressed JPEG/PNG don't
//!     benefit from re-deflate), preserve the metadata sidecars + foreign
//!     entries per [`archive::rewrite_policy`], and write a fresh `.cbz`.
//!     The atomic swap (temp → fsync → rename original → `<original>.bak` →
//!     rename `.cbz` → fsync-parent) is handled by
//!     [`crate::archive_rewrite::convert_atomic`], which keeps the original
//!     as a single `.bak` rollback slot. A 7z source is decoded in one pass
//!     ([`Cb7::preload_all`]) so a solid archive isn't re-decoded per page.
//!   - **anything else** — genuinely unsupported; the caller skips with an
//!     `UnsupportedArchiveFormat` health issue.
//!
//! The container decides the decoder, not the extension: a `.cb7` that is
//! really a RAR converts through the RAR reader and vice versa. The
//! *extension* decides which per-library opt-in gates the conversion (the
//! callers check that before calling in here).
//!
//! The RAR path mirrors the page editor's CBR branch
//! ([`crate::jobs::archive_edit::edit_one_issue`]) minus the page ops.

use crate::archive_rewrite::{self, RewriteError};
use archive::cb7::Cb7;
use archive::cbr::Cbr;
use archive::comic_archive::ComicArchive;
use archive::{ArchiveLimits, cbz_write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CbrConvertError {
    /// A `.cbz` sibling already exists at the destination. We refuse to
    /// overwrite it — the existing `.cbz` is ingested by the normal path and
    /// the source is left for the operator to resolve (likely a duplicate).
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    /// The file isn't a container we can read (not ZIP, RAR, or 7z) — the
    /// extension was misleading.
    #[error("unrecognized archive container (not ZIP, RAR, or 7z)")]
    UnknownContainer,
    #[error(transparent)]
    Rewrite(#[from] RewriteError),
}

/// What the file actually is, by magic bytes — independent of its extension.
enum Container {
    Zip,
    Rar,
    SevenZ,
    Unknown,
}

/// Sniff the leading magic bytes. ZIP: `PK\x03\x04` / `PK\x05\x06` (empty) /
/// `PK\x07\x08` (spanned). RAR: `Rar!\x1a\x07` (covers RAR4 and RAR5). 7z:
/// `7z\xBC\xAF\x27\x1C`.
fn detect_container(src: &Path) -> Result<Container, std::io::Error> {
    use std::io::Read;
    let mut head = [0u8; 8];
    let mut f = std::fs::File::open(src)?;
    let n = f.read(&mut head)?;
    let head = &head[..n];
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"PK\x05\x06")
        || head.starts_with(b"PK\x07\x08")
    {
        Ok(Container::Zip)
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Ok(Container::Rar)
    } else if head.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Ok(Container::SevenZ)
    } else {
        Ok(Container::Unknown)
    }
}

/// Convert `src` (a `.cbr`) into a sibling `.cbz`. Returns the new `.cbz`
/// path on success. A ZIP-disguised-as-CBR is renamed in place; a real RAR
/// (or 7z) is decompressed and repacked (keeping the original as
/// `<src>.bak`).
pub fn convert_cbr_to_cbz(src: &Path, limits: ArchiveLimits) -> Result<PathBuf, CbrConvertError> {
    convert_to_cbz(src, limits)
}

/// Convert `src` (a `.cb7`) into a sibling `.cbz` (WP-6.5). Same contract as
/// [`convert_cbr_to_cbz`]; the original is kept as `<src>.bak`
/// (`foo.cb7.bak`) unless it was a renamed ZIP.
pub fn convert_cb7_to_cbz(src: &Path, limits: ArchiveLimits) -> Result<PathBuf, CbrConvertError> {
    convert_to_cbz(src, limits)
}

fn convert_to_cbz(src: &Path, limits: ArchiveLimits) -> Result<PathBuf, CbrConvertError> {
    let dst = src.with_extension("cbz");
    if dst.exists() {
        return Err(CbrConvertError::DestinationExists(dst));
    }
    match detect_container(src).map_err(RewriteError::Io)? {
        Container::Zip => {
            // Already a valid CBZ wearing another extension — move it into
            // place byte-for-byte. The rename is atomic on the same
            // directory/filesystem, so a crash can't leave a half-file. No
            // `.bak`: the identical bytes now live at `dst`, nothing to roll
            // back to.
            std::fs::rename(src, &dst).map_err(RewriteError::Io)?;
        }
        Container::Rar => {
            archive_rewrite::convert_atomic(src, &dst, |tmp| {
                let mut cbr = Cbr::open(src, limits).map_err(RewriteError::ArchiveErr)?;
                repack(&mut cbr, tmp, limits)
            })?;
        }
        Container::SevenZ => {
            archive_rewrite::convert_atomic(src, &dst, |tmp| {
                let mut cb7 = Cb7::open(src, limits).map_err(RewriteError::ArchiveErr)?;
                // One decode pass for the whole archive; the reads below are
                // then served from memory (solid 7z can't seek per entry).
                cb7.preload_all().map_err(RewriteError::ArchiveErr)?;
                repack(&mut cb7, tmp, limits)
            })?;
        }
        Container::Unknown => return Err(CbrConvertError::UnknownContainer),
    }
    Ok(dst)
}

/// Read every page (natural-sort order — the order the reader uses) plus the
/// entries the rewrite policy keeps, and write them as a CBZ at `tmp`.
fn repack(
    src: &mut dyn ComicArchive,
    tmp: &Path,
    limits: ArchiveLimits,
) -> Result<(), RewriteError> {
    let page_names: Vec<String> = src.pages().iter().map(|e| e.name.clone()).collect();
    let mut pages: Vec<(String, Vec<u8>, i64)> = Vec::with_capacity(page_names.len());
    for name in &page_names {
        let bytes = src
            .read_entry_bytes(name)
            .map_err(RewriteError::ArchiveErr)?;
        pages.push((ext_of(name), bytes, 0));
    }
    // Preserve every non-page entry the rewrite policy keeps (root
    // ComicInfo/MetronInfo pair + foreign sidecars such as CoMet.xml /
    // notes.txt) verbatim, mirroring the page editor and the sidecar
    // rewrite. Junk is dropped.
    let extras =
        archive::rewrite_policy::preserved_extras(src, true).map_err(RewriteError::ArchiveErr)?;
    cbz_write::write_pages(pages, extras, tmp, limits).map_err(RewriteError::ArchiveErr)?;
    Ok(())
}

/// Lowercase extension (no dot) of an entry name, defaulting to `jpg`.
/// Matches [`crate::jobs::archive_edit::ext_of`] behavior.
fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 5)
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "jpg".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ext_of_handles_common_cases() {
        assert_eq!(ext_of("p001.JPG"), "jpg");
        assert_eq!(ext_of("foo/bar.png"), "png");
        assert_eq!(ext_of("noext"), "jpg");
    }

    fn write(dir: &Path, name: &str, magic: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(magic).unwrap();
        p
    }

    #[test]
    fn detects_container_from_magic_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = write(tmp.path(), "z.cbr", b"PK\x03\x04rest");
        let rar = write(tmp.path(), "r.cbr", b"Rar!\x1a\x07\x00x");
        let sz = write(tmp.path(), "s.cb7", b"7z\xBC\xAF\x27\x1C\x00\x04");
        let other = write(tmp.path(), "o.cbr", b"\x00\x01\x02\x03junk");
        assert!(matches!(detect_container(&zip).unwrap(), Container::Zip));
        assert!(matches!(detect_container(&rar).unwrap(), Container::Rar));
        assert!(matches!(detect_container(&sz).unwrap(), Container::SevenZ));
        assert!(matches!(
            detect_container(&other).unwrap(),
            Container::Unknown
        ));
    }

    #[test]
    fn zip_disguised_as_cbr_is_renamed_in_place() {
        // Build a real (tiny) ZIP, name it `.cbr`, and confirm the converter
        // moves it byte-for-byte to `.cbz`.
        let tmp = tempfile::tempdir().unwrap();
        let cbr = tmp.path().join("issue.cbr");
        {
            let f = std::fs::File::create(&cbr).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("p001.jpg", opts).unwrap();
            zw.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
            zw.finish().unwrap();
        }
        let original = std::fs::read(&cbr).unwrap();

        let dst = convert_cbr_to_cbz(&cbr, ArchiveLimits::default()).unwrap();
        assert_eq!(dst, tmp.path().join("issue.cbz"));
        assert!(dst.exists(), "renamed .cbz exists");
        assert!(!cbr.exists(), "original .cbr renamed away");
        // No .bak for the pure-rename path.
        assert!(!tmp.path().join("issue.cbr.bak").exists());
        // Byte-for-byte identical.
        assert_eq!(std::fs::read(&dst).unwrap(), original);
        // And it's a readable CBZ.
        let cbz = archive::cbz::Cbz::open(&dst, ArchiveLimits::default()).unwrap();
        assert_eq!(cbz.pages().len(), 1);
    }

    #[test]
    fn refuses_when_cbz_twin_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let cbr = write(tmp.path(), "dup.cbr", b"PK\x03\x04");
        std::fs::write(tmp.path().join("dup.cbz"), b"existing").unwrap();
        assert!(matches!(
            convert_cbr_to_cbz(&cbr, ArchiveLimits::default()),
            Err(CbrConvertError::DestinationExists(_))
        ));
        // Original left untouched.
        assert!(cbr.exists());
    }

    #[test]
    fn rejects_unknown_container() {
        let tmp = tempfile::tempdir().unwrap();
        let cbr = write(tmp.path(), "junk.cbr", b"\x00\x01\x02\x03not-an-archive");
        assert!(matches!(
            convert_cbr_to_cbz(&cbr, ArchiveLimits::default()),
            Err(CbrConvertError::UnknownContainer)
        ));
    }

    /// Copy a committed `fixtures/*.cb7` (see `fixtures/make-cb7-fixture.py`)
    /// into `dir` as `name`.
    fn cb7_fixture(dir: &Path, fixture: &str, name: &str) -> PathBuf {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(fixture);
        let dst = dir.join(name);
        std::fs::copy(&src, &dst).unwrap();
        dst
    }

    #[test]
    fn cb7_fixtures_convert_to_cbz_preserving_pages_and_sidecars() {
        for fixture in ["synthetic-3page.cb7", "synthetic-3page-solid.cb7"] {
            let tmp = tempfile::tempdir().unwrap();
            let src = cb7_fixture(tmp.path(), fixture, "issue.cb7");
            let original = std::fs::read(&src).unwrap();
            let mut expected = archive::cb7::Cb7::open(&src, ArchiveLimits::default()).unwrap();
            let want_pages: Vec<Vec<u8>> = ["page-001.jpg", "page-002.jpg", "page-003.jpg"]
                .iter()
                .map(|n| expected.read_entry_bytes(n).unwrap())
                .collect();
            let want_notes = expected.read_entry_bytes("notes.txt").unwrap();
            let want_xml = expected.read_entry_bytes("ComicInfo.xml").unwrap();

            let dst = convert_cb7_to_cbz(&src, ArchiveLimits::default()).expect(fixture);
            assert_eq!(dst, tmp.path().join("issue.cbz"));
            assert!(!src.exists(), "{fixture}: original moved away");
            // The original survives byte-for-byte as the single `.bak` slot.
            let bak = tmp.path().join("issue.cb7.bak");
            assert_eq!(std::fs::read(&bak).unwrap(), original, "{fixture}: .bak");

            let mut cbz = archive::cbz::Cbz::open(&dst, ArchiveLimits::default()).unwrap();
            let pages: Vec<String> = cbz.pages().iter().map(|e| e.name.clone()).collect();
            assert_eq!(pages.len(), 3, "{fixture}: page count");
            for (name, want) in pages.iter().zip(&want_pages) {
                assert_eq!(&cbz.read_entry_bytes_by_name(name).unwrap(), want);
            }
            assert_eq!(
                cbz.read_entry_bytes_by_name("notes.txt").unwrap(),
                want_notes
            );
            assert_eq!(
                cbz.read_entry_bytes_by_name("ComicInfo.xml").unwrap(),
                want_xml
            );
        }
    }

    #[test]
    fn encrypted_cb7_conversion_fails_and_leaves_original() {
        let tmp = tempfile::tempdir().unwrap();
        let src = cb7_fixture(tmp.path(), "synthetic-3page-encrypted.cb7", "locked.cb7");
        let err = convert_cb7_to_cbz(&src, ArchiveLimits::default()).unwrap_err();
        assert!(
            matches!(
                err,
                CbrConvertError::Rewrite(RewriteError::ArchiveErr(
                    archive::ArchiveError::Encrypted
                ))
            ),
            "got {err:?}"
        );
        assert!(src.exists(), "original untouched");
        assert!(!tmp.path().join("locked.cbz").exists());
        assert!(!tmp.path().join("locked.cb7.bak").exists());
    }

    #[test]
    fn zip_disguised_as_cb7_is_renamed_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let cb7 = tmp.path().join("issue.cb7");
        {
            let f = std::fs::File::create(&cb7).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("p001.jpg", opts).unwrap();
            zw.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
            zw.finish().unwrap();
        }
        let original = std::fs::read(&cb7).unwrap();
        let dst = convert_cb7_to_cbz(&cb7, ArchiveLimits::default()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), original);
        assert!(!tmp.path().join("issue.cb7.bak").exists());
    }
}
