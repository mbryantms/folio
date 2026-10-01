//! CB7 (7z-archived) comic reader — **read-only** (WP-6.5).
//!
//! Backed by `sevenz-rust2` (pure Rust, decode-only build — the maintained
//! fork of the abandoned `sevenz-rust`, which carried the
//! RUSTSEC-2026-0245/0246 extraction path-traversal advisories). We never
//! use the crate's extract-to-directory helpers: every byte goes through
//! [`Cb7::visit`], which decodes into memory under the same
//! [`ArchiveLimits`] the other readers apply. There is no 7z writer, so —
//! like CBR — a CB7 is ingested by **converting it to CBZ** at scan time
//! (`server::library::scanner::cbr_convert::convert_cb7_to_cbz`, gated per
//! library by `auto_convert_cb7_on_scan` + `allow_archive_writeback`).
//!
//! 7z specifics the reader has to respect:
//!
//!   - **Blocks / solid archives.** Entries are packed into compression
//!     blocks ("folders"). A non-solid archive has one block per file, so a
//!     single entry decodes on its own; a *solid* archive packs many files
//!     into one block, and reaching file N means decoding files 0..N of that
//!     block first. [`Cb7::visit`] therefore walks each touched block once
//!     front-to-back, draining entries it doesn't want, and stops the block
//!     after its last wanted entry. [`Cb7::preload_all`] uses that to pull a
//!     whole archive in **one** pass (the conversion path), instead of the
//!     O(N²) per-entry decode a naive `read_entry_bytes` loop would cost on
//!     a solid archive.
//!   - **Decompression bombs.** Every decoder is wrapped by the crate in a
//!     reader bounded to the entry's *declared* unpacked size (and CRC-
//!     checked), so output can never exceed what the header claims. The
//!     header's claims are what we cap at open: per-entry
//!     (`max_entry_bytes`), total (`max_total_bytes`), entry count
//!     (`max_entries`), and an **archive-level** compression ratio — total
//!     declared unpacked bytes over total packed bytes, against
//!     `max_compression_ratio`. (CBZ applies the ratio per entry as a soft
//!     skip; 7z has no per-file packed size inside a solid block, so the
//!     ratio is checked for the whole archive and is a hard reject.)
//!   - **Decoder memory.** LZMA/LZMA2 allocate their whole dictionary up
//!     front, and PPMd its model; all three sizes come from attacker-
//!     controlled coder properties and `sevenz-rust2` applies no limit. Open
//!     refuses any block whose decoder would need more than
//!     [`MAX_DECODER_MEMORY_BYTES`] (`CapExceeded("7z decoder memory")`).
//!     Residual risk: an *encoded* (compressed) archive header is decoded
//!     inside `sevenz-rust2`'s own `Archive::read` before we can inspect it;
//!     7-Zip writes headers with LZMA, whose dictionary the decoder clamps
//!     to the header's declared unpack size, but that size is itself
//!     attacker-controlled. Same class as the inert `subprocess_rss_bytes`
//!     limit — there is no RSS cap on in-process archive work.
//!   - **Encryption.** The `aes256` feature is off. An archive with an AES
//!     coder (or encrypted headers) is refused as [`ArchiveError::Encrypted`].
//!   - **Names** go through [`crate::entry_name::validate`] like every other
//!     reader (zip-slip defense); one bad name rejects the archive.
//!   - **Content sniff.** As in every reader, image-named entries whose
//!     leading bytes aren't an image are dropped at open and reported via
//!     [`ComicArchive::entries_skipped`]. In a one-file block only the
//!     prefix is decoded, so every candidate is sniffed; in a multi-file
//!     (solid) block a candidate costs decoding everything before it, so —
//!     mirroring [`crate::cbr::CBR_SNIFF_MAX_ENTRY_BYTES`] — only entries of
//!     at most [`CB7_SNIFF_MAX_ENTRY_BYTES`] are sniffed there.

use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, SkippedEntry, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name, image_sniff,
};
use sevenz_rust2::{Archive, BlockDecoder, EncoderMethod, Password};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

const IGNORED_NAMES: &[&str] = &["Thumbs.db", "desktop.ini"];

/// Largest entry the open-time content sniff decodes inside a multi-file
/// (solid) block. Same rationale and value as
/// [`crate::cbr::CBR_SNIFF_MAX_ENTRY_BYTES`].
pub const CB7_SNIFF_MAX_ENTRY_BYTES: u64 = 256 * 1024;

