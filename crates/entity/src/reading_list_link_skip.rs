//! A reader's "skip this event" on a linked reading list (PR 2).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "reading_list_link_skips")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub link_id: Uuid,
    pub skipped_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::reading_list_link::Entity",
        from = "Column::LinkId",
        to = "super::reading_list_link::Column::Id",
        on_delete = "Cascade"
    )]
    Link,
}

impl ActiveModelBehavior for ActiveModel {}
