//! M7 of `metadata-sidecar-writeback-1.0`: writeback-rollout visibility.
//!
//! Counts libraries the operator hasn't yet flipped to writeback mode.
//! Surfaced as the Prometheus gauge
//! `folio_metadata_writeback_libraries_remaining`, refreshed by the
//! [`crate::jobs::scheduler::refresh_writeback_remaining_gauge`] hook
//! at boot + weekly. Once the gauge stays at zero in production the
//! legacy DB-direct apply branch in
//! [`crate::metadata::apply::apply_issue`] /
//! [`apply_series`] becomes dead code and the follow-up code-quality
//! cleanup PR can drop it. This module owns the query only — emitting
//! the metric stays in the scheduler so the test surface is a pure
//! function.

use entity::library;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};

/// Number of libraries with `metadata_writeback_enabled = false`. Cheap
/// — single indexed COUNT, no joins. Returned as `u64` to match the
/// SeaORM `count()` shape; the gauge callsite casts to `f64`.
pub async fn count_libraries_without_writeback<C: ConnectionTrait>(
    db: &C,
) -> Result<u64, sea_orm::DbErr> {
    library::Entity::find()
        .filter(library::Column::MetadataWritebackEnabled.eq(false))
        .count(db)
        .await
}
