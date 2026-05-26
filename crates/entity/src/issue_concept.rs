//! Junction: issues ↔ concept (ComicVine-only).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_concepts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub concept_id: Uuid,
    pub is_first_appearance: bool,
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
        belongs_to = "super::concept::Entity",
        from = "Column::ConceptId",
        to = "super::concept::Column::Id",
        on_delete = "Cascade"
    )]
    Concept,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}
impl Related<super::concept::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Concept.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
