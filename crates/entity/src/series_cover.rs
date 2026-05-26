//! Per-series cover (banner / logo / catalog image). Mirrors
//! [`super::issue_cover`] without variants; series typically have a
//! single canonical banner per source.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_cover")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub series_id: Uuid,
    /// `'primary' | 'logo' | 'banner'`.
    pub kind: String,
    pub ordinal: i32,
    #[sea_orm(nullable)]
    pub source_provider: Option<String>,
    #[sea_orm(nullable)]
    pub source_external_id: Option<String>,
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    pub local_path: String,
    #[sea_orm(nullable)]
    pub width: Option<i32>,
    #[sea_orm(nullable)]
    pub height: Option<i32>,
    pub fetched_at: DateTimeWithTimeZone,
    pub is_active: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
}

impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
