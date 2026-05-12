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

use crate::{
    ArchiveEntry, ArchiveError, ArchiveLimits, comic_archive::ComicArchive,
    entry_name::validate as sanitize_entry_name,
};
use std::collections::HashMap;
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

        Ok(Self {
            path: path_buf,
            entries,
            offsets,
            limits,
        })
    }
}

impl ComicArchive for Cbt {
    fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }
    fn pages(&self) -> Vec<&ArchiveEntry> {
        let mut imgs: Vec<&ArchiveEntry> =
            self.entries.iter().filter(|e| is_image(&e.name)).collect();
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

fn is_image(name: &str) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "avif" | "gif" | "jxl")
    )
}
