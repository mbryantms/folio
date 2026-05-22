//! Person — a creator with a stable slug + display name. Aggregated
//! from the raw `person TEXT` columns on `series_credits` +
//! `issue_credits` via the M20261223 backfill. Acts as the URL
//! identity behind `/creators/<slug>` detail pages.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "person")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// URL-safe identifier, globally unique. Allocated at backfill
    /// time; collisions get `-2`, `-3`, … suffixes.
    #[sea_orm(unique)]
    pub slug: String,
    /// Display name as it appeared in the credits — preserves casing,
    /// punctuation, et al.
    pub name: String,
    /// Trimmed + lowercased form used for dedupe + similarity
    /// queries. Unique across persons; the search endpoint joins to
    /// `series_credits.person` / `issue_credits.person` via
    /// `lower(person) = normalized_name`.
    #[sea_orm(unique)]
    pub normalized_name: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
