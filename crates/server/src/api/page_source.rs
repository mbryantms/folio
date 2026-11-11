//! What the routes that read an issue's archive answer when they cannot
//! (audit S-23 / R-06a): page bytes and width variants
//! (`api/page_bytes.rs`), page thumbnails (`api/thumbnails.rs`), the
//! OPDS page stream (`api/opds_pse.rs`), OCR and text regions
//! (`api/issue_ocr.rs`). The OPDS download (`api/opds.rs`) streams the
//! file without opening it as an archive and uses only the missing-file
//! case.
//!
//! The one mapping, decided here and nowhere else:
//!
//! | Case | Status | `error.code` |
//! |---|---|---|
//! | No such issue, or the caller may not see it | 404 | `not_found` |
//! | The stored path is refused (a symbolic link, not a regular file, outside the library root) | 404 | `not_found` |
//! | The issue exists but its file is missing | 410 | `archive_missing` |
//! | The scanner marked the issue malformed or encrypted, or the file no longer opens as an archive | 410 | `archive_unreadable` |
//! | Reading the file failed with an I/O error | 503 + `Retry-After: 5` | `service_unavailable` |
//! | A database or task failure | 500 | `internal` |
//!
//! 410 means "this exists but its pages cannot be served, and asking again
//! will not change that until the file is replaced and the library
//! rescanned"; clients should stop retrying (the web reader shows a final
//! message for it). Every one of these answers carries
//! `Cache-Control: no-store`, so a browser never keeps an error in place of
//! a page once the file is repaired, and none names a server path.
//!
//! Soft-removed issues are not refused here: page bytes, the page stream,
//! thumbnails and OCR go by `issue_visible`, which keeps removed content
//! readable while its file is still there (`docs/dev/library-scanner.md`,
//! "Serving"). Only when that file is gone does the answer become 410. The
//! OPDS download keeps refusing soft-removed issues as a list surface.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use entity::issue::{self, IssueState};

use super::error;
use crate::library::no_follow::Refusal;

/// Why an issue's pages cannot be served. See the module docs for the
/// status each one answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageUnavailable {
    /// Unknown, hidden from this caller, or a refused path.
    NotFound,
    /// The row exists; its file does not.
    Missing,
    /// The file exists but is not a readable archive.
    Unreadable,
    /// An I/O error reading the file; may succeed on retry.
    Unavailable,
    /// A server fault (database, task).
    Internal,
}

/// Seconds a client should wait before retrying an I/O failure.
const RETRY_AFTER_SECS: &str = "5";

impl PageUnavailable {
    /// Refuse a row whose archive the scanner could not open (`malformed`
    /// or `encrypted`): it has no pages to serve.
    pub(crate) fn check_row(row: &issue::Model) -> Result<(), Self> {
        match row.state {
            IssueState::Active => Ok(()),
            IssueState::Malformed | IssueState::Encrypted => Err(Self::Unreadable),
        }
    }

    /// Classify a failed no-follow open of the issue's file.
    pub(crate) fn from_refusal(refusal: &Refusal) -> Self {
        match refusal {
            Refusal::Missing => Self::Missing,
            Refusal::OutsideRoot | Refusal::Symlink | Refusal::NotRegular => Self::NotFound,
            Refusal::Io(e) if e.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Refusal::Io(_) => Self::Unavailable,
            Refusal::Lookup(_) => Self::Internal,
        }
    }

    /// Classify a failed archive open or read.
    pub(crate) fn from_archive_error(e: &archive::ArchiveError) -> Self {
        use archive::ArchiveError as E;
        match e {
            E::Io(_) => Self::Unavailable,
            E::Malformed(_) | E::Encrypted | E::UnsafeEntry(_) | E::CapExceeded(_) => {
                Self::Unreadable
            }
        }
    }

    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Missing | Self::Unreadable => StatusCode::GONE,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Missing => "archive_missing",
            Self::Unreadable => "archive_unreadable",
            Self::Unavailable => "service_unavailable",
            Self::Internal => "internal",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotFound => "issue not found",
            Self::Missing => "the issue's file is missing",
            Self::Unreadable => "the issue's file is not a readable archive",
            Self::Unavailable => "the issue's file could not be read; retry shortly",
            Self::Internal => "internal",
        }
    }

    /// The canonical envelope for this case, never cacheable.
    pub(crate) fn response(self) -> Response {
        no_store(error(self.status(), self.code(), self.message()))
    }
}

/// Mark an error response as not to be stored, and add `Retry-After` to a
/// 503 that has none. Also used for the page-out-of-range 404s next to these answers.
pub(crate) fn no_store(mut resp: Response) -> Response {
    let headers = resp.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if resp.status() == StatusCode::SERVICE_UNAVAILABLE
        && !resp.headers().contains_key(header::RETRY_AFTER)
    {
        resp.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_static(RETRY_AFTER_SECS),
        );
    }
    resp
}

/// Short tag for a [`archive::FileStamp`], folded into page ETags so they
/// change when the file changes, even before a rescan updates
/// `content_hash` (audit R-05). 16 hex chars of a BLAKE3 digest; reveals
/// nothing about the file.
pub(crate) fn version_tag(stamp: &archive::FileStamp) -> String {
    let mut h = blake3::Hasher::new();
    h.update(&stamp.dev.to_le_bytes());
    h.update(&stamp.ino.to_le_bytes());
    h.update(&stamp.size.to_le_bytes());
    h.update(&stamp.mtime_ns.to_le_bytes());
    h.update(&stamp.ctime_ns.to_le_bytes());
    h.finalize().to_hex()[..16].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_stable() {
        use archive::ArchiveError as E;
        assert_eq!(
            PageUnavailable::from_refusal(&Refusal::Missing),
            PageUnavailable::Missing
        );
        assert_eq!(
            PageUnavailable::from_refusal(&Refusal::Symlink),
            PageUnavailable::NotFound
        );
        assert_eq!(
            PageUnavailable::from_refusal(&Refusal::Lookup("db".into())),
            PageUnavailable::Internal
        );
        assert_eq!(
            PageUnavailable::from_archive_error(&E::Malformed("x".into())),
            PageUnavailable::Unreadable
        );
        assert_eq!(
            PageUnavailable::from_archive_error(&E::Io("eio".into())),
            PageUnavailable::Unavailable
        );
        assert_eq!(PageUnavailable::Missing.status(), StatusCode::GONE);
        assert_eq!(PageUnavailable::Unreadable.status(), StatusCode::GONE);
    }

    #[test]
    fn responses_are_never_stored() {
        for case in [
            PageUnavailable::NotFound,
            PageUnavailable::Missing,
            PageUnavailable::Unreadable,
            PageUnavailable::Unavailable,
            PageUnavailable::Internal,
        ] {
            let resp = case.response();
            assert_eq!(resp.status(), case.status());
            assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
        }
        let busy = PageUnavailable::Unavailable.response();
        assert_eq!(busy.headers()[header::RETRY_AFTER], "5");
    }
}
