use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One widget pinned to the user's `/log` page. Reading-Log M3.
///
/// `kind` is a string discriminator — `chrono_feed`, `stats_hero`,
/// `heatmap`, `top_creators`, `top_publishers`, `top_imprints`,
/// `series_finishes`, `pace_chart`, `time_of_day`, `recent_bookmarks`,
/// `currently_reading`, `note`. Renderer registry lives on the web
/// side; the server validates the per-kind `config` blob's shape on
/// every write.
///
/// `position` is a 0-based dense rank — reorders rewrite every row's
/// position in one transaction so we never have to sort by
/// `created_at` as a tiebreaker.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "log_widgets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub position: i32,
    /// Kind-specific JSON config. Server validates against
    /// `api::log_widgets::WIDGET_KINDS` on POST/PATCH; the renderer
    /// re-validates client-side with Zod. `{}` (the column default)
    /// is always a legal value — kinds with no config (e.g.
    /// `time_of_day`) use it.
    pub config: Json,
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
