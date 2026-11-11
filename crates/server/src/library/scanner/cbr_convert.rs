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
//! *container* also decides which per-library opt-in gates the conversion
//! (the callers check that before calling in here): a RAR named `.cbz` is
//! gated by `auto_convert_cbr_on_scan` exactly like a RAR named `.cbr`.
//! Such a mislabeled file converts **in place** — the destination is its
//! own path, so the original survives as `<name>.cbz.bak`.
//!
//! Detection itself lives in [`archive::container`] so this converter, the
//! archive reader dispatch and the page-server cache can never disagree.
//!
//! Every file operation goes through the library root's no-follow walk
//! (`library::no_follow`): the source is read through the handle
//! [`archive_rewrite::convert_atomic`] opens, the move-into-place and the
//! swap are relative to the folder handle, and a source or destination
//! that is a symbolic link is refused (`RewriteError::Refused`).
//!
//! The RAR path mirrors the page editor's CBR branch
//! ([`crate::jobs::archive_edit::edit_one_issue`]) minus the page ops.

use crate::archive_rewrite::{self, Converted, RewriteError};
use crate::library::no_follow::{EntryKind, LibraryRoot};
use archive::cb7::Cb7;
use archive::cbr::Cbr;
use archive::comic_archive::ComicArchive;
use archive::container::{Container, detect_container};
use archive::{ArchiveLimits, cbz_write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CbrConvertError {
    /// A `.cbz` sibling already exists at the destination. We refuse to
    /// overwrite it — the existing `.cbz` is ingested by the normal path and
    /// the source is left for the operator to resolve (likely a duplicate).
    ///
    /// The message names the file only; the full path is logged where the
    /// error is raised and stays on the variant for the scanner.
    #[error(
        "{} already exists; nothing was written. If Folio wrote that file during a \
         conversion that did not finish, remove that file and retry: the original \
         and its .bak still hold the original bytes. Otherwise another file has \
         this name.",
        archive_rewrite::file_label(.0)
    )]
    DestinationExists(PathBuf),
    /// The file isn't a container we can read (not ZIP, RAR, or 7z) — the
    /// extension was misleading.
    #[error("unrecognized archive container (not ZIP, RAR, or 7z)")]
    UnknownContainer,
    #[error(transparent)]
    Rewrite(#[from] RewriteError),
}

/// Convert `src` (a `.cbr`) into a sibling `.cbz`. A ZIP-disguised-as-CBR
/// is given the new name byte-for-byte; a real RAR (or 7z) is decompressed
/// and repacked (keeping the original as `<src>.bak`). The returned
/// [`Converted`] still holds the original at its old name: the caller
/// repoints its row at [`Converted::outcome`]`.target` and then calls
/// [`Converted::finish`] (or [`Converted::abandon`] when it could not).
pub fn convert_cbr_to_cbz(
    root: &LibraryRoot,
    src: &Path,
    limits: ArchiveLimits,
) -> Result<Converted, CbrConvertError> {
    convert_to_cbz(root, src, limits)
}

/// Convert `src` (a `.cb7`) into a sibling `.cbz` (WP-6.5). Same contract as
/// [`convert_cbr_to_cbz`]; the original is kept as `<src>.bak`
/// (`foo.cb7.bak`) unless it was a renamed ZIP.
pub fn convert_cb7_to_cbz(
    root: &LibraryRoot,
    src: &Path,
    limits: ArchiveLimits,
) -> Result<Converted, CbrConvertError> {
    convert_to_cbz(root, src, limits)
}

/// A destination that exists is this module's own error, so callers that
/// match it (the scanner's "twin already there") keep working.
fn dest_exists(e: RewriteError) -> CbrConvertError {
    match e {
        RewriteError::DestinationExists(p) => CbrConvertError::DestinationExists(p),
        other => CbrConvertError::Rewrite(other),
    }
}

