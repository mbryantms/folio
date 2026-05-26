//! Reprint relationships from issue → another issue. The reprinted
//! side may be a `reprinted_issue_id` FK (when the library has that
//! issue) or just a `reprinted_label` string ("Amazing Spider-Man
//! #1") when it doesn't. Schema-level CHECK enforces at least one.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_reprints")]
pub struct Model {
    /// Synthetic PK. Dedup is enforced by the unique index over
    /// `(issue_id, COALESCE(reprinted_issue_id, ''), COALESCE(reprinted_label, ''))`.
    /// We can't put nullable columns in a sea-orm composite PK.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub issue_id: String,
    #[sea_orm(nullable)]
    pub reprinted_issue_id: Option<String>,
    #[sea_orm(nullable)]
    pub reprinted_label: Option<String>,
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
