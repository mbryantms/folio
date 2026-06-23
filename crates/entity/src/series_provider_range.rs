//! Per-(series, provider) issue-range mapping — provider series-boundary
//! divergence support. See migration
//! `m20270119_000001_series_provider_range` for the full rationale.
//!
//! Records the exceptions to the default whole-series provider mapping:
//! a contiguous issue-number range of a local series that routes to a
//! DIFFERENT provider series than the series-level `external_ids` row.
//! The series-level external id remains the implicit whole-series
//! default; an empty table reproduces pre-feature behaviour exactly.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_provider_range")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Local series this range belongs to (FK → series, ON DELETE CASCADE).
    pub series_id: Uuid,
    /// `'comicvine' | 'metron' | 'gcd' | 'marvel' | 'locg' | …` — same
    /// domain as `external_ids.source`.
    pub source: String,
    /// The provider's series identifier this range maps to.
    pub provider_series_id: String,
    /// Canonical link back to the provider series (TOS attribution + the
    /// "alternate provider series" UI link).
    #[sea_orm(nullable)]
    pub provider_series_url: Option<String>,
    /// Provider's display name for the mapped series (e.g. "Fantastic
    /// Four (2012)") — surfaced inline so the user sees the divergence.
    #[sea_orm(nullable)]
    pub provider_series_name: Option<String>,
    /// Inclusive lower bound, a *canonical* issue number
    /// (`metadata::matcher::canonical_issue_number`). NULL = open-ended.
    #[sea_orm(nullable)]
    pub range_low: Option<String>,
    /// Inclusive upper bound, canonical issue number. NULL = open-ended.
    #[sea_orm(nullable)]
    pub range_high: Option<String>,
    /// The mapped sub-series' start year. Consulted by the issue-search
    /// year gate so the splitter candidate isn't dropped against the
    /// parent series year.
    #[sea_orm(nullable)]
    pub declared_year: Option<i32>,
    /// `'user' | 'comicvine' | 'metron' | …`. A `'user'` row is sacred —
    /// never silently overwritten by an automated write.
    pub set_by: String,
    pub first_set_at: DateTimeWithTimeZone,
    pub last_synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
