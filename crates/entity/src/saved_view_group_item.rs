use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Placement of one saved view on one user's Views board. The
/// `(user_id, view_id)` primary key means a view sits in at most one
/// place per user: in a [`super::saved_view_group`] when `group_id` is
/// set, or at an explicit `position` inside the ungrouped remainder of
/// `scope` when it is `None`. Views without a row are ungrouped with no
/// pinned position. Rows cascade away with the group (→ ungrouped) and
/// with the view.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "saved_view_group_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub view_id: Uuid,
    /// Views tab (`filter` / `cbl` / `collection`); matches the group's
    /// scope when grouped, and locates ungrouped rows otherwise.
    pub scope: String,
    pub group_id: Option<Uuid>,
    /// Ordinal within the group, or within the ungrouped section.
    pub position: i32,
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
    #[sea_orm(
        belongs_to = "super::saved_view::Entity",
        from = "Column::ViewId",
        to = "super::saved_view::Column::Id",
        on_delete = "Cascade"
    )]
    SavedView,
    #[sea_orm(
        belongs_to = "super::saved_view_group::Entity",
        from = "Column::GroupId",
        to = "super::saved_view_group::Column::Id",
        on_delete = "Cascade"
    )]
    Group,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::saved_view::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SavedView.def()
    }
}
impl Related<super::saved_view_group::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Group.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
