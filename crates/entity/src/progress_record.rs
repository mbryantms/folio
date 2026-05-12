use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Phase 2 placeholder; superseded by Automerge sync in Phase 4 (§9.7).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "progress_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    pub last_page: i32,
    pub percent: f64,
    pub finished: bool,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub device: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
