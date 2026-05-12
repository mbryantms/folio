use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user library access (§5.1.1). Schema lands Phase 1; admin UI Phase 5.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_user_access")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub library_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,

    /// `reader` | `curator`
    pub role: String,

    /// ComicInfo AgeRating cap; NULL = unrestricted.
    #[sea_orm(nullable)]
    pub age_rating_max: Option<String>,

    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