fn convert_to_cbz(
    root: &LibraryRoot,
    src: &Path,
    limits: ArchiveLimits,
) -> Result<Converted, CbrConvertError> {
    let dst = src.with_extension("cbz");
    // A RAR / 7z already *named* `.cbz` (the extension lied) converts in
    // place: the destination is the source path itself, and `convert_atomic`
    // runs the in-place rewrite sequence (original kept as `<name>.cbz.bak`).
    let in_place = dst == src;
    // Sniff through a handle from the no-follow walk (a link is refused).
    // The writers below refuse an existing destination themselves (and
    // atomically); this early answer only spares a pointless sniff.
    let (dir, dst_name) = root.open_parent(&dst).map_err(RewriteError::from)?;
    if !in_place && dir.kind(&dst_name).map_err(RewriteError::Io)? != EntryKind::Missing {
        tracing::info!(path = %dst.display(), "archive conversion: destination already exists");
        return Err(CbrConvertError::DestinationExists(dst));
    }
    let source = root.open_archive(src).map_err(RewriteError::from)?;
    let converted = match detect_container(&source).map_err(RewriteError::Io)? {
        Container::Zip => {
            // Already a valid CBZ wearing another extension: it takes the
            // new name byte-for-byte (no `.bak`: the identical bytes live at
            // `dst`, nothing to roll back to). In place there is nothing to
            // do at all.
            if in_place {
                return Ok(Converted::unchanged(dst));
            }
            archive_rewrite::rename_into_place(root, src, &dst).map_err(dest_exists)?
        }
        Container::Rar => archive_rewrite::convert_atomic(root, src, &dst, |io| {
            let mut cbr = Cbr::open_file(io.source()?, limits).map_err(RewriteError::ArchiveErr)?;
            repack(&mut cbr, io.staging_file()?, limits)
        })
        .map_err(dest_exists)?,
        Container::SevenZ => archive_rewrite::convert_atomic(root, src, &dst, |io| {
            let mut cb7 = Cb7::open_file(io.source()?, limits).map_err(RewriteError::ArchiveErr)?;
            // One decode pass for the whole archive; the reads below are
            // then served from memory (solid 7z can't seek per entry).
            cb7.preload_all().map_err(RewriteError::ArchiveErr)?;
            repack(&mut cb7, io.staging_file()?, limits)
        })
        .map_err(dest_exists)?,
        // A tar has its own reader and writer (`.cbt`); it is not something
        // this converter repacks, so a tar wearing `.cbr` is "unknown" here.
        Container::Tar | Container::Unknown => return Err(CbrConvertError::UnknownContainer),
    };
    Ok(converted)
}

