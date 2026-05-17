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
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::ZipArchive;
use zip::read::ZipFile;

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
    /// Holds either the file-backed reader (happy path, default) or a
    /// memory-backed reader that owns a rewritten copy of the file with
    /// the malformed Unicode-Path extra fields stripped. See
    /// [`recover_zip_bytes`] for what triggers the rewrite.
    archive: OpenedZip,
    entries: Vec<ArchiveEntry>,
    /// canonical (lowercased) name → entry index
    by_canonical: HashMap<String, usize>,
}

/// Backing reader for the inner `ZipArchive`. Two flavors share Cbz so the
/// rare recovery path (see [`recover_zip_bytes`]) doesn't poison the type
/// of every Cbz handle. Methods that touch the archive dispatch via the
/// [`ZipFileLike`] trait so the duplication stays at the entry-open boundary.
enum OpenedZip {
    File(ZipArchive<File>),
    Mem(ZipArchive<Cursor<Vec<u8>>>),
}

impl OpenedZip {
    fn len(&self) -> usize {
        match self {
            Self::File(a) => a.len(),
            Self::Mem(a) => a.len(),
        }
    }
}

/// Common surface across the two `ZipArchive` reader types so the dispatch
/// helpers below don't need to be written twice. Mirrors the inherent
/// `ZipFile` methods used by `Cbz` (Read + a couple of size/compression
/// getters).
trait ZipFileLike: Read {
    fn entry_size(&self) -> u64;
    fn entry_compression(&self) -> zip::CompressionMethod;
}

