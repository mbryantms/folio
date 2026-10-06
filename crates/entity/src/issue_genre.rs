use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_genres")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub genre: String,
    /// Position within the source list (ComicInfo CSV order for scanner
    /// writes, provider order for applies). `0` for rows written before
    /// the column existed.
    pub ordinal: i32,
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
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
