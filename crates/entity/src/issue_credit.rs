use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_credits")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    /// One of: `writer`, `penciller`, `inker`, `colorist`, `letterer`,
    /// `cover_artist`, `editor`, `translator`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub role: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub person: String,
    /// FK to `person.id`. Backfilled from `person` for existing rows
    /// (m20261225) and kept in sync by the scanner's series rollup.
    /// Nullable so freshly-scanned credits aren't blocked by a
    /// not-yet-populated `person` row; the rollup fills it in shortly
    /// after.
    pub person_id: Option<Uuid>,
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
        belongs_to = "super::person::Entity",
        from = "Column::PersonId",
        to = "super::person::Column::Id",
        on_delete = "SetNull"
    )]
    Person,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl Related<super::person::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Person.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