impl<R: Read> ZipFileLike for ZipFile<'_, R> {
    fn entry_size(&self) -> u64 {
        self.size()
    }
    fn entry_compression(&self) -> zip::CompressionMethod {
        self.compression()
    }
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
        let archive = open_zip_with_recovery(&path)?;
        Self::finish(path, limits, archive)
    }

    /// Build the index/entry tables. Shared between the happy-path (File)
    /// open and the recovery-path (in-memory cursor over rewritten bytes).
    fn finish(
        path: PathBuf,
        limits: ArchiveLimits,
        mut archive: OpenedZip,
    ) -> Result<Self, ArchiveError> {
        let n = archive.len();
        if n as u64 > limits.max_entries {
            return Err(ArchiveError::CapExceeded("entry count"));
        }

        let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(n);
        let mut by_canonical: HashMap<String, usize> = HashMap::with_capacity(n);
        let mut total_uncompressed: u64 = 0;

        for i in 0..n {
            // Pull just the raw-entry fields we need, dropping the borrow
            // so subsequent loop iterations can re-borrow the archive.
            let (encrypted, name, unc, cmp, is_dir) = match &mut archive {
                OpenedZip::File(a) => {
                    let raw = a.by_index_raw(i).map_err(map_zip_err)?;
                    (
                        raw.encrypted(),
                        raw.name().to_string(),
                        raw.size(),
                        raw.compressed_size(),
                        raw.is_dir(),
                    )
                }
                OpenedZip::Mem(a) => {
                    let raw = a.by_index_raw(i).map_err(map_zip_err)?;
                    (
                        raw.encrypted(),
                        raw.name().to_string(),
                        raw.size(),
                        raw.compressed_size(),
                        raw.is_dir(),
                    )
                }
            };

            // Encryption check (§4.6) — done before name validation so
            // encrypted archives report `Encrypted`, not `UnsafeEntry`.
            if encrypted {
                return Err(ArchiveError::Encrypted);
            }

            // Directory placeholders carry no data — skip them BEFORE the
            // cap checks because some packagers stamp a bogus uncompressed
            // size on directory entries (observed in the wild: a
            // 264KB-claimed `Suiperman 082/` entry whose compressed_size
            // is 0). The compression-ratio check below would otherwise
            // reject the whole archive on the `cmp == 0 && unc > 0`
            // branch.
            if is_dir {
                continue;
            }

            if unc > limits.max_entry_bytes {
                return Err(ArchiveError::CapExceeded("single entry uncompressed bytes"));
            }
            // Compression-ratio defense — soft. If a single entry's
            // claimed uncompressed bytes blow past the ratio cap, we drop
            // it from the index rather than failing the whole archive.
            // The cap exists to keep us from being tricked into
            // decompressing a logic bomb; skipping the entry means no
            // page-byte handler can ever request its bytes, so the bomb
            // stays unread. The hard `max_total_bytes` check still
            // catches a genuine whole-archive expansion attack.
            let ratio_suspect = (cmp == 0 && unc > 0)
                || (cmp > 0 && unc / cmp > limits.max_compression_ratio as u64);
            if ratio_suspect {
                tracing::warn!(
                    path = %path.display(),
                    entry = %name,
                    unc, cmp,
                    "cbz: skipping entry with suspicious compression ratio",
                );
                continue;
            }
            total_uncompressed = total_uncompressed.saturating_add(unc);
            if total_uncompressed > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded("total uncompressed bytes"));
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

    /// Dispatch `f` over an opened entry, hiding the inner reader type.
    /// `f` receives a `&mut dyn ZipFileLike` so call sites stay generic
    /// over both `OpenedZip` variants.
    fn with_entry<F, T>(&mut self, index: usize, f: F) -> Result<T, ArchiveError>
    where
        F: FnOnce(&mut dyn ZipFileLike) -> Result<T, ArchiveError>,
    {
        match &mut self.archive {
            OpenedZip::File(a) => {
                let mut zf = a.by_index(index).map_err(map_zip_err)?;
                f(&mut zf)
            }
            OpenedZip::Mem(a) => {
                let mut zf = a.by_index(index).map_err(map_zip_err)?;
                f(&mut zf)
            }
        }
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
        let cap = self.limits.max_entry_bytes;
        self.with_entry(entry.index, |zf| {
            let mut out = Vec::with_capacity(zf.entry_size().min(64 * 1024) as usize);
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
        })
    }

    pub fn read_entry_prefix(
        &mut self,
        entry: &ArchiveEntry,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArchiveError> {
        let cap = self.limits.max_entry_bytes.min(max_bytes as u64);
        self.with_entry(entry.index, |zf| {
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
        })
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
        let cap = self.limits.max_entry_bytes;
        if start.saturating_add(len) > cap {
            return Err(ArchiveError::CapExceeded("range exceeds entry cap"));
        }
        let entry_name = entry.name.clone();
        self.with_entry(entry.index, |zf| {
            if zf.entry_compression() != zip::CompressionMethod::Stored {
                tracing::debug!(
                    name = %entry_name,
                    "Range request on DEFLATED entry; decompressing from offset 0"
                );
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
        })
    }

    /// Stream an entry into a writer, caps enforced.
    pub fn pipe_entry<W: std::io::Write>(
        &mut self,
        entry: &ArchiveEntry,
        sink: &mut W,
    ) -> Result<u64, ArchiveError> {
        let cap = self.limits.max_entry_bytes;
        self.with_entry(entry.index, |zf| {
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
        })
    }
}

/// Try the happy-path File-backed open first. If it fails with a symptom
/// of a malformed Info-ZIP Unicode Path extra (`0x7075`) whose stored
/// CRC32 doesn't match the CDFH's CP437 filename, fall back to a
/// memory-backed read of the file with every `0x7075` extra stripped
/// from the central directory. Observed on the Araña Heart of the
/// Spider (2005) CBZs in production 2026-05-14: the publisher's tool
/// wrote the UTF-8 path with a CRC of the *UTF-8* bytes, but the zip
/// crate computes the CRC against the raw CP437 file_name field, so
/// they never match. `unzip` ignores the mismatch; we have to rewrite
/// the bytes.
///
/// Three error surfaces trigger recovery:
///   - `"CRC32 checksum failed on Unicode extra field"` — zip 3+'s
///     precise diagnosis; this is the canonical signature.
///   - `"Could not find EOCD"` and `"No CDFH found"` — zip 2.x
///     surfaces when the CRC failure aborted the CD walk mid-way.
///     Retained so a future downgrade or fork still trips recovery.
fn open_zip_with_recovery(path: &Path) -> Result<OpenedZip, ArchiveError> {
    let f = File::open(path)?;
    match ZipArchive::new(f) {
        Ok(a) => Ok(OpenedZip::File(a)),
        Err(zip::result::ZipError::InvalidArchive(msg))
            if msg.contains("CRC32 checksum failed on Unicode extra field")
                || msg.contains("Could not find EOCD")
                || msg.contains("No CDFH found") =>
        {
            let bytes = std::fs::read(path)?;
            let recovered = recover_zip_bytes(&bytes).ok_or_else(|| {
                ArchiveError::Malformed(format!("{msg} (recovery rewrite failed)"))
            })?;
            let archive = ZipArchive::new(Cursor::new(recovered)).map_err(|e| {
                let err = map_zip_err(e);
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "cbz: recovery rewrite produced an archive the zip crate still rejects",
                );
                err
            })?;
            tracing::info!(
                path = %path.display(),
                "cbz: recovered from malformed Unicode-Path CRC by stripping 0x7075 extras",
            );
            Ok(OpenedZip::Mem(archive))
        }
        Err(e) => Err(map_zip_err(e)),
    }
}

/// Rewrite the in-memory byte buffer to strip the Info-ZIP Unicode Path
/// extra field (`0x7075`) from every Central Directory File Header, then
/// patch the EOCD's `central_directory_size`. Returns `None` if the
/// structure doesn't parse — recovery is opportunistic, so a non-match
/// just means the original parse error stands.
///
/// Local file headers aren't touched: the zip crate never validates LFH
/// extras (it only uses the length field to compute data_start), so the
/// "bad CRC" path never fires from the LFH side.
fn recover_zip_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CDFH_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    const SCAN_BYTES: usize = 65_557;

    if bytes.len() < 22 {
        return None;
    }

    // Scan backwards for the EOCD signature. Comments after the EOCD can
    // push it up to 65535 bytes from EOF, so cap the window at 65557
    // (22 + 65535).
    let scan_start = bytes.len().saturating_sub(SCAN_BYTES);
    let mut eocd_off = None;
    for i in (scan_start..bytes.len().saturating_sub(3)).rev() {
        if bytes[i..i + 4] == EOCD_SIG {
            eocd_off = Some(i);
            break;
        }
    }
    let eocd_off = eocd_off?;

    // Parse what we need from the EOCD.
    let body = &bytes[eocd_off + 4..eocd_off + 22];
    let n_files = u16::from_le_bytes(body[6..8].try_into().ok()?) as usize;
    let cd_size = u32::from_le_bytes(body[8..12].try_into().ok()?) as usize;
    let cd_offset = u32::from_le_bytes(body[12..16].try_into().ok()?) as usize;
    let comment_len = u16::from_le_bytes(body[16..18].try_into().ok()?) as usize;

    if cd_offset.checked_add(cd_size)? > bytes.len() {
        return None;
    }
    if eocd_off + 22 + comment_len > bytes.len() {
        return None;
    }

    // Walk the CDFH entries, building a cleaned copy with all `0x7075`
    // extras dropped.
    let mut cleaned_cd = Vec::with_capacity(cd_size);
    let mut p = cd_offset;
    for _ in 0..n_files {
        if p + 46 > bytes.len() || bytes[p..p + 4] != CDFH_SIG {
            return None;
        }
        let fname_len = u16::from_le_bytes(bytes[p + 28..p + 30].try_into().ok()?) as usize;
        let extra_len = u16::from_le_bytes(bytes[p + 30..p + 32].try_into().ok()?) as usize;
        let comment_len_entry = u16::from_le_bytes(bytes[p + 32..p + 34].try_into().ok()?) as usize;
        let total_len = 46 + fname_len + extra_len + comment_len_entry;
        if p + total_len > bytes.len() {
            return None;
        }
        let extra_start = p + 46 + fname_len;
        let extra_end = extra_start + extra_len;
        let cleaned_extra = strip_extras(&bytes[extra_start..extra_end], 0x7075)?;
        let new_extra_len = cleaned_extra.len();

        // Copy the 46-byte header, patch the extra_field_length, then
        // emit filename + cleaned-extra + comment.
        let mut header = bytes[p..p + 46].to_vec();
        let new_extra_len_u16: u16 = new_extra_len.try_into().ok()?;
        header[30..32].copy_from_slice(&new_extra_len_u16.to_le_bytes());
        cleaned_cd.extend_from_slice(&header);
        cleaned_cd.extend_from_slice(&bytes[p + 46..p + 46 + fname_len]);
        cleaned_cd.extend_from_slice(&cleaned_extra);
        cleaned_cd.extend_from_slice(&bytes[extra_end..extra_end + comment_len_entry]);

        p += total_len;
    }

    // Compose the rewritten file: data prefix (verbatim) + cleaned CD +
    // patched EOCD (cd_size updated, cd_offset unchanged) + EOCD comment.
    let new_cd_size: u32 = cleaned_cd.len().try_into().ok()?;
    let mut out = Vec::with_capacity(cd_offset + cleaned_cd.len() + 22 + comment_len);
    out.extend_from_slice(&bytes[..cd_offset]);
    out.extend_from_slice(&cleaned_cd);
    let mut eocd = bytes[eocd_off..eocd_off + 22].to_vec();
    eocd[12..16].copy_from_slice(&new_cd_size.to_le_bytes());
    out.extend_from_slice(&eocd);
    out.extend_from_slice(&bytes[eocd_off + 22..eocd_off + 22 + comment_len]);
    Some(out)
}

