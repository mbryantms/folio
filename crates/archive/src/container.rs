//! Container detection by magic bytes.
//!
//! A comic archive's extension is a hint, not a fact: a large fraction of
//! files in the wild are RAR archives named `.cbz`, ZIPs named `.cbr`, and
//! so on (packers and renamers don't check). Every mainstream reader
//! (YACreader, ComicRack, Komga, ComicTagger) therefore sniffs the leading
//! bytes and picks the decoder from what the file *is*. Folio does the
//! same: [`crate::open`] dispatches on [`detect_container`] first and only
//! falls back to the extension when the bytes carry no recognizable magic.
//!
//! The scanner's CBR/CB7 → CBZ converter
//! (`server::library::scanner::cbr_convert`) and the page-server reader
//! cache (`server::library::zip_lru`) share this module so the three sites
//! can never disagree about what a file is.

use std::io::Read;
use std::path::Path;

/// What an archive file actually is, independent of its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// `PK\x03\x04` (first local header), `PK\x05\x06` (empty archive) or
    /// `PK\x07\x08` (spanned). The `.cbz` format.
    Zip,
    /// `Rar!\x1a\x07` — covers RAR4 (`…\x00`) and RAR5 (`…\x01\x00`). The
    /// `.cbr` format.
    Rar,
    /// `7z\xBC\xAF\x27\x1C`. The `.cb7` format.
    SevenZ,
    /// POSIX ustar / GNU tar: `ustar` at byte offset 257. The `.cbt`
    /// format. A pre-POSIX (v7) tar has no magic and sniffs as
    /// [`Container::Unknown`]; the extension fallback still opens it.
    Tar,
    /// No recognizable magic. Callers fall back to the extension.
    Unknown,
}

impl Container {
    /// The canonical comic extension for this container (`"cbz"`, …);
    /// `None` for [`Container::Unknown`].
    pub fn comic_ext(self) -> Option<&'static str> {
        match self {
            Self::Zip => Some("cbz"),
            Self::Rar => Some("cbr"),
            Self::SevenZ => Some("cb7"),
            Self::Tar => Some("cbt"),
            Self::Unknown => None,
        }
    }
}

/// Bytes to read for a sniff: the tar magic sits at offset 257, so one
/// 512-byte header block covers every format we recognize.
pub const SNIFF_LEN: usize = 512;

const TAR_MAGIC_OFFSET: usize = 257;

/// Classify a file head (up to [`SNIFF_LEN`] bytes) by magic. Pure; the
/// file-reading wrapper is [`detect_container`].
pub fn sniff_container(head: &[u8]) -> Container {
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"PK\x05\x06")
        || head.starts_with(b"PK\x07\x08")
    {
        Container::Zip
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Container::Rar
    } else if head.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Container::SevenZ
    } else if head
        .get(TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + 5)
        .is_some_and(|m| m == b"ustar")
    {
        Container::Tar
    } else {
        Container::Unknown
    }
}

/// Read the leading bytes of `path` and classify them. Only I/O failures
/// are errors; an unrecognized file is `Ok(Container::Unknown)`.
pub fn detect_container(path: &Path) -> std::io::Result<Container> {
    let mut head = [0u8; SNIFF_LEN];
    let mut f = std::fs::File::open(path)?;
    let mut filled = 0;
    // `read` may return short; loop until EOF or the buffer is full so a
    // slow pipe/network mount can't truncate the tar-magic window.
    while filled < SNIFF_LEN {
        let n = f.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(sniff_container(&head[..filled]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_each_magic() {
        assert_eq!(sniff_container(b"PK\x03\x04rest"), Container::Zip);
        assert_eq!(sniff_container(b"PK\x05\x06"), Container::Zip);
        assert_eq!(sniff_container(b"Rar!\x1a\x07\x00\xcf"), Container::Rar);
        assert_eq!(sniff_container(b"Rar!\x1a\x07\x01\x00"), Container::Rar);
        assert_eq!(
            sniff_container(b"7z\xBC\xAF\x27\x1C\x00\x04"),
            Container::SevenZ
        );
        let mut tar = vec![0u8; 512];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(sniff_container(&tar), Container::Tar);
        assert_eq!(sniff_container(b"\x00\x01\x02junk"), Container::Unknown);
        assert_eq!(sniff_container(b""), Container::Unknown);
        // Short heads never panic on the tar window.
        assert_eq!(sniff_container(&[0u8; 300]), Container::Unknown);
    }

    #[test]
    fn detect_reads_file_head() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.cbz");
        std::fs::write(&p, b"Rar!\x1a\x07\x00").unwrap();
        assert_eq!(detect_container(&p).unwrap(), Container::Rar);
        let empty = tmp.path().join("e.cbz");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(detect_container(&empty).unwrap(), Container::Unknown);
        assert!(detect_container(&tmp.path().join("missing")).is_err());
    }

    #[test]
    fn comic_ext_round_trips() {
        assert_eq!(Container::Zip.comic_ext(), Some("cbz"));
        assert_eq!(Container::Rar.comic_ext(), Some("cbr"));
        assert_eq!(Container::SevenZ.comic_ext(), Some("cb7"));
        assert_eq!(Container::Tar.comic_ext(), Some("cbt"));
        assert_eq!(Container::Unknown.comic_ext(), None);
    }
}
