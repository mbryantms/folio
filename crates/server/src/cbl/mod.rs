//! CBL reading-list ingestion (saved-views M4).
//!
//! Five sub-modules:
//!
//!   - [`parser`] — re-export of the wire-level XML parser
//!     (`parsers::cbl`) under a stable name.
//!   - [`matcher`] — resolves parsed `<Book>` entries against the local
//!     library. Three-tier strategy: ComicVine ID → Metron ID →
//!     trigram name + exact volume + exact number fallback.
//!   - [`catalog`] — GitHub repo browser. Indexes a `catalog_sources`
//!     row's tree (cached by ETag), fetches raw `.cbl` blobs by path.
//!   - [`import`] — orchestration: parse → diff against existing entries
//!     → persist → run matcher → write `cbl_refresh_log` row.
//!   - [`refresh`] — the scheduled / manual refresh entrypoint that
//!     dispatches per `source_kind`.
//!   - [`fetch`] — the vetted outbound fetch for URL lists and the
//!     refresh time budgets.

pub use parsers::cbl as parser;

/// Most entries a reading list may have. Enforced at import (upload, URL,
/// catalog) and again on every refresh and check, so a remote file that
/// grows past it is refused rather than stored or truncated.
pub const MAX_BOOKS_PER_LIST: usize = 5_000;

/// The refusal text for a file over [`MAX_BOOKS_PER_LIST`].
pub fn too_many_entries_message() -> String {
    format!("exceeds {MAX_BOOKS_PER_LIST} entries")
}

pub mod catalog;
pub mod fetch;
pub mod import;
pub mod matcher;
pub mod quota;
pub mod refresh;
