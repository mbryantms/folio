use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user record of "hide this thing from the home rails". Composite PK
/// on `(user_id, target_kind, target_id)` so re-dismissing the same target
/// is idempotent. Auto-restore is enforced in the rail query (the row
/// stays around and is just filtered out when the target sees new
/// `progress_records.updated_at`).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "rail_dismissals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    /// `'issue'`, `'series'`, or `'cbl'`. CHECK-enforced at the schema layer.
    #[sea_orm(primary_key, auto_increment = false)]
    pub target_kind: String,
    /// Issue id (text) or series/cbl UUID rendered as text. Stored as text
    /// so we don't need a kind-specific FK column for each polymorphic case.
    #[sea_orm(primary_key, auto_increment = false)]
    pub target_id: String,
    pub dismissed_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
