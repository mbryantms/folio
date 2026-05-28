//! CBT writer — rebuild a tar-based comic archive from materialized page
//! payloads (`archive-rewrite-1.0` M4).
//!
//! Tar is uncompressed and has no central directory, so there's no
//! stream-copy-preserve optimization to make (unlike the CBZ writer's
//! `Keep` path): the page-editor job hands this writer fully-materialized
//! bytes for every output page. Pages are emitted under contiguous names
//! (`p0001.<ext>`, …) so the reader's natural sort reproduces the
//! requested order; `extras` (preserved `ComicInfo.xml` / `MetronInfo.xml`)
//! are appended verbatim.
//!
//! Output is a standard `ustar` archive (the `tar` crate's default
//! header). Caller-supplied deflate levels are ignored — tar stores
//! everything uncompressed.

use crate::cbz_write::RebuildSummary;
use crate::{ArchiveError, ArchiveLimits};
use std::fs::File;
use std::path::Path;
use tar::{Builder, Header};

/// Write a fresh CBT from materialized pages + preserved sidecars.
///
/// `pages` are `(extension, bytes, _level)` — emitted as `p0001.<ext>`,
/// `p0002.<ext>`, … in list order. `extras` are `(name, bytes, _level)`
/// written verbatim after the pages. Both ignore the level field. Fails
/// with [`ArchiveError::CapExceeded`] when the running byte / entry total
/// exceeds `limits`.
pub fn write_pages(
    pages: Vec<(String, Vec<u8>, i64)>,
    extras: Vec<(String, Vec<u8>, i64)>,
    dst_path: &Path,
    limits: ArchiveLimits,
) -> Result<RebuildSummary, ArchiveError> {
    let file = File::create(dst_path)?;
    let mut builder = Builder::new(file);
    let mut summary = RebuildSummary::default();

    for (i, (ext, bytes, _level)) in pages.iter().enumerate() {
        let name = format!("p{:04}.{}", i + 1, ext);
        append(&mut builder, &name, bytes)?;
        summary.replaced_count += 1;
        summary.entries_written += 1;
        summary.uncompressed_bytes = summary
            .uncompressed_bytes
            .saturating_add(bytes.len() as u64);
        enforce(&summary, limits)?;
    }

    for (name, bytes, _level) in &extras {
        append(&mut builder, name, bytes)?;
        summary.added_count += 1;
        summary.entries_written += 1;
        summary.uncompressed_bytes = summary
            .uncompressed_bytes
            .saturating_add(bytes.len() as u64);
        enforce(&summary, limits)?;
    }

    // `into_inner` writes the two zero-block trailer + returns the file.
    builder.into_inner().map_err(ArchiveError::from)?;
    Ok(summary)
}

fn append(builder: &mut Builder<File>, name: &str, bytes: &[u8]) -> Result<(), ArchiveError> {
    let mut header = Header::new_ustar();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_entry_type(tar::EntryType::Regular);
    // `append_data` sets the path on the header (long names spill into a
    // GNU/PAX extension) and recomputes the checksum before writing.
    builder
        .append_data(&mut header, name, bytes)
        .map_err(ArchiveError::from)
}

fn enforce(summary: &RebuildSummary, limits: ArchiveLimits) -> Result<(), ArchiveError> {
    if summary.uncompressed_bytes > limits.max_total_bytes {
        return Err(ArchiveError::CapExceeded(
            "rebuild output exceeded max_total_bytes",
        ));
    }
    if summary.entries_written > limits.max_entries {
        return Err(ArchiveError::CapExceeded(
            "rebuild output exceeded max_entries",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbt::Cbt;
    use crate::comic_archive::ComicArchive;
    use tempfile::NamedTempFile;

    #[test]
    fn write_pages_roundtrips_via_reader() {
        let dst = NamedTempFile::new().unwrap();
        let pages = vec![
            ("png".to_string(), b"PAGEONE".to_vec(), 0),
            ("png".to_string(), b"PAGETWO".to_vec(), 0),
        ];
        let extras = vec![("ComicInfo.xml".to_string(), b"<ComicInfo/>".to_vec(), 0)];
        write_pages(pages, extras, dst.path(), ArchiveLimits::default()).unwrap();

        let mut c = Cbt::open(dst.path(), ArchiveLimits::default()).unwrap();
        let names: Vec<String> = c.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["p0001.png", "p0002.png"]);
        assert_eq!(c.read_entry_bytes("p0001.png").unwrap(), b"PAGEONE");
        assert_eq!(c.read_entry_bytes("p0002.png").unwrap(), b"PAGETWO");
        // The sidecar is preserved (surfaced via the reader's lookup map).
        assert!(c.find("ComicInfo.xml").is_some());
    }

    #[test]
    fn write_pages_respects_cap() {
        let dst = NamedTempFile::new().unwrap();
        let pages = vec![("png".to_string(), vec![0u8; 4096], 0)];
        let limits = ArchiveLimits {
            max_total_bytes: 1024,
            ..ArchiveLimits::default()
        };
        let err = write_pages(pages, Vec::new(), dst.path(), limits).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded(_)));
    }
}