/// Ceiling on the memory a single block's decoder chain may allocate
/// (LZMA/LZMA2 dictionary, PPMd model). 7-Zip's "Ultra" preset uses a
/// 64 MiB dictionary (and shrinks it to the input size for small inputs),
/// so honest comic archives sit far below this; a header claiming a
/// multi-GiB dictionary is refused before any decode.
pub const MAX_DECODER_MEMORY_BYTES: u64 = 256 * 1024 * 1024;

pub struct Cb7 {
    path: PathBuf,
    archive: Archive,
    entries: Vec<ArchiveEntry>,
    /// `ArchiveEntry::index` → index into `archive.files`.
    file_index: Vec<usize>,
    limits: ArchiveLimits,
    skipped: Vec<SkippedEntry>,
    /// Entry bytes decoded ahead of time by [`Cb7::preload_all`], keyed by
    /// `ArchiveEntry::index`. `read_entry_bytes` moves them out, so the
    /// cache never holds a second copy of what a caller already owns.
    cache: HashMap<usize, Vec<u8>>,
}

impl std::fmt::Debug for Cb7 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cb7")
            .field("path", &self.path)
            .field("entries", &self.entries.len())
            .field("solid", &self.archive.is_solid)
            .finish_non_exhaustive()
    }
}

impl Cb7 {
    pub fn open(path: impl AsRef<Path>, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let path_buf = path.as_ref().to_path_buf();
        let archive = Archive::open(&path_buf).map_err(map_err)?;
        preflight_blocks(&archive, limits)?;

        let mut entries: Vec<ArchiveEntry> = Vec::new();
        let mut file_index: Vec<usize> = Vec::new();
        let mut total_bytes: u64 = 0;
        for (fi, file) in archive.files.iter().enumerate() {
            if file.is_directory || file.is_anti_item {
                continue;
            }
            let safe = sanitize_entry_name(&file.name)?;
            let safe_name = safe.display;
            let leaf = safe_name.rsplit('/').next().unwrap_or(&safe_name);
            if IGNORED_NAMES.contains(&leaf) || leaf.starts_with('.') || leaf == "__MACOSX" {
                continue;
            }
            let size = file.size;
            if size > limits.max_entry_bytes {
                return Err(ArchiveError::CapExceeded("entry size"));
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded("total bytes"));
            }
            entries.push(ArchiveEntry {
                index: entries.len(),
                name: safe_name,
                uncompressed_size: size,
                // 7z keeps no per-file packed size inside a solid block.
                compressed_size: file.compressed_size,
            });
            file_index.push(fi);
            if entries.len() as u64 > limits.max_entries {
                return Err(ArchiveError::CapExceeded("entry count"));
            }
        }

