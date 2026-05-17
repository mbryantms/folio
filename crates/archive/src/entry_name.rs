//! Archive entry-name validation (§4.1.1 — zip-slip defense).
//!
//! Rules (rejection):
//!   - absolute paths
//!   - path components equal to `..`
//!   - NUL bytes (C-string truncation footgun on every downstream filesystem
//!     API; legitimate filenames never contain `\0`)
//!   - backslash separators (DOS-style — accept by translating to `/`? **no, reject**)
//!   - empty components produced by leading/trailing/duplicate slashes
//!   - explicit symlink-like or device-like markers (most ZIP impls don't support
//!     symlinks anyway; we reject if the unix mode bits indicate symlink/device,
//!     which the caller must check separately).
//!
//! Other C0/DEL control characters (`\x01`–`\x1F` except `\0`, plus `\x7F`)
//! are **allowed**: they don't enable path traversal — at worst they produce
//! a visually-weird filename — and rejecting them broke real publisher
//! archives that contain valid JPGs with stray control bytes in the
//! basename (observed: `CaptainAtom_7_TheGroup_001\x7f2.jpg` in a
//! Marvel-published CBZ).
//!
//! Valid input → returns the canonical lowercased path with forward slashes,
//! suitable for path-match and ordering. The original case is *also* returned
//! so the original spelling can be displayed.

use crate::ArchiveError;

#[derive(Debug, Clone)]
pub struct SafeEntryName {
    pub display: String,
    pub canonical: String,
}

pub fn validate(name: &str) -> Result<SafeEntryName, ArchiveError> {
    if name.is_empty() {
        return Err(ArchiveError::UnsafeEntry("empty entry name".into()));
    }
    if name.contains('\\') {
        return Err(ArchiveError::UnsafeEntry(format!("backslash in: {name}")));
    }
    if name.starts_with('/') {
        return Err(ArchiveError::UnsafeEntry(format!("absolute path: {name}")));
    }
    // Only NUL is a genuine traversal/security issue (C-string truncation
    // on every fs API downstream). DEL and the rest of the C0 range are
    // weird-looking but harmless — see the module doc-comment.
    if name.contains('\0') {
        return Err(ArchiveError::UnsafeEntry(format!("NUL in: {name:?}")));
    }
    let mut depth: i32 = 0;
    for component in name.split('/') {
        if component.is_empty() {
            return Err(ArchiveError::UnsafeEntry(format!(
                "empty component in: {name}"
            )));
        }
        match component {
            "." => {
                return Err(ArchiveError::UnsafeEntry(format!(
                    "`.` component in: {name}"
                )));
            }
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(ArchiveError::UnsafeEntry(format!(
                        "escapes archive root: {name}"
                    )));
                }
            }
            _ => depth += 1,
        }
    }
    let display = name.to_string();
    let canonical = name.to_ascii_lowercase();
    Ok(SafeEntryName { display, canonical })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_accepted() {
        for n in [
            "page1.jpg",
            "pages/001.jpg",
            "deep/nested/chapter/01.png",
            "ComicInfo.xml",
            "Pages/001 - Cover.jpg",
        ] {
            validate(n).expect(n);
        }
    }

    #[test]
    fn zip_slip_rejected() {
        for bad in [
            "../etc/passwd",
            "../../etc/passwd",
            "foo/../../etc/passwd",
            "/etc/passwd",
            "..\\winnt\\system32\\config",
            "evil\\file.png",
        ] {
            assert!(validate(bad).is_err(), "must reject {bad}");
        }
    }

    #[test]
    fn nul_rejected() {
        assert!(validate("page\0.jpg").is_err());
    }

    /// Real-world regression: a Captain Atom CBZ shipped from the
    /// publisher contained `CaptainAtom_7_TheGroup_001\x7f2.jpg` — a
    /// legitimate page with a stray DEL byte in the basename. The
    /// historical "any control char rejects the whole archive" rule
    /// blocked the file from scanning at all. DEL doesn't enable
    /// path traversal (it's not a separator, doesn't truncate, doesn't
    /// escape), so it must be allowed through.
    #[test]
    fn del_and_other_c0_controls_allowed() {
        // DEL in the middle of a basename (the wild case).
        let safe = validate("CaptainAtom_7_TheGroup_001\u{7f}2.jpg").expect("DEL ok");
        assert!(safe.display.contains('\u{7f}'));
        // Other C0 bytes: tab/newline/cr were already allowed; the
        // others (0x01-0x1F minus \0) are now permitted too.
        for ch in ['\u{01}', '\u{0b}', '\u{1f}'] {
            let s = format!("pages/file{ch}.jpg");
            validate(&s).unwrap_or_else(|e| panic!("control U+{:04X}: {e}", ch as u32));
        }
    }

    #[test]
    fn empty_components_rejected() {
        assert!(validate("//double-slash.png").is_err());
        assert!(validate("trail/").is_err());
    }

    #[test]
    fn dot_component_rejected() {
        assert!(validate("./file.png").is_err());
    }
}
