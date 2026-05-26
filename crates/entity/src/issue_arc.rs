//! Junction: issues ↔ story_arc. FK-PK from the start (no legacy
//! string-keyed data — populated from CSV in the M0 migration).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_arcs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub arc_id: Uuid,
    #[sea_orm(nullable)]
    pub position_in_arc: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
    #[sea_orm(
        belongs_to = "super::story_arc::Entity",
        from = "Column::ArcId",
        to = "super::story_arc::Column::Id",
        on_delete = "Cascade"
    )]
    StoryArc,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}
impl Related<super::story_arc::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::StoryArc.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
