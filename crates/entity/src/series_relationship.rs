//! Typed, directed series → series edge (WP-7.1). See migration
//! `m20270501_000001_series_relationship` for the schema rationale.
//!
//! Rows always come in inverse pairs (`A sequel_of B` + `B prequel_of A`);
//! never insert or delete one directly — go through
//! `server::relationships::{create_pair, delete_pair}`, which keep both
//! halves in sync inside one transaction.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_relationship")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Subject of the edge: "`from` is a sequel of `to`".
    pub from_series_id: Uuid,
    pub to_series_id: Uuid,
    /// `sequel_of | prequel_of | spin_off_of | has_spin_off |
    /// crossover_with | collects | collected_in | same_universe |
    /// see_also` (DB CHECK; typed as `server::relationships::RelationshipKind`).
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
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