        let mut me = Self {
            path: path_buf,
            archive,
            entries,
            file_index,
            limits,
            skipped: Vec::new(),
            cache: HashMap::new(),
        };
        me.drop_non_image_pages();
        Ok(me)
    }

    /// Whether the archive is solid (several files per compression block).
    pub fn is_solid(&self) -> bool {
        self.archive.is_solid
    }

    /// Decode every indexed entry into memory in a single pass over the
    /// archive's blocks. Subsequent [`ComicArchive::read_entry_bytes`] calls
    /// are served from (and drain) that cache. The conversion path calls
    /// this first so a solid archive is decoded once instead of once per
    /// page. Memory is bounded by `max_total_bytes` (checked at open) — the
    /// same footprint the converter's page buffer would reach anyway.
    pub fn preload_all(&mut self) -> Result<(), ArchiveError> {
        let wanted: Vec<usize> = (0..self.entries.len())
            .filter(|i| !self.cache.contains_key(i))
            .collect();
        let max_entry = self.limits.max_entry_bytes;
        let mut loaded: Vec<(usize, Vec<u8>)> = Vec::with_capacity(wanted.len());
        self.visit(&wanted, |idx, reader| {
            loaded.push((idx, read_bounded(reader, max_entry)?));
            Ok(())
        })?;
        self.cache.extend(loaded);
        Ok(())
    }

    /// Decode the entries named by `wanted` (indices into `self.entries`),
    /// calling `f` once per entry with a reader over its bytes. Each touched
    /// block is decoded once, front to back; entries in it that weren't
    /// asked for are drained (a solid block's stream can't be skipped), and
    /// decoding stops after the block's last wanted entry. `f` may read any
    /// prefix of its reader — the remainder is drained here.
    fn visit(
        &self,
        wanted: &[usize],
        mut f: impl FnMut(usize, &mut dyn Read) -> Result<(), ArchiveError>,
    ) -> Result<(), ArchiveError> {
        // block index → (archive file index → entry index), ordered so we can
        // tell when a block's last wanted file has been handled.
        let mut by_block: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
        for &idx in wanted {
            let fi = self.file_index[idx];
            match self.block_of(fi) {
                Some(block) => {
                    by_block.entry(block).or_default().insert(fi, idx);
                }
                // No stream (zero-length file): nothing to decode.
                None => f(idx, &mut std::io::empty())?,
            }
        }
        if by_block.is_empty() {
            return Ok(());
        }

        // `BlockDecoder::for_each_entries` hands back `&ArchiveEntry`s that
        // live in `self.archive.files`; map their addresses to file indices.
        let addr_to_fi: HashMap<usize, usize> = self
            .archive
            .files
            .iter()
            .enumerate()
            .map(|(i, e)| (std::ptr::from_ref(e) as usize, i))
            .collect();
        let password = Password::empty();
        let mut source = std::fs::File::open(&self.path)?;
        for (block, mut targets) in by_block {
            let mut failure: Option<ArchiveError> = None;
            let decoder = BlockDecoder::new(1, block, &self.archive, &password, &mut source);
            let res = decoder.for_each_entries(&mut |entry, reader| {
                let fi = addr_to_fi
                    .get(&(std::ptr::from_ref(entry) as usize))
                    .copied();
                if let Some(idx) = fi.and_then(|fi| targets.remove(&fi))
                    && let Err(e) = f(idx, reader)
                {
                    failure = Some(e);
                    return Ok(false);
                }
                if targets.is_empty() {
                    return Ok(false);
                }
                // Drain the rest of this entry so the shared block stream
                // lines up with the next one (bounded by the declared size).
                std::io::copy(reader, &mut std::io::sink())?;
                Ok(true)
            });
            if let Some(e) = failure {
                return Err(e);
            }
            res.map_err(map_err)?;
            if let Some((_, idx)) = targets.first_key_value() {
                return Err(ArchiveError::Malformed(format!(
                    "cb7: entry {:?} not reached in block {block}",
                    self.entries[*idx].name
                )));
            }
        }
        Ok(())
    }

    fn block_of(&self, file_index: usize) -> Option<usize> {
        self.archive
            .stream_map
            .file_block_index
            .get(file_index)
            .copied()
            .flatten()
    }

    /// Content-sniff page candidates (see the module docs for which ones)
    /// and drop those whose leading bytes aren't an image signature. A
    /// failing sniff pass keeps every entry — the real read reports the
    /// real error later (same policy as CBR).
    fn drop_non_image_pages(&mut self) {
        let mut files_per_block: HashMap<usize, usize> = HashMap::new();
        for block in self.archive.stream_map.file_block_index.iter().flatten() {
            *files_per_block.entry(*block).or_default() += 1;
        }
        let candidates: Vec<usize> = self
            .entries
            .iter()
            .filter(|e| image_sniff::has_image_extension(&e.name))
            .filter(|e| match self.block_of(self.file_index[e.index]) {
                Some(block) => {
                    e.uncompressed_size <= CB7_SNIFF_MAX_ENTRY_BYTES
                        || files_per_block.get(&block) == Some(&1)
                }
                // Zero-length: no bytes, so certainly not an image.
                None => true,
            })
            .map(|e| e.index)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let mut non_images: BTreeSet<usize> = BTreeSet::new();
        let res = self.visit(&candidates, |idx, reader| {
            let mut head = Vec::with_capacity(image_sniff::SNIFF_LEN);
            reader
                .take(image_sniff::SNIFF_LEN as u64)
                .read_to_end(&mut head)?;
            if image_sniff::sniff(&head).is_none() {
                non_images.insert(idx);
            }
            Ok(())
        });
        if let Err(e) = res {
            tracing::debug!(
                path = %self.path.display(),
                error = %e,
                "cb7: content sniff pass failed; keeping every entry",
            );
            return;
        }
        if non_images.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.entries);
        let file_index = std::mem::take(&mut self.file_index);
        for (entry, fi) in entries.into_iter().zip(file_index) {
            if !non_images.contains(&entry.index) {
                self.file_index.push(fi);
                self.entries.push(ArchiveEntry {
                    index: self.entries.len(),
                    ..entry
                });
                continue;
            }
            tracing::warn!(
                path = %self.path.display(),
                entry = %entry.name,
                size = entry.uncompressed_size,
                "cb7: dropping image-named entry whose bytes aren't an image",
            );
            self.skipped.push(SkippedEntry {
                name: entry.name,
                uncompressed_size: entry.uncompressed_size,
                compressed_size: entry.compressed_size,
                reason: image_sniff::SKIP_REASON_NOT_AN_IMAGE,
            });
        }
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        let want = sanitize_entry_name(name)
            .map(|s| s.canonical)
            .unwrap_or_else(|_| name.to_ascii_lowercase());
        self.entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == want)
            .map(|e| e.index)
    }

    fn read_one(&mut self, name: &str, max_bytes: u64) -> Result<Vec<u8>, ArchiveError> {
        let idx = self
            .lookup(name)
            .ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))?;
        if self.entries[idx].uncompressed_size > self.limits.max_entry_bytes {
            return Err(ArchiveError::CapExceeded("entry size"));
        }
        let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        if let Some(bytes) = self.cache.get(&idx) {
            // A prefix read copies out and leaves the cached entry for the
            // full read that usually follows; a full read moves it out.
            if bytes.len() > cap {
                return Ok(bytes[..cap].to_vec());
            }
            return Ok(self.cache.remove(&idx).unwrap_or_default());
        }
        let mut out = None;
        self.visit(&[idx], |_, reader| {
            out = Some(read_bounded(reader, max_bytes)?);
            Ok(())
        })?;
        out.ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))
    }
}

