//! Shared input normalization for the XML sidecar parsers.

use std::borrow::Cow;

/// Coerce sidecar bytes to UTF-8 before handing them to `quick-xml`.
///
/// quick-xml 0.42 validates UTF-8 while constructing events, so a single
/// stray byte anywhere in the file aborts the whole parse with
/// `Error::Encoding`. Comic sidecars in the wild are frequently
/// ComicRack-era files written as windows-1252/latin-1, and rejecting an
/// entire `ComicInfo.xml` because one credit name has an accented
/// character would silently drop metadata the scanner used to ingest.
///
/// Lossy conversion keeps the tolerant posture: well-formed UTF-8 (the
/// overwhelming majority) borrows without copying, and an undecodable byte
/// degrades to U+FFFD in that one field instead of failing the document.
/// Pre-0.42 the same input dropped the offending field entirely, so this is
/// no worse and usually better.
///
/// Callers enforce their own size cap before this runs, so the owned branch
/// is bounded by that cap.
///
/// Note: this does not *decode* legacy encodings — a windows-1252 `é`
/// becomes U+FFFD rather than `é`. Honouring the declaration's `encoding`
/// attribute would need quick-xml's `encoding` feature plus a
/// `DecodingReader`; that is an enhancement over the historical behaviour,
/// not a regression this restores.
pub(crate) fn to_utf8(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_borrows_without_copying() {
        let s = to_utf8(b"<ComicInfo><Title>Hi</Title></ComicInfo>");
        assert!(
            matches!(s, Cow::Borrowed(_)),
            "valid UTF-8 must not allocate"
        );
    }

    #[test]
    fn invalid_bytes_degrade_to_replacement_char() {
        let s = to_utf8(&[b'a', 0xE9, b'b']);
        assert_eq!(s, "a\u{FFFD}b");
    }
}
