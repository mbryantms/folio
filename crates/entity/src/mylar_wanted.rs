//! An issue Mylar wants, has snatched, or failed to get (migration
//! `m20271119`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mylar_wanted")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(nullable)]
    pub cv_series_id: Option<i64>,
    #[sea_orm(nullable)]
    pub cv_issue_id: Option<i64>,
    pub series_name: String,
    pub issue_number: String,
    #[sea_orm(nullable)]
    pub store_date: Option<Date>,
    /// `Wanted`, `Snatched` or `Failed`.
    pub status: String,
    #[sea_orm(nullable)]
    pub added_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