/// Read at most `max` bytes — the declared-size bound inside the decoder
/// already holds, this is belt-and-braces against a lying stream.
fn read_bounded(reader: &mut dyn Read, max: u64) -> Result<Vec<u8>, ArchiveError> {
    let mut buf = Vec::new();
    reader.take(max).read_to_end(&mut buf)?;
    Ok(buf)
}

fn le_u32(props: &[u8], what: &str) -> Result<u64, ArchiveError> {
    props
        .get(1..5)
        .map(|b| u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
        .ok_or_else(|| ArchiveError::Malformed(format!("cb7: short {what} properties")))
}

/// Memory (bytes) the decoder for one coder would allocate, from its method
/// id + properties. `coder_unpack` is the coder's declared output size
/// (LZMA clamps its dictionary to it). AES → [`ArchiveError::Encrypted`].
fn decoder_memory(id: &[u8], props: &[u8], coder_unpack: u64) -> Result<u64, ArchiveError> {
    if id == EncoderMethod::ID_AES256_SHA256 {
        Err(ArchiveError::Encrypted)
    } else if id == EncoderMethod::ID_LZMA {
        // props[0] = lc/lp/pb, props[1..5] = dictionary size (LE).
        Ok(le_u32(props, "LZMA")?.min(coder_unpack))
    } else if id == EncoderMethod::ID_LZMA2 {
        // props[0] encodes the dictionary as 2^(d/2+12) / 3·2^(d/2+11);
        // the LZMA2 decoder allocates it in full (no clamp).
        match props.first() {
            Some(&d @ 0..=39) => Ok((2 | u64::from(d & 1)) << (d / 2 + 11)),
            Some(40) => Ok(u64::from(u32::MAX)),
            Some(_) => Err(ArchiveError::Malformed(
                "cb7: invalid LZMA2 dictionary".into(),
            )),
            None => Err(ArchiveError::Malformed(
                "cb7: empty LZMA2 properties".into(),
            )),
        }
    } else if id == EncoderMethod::ID_PPMD {
        // props[0] = model order, props[1..5] = model memory (LE).
        le_u32(props, "PPMd")
    } else {
        Ok(0)
    }
}

/// Refuse archives whose blocks are encrypted, would allocate an oversized
/// decoder, or claim an implausible overall compression ratio. Runs on the
/// parsed header only — nothing is decompressed.
fn preflight_blocks(archive: &Archive, limits: ArchiveLimits) -> Result<(), ArchiveError> {
    for block in &archive.blocks {
        for coder in &block.coders {
            let needs = decoder_memory(
                coder.encoder_method_id(),
                coder.properties(),
                block.get_unpack_size_for_coder(coder),
            )?;
            if needs > MAX_DECODER_MEMORY_BYTES {
                return Err(ArchiveError::CapExceeded("7z decoder memory"));
            }
        }
    }

    let unpacked: u64 = archive
        .blocks
        .iter()
        .map(sevenz_rust2::Block::get_unpack_size)
        .fold(0u64, u64::saturating_add);
    let packed: u64 = archive
        .pack_sizes()
        .iter()
        .copied()
        .fold(0u64, u64::saturating_add);
    if unpacked > 0 {
        if unpacked > limits.max_total_bytes {
            return Err(ArchiveError::CapExceeded("total bytes"));
        }
        let ratio_cap = u64::from(limits.max_compression_ratio);
        if packed == 0 || unpacked / packed > ratio_cap {
            return Err(ArchiveError::CapExceeded("compression ratio"));
        }
    }
    Ok(())
}

fn map_err(e: sevenz_rust2::Error) -> ArchiveError {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired | E::MaybeBadPassword(_) => ArchiveError::Encrypted,
        // With the `aes256` feature off, an encrypted *header* (`7z -mhe=on`)
        // fails inside `Archive::read` as an unsupported AES coder — that is
        // still an encrypted archive, not a malformed one.
        E::UnsupportedCompressionMethod(m) if m.contains("AES") => ArchiveError::Encrypted,
        E::FileOpen(io, _) => ArchiveError::Io(io.to_string()),
        other => ArchiveError::Malformed(format!("cb7: {other}")),
    }
}

