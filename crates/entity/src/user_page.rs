use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user **page** — a named container of pinned saved-view rails
/// (multi-page rails M1). Every user has exactly one `is_system = true`
/// row (slug `"home"`, default name `"Home"`) that owns today's pins and
/// is reachable at `/`. Custom pages live at `/pages/{slug}` and may be
/// renamed, reordered, or deleted; the system page can be renamed but
/// never deleted. Pin order is stored on `user_view_pins` scoped by
/// `(user_id, page_id)`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_page")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    /// Slug used to address the page at `/pages/{slug}`. Unique per user.
    /// For the system page this is always `"home"` and never regenerates
    /// on rename.
    pub slug: String,
    /// `true` for the auto-created Home row. Enforced one-per-user by a
    /// partial unique index on `(user_id) WHERE is_system`.
    pub is_system: bool,
    /// Ordinal within the user's page list. Drives sidebar order.
    pub position: i32,
    /// Optional free-form description rendered under the page title.
    /// `None` hides the descriptor row entirely.
    #[sea_orm(nullable)]
    pub description: Option<String>,
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
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
