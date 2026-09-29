use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Search-time cover perceptual-hash cache keyed by provider image URL
/// (WP-2.9). See `server::metadata::cover_hash_cache`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_cover_hash")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub url: String,
    pub phash: i64,
    pub dhash: i64,
    pub ahash: i64,
    pub fetched_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
