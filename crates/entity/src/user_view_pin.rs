use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user-per-page-per-view preferences. One row per
/// `(user_id, page_id, view_id)` storing the user's pin order on a given
/// page plus the two opt-in flags `pinned` (rail visibility) and
/// `show_in_sidebar` (left-nav visibility — only meaningful for pins on
/// the system page). Rows persist even when both flags are off so the
/// chosen `position` survives an unpin → re-pin round trip.
///
/// `page_id` was added in the multi-page rails M1 migration; existing rows
/// were back-filled onto each user's auto-created system page so the
/// "home" surface keeps rendering the same set it always did.
///
/// Table name kept as `user_view_pins` for migration continuity.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_view_pins")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub page_id: Uuid,
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
    #[sea_orm(
        belongs_to = "super::user_page::Entity",
        from = "Column::PageId",
        to = "super::user_page::Column::Id",
        on_delete = "Cascade"
    )]
    UserPage,
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
impl Related<super::user_page::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserPage.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
