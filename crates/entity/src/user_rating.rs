use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user 0..=5 star rating against an issue or series. Composite primary
/// key on `(user_id, target_type, target_id)` so upserts are direct PK
/// writes; `target_type` distinguishes the two scopes (`"issue"` keys on
/// the BLAKE3 issue id, `"series"` keys on the UUID rendered as text).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_ratings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub target_type: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub target_id: String,
    pub rating: f64,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
