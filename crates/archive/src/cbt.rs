//! CBT (tar-archived) comic reader.
//!
//! Library Scanner v1, Milestone 12.
//!
//! Tar is a sequential format with no central directory — to enumerate we
//! walk the file once at `open` time, recording entry metadata and byte
//! offsets. `read_entry_bytes` then opens a fresh file handle and seeks to
//! the recorded offset (tar entries' header is 512 bytes; data follows
//! immediately).
//!
//! Same security limits as `cbz.rs` (entry count, total bytes, per-entry
//! size). Tar has no compression — ratio guard reduces to total-bytes
//! enforcement.
//!
//! Page candidates are content-sniffed at open ([`crate::image_sniff`]):
//! one seek + short read per image-named entry, then the ones whose bytes
//! aren't an image are dropped from the index and reported via
//! [`ComicArchive::entries_skipped`].

use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, SkippedEntry, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name, image_sniff,
};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const IGNORED_NAMES: &[&str] = &["Thumbs.db", "desktop.ini"];

#[derive(Debug)]
pub struct Cbt {
    path: PathBuf,
    entries: Vec<ArchiveEntry>,
    /// `entries[i].name -> data offset in the file`. Built once at open;
    /// used by `read_entry_bytes` to seek without re-walking the tar.
    offsets: HashMap<String, u64>,
    limits: ArchiveLimits,
    /// Image-named entries whose bytes failed the content sniff at open.
    skipped: Vec<SkippedEntry>,
}

impl Cbt {
    pub fn open(path: impl AsRef<Path>, limits: ArchiveLimits) -> Result<Self, ArchiveError> {
        let path_buf = path.as_ref().to_path_buf();
        let f = File::open(&path_buf)?;
        let mut archive = tar::Archive::new(f);

        let mut entries: Vec<ArchiveEntry> = Vec::new();
        let mut offsets: HashMap<String, u64> = HashMap::new();
        let mut total_bytes: u64 = 0;

        for entry_res in archive
            .entries_with_seek()
            .map_err(|e| ArchiveError::Malformed(e.to_string()))?
        {
            let entry = entry_res.map_err(|e| ArchiveError::Malformed(e.to_string()))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let raw_path = entry
                .path()
                .map_err(|e| ArchiveError::Malformed(e.to_string()))?
                .into_owned();
            let raw_name = raw_path.to_string_lossy().into_owned();

            let safe = sanitize_entry_name(&raw_name)?;
            let safe_name = safe.display;

            let leaf = std::path::Path::new(&safe_name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&safe_name);
            if IGNORED_NAMES.contains(&leaf) || leaf.starts_with('.') || leaf == "__MACOSX" {
                continue;
            }

            let size = entry.header().size().unwrap_or(0);
            if size > limits.max_entry_bytes {
                return Err(ArchiveError::CapExceeded("entry size"));
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > limits.max_total_bytes {
                return Err(ArchiveError::CapExceeded("total bytes"));
            }

            let data_offset = entry.raw_file_position();

            let idx = entries.len();
            offsets.insert(safe_name.clone(), data_offset);
            entries.push(ArchiveEntry {
                index: idx,
                name: safe_name,
                uncompressed_size: size,
                compressed_size: size,
            });
            if entries.len() as u64 > limits.max_entries {
                return Err(ArchiveError::CapExceeded("entry count"));
            }
        }

        let mut me = Self {
            path: path_buf,
            entries,
            offsets,
            limits,
            skipped: Vec::new(),
        };
        me.drop_non_image_pages();
        Ok(me)
    }

    /// Content-sniff every page candidate; drop the ones whose leading
    /// bytes aren't an image signature. Tar entries are uncompressed and
    /// we already know every data offset, so this is one `seek` + one
    /// `SNIFF_LEN`-byte read per candidate. A candidate whose prefix can't
    /// be read is kept — that's for the consumer to report.
    fn drop_non_image_pages(&mut self) {
        let candidates: Vec<ArchiveEntry> = self
            .entries
            .iter()
            .filter(|e| image_sniff::has_image_extension(&e.name))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return;
        }
        let Ok(mut f) = File::open(&self.path) else {
            return;
        };
        let mut dropped: HashSet<String> = HashSet::new();
        for entry in candidates {
            let Some(&offset) = self.offsets.get(&entry.name) else {
                continue;
            };
            let len = entry.uncompressed_size.min(image_sniff::SNIFF_LEN as u64) as usize;
            let mut head = vec![0u8; len];
            let read_ok = f
                .seek(SeekFrom::Start(offset))
                .and_then(|_| f.read_exact(&mut head))
                .is_ok();
            if !read_ok {
                tracing::debug!(
                    path = %self.path.display(),
                    entry = %entry.name,
                    "cbt: page prefix unreadable during sniff; keeping entry",
                );
                continue;
            }
            if image_sniff::sniff(&head).is_some() {
                continue;
            }
            tracing::warn!(
                path = %self.path.display(),
                entry = %entry.name,
                size = entry.uncompressed_size,
                "cbt: dropping image-named entry whose bytes aren't an image",
            );
            self.skipped.push(SkippedEntry {
                name: entry.name.clone(),
                uncompressed_size: entry.uncompressed_size,
                compressed_size: entry.compressed_size,
                reason: image_sniff::SKIP_REASON_NOT_AN_IMAGE,
            });
            self.offsets.remove(&entry.name);
            dropped.insert(entry.name);
        }
        if !dropped.is_empty() {
            self.entries.retain(|e| !dropped.contains(&e.name));
        }
    }
}

