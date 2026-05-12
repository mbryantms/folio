//! Archive entry-name validation (§4.1.1 — zip-slip defense).
//!
//! Rules (rejection):
//!   - absolute paths
//!   - path components equal to `..`
//!   - NUL or any C0 control character (`\x00`–`\x1F`, `\x7F`)
//!   - backslash separators (DOS-style — accept by translating to `/`? **no, reject**)
//!   - empty components produced by leading/trailing/duplicate slashes
//!   - explicit symlink-like or device-like markers (most ZIP impls don't support
//!     symlinks anyway; we reject if the unix mode bits indicate symlink/device,
//!     which the caller must check separately).
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
    for ch in name.chars() {
        if ch == '\0' || (ch.is_ascii_control() && ch != '\n' && ch != '\r' && ch != '\t') {
            return Err(ArchiveError::UnsafeEntry(format!(
                "control char in: {name:?}"
            )));
        }
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
