//! A chapter break in a reading list (linked reading lists, PR 3a): a new
//! chapter starts at the anchor issue. See
//! `m20271124_000003_reading_list_breaks`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use crate::reading_list_link::AnchorStatus;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "reading_list_breaks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub list_id: Uuid,
    #[sea_orm(nullable)]
    pub label: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_cv_issue_id: Option<i32>,
    #[sea_orm(nullable)]
    pub anchor_series: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_number: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_volume: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_year: Option<String>,
    #[sea_orm(nullable)]
    pub anchor_position_hint: Option<i32>,
    pub anchor_status: AnchorStatus,
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cbl_list::Entity",
        from = "Column::ListId",
        to = "super::cbl_list::Column::Id",
        on_delete = "Cascade"
    )]
    List,
}

impl Related<super::cbl_list::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::List.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
