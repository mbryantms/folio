//! A request to add a series to Mylar, or to mark one issue Wanted there
//! (migration `m20271127`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mylar_add_request")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub cv_series_id: i64,
    /// `None` = add the series; set = mark this issue Wanted.
    #[sea_orm(nullable)]
    pub cv_issue_id: Option<i64>,
    pub series_name: String,
    #[sea_orm(nullable)]
    pub issue_number: Option<String>,
    /// `pending`, `done` or `failed`.
    pub status: String,
    pub attempts: i32,
    #[sea_orm(nullable)]
    pub last_error: Option<String>,
    pub next_attempt_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub requested_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
