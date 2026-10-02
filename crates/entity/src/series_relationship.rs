//! Typed, directed series → series (or series → story arc) edge (WP-7.1,
//! WP-7.5). See migrations `m20270501_000001_series_relationship` and
//! `m20270505_000001_relationship_taxonomy` for the schema rationale.
//!
//! Series → series rows always come in inverse pairs (`A sequel_of B` +
//! `B has_sequel A`); series → arc rows (`to_arc_id`, `tie_in_to` only) are
//! single rows. Never insert or delete one directly — go through
//! `server::relationships`, which keeps both halves in sync inside one
//! transaction.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_relationship")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Subject of the edge: "`from` is a sequel of `to`".
    pub from_series_id: Uuid,
    /// The other series. NULL exactly when `to_arc_id` is set (DB CHECK).
    #[sea_orm(nullable)]
    pub to_series_id: Option<Uuid>,
    /// Story-arc target (WP-7.5; `tie_in_to` only, no inverse row).
    #[sea_orm(nullable)]
    pub to_arc_id: Option<Uuid>,
    /// One of the 31 kinds (DB CHECK; typed as
    /// `server::relationships::RelationshipKind`).
    pub kind: String,
    /// `manual | suggested`.
    pub source: String,
    /// Suggestion confidence 0.0–1.0; NULL for manual edges.
    #[sea_orm(nullable)]
    pub confidence: Option<f32>,
    /// Admin who created the edge (NULL once the account is deleted).
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    /// Issue range on the `from` side ("1-6", "1-6,Annual 1"); ≤ 100 chars.
    #[sea_orm(nullable)]
    pub from_range: Option<String>,
    /// Issue range on the `to` side; ≤ 100 chars.
    #[sea_orm(nullable)]
    pub to_range: Option<String>,
    /// `full | partial | unknown` — collects / reprints (and inverses) only.
    #[sea_orm(nullable)]
    pub coverage: Option<String>,
    /// Continuation qualifier (`relaunch | retitle | merge | split |
    /// numbering`) or tie-in role (`main | tie_in | prelude | aftermath`).
    #[sea_orm(nullable)]
    pub qualifier: Option<String>,
    /// Free text, ≤ 500 chars.
    #[sea_orm(nullable)]
    pub note: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
