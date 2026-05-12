use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user-per-view preferences (saved-views M3 / M9 polish). One row
/// per `(user_id, view_id)` storing the user's home-rail pin order plus
/// the two opt-in flags `pinned` (rail visibility) and
/// `show_in_sidebar` (left-nav visibility). Rows persist even when both
/// flags are off so the chosen `position` survives an unpin → re-pin
/// round trip.
///
/// Table name kept as `user_view_pins` for migration continuity.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_view_pins")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub view_id: Uuid,
    pub position: i32,
    pub pinned: bool,
    pub show_in_sidebar: bool,
    /// Per-user icon override key (e.g. `'sparkles'`, `'book-open'`,
    /// `'shield'`). NULL → kind-based default resolved client-side. The
    /// client maps the key to a Lucide icon via its rail-icon registry;
    /// unknown keys silently fall back to the default.
    #[sea_orm(nullable)]
    pub icon: Option<String>,
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

impl ActiveModelBehavior for ActiveModel {}
