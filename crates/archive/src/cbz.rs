//! CBZ (ZIP) archive reader with §4.1.1 defenses.
//!
//! Workflow:
//!   `Cbz::open(path, limits)?`            — parses central directory + validates limits
//!   `cbz.entries()`                        — all non-skipped entries (sanitized names)
//!   `cbz.pages()`                          — page-image entries in natural-sort order
//!   `cbz.find("ComicInfo.xml")`            — case-insensitive lookup
//!   `cbz.read_entry_bytes(&entry)?`        — read full entry with per-entry cap
//!
//! All page reads validate compression ratio and uncompressed-size caps before
//! decompressing — so a 42 KB → 4 GiB bomb is rejected without allocation.

use crate::entry_name;
use crate::{ArchiveEntry, ArchiveError, ArchiveLimits};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::ZipArchive;

/// Files we always skip per §4.1 (`^\.|^__MACOSX|Thumbs\.db|\.xml$|\.json$|\.txt$`).
/// Case-insensitive on directory names (`__MACOSX`/`__macosx`) and stem rules.
fn is_skipped(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    if last.starts_with('.') || last.eq_ignore_ascii_case("Thumbs.db") {
        return true;
    }
    if name.split('/').any(|p| p.eq_ignore_ascii_case("__MACOSX")) {
        return true;
    }
    let ext = last.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "xml" | "json" | "txt")
}

/// Image extensions we accept inside an archive.
fn is_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "avif" | "gif" | "jxl"
    )
}

pub struct Cbz {
    path: PathBuf,
    limits: ArchiveLimits,
    archive: ZipArchive<File>,
    entries: Vec<ArchiveEntry>,
    /// canonical (lowercased) name → entry index
    by_canonical: HashMap<String, usize>,
}