/// Read every page (natural-sort order — the order the reader uses) plus the
/// entries the rewrite policy keeps, and write them as a CBZ at `tmp`.
fn repack(
    src: &mut dyn ComicArchive,
    tmp: std::fs::File,
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

    /// The committed `fixtures/synthetic-3page.cbr` copied into `dir` as
    /// `name` — any name, so a test can give a RAR the wrong extension.
    fn cbr_fixture(dir: &Path, name: &str) -> PathBuf {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic-3page.cbr");
        let dst = dir.join(name);
        std::fs::copy(&src, &dst).unwrap();
        dst
    }

    /// A RAR named `.cbz` (the Martian Manhunter (2006) #2/#3 case: opens in
    /// YACreader, which sniffs; used to land as `MalformedComicInfo` here)
    /// converts in place — same path, now a real ZIP, original kept as
    /// `<name>.cbz.bak`.
    #[test]
    fn rar_disguised_as_cbz_converts_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let src = cbr_fixture(tmp.path(), "issue.cbz");
        let original = std::fs::read(&src).unwrap();
        assert!(matches!(
            archive::container::detect_container_path_unchecked(&src).unwrap(),
            Container::Rar
        ));

        let dst = convert_cbr_to_cbz(
            &LibraryRoot::resolve(tmp.path()),
            &src,
            ArchiveLimits::default(),
        )
        .unwrap()
        .finish()
        .target;
        assert_eq!(dst, src, "in-place conversion keeps the path");
        assert!(matches!(
            archive::container::detect_container_path_unchecked(&dst).unwrap(),
            Container::Zip
        ));
        let bak = tmp.path().join("issue.cbz.bak");
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            original,
            "original parked as .bak"
        );

        let cbz = archive::cbz::Cbz::open_path_unchecked(&dst, ArchiveLimits::default()).unwrap();
        assert_eq!(cbz.pages().len(), 3);
    }

    /// A genuine ZIP named `.cbz` never reaches the converter from the
    /// scanner, but the in-place path must still be a no-op if it does.
    #[test]
    fn zip_named_cbz_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("issue.cbz");
        {
            let f = std::fs::File::create(&src).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("p001.jpg", opts).unwrap();
            zw.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
            zw.finish().unwrap();
        }
        let original = std::fs::read(&src).unwrap();
        let dst = convert_cbr_to_cbz(
            &LibraryRoot::resolve(tmp.path()),
            &src,
            ArchiveLimits::default(),
        )
        .unwrap()
        .finish()
        .target;
        assert_eq!(dst, src);
        assert_eq!(std::fs::read(&dst).unwrap(), original);
        assert!(!tmp.path().join("issue.cbz.bak").exists());
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

        let dst = convert_cbr_to_cbz(
            &LibraryRoot::resolve(tmp.path()),
            &cbr,
            ArchiveLimits::default(),
        )
        .unwrap()
        .finish()
        .target;
        assert_eq!(dst, tmp.path().join("issue.cbz"));
        assert!(dst.exists(), "renamed .cbz exists");
        assert!(!cbr.exists(), "original .cbr renamed away");
        // No .bak for the pure-rename path.
        assert!(!tmp.path().join("issue.cbr.bak").exists());
        // Byte-for-byte identical.
        assert_eq!(std::fs::read(&dst).unwrap(), original);
        // And it's a readable CBZ.
        let cbz = archive::cbz::Cbz::open_path_unchecked(&dst, ArchiveLimits::default()).unwrap();
        assert_eq!(cbz.pages().len(), 1);
    }

    #[test]
    fn refuses_when_cbz_twin_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let cbr = write(tmp.path(), "dup.cbr", b"PK\x03\x04");
        std::fs::write(tmp.path().join("dup.cbz"), b"existing").unwrap();
        assert!(matches!(
            convert_cbr_to_cbz(
                &LibraryRoot::resolve(tmp.path()),
                &cbr,
                ArchiveLimits::default()
            ),
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
            convert_cbr_to_cbz(
                &LibraryRoot::resolve(tmp.path()),
                &cbr,
                ArchiveLimits::default()
            ),
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
            let mut expected =
                archive::cb7::Cb7::open_path_unchecked(&src, ArchiveLimits::default()).unwrap();
            let want_pages: Vec<Vec<u8>> = ["page-001.jpg", "page-002.jpg", "page-003.jpg"]
                .iter()
                .map(|n| expected.read_entry_bytes(n).unwrap())
                .collect();
            let want_notes = expected.read_entry_bytes("notes.txt").unwrap();
            let want_xml = expected.read_entry_bytes("ComicInfo.xml").unwrap();

            let dst = convert_cb7_to_cbz(
                &LibraryRoot::resolve(tmp.path()),
                &src,
                ArchiveLimits::default(),
            )
            .expect(fixture);
            assert!(
                src.exists(),
                "{fixture}: the original keeps its name until finish"
            );
            let dst = dst.finish().target;
            assert_eq!(dst, tmp.path().join("issue.cbz"));
            assert!(!src.exists(), "{fixture}: original moved away");
            // The original survives byte-for-byte as the single `.bak` slot.
            let bak = tmp.path().join("issue.cb7.bak");
            assert_eq!(std::fs::read(&bak).unwrap(), original, "{fixture}: .bak");

            let mut cbz =
                archive::cbz::Cbz::open_path_unchecked(&dst, ArchiveLimits::default()).unwrap();
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
        let err = convert_cb7_to_cbz(
            &LibraryRoot::resolve(tmp.path()),
            &src,
            ArchiveLimits::default(),
        )
        .unwrap_err();
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
        let dst = convert_cb7_to_cbz(
            &LibraryRoot::resolve(tmp.path()),
            &cb7,
            ArchiveLimits::default(),
        )
        .unwrap()
        .finish()
        .target;
        assert_eq!(std::fs::read(&dst).unwrap(), original);
        assert!(!tmp.path().join("issue.cb7.bak").exists());
    }

    /// A source that is a link, or one whose folder is a link, is refused
    /// and nothing is written; the link's target is untouched.
    #[test]
    fn conversion_refuses_a_linked_source_or_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("library");
        let series = lib.join("Series");
        std::fs::create_dir_all(&series).unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let victim = cbr_fixture(&outside, "victim.cbr");
        let victim_bytes = std::fs::read(&victim).unwrap();
        let root = LibraryRoot::resolve(&lib);

        let link = series.join("issue.cbr");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let err = convert_cbr_to_cbz(&root, &link, ArchiveLimits::default()).unwrap_err();
        assert!(
            matches!(err, CbrConvertError::Rewrite(RewriteError::Refused(_))),
            "{err:?}"
        );
        assert!(!series.join("issue.cbz").exists());
        assert!(!series.join("issue.cbr.bak").exists());

        let linked_folder = lib.join("Linked");
        std::os::unix::fs::symlink(&outside, &linked_folder).unwrap();
        let err = convert_cbr_to_cbz(
            &root,
            &linked_folder.join("victim.cbr"),
            ArchiveLimits::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, CbrConvertError::Rewrite(RewriteError::Refused(_))),
            "{err:?}"
        );
        assert_eq!(std::fs::read(&victim).unwrap(), victim_bytes);
        let mut names: Vec<_> = std::fs::read_dir(&outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["victim.cbr"], "nothing created outside the root");
    }
}
