use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Authoritative per-(user, issue) reading-progress store. The spec's
/// original §9 plan to replace this with Automerge CRDT documents was
/// reconsidered and dropped on 2026-05-15 — server-side
/// `max(last_page)` resolves every multi-device conflict this table
/// actually sees. See the decision note at spec §9.
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
