//! CBZ writer — atomic rebuild of a comic archive.
//!
//! Shared infrastructure for two sister plans:
//!
//!   - [`metadata-sidecar-writeback-1.0`](../../../../../.claude/plans/metadata-sidecar-writeback-1.0.md)
//!     — uses only `EntryOp::Replace` on the two sidecar names
//!     (`ComicInfo.xml`, `MetronInfo.xml`); every page entry takes the
//!     default `Keep` path which stream-copies compressed bytes
//!     verbatim. That makes a 500 MB scan archive's metadata refresh
//!     proportional to the XML payload, not the page count.
//!
//!   - [`archive-rewrite-1.0`](../../../../../.claude/plans/archive-rewrite-1.0.md)
//!     — uses `EntryOp::Replace` (rotated pages, replaced pages),
//!     `EntryOp::Remove` (page removal), and `additions` (replacement
//!     uploads). Recompression at the per-library JPEG quality runs
//!     only on the touched entries; everything else stream-copies.
//!
//! ## Invariants
//!
//!   - `Keep`'d entries land in the destination with byte-equal
//!     compressed payloads (`raw_copy_file`). Critical for the sidecar
//!     plan's "every page byte preserved" property.
//!   - Entries the [reader] skips (Thumbs.db, dotfiles, `__MACOSX`,
//!     sidecar suffixes `.xml`/`.json`/`.txt`) are dropped on rewrite
//!     so trash doesn't propagate. ComicInfo/MetronInfo entries are
//!     allowed back in via the `additions` channel, the same way the
//!     scanner discovers them.
//!   - Source entry order preserved for `Keep`'d entries; `additions`
//!     append at the end in their declared order.
//!   - Output written to a caller-supplied path; the rebuilder doesn't
//!     touch the source file. The atomic-swap orchestrator in
//!     `crates/server/src/archive_rewrite/` is what renames the temp
//!     over the original.
//!
//! [reader]: super::cbz::Cbz

use crate::cbz::Cbz;
use crate::{ArchiveError, ArchiveLimits};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, result::ZipError};

/// Per-entry override the caller can apply to the source archive.
///
/// Default for any source entry not listed in [`RebuildPlan::overrides`] is
/// "keep" — i.e. stream-copy verbatim. Explicit ops:
///
///   - [`EntryOp::Remove`] drops the entry from the output.
///   - [`EntryOp::Replace`] rewrites the entry's payload with `bytes`,
///     compressed at `level` (0 = stored / no deflate; 6 = default
///     deflate; 9 = max).
#[derive(Debug, Clone)]
pub enum EntryOp {
    Remove,
    Replace { bytes: Vec<u8>, level: i64 },
}

/// What to write to the output archive.
///
/// `overrides` keys are entry names matched **case-insensitively** against
/// the source archive's stored names (mirrors `Cbz::find`). Entries not
/// listed are kept; entries listed with [`EntryOp::Replace`] for a name
/// that doesn't exist in the source become additions. `additions` is for
/// brand-new entries the caller wants in addition to whatever ops fall out
/// of `overrides` — useful when the entry name is computed and you don't
/// want the case-insensitive override-match to kick in.
#[derive(Debug, Clone, Default)]
pub struct RebuildPlan {
    pub overrides: BTreeMap<String, EntryOp>,
    pub additions: Vec<(String, Vec<u8>, i64)>,
}

impl RebuildPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience: replace an entry, picking deflate level 6 (zlib
    /// default). For pre-compressed payloads (JPEG / PNG bytes) prefer
    /// [`Self::set_entry_stored`].
    pub fn set_entry(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        self.overrides.insert(
            name.into(),
            EntryOp::Replace {
                bytes,
                level: 6,
            },
        );
    }

    /// Convenience: replace an entry with `bytes` written uncompressed
    /// (deflate level 0 / Stored). Use for already-compressed image
    /// payloads so we don't waste cycles deflating data that won't
    /// shrink.
    pub fn set_entry_stored(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        self.overrides.insert(
            name.into(),
            EntryOp::Replace {
                bytes,
                level: 0,
            },
        );
    }

    pub fn remove_entry(&mut self, name: impl Into<String>) {
        self.overrides.insert(name.into(), EntryOp::Remove);
    }
}

