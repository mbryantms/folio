//! Metadata-provider integration plumbing.
//!
//! Layers in dependency order:
//! - [`identifier`], [`field`] — value types (`Identifier`, `Source`,
//!   `MetadataField`) shared across DB writes, provider responses, and
//!   the `<ExternalIdsCard>` payload.
//! - [`writers`] — single audited DB write surface (scanner, bulk-edit,
//!   future Apply jobs, manual external-id edits all funnel through).
//! - [`provider`] — `MetadataProvider` trait + `GenericMetadata`. The
//!   only shape Apply jobs see; CV/Metron dialect dies at the client
//!   boundary.
//! - [`rate_limit`] — Redis-backed atomic token buckets. Per-provider,
//!   per-window (CV hourly; Metron minute + day). Survives restarts.
//! - [`cache`] — TTL-bounded JSON cache for normalized `GenericMetadata`
//!   payloads (`metadata_cache` table from M1 migration).
//! - [`comicvine`] — first concrete provider impl (M1).

pub mod apply;
pub mod cache;
pub mod comicvine;
pub mod diff;
pub mod field;
pub mod identifier;
pub mod matcher;
pub mod metron;
pub mod orchestrator;
pub mod provider;
pub mod rate_limit;
pub mod writers;

pub use field::MetadataField;
pub use identifier::{Identifier, Source};
pub use provider::{
    GenericMetadata, IssueQuery, MetadataProvider, ProviderError, ProviderResult, QuotaSnapshot,
    SeriesQuery,
};
