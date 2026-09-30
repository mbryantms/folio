//! Admin verdict on an issue surfaced by the Duplicates page (WP-3.3).
//! See `m20270212_000001_issue_duplicate_decision` for semantics.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_duplicate_decision")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    pub library_id: Uuid,
    /// `'keep'` | `'remove'` (CHECK-enforced).
    pub decision: String,
    #[sea_orm(nullable)]
    pub decided_by: Option<Uuid>,
    pub decided_at: DateTimeWithTimeZone,
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
