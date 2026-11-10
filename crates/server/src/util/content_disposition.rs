//! The one builder for `Content-Disposition` on every download the server
//! sends (archive downloads, page bytes, exports).
//!
//! A file name can come from the library (an archive's file name), from a
//! user (a list or collection name) or from the server itself. The header
//! carries it twice:
//!
//! - `filename="..."` — an ASCII fallback for old clients: quotes,
//!   backslashes, path separators, `%`, control characters and every
//!   non-ASCII character become `_`;
//! - `filename*=UTF-8''...` — the full name (path separators and control
//!   characters still replaced) percent-encoded per RFC 5987 / RFC 6266,
//!   which current browsers and OPDS clients prefer.
//!
//! Names longer than [`MAX_NAME_BYTES`] are shortened on a character
//! boundary, keeping a short extension.

use axum::http::HeaderValue;

/// Longest name (in UTF-8 bytes, before encoding) the header carries.
/// Common file systems cap a name at 255 bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Name used when nothing usable is left.
const FALLBACK_NAME: &str = "download";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Attachment,
    Inline,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Disposition::Attachment => "attachment",
            Disposition::Inline => "inline",
        }
    }
}

/// Build the header value for `name`. Always valid: every byte of the
/// result is visible ASCII.
pub fn content_disposition(kind: Disposition, name: &str) -> HeaderValue {
    let clean = shorten(&strip_unsafe(name));
    let clean = if clean.trim().is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        clean
    };
    let ascii = ascii_fallback(&clean);
    let value = format!(
        "{}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        kind.as_str(),
        percent_encode(&clean)
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// Replace path separators and control characters (CR, LF, NUL, …) in
/// the full name; they have no place in a file name on any client.
fn strip_unsafe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Cap at [`MAX_NAME_BYTES`], keeping an extension of up to 16 bytes.
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if name.len() - i <= 16 && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let budget = MAX_NAME_BYTES - ext.len();
    let mut end = budget.min(stem.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ext}", &stem[..end])
}

/// ASCII-only, quote-safe form for `filename="..."`.
fn ascii_fallback(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' '..='~' if !matches!(c, '"' | '\\' | '/' | '%') => c,
            _ => '_',
        })
        .collect()
}

/// RFC 5987 `value-chars`: `attr-char` stays, every other byte is `%XX`.
fn percent_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 3);
    for b in name.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str) -> String {
        content_disposition(Disposition::Attachment, name)
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// The `filename="..."` part.
    fn fallback(h: &str) -> &str {
        let start = h.find("filename=\"").unwrap() + "filename=\"".len();
        let end = start + h[start..].find('"').unwrap();
        &h[start..end]
    }

    /// The `filename*=` part, decoded.
    fn extended(h: &str) -> String {
        let enc = &h[h.find("filename*=UTF-8''").unwrap() + "filename*=UTF-8''".len()..];
        assert!(!enc.contains(';') && !enc.contains(' ') && !enc.contains('"'));
        let bytes = enc.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&enc[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn plain_name() {
        let h = header("Batman 001 (1940).cbz");
        assert_eq!(
            h,
            "attachment; filename=\"Batman 001 (1940).cbz\"; filename*=UTF-8''Batman%20001%20%281940%29.cbz"
        );
    }

    #[test]
    fn quotes_and_semicolons_cannot_break_out() {
        let h = header("a\"; filename=\"evil.exe;b.cbz");
        assert_eq!(fallback(&h), "a_; filename=_evil.exe;b.cbz");
        assert_eq!(extended(&h), "a\"; filename=\"evil.exe;b.cbz");
        // Exactly one quoted parameter and one extended parameter.
        assert_eq!(h.matches("filename*=").count(), 1);
        assert_eq!(h.matches('"').count(), 2);
    }

    #[test]
    fn cr_lf_and_controls_are_replaced() {
        let h = header("a\r\nSet-Cookie: x=1\0.cbz");
        assert!(!h.contains('\r') && !h.contains('\n') && !h.contains('\0'));
        assert_eq!(fallback(&h), "a__Set-Cookie: x=1_.cbz");
        assert_eq!(extended(&h), "a__Set-Cookie: x=1_.cbz");
    }

    #[test]
    fn path_separators_and_backslashes_are_replaced() {
        let h = header("../..\\etc/passwd.cbz");
        assert_eq!(fallback(&h), ".._.._etc_passwd.cbz");
        assert_eq!(extended(&h), ".._.._etc_passwd.cbz");
    }

    #[test]
    fn non_ascii_is_kept_in_the_extended_form_only() {
        let h = header("Astérix — 東京 100%.cbz");
        assert!(h.is_ascii());
        assert_eq!(fallback(&h), "Ast_rix _ __ 100_.cbz");
        assert_eq!(extended(&h), "Astérix — 東京 100%.cbz");
    }

    #[test]
    fn very_long_names_are_shortened_keeping_the_extension() {
        let long = format!("{}.cbz", "é".repeat(400));
        let h = header(&long);
        let full = extended(&h);
        assert!(full.len() <= MAX_NAME_BYTES, "{}", full.len());
        assert!(full.ends_with(".cbz"));
        assert!(full.starts_with('é'));
        assert!(fallback(&h).ends_with(".cbz"));
    }

    #[test]
    fn empty_or_blank_names_fall_back() {
        assert_eq!(fallback(&header("")), "download");
        assert_eq!(fallback(&header("   ")), "download");
    }

    #[test]
    fn inline_disposition() {
        let h = content_disposition(Disposition::Inline, "page-3.webp");
        assert_eq!(
            h.to_str().unwrap(),
            "inline; filename=\"page-3.webp\"; filename*=UTF-8''page-3.webp"
        );
    }
}
