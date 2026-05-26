//! Metadata-provider integration plumbing (metadata-providers-1.0 M0).
//!
//! Houses the value types (`Identifier`, `Source`, `MetadataField`) and
//! the writer helpers (`writers`) that every metadata-producing code
//! path — scanner, bulk-edit dialog, M4 Apply jobs, manual
//! `<ExternalIdsCard>` edits — funnels through. Single audited
//! surface, single de-dup rule, single CSV-cache rebuild trigger.
//!
//! The provider HTTP clients (ComicVineClient / MetronClient) land in
//! M1+ as a separate `crates/metadata/` crate; this module owns only
//! the in-DB side of the integration.

pub mod field;
pub mod identifier;
pub mod writers;

pub use field::MetadataField;
pub use identifier::{Identifier, Source};
