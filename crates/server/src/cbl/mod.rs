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

pub use parsers::cbl as parser;

pub mod catalog;
pub mod import;
pub mod matcher;
pub mod refresh;