impl ComicArchive for Cb7 {
    fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        let mut imgs: Vec<&ArchiveEntry> = self
            .entries
            .iter()
            .filter(|e| image_sniff::has_image_extension(&e.name))
            .collect();
        imgs.sort_by(|a, b| natord::compare(&a.name, &b.name));
        imgs
    }
    fn find(&self, name: &str) -> Option<&ArchiveEntry> {
        let lower = name.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == lower)
    }
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let max = self.limits.max_entry_bytes;
        self.read_one(name, max)
    }
    fn read_entry_prefix(&mut self, name: &str, max_bytes: usize) -> Result<Vec<u8>, ArchiveError> {
        let max = (max_bytes as u64).min(self.limits.max_entry_bytes);
        self.read_one(name, max)
    }
    fn entries_skipped(&self) -> &[SkippedEntry] {
        &self.skipped
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_memory_reads_dictionary_sizes() {
        let lzma2 = EncoderMethod::ID_LZMA2;
        // d=24 → 16 MiB, d=25 → 24 MiB (3·2^23), d=40 → 4 GiB − 1.
        assert_eq!(decoder_memory(lzma2, &[24], 0).unwrap(), 16 << 20);
        assert_eq!(decoder_memory(lzma2, &[25], 0).unwrap(), 24 << 20);
        assert!(decoder_memory(lzma2, &[40], 0).unwrap() > MAX_DECODER_MEMORY_BYTES);
        assert!(matches!(
            decoder_memory(lzma2, &[41], 0),
            Err(ArchiveError::Malformed(_))
        ));
        // LZMA clamps the dictionary to the coder's output size.
        let lzma = EncoderMethod::ID_LZMA;
        let huge = [0x5D, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(decoder_memory(lzma, &huge, 4096).unwrap(), 4096);
        assert!(decoder_memory(lzma, &huge, u64::MAX).unwrap() > MAX_DECODER_MEMORY_BYTES);
        assert!(matches!(
            decoder_memory(lzma, &[0x5D], 1),
            Err(ArchiveError::Malformed(_))
        ));
        // PPMd model memory is taken as declared.
        let ppmd = [6, 0x00, 0x00, 0x00, 0x40]; // 1 GiB
        assert!(
            decoder_memory(EncoderMethod::ID_PPMD, &ppmd, 0).unwrap() > MAX_DECODER_MEMORY_BYTES
        );
        // COPY needs nothing; AES is refused as encrypted.
        assert_eq!(decoder_memory(EncoderMethod::ID_COPY, &[], 0).unwrap(), 0);
        assert!(matches!(
            decoder_memory(EncoderMethod::ID_AES256_SHA256, &[], 0),
            Err(ArchiveError::Encrypted)
        ));
    }
}
