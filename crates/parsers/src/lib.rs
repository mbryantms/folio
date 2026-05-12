//! Metadata parsers (Phase 1a/1b).
//!
//! All XML parsers MUST disable DOCTYPE / external entity resolution (§17.12).
//! All JSON parsers cap input size at 256 KiB before parse (§A7).

pub mod cbl;
pub mod comicinfo;
pub mod filename;
pub mod metroninfo;
pub mod series_json;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("input too large: {actual} > {limit} bytes")]
    TooLarge { actual: usize, limit: usize },
    #[error("XML DOCTYPE rejected (XXE-safe parser)")]
    DoctypeRejected,
    #[error("malformed: {0}")]
    Malformed(String),
}