impl ComicArchive for Cbt {
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
    fn entries_skipped(&self) -> &[SkippedEntry] {
        &self.skipped
    }
    fn read_entry_bytes(&mut self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        let lower = name.to_ascii_lowercase();
        let entry = self
            .entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == lower)
            .ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))?
            .clone();
        let offset = *self
            .offsets
            .get(&entry.name)
            .ok_or_else(|| ArchiveError::Malformed("entry offset missing".into()))?;
        if entry.uncompressed_size > self.limits.max_entry_bytes {
            return Err(ArchiveError::CapExceeded("entry size"));
        }
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; entry.uncompressed_size as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
    fn read_entry_prefix(&mut self, name: &str, max_bytes: usize) -> Result<Vec<u8>, ArchiveError> {
        let lower = name.to_ascii_lowercase();
        let entry = self
            .entries
            .iter()
            .find(|e| e.name.to_ascii_lowercase() == lower)
            .ok_or_else(|| ArchiveError::Malformed(format!("entry not found: {name}")))?
            .clone();
        let offset = *self
            .offsets
            .get(&entry.name)
            .ok_or_else(|| ArchiveError::Malformed("entry offset missing".into()))?;
        let len = entry.uncompressed_size.min(max_bytes as u64);
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_SIG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    fn build_cbt(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let f = tempfile::Builder::new()
            .suffix(".cbt")
            .tempfile()
            .expect("tempfile");
        let mut tw = tar::Builder::new(f.reopen().expect("reopen"));
        for (name, bytes) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tw.append_data(&mut header, name, *bytes).expect("append");
        }
        tw.finish().expect("finish");
        f
    }

    /// Production shape: a ComicInfo document saved under a `.jpg` name
    /// must not count as a page, and must be reported as skipped.
    #[test]
    fn image_named_non_image_entry_is_dropped_and_reported() {
        let xml = b"<?xml version='1.0' encoding='utf-8'?>\n<ComicInfo/>";
        let mut png = PNG_SIG.to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        let tmp = build_cbt(&[
            ("Issue/x-0001.jpg", xml),
            ("Issue/x-0002.jpg", &png),
            ("ComicInfo.xml", b"<ComicInfo/>"),
        ]);
        let a = Cbt::open(tmp.path(), ArchiveLimits::default()).expect("open");
        let pages: Vec<String> = a.pages().iter().map(|e| e.name.clone()).collect();
        assert_eq!(pages, vec!["Issue/x-0002.jpg"]);
        assert!(
            a.find("Issue/x-0001.jpg").is_none(),
            "dropped from the index"
        );
        assert!(a.find("ComicInfo.xml").is_some(), "sidecars untouched");
        let skipped = a.entries_skipped();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "Issue/x-0001.jpg");
        assert_eq!(skipped[0].reason, image_sniff::SKIP_REASON_NOT_AN_IMAGE);
    }

    #[test]
    fn real_image_entries_are_untouched() {
        let mut png = PNG_SIG.to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        let tmp = build_cbt(&[("01.png", &png), ("02.png", &png)]);
        let a = Cbt::open(tmp.path(), ArchiveLimits::default()).expect("open");
        assert_eq!(a.pages().len(), 2);
        assert!(a.entries_skipped().is_empty());
    }
}
