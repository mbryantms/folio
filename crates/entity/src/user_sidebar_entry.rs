use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user override of one sidebar entry (navigation customization M1).
/// Missing rows mean "default behavior" — the layout resolver
/// (`server::sidebar_layout::compute_layout`) merges these overrides on
/// top of a computed default list, so adding a new library or saved view
/// doesn't require a backfill.
///
/// `kind` ∈ `{'builtin', 'library', 'view'}`; `ref_id` is the registry
/// key, library UUID, or saved-view UUID respectively (TEXT because the
/// builtin keys are strings).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_sidebar_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub ref_id: String,
    pub visible: bool,
    pub position: i32,
    /// Optional label override. For `kind='header'` this is the section
    /// title; for `kind='spacer'` it's ignored; for everything else it
    /// overrides the server-resolved label (e.g. rename "Bookmarks"
    /// → "Pins"). `NULL` falls back to the default.
    #[sea_orm(nullable)]
    pub label: Option<String>,
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
