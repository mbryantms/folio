use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user library access (§5.1.1). Membership grants read access to
/// the library; `age_rating_max` caps what the member sees inside it
/// (WP-2.7). The `role` column (`reader` plus a never-enforced editor value) was dropped in
/// `m20270123_000001_drop_library_access_role` — nothing ever read it.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_user_access")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub library_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,

    /// ComicInfo `AgeRating` cap (a rung of
    /// `server::library::age_rating::LADDER`); NULL = unrestricted.
    /// Rows rated above the cap are hidden; unrated rows pass.
    #[sea_orm(nullable)]
    pub age_rating_max: Option<String>,

    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