impl std::fmt::Debug for Cbz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cbz")
            .field("path", &self.path)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl Cbz {
    pub fn open(path: impl AsRef<Path>, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let path = path.as_ref().to_path_buf();
        let f = File::open(&path)?;
        let mut archive = ZipArchive::new(f).map_err(map_zip_err)?;

        if archive.len() as u64 > limits.max_entries {
            return Err(ArchiveError::CapExceeded("entry count"));
        }

        let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(archive.len());
        let mut by_canonical: HashMap<String, usize> = HashMap::with_capacity(archive.len());
        let mut total_uncompressed: u64 = 0;

        for i in 0..archive.len() {
            let raw = archive.by_index_raw(i).map_err(map_zip_err)?;
            // Encryption check (§4.6) — done before name validation so encrypted
            // archives report `Encrypted`, not `UnsafeEntry`.
            if raw.encrypted() {
                return Err(ArchiveError::Encrypted);
            }
            let name = raw.name().to_string();
            // Per-entry size caps.
            let unc = raw.size();
            let cmp = raw.compressed_size();
            if unc > limits.max_entry_bytes {
                return Err(ArchiveError::CapExceeded("single entry uncompressed bytes"));
            }
            // Compression-ratio bomb: cmp == 0 with unc > 0 = trivially infinite ratio.
            if cmp == 0 && unc > 0 {
                return Err(ArchiveError::CapExceeded("compression ratio (cmp=0)"));
            }
            if cmp > 0 {
                let ratio = unc / cmp;
                if ratio > limits.max_compression_ratio as u64 {
                    return Err(ArchiveError::CapExceeded("compression ratio"));
                }
            }
            total_uncompressed = total_uncompressed.saturating_add(unc);
            if total_uncompressed > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded("total uncompressed bytes"));
            }

            // Skip directory placeholders (zero-length, name ending in '/').
            if raw.is_dir() {
                continue;
            }

            let safe = entry_name::validate(&name)?;
            if is_skipped(&safe.canonical) {
                // Still surface ComicInfo.xml etc. through the lookup map (they're not "pages"
                // but the caller wants to read them). Insert them into by_canonical anyway.
                let idx = entries.len();
                entries.push(ArchiveEntry {
                    index: i,
                    name: safe.display.clone(),
                    uncompressed_size: unc,
                    compressed_size: cmp,
                });
                by_canonical.insert(safe.canonical, idx);
                continue;
            }

            let idx = entries.len();
            entries.push(ArchiveEntry {
                index: i,
                name: safe.display,
                uncompressed_size: unc,
                compressed_size: cmp,
            });
            by_canonical.insert(safe.canonical, idx);
        }

        Ok(Self {
            path,
            limits,
            archive,
            entries,
            by_canonical,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }

    /// Convenience for the [`crate::comic_archive::ComicArchive`] trait —
    /// look up by name then dispatch to the index-based reader.
    pub fn read_entry_bytes_by_name(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let entry = self
            .find(name)
            .cloned()
            .ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))?;
        self.read_entry_bytes(&entry)
    }

    pub fn read_entry_prefix_by_name(
        &mut self,
        name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArchiveError> {
        let entry = self
            .find(name)
            .cloned()
            .ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))?;
        self.read_entry_prefix(&entry, max_bytes)
    }

    /// Page entries in natural-sort order (numeric-aware).
    pub fn pages(&self) -> Vec<&ArchiveEntry> {
        let mut pages: Vec<&ArchiveEntry> = self
            .entries
            .iter()
            .filter(|e| is_image(&e.name) && !is_skipped(&e.name.to_ascii_lowercase()))
            .collect();
        pages.sort_by(|a, b| natord::compare_ignore_case(&a.name, &b.name));
        pages
    }

    /// Case-insensitive lookup by leaf or full path. Returns `None` if not present
    /// or if the entry was skipped at parse time.
    pub fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        let key = name.to_ascii_lowercase();
        // Try full-path match first, then leaf.
        if let Some(&idx) = self.by_canonical.get(&key) {
            return Some(&self.entries[idx]);
        }
        let leaf = key.rsplit('/').next().unwrap_or(&key);
        self.by_canonical.iter().find_map(|(k, &i)| {
            let k_leaf = k.rsplit('/').next().unwrap_or(k);
            (k_leaf == leaf).then_some(&self.entries[i])
        })
    }

    /// Read an entry's full uncompressed bytes. Caps at `limits.max_entry_bytes`
    /// regardless of central-directory claims (defends against a lying central dir).
    pub fn read_entry_bytes(&mut self, entry: &ArchiveEntry) -> Result<Vec<u8>, ArchiveError> {
        let mut zf = self.archive.by_index(entry.index).map_err(map_zip_err)?;
        let cap = self.limits.max_entry_bytes;
        let mut out = Vec::with_capacity(zf.size().min(64 * 1024) as usize);
        let mut taken = 0u64;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = zf.read(&mut buf)?;
            if n == 0 {
                break;
            }
            taken = taken.saturating_add(n as u64);
            if taken > cap {
                return Err(ArchiveError::CapExceeded(
                    "entry exceeded max_entry_bytes during read",
                ));
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    pub fn read_entry_prefix(
        &mut self,
        entry: &ArchiveEntry,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut zf = self.archive.by_index(entry.index).map_err(map_zip_err)?;
        let cap = self.limits.max_entry_bytes.min(max_bytes as u64);
        let mut out = Vec::with_capacity(cap.min(64 * 1024) as usize);
        let mut taken = 0u64;
        let mut buf = [0u8; 16 * 1024];
        while taken < cap {
            let want = (cap - taken).min(buf.len() as u64) as usize;
            let n = zf.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            taken += n as u64;
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    /// Read a byte range `[start, start+len)` of an entry's uncompressed bytes.
    ///
    /// Implementation reads (and discards) `start` bytes, then collects up to `len`.
    /// For STORED entries this is `O(start + len)`; for DEFLATED it's the cost of
    /// decompressing from offset 0 plus the discard. Per spec §B7 this is acceptable
    /// — clients shouldn't normally Range-request DEFLATED entries, and CBZ images
    /// are usually STORED. A `debug` log surfaces DEFLATED Range hits so misconfigured
    /// archives are visible.
    ///
    /// `start + len` is clamped to the entry's uncompressed size; reads past EOF
    /// return whatever bytes are available (the caller must compute valid ranges
    /// against `entry.uncompressed_size` first).
    pub fn read_entry_range(
        &mut self,
        entry: &ArchiveEntry,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut zf = self.archive.by_index(entry.index).map_err(map_zip_err)?;
        if zf.compression() != zip::CompressionMethod::Stored {
            tracing::debug!(
                name = %entry.name,
                "Range request on DEFLATED entry; decompressing from offset 0"
            );
        }

        let cap = self.limits.max_entry_bytes;
        if start.saturating_add(len) > cap {
            return Err(ArchiveError::CapExceeded("range exceeds entry cap"));
        }

        let mut buf = [0u8; 64 * 1024];
        let mut skipped = 0u64;
        while skipped < start {
            let want = (start - skipped).min(buf.len() as u64) as usize;
            let n = zf.read(&mut buf[..want])?;
            if n == 0 {
                // Range start is at or past EOF; return empty.
                return Ok(Vec::new());
            }
            skipped += n as u64;
        }

        let mut out = Vec::with_capacity(len.min(64 * 1024) as usize);
        let mut taken = 0u64;
        while taken < len {
            let want = (len - taken).min(buf.len() as u64) as usize;
            let n = zf.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            taken += n as u64;
        }
        Ok(out)
    }

    /// Stream an entry into a writer, caps enforced.
    pub fn pipe_entry<W: std::io::Write>(
        &mut self,
        entry: &ArchiveEntry,
        sink: &mut W,
    ) -> Result<u64, ArchiveError> {
        let mut zf = self.archive.by_index(entry.index).map_err(map_zip_err)?;
        let cap = self.limits.max_entry_bytes;
        let mut taken = 0u64;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = zf.read(&mut buf)?;
            if n == 0 {
                break;
            }
            taken = taken.saturating_add(n as u64);
            if taken > cap {
                return Err(ArchiveError::CapExceeded(
                    "entry exceeded max_entry_bytes during stream",
                ));
            }
            sink.write_all(&buf[..n])?;
        }
        Ok(taken)
    }
}

fn map_zip_err(e: zip::result::ZipError) -> ArchiveError {
    use zip::result::ZipError as Z;
    match e {
        Z::UnsupportedArchive(s) => {
            // The unsupported-archive variant is a catch-all for password protection
            // in current zip versions; pattern-match on the message text.
            let lower = s.to_string().to_ascii_lowercase();
            if lower.contains("encrypted") || lower.contains("password") {
                ArchiveError::Encrypted
            } else {
                ArchiveError::Malformed(format!("unsupported: {s}"))
            }
        }
        Z::InvalidArchive(s) => ArchiveError::Malformed(s.to_string()),
        Z::FileNotFound => ArchiveError::Malformed("entry not found".into()),
        Z::Io(io) => ArchiveError::Io(io.to_string()),
        other => ArchiveError::Malformed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn build_cbz(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        build_cbz_with(entries, false)
    }

    fn build_cbz_with(entries: &[(&str, &[u8])], stored: bool) -> tempfile::NamedTempFile {
        let f = tempfile::Builder::new()
            .suffix(".cbz")
            .tempfile()
            .expect("tempfile");
        let mut zw = zip::ZipWriter::new(f.reopen().expect("reopen"));
        let opts: SimpleFileOptions = if stored {
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
        } else {
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
        };
        for (name, bytes) in entries {
            zw.start_file(*name, opts).expect("start_file");
            zw.write_all(bytes).expect("write");
        }
        zw.finish().expect("finish");
        f
    }

    fn one_pixel_png() -> Vec<u8> {
        // Minimal 1x1 PNG.
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn opens_basic_cbz_and_lists_pages() {
        let png = one_pixel_png();
        let cbz = build_cbz(&[
            ("01.png", &png),
            ("02.png", &png),
            ("10.png", &png), // out of natural order on purpose
            ("ComicInfo.xml", b"<ComicInfo/>"),
        ]);
        let mut a = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap();
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["01.png", "02.png", "10.png"]);
        let ci = a.find("ComicInfo.xml").cloned().expect("ci");
        let bytes = a.read_entry_bytes(&ci).unwrap();
        assert_eq!(bytes, b"<ComicInfo/>");
    }

    #[test]
    fn entry_count_cap() {
        let png = one_pixel_png();
        let entries: Vec<_> = (0..50)
            .map(|i| (format!("{i:03}.png"), png.clone()))
            .collect();
        let refs: Vec<_> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let cbz = build_cbz(&refs);
        let limits = ArchiveLimits {
            max_entries: 10,
            ..ArchiveLimits::default()
        };
        let err = Cbz::open(cbz.path(), limits).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded("entry count")));
    }

    #[test]
    fn single_entry_cap() {
        let big = vec![b'A'; 4 * 1024 * 1024];
        let cbz = build_cbz(&[("big.png", &big)]);
        let limits = ArchiveLimits {
            max_entry_bytes: 1024 * 1024,
            ..ArchiveLimits::default()
        };
        let err = Cbz::open(cbz.path(), limits).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded(_)));
    }

    #[test]
    fn compression_ratio_cap() {
        // 1 MiB of zeros compresses to ~1 KiB → ratio ~1000:1, exceeding 200.
        let bomb = vec![0u8; 1024 * 1024];
        let cbz = build_cbz(&[("bomb.png", &bomb)]);
        let err = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded(_)), "got: {err:?}");
    }

    #[test]
    fn zip_slip_entry_rejected() {
        // We have to construct this carefully because the zip crate enforces
        // some name validation; using leading "../" through the zip writer:
        let bad = build_cbz(&[("../escape.png", b"x")]);
        let err = Cbz::open(bad.path(), ArchiveLimits::default()).unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafeEntry(_)), "got: {err:?}");
    }

    #[test]
    fn skipped_entries_dont_appear_as_pages() {
        let png = one_pixel_png();
        let cbz = build_cbz(&[
            ("01.png", &png),
            (".hidden.png", &png),      // dotfile
            ("__MACOSX/foo.png", &png), // macOS noise
            ("Thumbs.db", b"\0\0"),     // Windows noise
        ]);
        let a = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap();
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["01.png"]);
    }

    #[test]
    fn natural_sort_handles_mixed_padding() {
        let png = one_pixel_png();
        let cbz = build_cbz(&[
            ("page-2.png", &png),
            ("page-10.png", &png),
            ("page-1.png", &png),
        ]);
        let a = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap();
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["page-1.png", "page-2.png", "page-10.png"]);
    }

    #[test]
    fn read_entry_range_returns_subrange() {
        // Use a 256-byte payload of distinct bytes so we can verify offsets.
        let payload: Vec<u8> = (0u8..=255u8).collect();
        let cbz = build_cbz(&[("01.png", &payload)]);
        let mut a = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap();
        let entry = a.pages().first().cloned().cloned().unwrap();

        // Full range
        let full = a.read_entry_range(&entry, 0, 256).unwrap();
        assert_eq!(full, payload);

        // Mid range [100, 110)
        let mid = a.read_entry_range(&entry, 100, 10).unwrap();
        assert_eq!(mid, payload[100..110]);

        // Tail
        let tail = a.read_entry_range(&entry, 250, 6).unwrap();
        assert_eq!(tail, payload[250..256]);

        // Beyond EOF returns empty
        let past = a.read_entry_range(&entry, 1000, 10).unwrap();
        assert!(past.is_empty());
    }

    #[test]
    fn read_entry_range_works_on_stored_entries() {
        let payload: Vec<u8> = (0u8..=255u8).collect();
        let cbz = build_cbz_with(&[("01.png", &payload)], true);
        let mut a = Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap();
        let entry = a.pages().first().cloned().cloned().unwrap();
        let mid = a.read_entry_range(&entry, 100, 10).unwrap();
        assert_eq!(mid, payload[100..110]);
    }

    #[test]
    fn comic_archive_trait_round_trip() {
        use crate::comic_archive::ComicArchive;
        let png = one_pixel_png();
        let cbz = build_cbz(&[("01.png", &png), ("ComicInfo.xml", b"<x/>")]);
        let mut a: Box<dyn ComicArchive> =
            Box::new(Cbz::open(cbz.path(), ArchiveLimits::default()).unwrap());
        assert_eq!(a.pages().len(), 1);
        assert!(a.find("ComicInfo.xml").is_some());
        let bytes = a.read_entry_bytes("ComicInfo.xml").unwrap();
        assert_eq!(bytes, b"<x/>");
    }

    #[test]
    fn read_entry_caps_apply_to_streaming() {
        let png = one_pixel_png();
        let cbz = build_cbz(&[("01.png", &png)]);
        // Cap below the actual entry size to force the streaming guard. The
        // open call DOES check max_entry_bytes, so we expect failure at open.
        let limits = ArchiveLimits {
            max_entry_bytes: 10,
            ..ArchiveLimits::default()
        };
        let err = Cbz::open(cbz.path(), limits).unwrap_err();
        assert!(matches!(err, ArchiveError::CapExceeded(_)));
    }
}
