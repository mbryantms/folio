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
//! - [`http`] — shared outbound layer: hardened client builder, bounded
//!   retry with jittered backoff, body cap, `Retry-After` parsing.
//! - [`rate_limit`] — Redis-backed atomic token buckets. Per-provider,
//!   per-window (CV hourly; Metron minute + day). Survives restarts.
//! - [`budget`] — what the *upstream* says is left (Metron's
//!   `X-RateLimit-*` headers), plus the last provider error, in Redis.
//! - [`cover_hash_cache`] — search-time cover pHash cache keyed by
//!   provider image URL (`metadata_cover_hash` table).
//! - [`cache`] — TTL-bounded JSON cache for normalized `GenericMetadata`
//!   payloads (`metadata_cache` table from M1 migration).
//! - [`comicvine`] — first concrete provider impl (M1); [`metron`] (M2);
//!   [`gcd`] — Grand Comics Database (WP-6.1).

pub mod apply;
pub mod auto_split;
pub mod budget;
pub mod cache;
pub mod comicvine;
pub mod completeness;
pub mod composite;
pub mod cover_block;
pub mod cover_hash_cache;
pub mod diff;
pub mod drift;
pub mod field;
pub mod gcd;
pub mod http;
pub mod identifier;
pub mod lookup;
pub mod manual_writeback;
pub mod match_outcome;
pub mod matcher;
pub mod merge;
pub mod metron;
pub mod orchestrator;
pub mod phash;
pub mod provider;
pub mod range_map;
pub mod ratcliff;
pub mod rate_limit;
pub mod refresh;
pub mod sidecar_compose;
pub mod title_norm;
pub mod writeback_progress;
pub mod writers;

pub use field::MetadataField;
pub use identifier::{Identifier, Source};
pub use provider::{
    GenericMetadata, IssueQuery, MetadataProvider, ProviderError, ProviderResult, QuotaSnapshot,
    SeriesQuery,
};
