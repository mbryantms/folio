//! Image detection for archive page entries — by extension **and** by
//! content.
//!
//! The page index every reader exposes via `ComicArchive::pages()` used
//! to be purely extension-driven: anything named `*.jpg` / `*.png` / …
//! was a page. Real-world archives break that assumption — the
//! motivating case was a batch of publisher CBZs whose first entry was a
//! `ComicInfo.xml` document saved under a `-0001.jpg` name. Every
//! downstream consumer (scanner page count, cover thumbnail, reader
//! page 0, OCR) then treated the XML as the cover and failed with
//! "the image format could not be determined".
//!
//! Each reader now runs [`sniff`] over the leading bytes of every
//! extension-matched entry at open time and drops the ones whose bytes
//! don't carry a known image signature. Dropped entries are reported via
//! `ComicArchive::entries_skipped` with [`SKIP_REASON_NOT_AN_IMAGE`] so
//! the scanner surfaces them as a `SkippedArchiveEntries` health issue
//! instead of the failure showing up one consumer at a time.
//!
//! The signature table matches the §17.5 allowlist the page-bytes
//! handler enforces per request (`api::page_bytes::sniff`); that
//! handler keeps its own copy as an independently-auditable last line
//! of defense.

/// Bytes a reader needs from the head of an entry to run [`sniff`].
/// The longest signature here is 12 bytes (WebP / AVIF / JXL container);
/// 16 leaves headroom and matches the per-request sniff window.
pub const SNIFF_LEN: usize = 16;

/// `SkippedEntry::reason` for an entry whose name says image but whose
/// bytes don't. Kept short and paren-free because the admin Health tab
/// renders it inside "N of M entries dropped (…)".
pub const SKIP_REASON_NOT_AN_IMAGE: &str = "image extension but non-image content";

/// Image container recognised by [`sniff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
    Gif,
    Webp,
    Avif,
    Jxl,
}

impl ImageKind {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Jxl => "image/jxl",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jxl => "jxl",
        }
    }
}

/// Extension check — the *candidate* filter. An entry has to pass this
/// AND [`sniff`] to count as a page.
pub fn has_image_extension(name: &str) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "avif" | "gif" | "jxl")
    )
}

/// Identify an image container from its leading bytes (at most
/// [`SNIFF_LEN`] are consulted). Returns `None` for anything that isn't
/// on the allowlist — text, XML, SVG, truncated headers, empty input.
pub fn sniff(head: &[u8]) -> Option<ImageKind> {
    // JPEG: FF D8 FF
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageKind::Jpeg);
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if head.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageKind::Png);
    }
    // GIF: "GIF87a" or "GIF89a"
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(ImageKind::Gif);
    }
    // WebP: "RIFF....WEBP"
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some(ImageKind::Webp);
    }
    // AVIF: "....ftypavif" or "....ftypavis"
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some(ImageKind::Avif);
        }
    }
    // JXL: bare codestream "FF 0A" or the ISO-BMFF container signature.
    if head.starts_with(&[0xFF, 0x0A]) {
        return Some(ImageKind::Jxl);
    }
    if head.starts_with(&[
        0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
    ]) {
        return Some(ImageKind::Jxl);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_every_allowlisted_container() {
        assert_eq!(
            sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(
            sniff(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some(ImageKind::Png)
        );
        assert_eq!(sniff(b"GIF89a\x01\x00"), Some(ImageKind::Gif));
        assert_eq!(sniff(b"GIF87a\x01\x00"), Some(ImageKind::Gif));
        assert_eq!(
            sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some(ImageKind::Webp)
        );
        assert_eq!(sniff(b"\x00\x00\x00\x1cftypavif"), Some(ImageKind::Avif));
        assert_eq!(sniff(b"\x00\x00\x00\x1cftypavis"), Some(ImageKind::Avif));
        assert_eq!(sniff(&[0xFF, 0x0A, 0x00]), Some(ImageKind::Jxl));
        assert_eq!(
            sniff(&[
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A, 0x00,
            ]),
            Some(ImageKind::Jxl)
        );
    }

    /// The production shape: a ComicInfo document under a `.jpg` name.
    #[test]
    fn rejects_xml_text_and_svg() {
        assert_eq!(
            sniff(b"<?xml version='1.0' encoding='utf-8'?>\n<ComicInfo"),
            None
        );
        assert_eq!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"), None);
        assert_eq!(sniff(b"not an image, just plain text bytes"), None);
    }

    #[test]
    fn rejects_empty_and_truncated_signatures() {
        assert_eq!(sniff(&[]), None);
        // Four bytes of the eight-byte PNG signature is not a PNG.
        assert_eq!(sniff(&[0x89, 0x50, 0x4E, 0x47]), None);
        // RIFF without the WEBP form tag (a WAV file, say).
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WAVE"), None);
        // ftyp with a non-AVIF brand (HEIC / MP4).
        assert_eq!(sniff(b"\x00\x00\x00\x1cftypheic"), None);
    }

    #[test]
    fn extension_filter_is_case_insensitive_and_path_aware() {
        assert!(has_image_extension("page.jpg"));
        assert!(has_image_extension("Pages/001 - Cover.JPEG"));
        assert!(has_image_extension("a.b/c.png"));
        assert!(has_image_extension("x.webp"));
        assert!(has_image_extension("x.avif"));
        assert!(has_image_extension("x.gif"));
        assert!(has_image_extension("x.jxl"));
        assert!(!has_image_extension("ComicInfo.xml"));
        assert!(!has_image_extension("noext"));
        assert!(!has_image_extension("dir.jpg/inner"));
        assert!(!has_image_extension("page.jpg.txt"));
    }

    #[test]
    fn kinds_carry_mime_and_extension() {
        assert_eq!(ImageKind::Jpeg.mime(), "image/jpeg");
        assert_eq!(ImageKind::Jpeg.ext(), "jpg");
        assert_eq!(ImageKind::Webp.mime(), "image/webp");
        assert_eq!(ImageKind::Jxl.ext(), "jxl");
    }
}