/// Walk a CDFH/LFH extra-field block and return a copy with every entry
/// whose tag matches `drop_tag` removed. Returns `None` if the structure
/// is malformed (a length runs past the end of the block).
fn strip_extras(extra: &[u8], drop_tag: u16) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(extra.len());
    let mut p = 0;
    while p + 4 <= extra.len() {
        let tag = u16::from_le_bytes(extra[p..p + 2].try_into().ok()?);
        let len = u16::from_le_bytes(extra[p + 2..p + 4].try_into().ok()?) as usize;
        if p + 4 + len > extra.len() {
            return None;
        }
        if tag != drop_tag {
            out.extend_from_slice(&extra[p..p + 4 + len]);
        }
        p += 4 + len;
    }
    if p != extra.len() {
        // Trailing bytes that don't form a complete TLV — unusual but
        // not necessarily fatal; preserve them so we don't corrupt
        // anything we don't understand.
        out.extend_from_slice(&extra[p..]);
    }
    Some(out)
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
    fn compression_ratio_cap_skips_entry_not_archive() {
        // 1 MiB of zeros compresses to ~1 KiB → ratio ~1000:1, exceeding
        // the default 200x cap. Soft behavior: the bomb entry drops out
        // of `pages()` but the archive otherwise opens cleanly. The
        // hard `max_total_bytes` check still defends against
        // whole-archive expansion attacks.
        let bomb = vec![0u8; 1024 * 1024];
        let cbz = build_cbz(&[("bomb.png", &bomb)]);
        let a = Cbz::open(cbz.path(), ArchiveLimits::default()).expect("open");
        assert!(a.pages().is_empty(), "bomb entry must be dropped");
    }

    #[test]
    fn high_ratio_entry_doesnt_block_other_entries() {
        // Regression for the Swamp Thing v2 trade: 215 entries, one of
        // which is a near-blank credits page that compresses ~276x. The
        // old hard-fail rule rejected the whole archive on that single
        // entry. The new soft-skip rule drops only the offending entry
        // and keeps every other page available.
        let png = one_pixel_png();
        let bomb = vec![0u8; 1024 * 1024]; // ratio ~1000:1
        let cbz = build_cbz(&[
            ("01.png", &png),
            ("bomb.png", &bomb),
            ("02.png", &png),
            ("03.png", &png),
        ]);
        let a = Cbz::open(cbz.path(), ArchiveLimits::default()).expect("open");
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["01.png", "02.png", "03.png"]);
    }

    /// Regression for the Superman V1939 082 case: the publisher's CBZ
    /// contains a directory entry (`Suiperman 082/`) whose central
    /// directory header carries a bogus `uncompressed_size > 0` with
    /// `compressed_size = 0`. The old order — cap checks before the
    /// is_dir skip — fired the "compression ratio (cmp=0)" branch on
    /// that entry and aborted the whole open. The fix moves `is_dir`
    /// above every cap check.
    #[test]
    fn directory_entry_with_bogus_size_doesnt_fail_open() {
        // Build a CBZ with a directory entry sitting alongside a real
        // page, then hand-patch the CDFH for the directory entry to
        // claim a non-zero uncompressed size (and a zero compressed
        // size, which the standard already produces for directories).
        let png = one_pixel_png();
        let scratch = tempfile::Builder::new().suffix(".cbz").tempfile().unwrap();
        {
            let mut zw = zip::ZipWriter::new(scratch.reopen().unwrap());
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.add_directory("subdir/", opts).unwrap();
            zw.start_file("subdir/page.png", opts).unwrap();
            zw.write_all(&png).unwrap();
            zw.finish().unwrap();
        }

        let mut bytes = std::fs::read(scratch.path()).unwrap();
        let eocd_off = bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("EOCD");
        let cd_offset =
            u32::from_le_bytes(bytes[eocd_off + 16..eocd_off + 20].try_into().unwrap()) as usize;
        assert_eq!(&bytes[cd_offset..cd_offset + 4], b"PK\x01\x02");
        // Patch the first CDFH's uncompressed_size (bytes 24..28) to a
        // bogus 264148 — the exact pathology observed in the wild.
        let bogus_unc: u32 = 264_148;
        bytes[cd_offset + 24..cd_offset + 28].copy_from_slice(&bogus_unc.to_le_bytes());
        std::fs::write(scratch.path(), &bytes).unwrap();

        let a = Cbz::open(scratch.path(), ArchiveLimits::default())
            .expect("open should survive bogus dir-entry size");
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["subdir/page.png"]);
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
    fn opens_cbz_with_corrupt_unicode_path_crc() {
        // Regression for the Araña Heart of the Spider (2005) CBZs (dev DB
        // 2026-05-14): the publisher's tool stored an Info-ZIP Unicode
        // Path extra field (`0x7075`) with a CRC32 of the *UTF-8* filename
        // bytes, but the zip crate computes the CRC against the raw CP437
        // `file_name_raw` field. They never match, the zip crate bails
        // mid-CD-read, the find-EOCD loop swallows the real error and
        // surfaces it as `InvalidArchive("Could not find EOCD")`.
        //
        // Build a CBZ that simulates the same failure mode (poisoned
        // `0x7075` extra in the first CDFH), then assert `Cbz::open`
        // recovers it.
        let png = one_pixel_png();
        let cbz = build_cbz(&[("page.png", &png), ("ComicInfo.xml", b"<ComicInfo/>")]);
        let bytes = std::fs::read(cbz.path()).unwrap();

        // Locate EOCD + first CDFH.
        let eocd_off = bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("EOCD");
        let cd_size =
            u32::from_le_bytes(bytes[eocd_off + 12..eocd_off + 16].try_into().unwrap()) as usize;
        let cd_offset =
            u32::from_le_bytes(bytes[eocd_off + 16..eocd_off + 20].try_into().unwrap()) as usize;
        assert_eq!(&bytes[cd_offset..cd_offset + 4], b"PK\x01\x02");
        let fname_len =
            u16::from_le_bytes(bytes[cd_offset + 28..cd_offset + 30].try_into().unwrap()) as usize;
        let extra_len =
            u16::from_le_bytes(bytes[cd_offset + 30..cd_offset + 32].try_into().unwrap()) as usize;

        // Build a poisoned `0x7075` Info-ZIP Unicode Path extra:
        //   tag(2) + len(2) + version(1) + crc32(4, deliberately wrong) + utf8 name
        let poison: Vec<u8> = vec![
            0x75, 0x70, // tag = 0x7075 (LE)
            0x09, 0x00, // payload length = 9
            0x01, // version
            0xDE, 0xAD, 0xBE, 0xEF, // CRC32 that won't match "page.png"
            b'p', b'a', b'g', b'e',
        ];

        // Splice the poisoned extra in after the filename in the first
        // CDFH, growing the CD by `poison.len()` bytes. Patch the CDFH's
        // extra_field_length and the EOCD's central_directory_size to
        // match.
        let inject_at = cd_offset + 46 + fname_len + extra_len;
        let mut poisoned = Vec::with_capacity(bytes.len() + poison.len());
        poisoned.extend_from_slice(&bytes[..inject_at]);
        poisoned.extend_from_slice(&poison);
        poisoned.extend_from_slice(&bytes[inject_at..]);
        let new_extra_len = (extra_len + poison.len()) as u16;
        poisoned[cd_offset + 30..cd_offset + 32].copy_from_slice(&new_extra_len.to_le_bytes());
        let new_eocd_off = eocd_off + poison.len();
        let new_cd_size = (cd_size + poison.len()) as u32;
        poisoned[new_eocd_off + 12..new_eocd_off + 16].copy_from_slice(&new_cd_size.to_le_bytes());

        // Sanity: the standalone zip crate should now reject this file —
        // that's the symptom we're recovering from.
        let bare = zip::ZipArchive::new(Cursor::new(poisoned.clone()));
        assert!(
            bare.is_err(),
            "test setup is broken: poisoned CBZ still opens cleanly via zip crate",
        );

        // Recovery path opens it successfully.
        let tmp = tempfile::Builder::new()
            .suffix(".cbz")
            .tempfile()
            .expect("tempfile");
        std::fs::write(tmp.path(), &poisoned).expect("write poisoned");
        let mut a = Cbz::open(tmp.path(), ArchiveLimits::default())
            .expect("recovery should make the archive openable");
        let pages: Vec<_> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["page.png"]);
        let ci = a
            .read_entry_bytes_by_name("ComicInfo.xml")
            .expect("read ComicInfo");
        assert_eq!(ci, b"<ComicInfo/>");
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
