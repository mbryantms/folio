//! A relationship from a local series to a provider series that is not in
//! the library (WP-7.8). See migration
//! `m20270508_000001_series_external_relationship`.
//!
//! One-directional — no inverse row. `set_by = 'provider'` rows come from
//! provider data (Metron `associated`) and are refreshed by every series
//! apply; `set_by = 'user'` rows are admin-made and never touched by a
//! provider write. Write through `server::relationships::external`, which
//! also promotes a row once its target series is matched locally.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_external_relationship")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// The local series: "`from` `kind` the provider series".
    pub from_series_id: Uuid,
    /// One of the 31 relationship kinds (DB CHECK).
    pub kind: String,
    /// Continuation qualifier or tie-in role (same CHECK as
    /// `series_relationship`).
    #[sea_orm(nullable)]
    pub qualifier: Option<String>,
    /// `metron | comicvine | gcd`.
    pub source: String,
    pub provider_series_id: String,
    /// Display name ("Saga (2018)" style labels are split into name + year).
    #[sea_orm(nullable)]
    pub provider_series_name: Option<String>,
    /// Canonical link to the provider page (TOS attribution).
    #[sea_orm(nullable)]
    pub provider_series_url: Option<String>,
    #[sea_orm(nullable)]
    pub provider_year: Option<i32>,
    /// `user | provider`. A `user` row is never overwritten by a provider
    /// write.
    pub set_by: String,
    /// 0–1 for provider rows; NULL for user rows.
    #[sea_orm(nullable)]
    pub confidence: Option<f32>,
    /// `{ "source": "metron", "field": "associated", "ids": [...] }` for
    /// provider rows.
    pub evidence: Json,
    /// Admin who added a `user` row.
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    /// Set when a `provider` row's target is matched to a local series (the
    /// suggestion engine takes over). User rows are deleted on promotion
    /// instead.
    #[sea_orm(nullable)]
    pub promoted_series_id: Option<Uuid>,
    /// A `provider` row an admin removed: hidden, never re-created by a
    /// later apply, and ignored by the suggestion engine (rejection memory).
    #[sea_orm(nullable)]
    pub dismissed_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub dismissed_by: Option<Uuid>,
    pub first_set_at: DateTimeWithTimeZone,
    pub last_synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
