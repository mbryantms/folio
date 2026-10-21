use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-user closing note on an issue — issue-level, not page-anchored
/// (page notes are `marker` rows with `kind = 'note'`). Written from the
/// reader's end-of-issue Wrap up pane; shown beside the user's rating on
/// the issue page. One row per `(user, issue)`; clearing the note deletes
/// the row.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_issue_notes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    pub body: String,
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
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
