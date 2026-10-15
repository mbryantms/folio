//! Per-user, per-series reader overrides (post-audit T4.4). One row per
//! `(user_id, series_id)`; each slice column is NULL when the user has
//! no override for it, so the reader falls through to the account
//! default. Values are the wire tokens of `auth::preferences` enums
//! (`ltr` / `rtl`, `width` / `height` / `original` / `contain`,
//! `single` / `double` / `webtoon`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_series_reader_prefs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub series_id: Uuid,
    #[sea_orm(nullable)]
    pub reading_direction: Option<String>,
    #[sea_orm(nullable)]
    pub fit_mode: Option<String>,
    #[sea_orm(nullable)]
    pub view_mode: Option<String>,
    #[sea_orm(nullable)]
    pub cover_solo: Option<bool>,
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
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
