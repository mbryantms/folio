//! Scan-time CBR→CBZ conversion (per-library `auto_convert_cbr_on_scan`).
//!
//! A `.cbr` extension is not a reliable signal of the real container: a large
//! fraction of "CBR" files in the wild are actually ZIP archives that were
//! renamed (or mislabeled by the tool that produced them). So this converter
//! sniffs the magic bytes rather than trusting the extension:
//!
//!   - **ZIP** (`PK\x03\x04`, …) — already a valid CBZ. We just move it into
//!     place byte-for-byte via an atomic rename to the `.cbz` sibling. No
//!     decompression, no `.bak` (the original bytes survive unchanged at the
//!     new path).
//!   - **RAR** (`Rar!\x1a\x07`) — a true CBR. We decompress every page with
//!     the `unrar`-backed reader, store them verbatim (deflate level 0;
//!     already-compressed JPEG/PNG don't benefit from re-deflate), preserve
//!     the metadata sidecars, and write a fresh `.cbz`. The atomic swap
//!     (temp → fsync → rename `.cbr`→`.cbr.bak` → rename `.cbz` →
//!     fsync-parent) is handled by [`crate::archive_rewrite::convert_atomic`],
//!     which keeps the original as a single `.bak` rollback slot.
//!   - **anything else** — genuinely unsupported; the caller skips with an
//!     `UnsupportedArchiveFormat` health issue.
//!
//! The RAR path mirrors the page editor's CBR branch
//! ([`crate::jobs::archive_edit::edit_one_issue`]) minus the page ops.

use crate::archive_rewrite::{self, RewriteError};
use archive::cbr::Cbr;
use archive::comic_archive::ComicArchive;
use archive::{ArchiveLimits, cbz_write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CbrConvertError {
    /// A `.cbz` sibling already exists at the destination. We refuse to
    /// overwrite it — the existing `.cbz` is ingested by the normal path and
    /// the `.cbr` is left for the operator to resolve (likely a duplicate).
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    /// The file isn't a container we can read (neither ZIP nor RAR) — the
    /// `.cbr` extension was misleading.
    #[error("unrecognized archive container (not ZIP or RAR)")]
    UnknownContainer,
    #[error(transparent)]
    Rewrite(#[from] RewriteError),
}

/// What the file actually is, by magic bytes — independent of its extension.
enum Container {
    Zip,
    Rar,
    Unknown,
}

/// Sniff the leading magic bytes. ZIP: `PK\x03\x04` / `PK\x05\x06` (empty) /
/// `PK\x07\x08` (spanned). RAR: `Rar!\x1a\x07` (covers RAR4 and RAR5).
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
    } else {
        Ok(Container::Unknown)
    }
}

/// Convert `src` (a `.cbr`) into a sibling `.cbz`. Returns the new `.cbz`
/// path on success. A ZIP-disguised-as-CBR is renamed in place; a real RAR is
/// decompressed and repacked (keeping the original as `<src>.bak`).
pub fn convert_cbr_to_cbz(src: &Path, limits: ArchiveLimits) -> Result<PathBuf, CbrConvertError> {
    let dst = src.with_extension("cbz");
    if dst.exists() {
        return Err(CbrConvertError::DestinationExists(dst));
    }
    match detect_container(src).map_err(RewriteError::Io)? {
        Container::Zip => {
            // Already a valid CBZ wearing a `.cbr` extension — move it into
            // place byte-for-byte. The rename is atomic on the same
            // directory/filesystem, so a crash can't leave a half-file. No
            // `.bak`: the identical bytes now live at `dst`, nothing to roll
            // back to.
            std::fs::rename(src, &dst).map_err(RewriteError::Io)?;
        }
        Container::Rar => {
            archive_rewrite::convert_atomic(src, &dst, |tmp| {
                let mut cbr = Cbr::open(src, limits).map_err(RewriteError::ArchiveErr)?;
                // Page list in natural-sort order — the same order the reader
                // uses.
                let page_names: Vec<String> = cbr.pages().iter().map(|e| e.name.clone()).collect();
                let mut pages: Vec<(String, Vec<u8>, i64)> = Vec::with_capacity(page_names.len());
                for name in &page_names {
                    let bytes = cbr
                        .read_entry_bytes(name)
                        .map_err(RewriteError::ArchiveErr)?;
                    pages.push((ext_of(name), bytes, 0));
                }
                // Preserve ComicInfo.xml / MetronInfo.xml verbatim (deflate
                // level 6), mirroring the page-edit rewrite. Other non-page
                // trash is dropped.
                let mut extras: Vec<(String, Vec<u8>, i64)> = Vec::new();
                for sidecar in ["ComicInfo.xml", "MetronInfo.xml"] {
                    if cbr.find(sidecar).is_some() {
                        let bytes = cbr
                            .read_entry_bytes(sidecar)
                            .map_err(RewriteError::ArchiveErr)?;
                        extras.push((sidecar.to_string(), bytes, 6));
                    }
                }
                cbz_write::write_pages(pages, extras, tmp, limits)
                    .map_err(RewriteError::ArchiveErr)?;
                Ok(())
            })?;
        }
        Container::Unknown => return Err(CbrConvertError::UnknownContainer),
    }
    Ok(dst)
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
        let other = write(tmp.path(), "o.cbr", b"\x00\x01\x02\x03junk");
        assert!(matches!(detect_container(&zip).unwrap(), Container::Zip));
        assert!(matches!(detect_container(&rar).unwrap(), Container::Rar));
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
}