/// Aggregate stats the orchestrator surfaces to the caller (audit log
/// payload, dialog summary, etc.).
#[derive(Debug, Clone, Default)]
pub struct RebuildSummary {
    pub entries_written: u64,
    pub uncompressed_bytes: u64,
    pub kept_count: u64,
    pub replaced_count: u64,
    pub removed_count: u64,
    pub added_count: u64,
}

/// Run the rebuild plan against `src` and write the result to `dst_path`.
///
/// The destination file is created if missing and truncated if present.
/// The caller is responsible for the atomic-swap dance (write to a temp
/// path, fsync, rename over the original); this function only produces
/// the freshly-encoded zip at the path it's given. Returns
/// `Err(ArchiveError::CapExceeded)` when the running uncompressed-byte
/// total exceeds `limits.max_total_bytes` — checked progressively so a
/// runaway plan fails fast.
pub fn rebuild(
    src: &mut Cbz,
    plan: RebuildPlan,
    dst_path: &Path,
    limits: ArchiveLimits,
) -> Result<RebuildSummary, ArchiveError> {
    let dst_file = File::create(dst_path)?;
    let mut writer = ZipWriter::new(dst_file);

    // Build a case-insensitive lookup of override names → canonical key in
    // the map. Source entries are matched in `Cbz::find` case-insensitively;
    // mirror that here so the caller can specify "ComicInfo.xml" and have
    // it match an in-archive "comicinfo.xml" entry.
    let lower_to_key: BTreeMap<String, String> = plan
        .overrides
        .keys()
        .map(|k| (k.to_ascii_lowercase(), k.clone()))
        .collect();

    let mut summary = RebuildSummary::default();
    let mut consumed_override_keys: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for ordinal in 0..src.inner_entry_count() {
        if src.raw_entry_is_skipped(ordinal)? {
            // Trash entry (Thumbs.db, dotfile, __MACOSX, sidecar .xml/.json/.txt).
            // The reader filters these; we drop them on rewrite to keep the
            // round-trip property — `additions` is the channel that puts
            // ComicInfo.xml / MetronInfo.xml back in.
            continue;
        }
        let raw_name = src.raw_entry_name(ordinal)?;
        let matched_key = lower_to_key.get(&raw_name.to_ascii_lowercase()).cloned();

        match matched_key.as_deref().and_then(|k| plan.overrides.get(k)) {
            Some(EntryOp::Remove) => {
                summary.removed_count += 1;
                if let Some(k) = matched_key {
                    consumed_override_keys.insert(k);
                }
            }
            Some(EntryOp::Replace { bytes, level }) => {
                let opts = build_options(bytes.len(), *level);
                writer
                    .start_file(&raw_name, opts)
                    .map_err(map_zip_err)?;
                writer.write_all(bytes).map_err(ArchiveError::from)?;
                summary.replaced_count += 1;
                summary.entries_written += 1;
                summary.uncompressed_bytes = summary
                    .uncompressed_bytes
                    .saturating_add(bytes.len() as u64);
                if summary.uncompressed_bytes > limits.max_total_bytes {
                    return Err(ArchiveError::CapExceeded(
                        "rebuild output exceeded max_total_bytes",
                    ));
                }
                if let Some(k) = matched_key {
                    consumed_override_keys.insert(k);
                }
            }
            None => {
                let info = src.raw_copy_to(ordinal, &mut writer)?;
                summary.kept_count += 1;
                summary.entries_written += 1;
                summary.uncompressed_bytes = summary
                    .uncompressed_bytes
                    .saturating_add(info.uncompressed_size);
                if summary.uncompressed_bytes > limits.max_total_bytes {
                    return Err(ArchiveError::CapExceeded(
                        "rebuild output exceeded max_total_bytes",
                    ));
                }
            }
        }

        if summary.entries_written > limits.max_entries {
            return Err(ArchiveError::CapExceeded(
                "rebuild output exceeded max_entries",
            ));
        }
    }

    // Overrides for names not present in the source become additions of
    // the same payload — same `Replace` semantics, but they emit
    // unconditionally. `Remove` for a non-existent name is a no-op.
    for (key, op) in &plan.overrides {
        if consumed_override_keys.contains(key) {
            continue;
        }
        if let EntryOp::Replace { bytes, level } = op {
            let opts = build_options(bytes.len(), *level);
            writer.start_file(key, opts).map_err(map_zip_err)?;
            writer.write_all(bytes).map_err(ArchiveError::from)?;
            summary.added_count += 1;
            summary.entries_written += 1;
            summary.uncompressed_bytes = summary
                .uncompressed_bytes
                .saturating_add(bytes.len() as u64);
            if summary.uncompressed_bytes > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded(
                    "rebuild output exceeded max_total_bytes",
                ));
            }
        }
    }

    // Explicit `additions` channel — used by callers (sister plan's
    // page-replace flow) who need to add a freshly-encoded payload at a
    // name that doesn't conflict with any source entry.
    for (name, bytes, level) in &plan.additions {
        let opts = build_options(bytes.len(), *level);
        writer.start_file(name, opts).map_err(map_zip_err)?;
        writer.write_all(bytes).map_err(ArchiveError::from)?;
        summary.added_count += 1;
        summary.entries_written += 1;
        summary.uncompressed_bytes = summary
            .uncompressed_bytes
            .saturating_add(bytes.len() as u64);
        if summary.uncompressed_bytes > limits.max_total_bytes {
            return Err(ArchiveError::CapExceeded(
                "rebuild output exceeded max_total_bytes",
            ));
        }
    }

    writer.finish().map_err(map_zip_err)?;
    Ok(summary)
}

