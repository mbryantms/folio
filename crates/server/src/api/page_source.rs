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
//! | The issue exists but its file is missing (the library root is there) | 410 | `archive_missing` |
//! | The scanner marked the issue encrypted, or the file no longer opens as an archive | 410 | `archive_unreadable` |
//! | The file is a format the page reader does not read (an unconverted CBR / CB7) | 409 | `archive_needs_conversion` |
//! | The library root itself cannot be opened or is empty (an unmounted share) | 503 + `Retry-After: 30` | `library_offline` |
//! | Reading the file failed with an I/O error | 503 + `Retry-After: 5` | `service_unavailable` |
//! | A database or task failure | 500 | `internal` |
//!
//! 410 means "this exists but its pages cannot be served, and asking again
//! will not change that until the file is replaced and the library
//! rescanned"; clients should stop retrying (the web reader shows a final
//! message for it). 409 says the same for a file that is fine but needs
//! converting to CBZ first (the library's convert-on-scan setting, or the
//! page editor); it is a client error, so clients do not retry it, and a
//! different status from 410, so the reader can say what is actually
//! wrong. 503 says "retry later". Every one of these answers, and the
//! not-found / page-out-of-range / range-not-satisfiable answers of the
//! same routes, carries `Cache-Control: no-store`, so a browser never
//! keeps an error in place of a page once the file is repaired, and none
//! names a server path.
//!
//! A row the scanner marked `malformed` is not refused from the row: the
//! scanner uses that state for a bad or oversized ComicInfo.xml too, on
//! archives whose pages open fine. It goes through the real open, and a
//! truly broken archive costs one read per failed-open window
//! (`library::zip_lru`). An `encrypted` row has no readable pages and is
//! refused before any I/O.
//!
//! Soft-removed issues are not refused here: page bytes, the page stream,
//! thumbnails and OCR go by `issue_visible`, which keeps removed content
//! readable while its file is still there (`docs/dev/library-scanner.md`,
//! "Serving"). Only when that file is gone does the answer become 410. The
//! OPDS download keeps refusing soft-removed issues as a list surface.
//! Visibility is always checked first, so a caller who may not see the
//! issue gets the same 404 as for an unknown id, whatever state its file
//! is in.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use entity::issue::{self, IssueState};

use super::error;
use crate::library::no_follow::Refusal;
use crate::state::AppState;

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
    /// The file is a format the page reader cannot read until converted.
    NeedsConversion,
    /// The library root is not there (unmounted); may come back.
    LibraryOffline,
    /// An I/O error reading the file; may succeed on retry.
    Unavailable,
    /// A server fault (database, task).
    Internal,
}

/// Seconds a client should wait before retrying an I/O failure.
const RETRY_AFTER_SECS: &str = "5";
/// Seconds a client should wait before retrying an offline library.
const OFFLINE_RETRY_AFTER_SECS: &str = "30";

impl PageUnavailable {
    /// Refuse a row with no readable pages before any I/O: only an
    /// `encrypted` one. A `malformed` row goes through the real open (see
    /// the module docs).
    pub(crate) fn check_row(row: &issue::Model) -> Result<(), Self> {
        match row.state {
            IssueState::Active | IssueState::Malformed => Ok(()),
            IssueState::Encrypted => Err(Self::Unreadable),
        }
    }

    /// Classify a failed no-follow open of the issue's file, without
    /// looking at the library root. Request paths use
    /// [`PageUnavailable::for_refusal`], which tells a missing file from
    /// an offline library.
    pub(crate) fn from_refusal(refusal: &Refusal) -> Self {
        match refusal {
            Refusal::Missing => Self::Missing,
            Refusal::OutsideRoot | Refusal::Symlink | Refusal::NotRegular => Self::NotFound,
            Refusal::Io(e) if e.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Refusal::Io(_) => Self::Unavailable,
            Refusal::Lookup(_) => Self::Internal,
        }
    }

    /// [`PageUnavailable::from_refusal`], plus: a missing file under a
    /// library root that cannot be opened, or is empty, is the library
    /// being offline (an unmounted share or bind mount), not the file
    /// being gone. Only the missing-file path pays for the check.
    pub(crate) async fn for_refusal(app: &AppState, row: &issue::Model, refusal: &Refusal) -> Self {
        let class = Self::from_refusal(refusal);
        if class != Self::Missing {
            return class;
        }
        if crate::library::no_follow::library_root_offline(app, row.library_id).await {
            Self::LibraryOffline
        } else {
            Self::Missing
        }
    }

    /// Classify a failed archive open or read.
    pub(crate) fn from_archive_error(e: &archive::ArchiveError) -> Self {
        use archive::ArchiveError as E;
        match e {
            E::Io(_) => Self::Unavailable,
            E::NoPageReader(_) => Self::NeedsConversion,
            E::Malformed(_) | E::Encrypted | E::UnsafeEntry(_) | E::CapExceeded(_) => {
                Self::Unreadable
            }
        }
    }

    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Missing | Self::Unreadable => StatusCode::GONE,
            Self::NeedsConversion => StatusCode::CONFLICT,
            Self::LibraryOffline | Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Missing => "archive_missing",
            Self::Unreadable => "archive_unreadable",
            Self::NeedsConversion => "archive_needs_conversion",
            Self::LibraryOffline => "library_offline",
            Self::Unavailable => "service_unavailable",
            Self::Internal => "internal",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotFound => "issue not found",
            Self::Missing => "the issue's file is missing",
            Self::Unreadable => "the issue's file is not a readable archive",
            Self::NeedsConversion => {
                "the issue's file format cannot be shown as pages until it is converted to CBZ"
            }
            Self::LibraryOffline => "the library's folder is not available; retry shortly",
            Self::Unavailable => "the issue's file could not be read; retry shortly",
            Self::Internal => "internal",
        }
    }

    /// The canonical envelope for this case, never cacheable.
    pub(crate) fn response(self) -> Response {
        let mut resp = error(self.status(), self.code(), self.message());
        if self == Self::LibraryOffline {
            resp.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(OFFLINE_RETRY_AFTER_SECS),
            );
        }
        no_store(resp)
    }
}

/// The plain 404 these routes answer for an unknown or hidden issue,
/// never cacheable.
pub(crate) fn issue_not_found() -> Response {
    PageUnavailable::NotFound.response()
}

/// Mark an error response as not to be stored, and add `Retry-After` to a
/// 503 that has none. Also used for the page-out-of-range 404s and the
/// 416s next to these answers.
pub(crate) fn no_store(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

/// Short tag for a [`archive::FileStamp`], folded into page and variant
/// ETags (and the variant cache key) so they change when the file changes,
/// even before a rescan updates `content_hash` (audit R-05). 16 hex chars
/// of a BLAKE3 digest; reveals nothing about the file.
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
            PageUnavailable::NeedsConversion,
            PageUnavailable::LibraryOffline,
            PageUnavailable::Unavailable,
            PageUnavailable::Internal,
        ] {
            let resp = case.response();
            assert_eq!(resp.status(), case.status());
            assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
        }
        let busy = PageUnavailable::Unavailable.response();
        assert_eq!(busy.headers()[header::RETRY_AFTER], "5");
        let offline = PageUnavailable::LibraryOffline.response();
        assert_eq!(offline.headers()[header::RETRY_AFTER], "30");
        assert_eq!(
            PageUnavailable::from_archive_error(&archive::ArchiveError::NoPageReader("cbr".into()))
                .status(),
            StatusCode::CONFLICT
        );
    }
}
