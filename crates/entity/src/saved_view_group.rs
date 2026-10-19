use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A per-user bucket on the Views index (Settings → Views). Groups are
/// scoped to one index tab via `scope` (`'filter'`, `'cbl'`,
/// `'collection'`) and ordered by `position` within that scope.
/// Membership is in [`super::saved_view_group_item`]; a group with no
/// items is still rendered (as an empty drop target).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "saved_view_groups")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    /// `'filter'` | `'cbl'` | `'collection'` — which Views tab the group
    /// belongs to. CHECK-constrained in the migration.
    pub scope: String,
    pub name: String,
    /// Ordinal within `(user_id, scope)`.
    pub position: i32,
    /// UI state: whether the group renders collapsed on the index.
    pub collapsed: bool,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
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
    #[sea_orm(has_many = "super::saved_view_group_item::Entity")]
    Items,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::saved_view_group_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Items.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