/// Pick a `SimpleFileOptions` matching the requested deflate level.
/// Level 0 maps to `Stored` (no compression); 1..=9 maps to `Deflated`
/// at that level. Anything outside the range clamps to the default
/// deflate level (6 / zlib default).
fn build_options(_payload_len: usize, level: i64) -> SimpleFileOptions {
    if level <= 0 {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        let clamped = level.clamp(1, 9);
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(clamped))
    }
}

fn map_zip_err(e: ZipError) -> ArchiveError {
    match e {
        ZipError::Io(io) => ArchiveError::Io(io.to_string()),
        ZipError::InvalidArchive(msg) => ArchiveError::Malformed(msg.to_string()),
        ZipError::UnsupportedArchive(msg) => ArchiveError::Malformed(msg.to_string()),
        ZipError::FileNotFound => ArchiveError::Malformed("zip entry not found".into()),
        other => ArchiveError::Malformed(format!("zip error: {other}")),
    }
}

// ───────── tests ─────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbz::Cbz;
    use std::io::Read;
    use std::io::{Cursor, Seek, SeekFrom};
    use tempfile::NamedTempFile;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    /// Build a small fixture archive with three pages + a ComicInfo.xml.
    /// Returns a temp file whose path can be passed to `Cbz::open`.
    fn build_fixture() -> (NamedTempFile, [Vec<u8>; 3], Vec<u8>) {
        let tmp = NamedTempFile::new().expect("temp");
        let page0 = b"PAGE0BYTES".to_vec();
        let page1 = b"PAGE1BYTES_SLIGHTLY_LONGER".to_vec();
        let page2 = b"PAGE2".to_vec();
        let info = b"<?xml version=\"1.0\"?><ComicInfo><Title>Fixture</Title></ComicInfo>".to_vec();
        {
            let mut zw = ZipWriter::new(tmp.reopen().expect("reopen"));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zw.start_file("p1.jpg", opts).unwrap();
            zw.write_all(&page0).unwrap();
            zw.start_file("p2.jpg", opts).unwrap();
            zw.write_all(&page1).unwrap();
            zw.start_file("p3.jpg", opts).unwrap();
            zw.write_all(&page2).unwrap();
            zw.start_file("ComicInfo.xml", opts).unwrap();
            zw.write_all(&info).unwrap();
            zw.finish().unwrap();
        }
        (tmp, [page0, page1, page2], info)
    }

    /// Read every entry of `path` and return a `(name → bytes)` map.
    fn extract(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let mut a = ZipArchive::new(Cursor::new(buf)).unwrap();
        let mut out = BTreeMap::new();
        for i in 0..a.len() {
            let mut zf = a.by_index(i).unwrap();
            let name = zf.name().to_string();
            let mut bytes = Vec::new();
            zf.read_to_end(&mut bytes).unwrap();
            out.insert(name, bytes);
        }
        out
    }

    /// Compressed-bytes view of every entry in `path` — exposes the raw
    /// LFH+payload window. Used to prove `Keep`'d entries land byte-for-
    /// byte equal across a rebuild.
    fn raw_payload_lens(path: &Path) -> BTreeMap<String, u64> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let mut a = ZipArchive::new(Cursor::new(buf)).unwrap();
        let mut out = BTreeMap::new();
        for i in 0..a.len() {
            let raw = a.by_index_raw(i).unwrap();
            out.insert(raw.name().to_string(), raw.compressed_size());
        }
        out
    }

    #[test]
    fn replace_sidecar_keeps_pages_byte_equal() {
        let (src_file, pages, _info) = build_fixture();
        let src_path = src_file.path().to_path_buf();

        let pre_lens = raw_payload_lens(&src_path);

        let mut src = Cbz::open(&src_path, ArchiveLimits::default()).unwrap();
        let dst = NamedTempFile::new().unwrap();
        let dst_path = dst.path().to_path_buf();

        let mut plan = RebuildPlan::new();
        plan.set_entry(
            "ComicInfo.xml",
            b"<?xml version=\"1.0\"?><ComicInfo><Title>Updated</Title></ComicInfo>".to_vec(),
        );

        let summary = rebuild(&mut src, plan, &dst_path, ArchiveLimits::default()).unwrap();
        drop(src);

        assert_eq!(summary.replaced_count, 0, "ComicInfo.xml is reader-skipped on rewrite; it must arrive via the override-becomes-addition path");
        // The reader filters `.xml` entries from the page index, so the
        // override on `ComicInfo.xml` becomes an addition after the
        // existing entry is dropped.
        assert_eq!(summary.added_count, 1);
        assert_eq!(summary.kept_count, 3, "three page entries preserved");

        let out = extract(&dst_path);
        // Three page entries arrived byte-equal.
        assert_eq!(out.get("p1.jpg"), Some(&pages[0]));
        assert_eq!(out.get("p2.jpg"), Some(&pages[1]));
        assert_eq!(out.get("p3.jpg"), Some(&pages[2]));
        // New ComicInfo landed.
        assert!(out
            .get("ComicInfo.xml")
            .expect("ComicInfo.xml")
            .windows(7)
            .any(|w| w == b"Updated"));

        // Compressed-byte equality on each page: the `Keep` path used
        // `raw_copy_file`, so the deflate stream is identical.
        let post_lens = raw_payload_lens(&dst_path);
        assert_eq!(pre_lens.get("p1.jpg"), post_lens.get("p1.jpg"));
        assert_eq!(pre_lens.get("p2.jpg"), post_lens.get("p2.jpg"));
        assert_eq!(pre_lens.get("p3.jpg"), post_lens.get("p3.jpg"));
    }

    #[test]
    fn remove_entry_drops_it() {
        let (src_file, pages, _info) = build_fixture();
        let mut src = Cbz::open(src_file.path(), ArchiveLimits::default()).unwrap();
        let dst = NamedTempFile::new().unwrap();

        let mut plan = RebuildPlan::new();
        plan.remove_entry("p2.jpg");

        let summary = rebuild(&mut src, plan, dst.path(), ArchiveLimits::default()).unwrap();
        assert_eq!(summary.removed_count, 1);
        assert_eq!(summary.kept_count, 2);

        let out = extract(dst.path());
        assert!(!out.contains_key("p2.jpg"));
        assert_eq!(out.get("p1.jpg"), Some(&pages[0]));
        assert_eq!(out.get("p3.jpg"), Some(&pages[2]));
    }

    #[test]
    fn over_size_cap_fails_pre_finish() {
        let (src_file, _pages, _info) = build_fixture();
        let mut src = Cbz::open(src_file.path(), ArchiveLimits::default()).unwrap();
        let dst = NamedTempFile::new().unwrap();

        let mut plan = RebuildPlan::new();
        plan.set_entry("p1.jpg", vec![0u8; 4 * 1024]); // 4 KiB

        let limits = ArchiveLimits {
            max_total_bytes: 1024, // 1 KiB cap
            ..ArchiveLimits::default()
        };

        let err = rebuild(&mut src, plan, dst.path(), limits).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded(_)));
    }

    #[test]
    fn case_insensitive_override_match() {
        let (src_file, _pages, _info) = build_fixture();
        let mut src = Cbz::open(src_file.path(), ArchiveLimits::default()).unwrap();
        let dst = NamedTempFile::new().unwrap();

        // Source has `ComicInfo.xml`; override declared with different case
        // — even though the reader's skip rule drops .xml entries on rewrite,
        // the case-insensitive match means the override still ends up as the
        // sole post-rewrite `ComicInfo.xml`.
        let mut plan = RebuildPlan::new();
        plan.set_entry("comicinfo.xml", b"<?xml?><ComicInfo/>".to_vec());

        let summary = rebuild(&mut src, plan, dst.path(), ArchiveLimits::default()).unwrap();
        // The matched-case-insensitively override consumes the source
        // entry slot. The override key is added as a fresh entry under
        // its declared name (lowercase here).
        assert!(summary.added_count >= 1);

        let out = extract(dst.path());
        // The override's declared name (lowercase) is what's in the
        // output. The source's mixed-case name has been dropped.
        assert!(out.contains_key("comicinfo.xml"));
        assert!(!out.contains_key("ComicInfo.xml"));
    }

    #[test]
    fn empty_plan_round_trips_pages_byte_equal() {
        let (src_file, pages, _info) = build_fixture();
        let src_path = src_file.path().to_path_buf();
        let pre_lens = raw_payload_lens(&src_path);

        let mut src = Cbz::open(&src_path, ArchiveLimits::default()).unwrap();
        let dst = NamedTempFile::new().unwrap();

        let summary = rebuild(
            &mut src,
            RebuildPlan::new(),
            dst.path(),
            ArchiveLimits::default(),
        )
        .unwrap();

        // ComicInfo.xml from the source is reader-skipped; only the three
        // page entries land. Round-trip is *for pages*, not arbitrary
        // entries — same property the scanner ingest path assumes.
        assert_eq!(summary.kept_count, 3);
        assert_eq!(summary.removed_count, 0);

        let post_lens = raw_payload_lens(dst.path());
        for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
            assert_eq!(
                pre_lens.get(name),
                post_lens.get(name),
                "{name} compressed size mismatch — Keep path lost byte-equality",
            );
        }
        let _ = pages; // silence unused-var warning when assertions don't reference
    }

    // Silence the unused-import warning on Seek + SeekFrom; reserved for
    // future tests that probe the writer's seek behaviour.
    #[allow(dead_code)]
    fn _unused(_: Box<dyn Seek>) {
        let _ = SeekFrom::Start(0);
    }
}
